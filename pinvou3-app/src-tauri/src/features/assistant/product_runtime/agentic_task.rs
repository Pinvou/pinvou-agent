//! 无头单任务 agentic 入口(外部 harness 用,如 Terminal-Bench/Harbor)。
//!
//! 与 [`super::headless_bridge`] 的评测后端不同,这里走**产品等价**的 agentic
//! 轮次:`TurnInput::eval_tool_policy = None` → `EnginePool::send_user_message`,
//! 即 GUI 同一条链路(Yolo 模式、产品工具白名单、Bash/File 写权限、真实 shell)。
//! 评测只读隔离不受影响:GAIA 路径仍强制 eval policy,本入口不经过
//! `HeadlessAgentBackend`,也不触碰任何 eval 工具策略。
//!
//! 会话执行根通过 `ExecutionRootResolver` 绑定到调用方提供的任务目录——
//! 与原生代码会话绑定项目目录是同一机制,shell/File 的 cwd 即任务目录。
//! resolver 闭包只认本次生成的会话 id,不影响宿主内其它会话。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use deepseek_tui::tui::app::AppMode;
use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::features::assistant::engine_pool::EnginePool;
use crate::features::assistant::product_runtime::headless_bridge::build_pool;
use crate::features::assistant::product_runtime::{
    EnginePoolRuntime, ProductChatRuntime, SessionSpec, TurnInput,
};
use crate::features::sessions::{ExecutionRootResolver, SessionStore};

const DEFAULT_TIMEOUT_SECS: u64 = 600;
/// 取消后的沉淀窗口:给引擎时间收尾落盘,超窗即放弃等待部分结果。
const CANCEL_SETTLE_SECS: u64 = 30;

/// 一次 agentic 任务的输入。`prompt` 原样进入产品发送链路(不加评测信封)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgenticTaskRequest {
    pub prompt: String,
    /// 任务工作目录;None = 会话私有目录(与评测会话一致的隔离 scratch)。
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

/// 工具调用摘要:只暴露名字与成败,不携带任何参数/结果内容。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgenticToolEvent {
    pub name: String,
    pub failed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgenticUsageReport {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_hit_tokens: u64,
    pub cache_miss_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    pub context_window: u64,
}

/// 一次 agentic 任务的最终报告。`assistant_text` 为最后一轮助手文本。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgenticTaskReport {
    pub session_id: String,
    pub status: String,
    pub timed_out: bool,
    pub assistant_text: String,
    pub tool_events: Vec<AgenticToolEvent>,
    pub usage: Option<AgenticUsageReport>,
    pub error: Option<String>,
}

/// 在窗口化 Tauri 宿主里跑一次 agentic 任务并返回结构化报告。
///
/// 宿主引导与 [`super::headless_bridge::run_headless_host`] 保持同构
/// (同一 `build_tauri_context` 展开点、同一 store 启动序列),区别仅在
/// 交给工作闭包的是 `Arc<EnginePool>` + `SessionStore` 而非评测后端。
pub fn run_agentic_task_headless(request: AgenticTaskRequest) -> Result<AgenticTaskReport> {
    crate::install_rustls_provider();
    crate::ensure_release_env();
    crate::startup_process_env();
    let async_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(16 * 1024 * 1024)
        .build()
        .context("build headless async runtime")?;
    tauri::async_runtime::set(async_runtime.handle().clone());
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    // 复用 lib.rs 的单一 generate_context 展开点:本 crate 内二次展开会在
    // macOS 触发 embed_plist 的 _EMBED_INFO_PLIST 重复符号链接错误。
    let mut context = crate::build_tauri_context();
    context.config_mut().app.windows.clear();
    let app = tauri::Builder::default()
        .setup(move |app| {
            if let Ok(resource_dir) = app.path().resource_dir() {
                crate::platform::paths::set_runtime_resource_dir(resource_dir);
            }
            let store = SessionStore::boot().context("boot headless session store")?;
            store.load_session_models();
            store.load_pinned_sessions();
            store.load_hidden_sessions();
            app.manage(store.clone());
            let pool = build_pool(app.handle().clone(), store.clone())?;
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let result = run_agentic_task(pool, store, request).await;
                let _ = result_tx.send(result);
                handle.exit(0);
            });
            Ok(())
        })
        .build(context)
        .context("build windowless Pinvou host")?;
    app.run_return(|_, _| {});
    result_rx
        .blocking_recv()
        .context("headless host exited before work completed")?
}

/// 驱动一次 agentic 轮次:绑定执行根 → 钉住 active model → Yolo 提交 →
/// 超时看护 → 收报告 → 清理临时会话。报告总是返回(内部错误进 `error`
/// 字段),只有宿主级故障才向上传播 `Err`。
///
/// 执行根 resolver 必须在 pool 进入 `Arc` 之前注册(bridge 的 setter
/// 需要 `&mut self`),因此本函数按值接收 `EnginePool`。
pub async fn run_agentic_task(
    pool: EnginePool,
    store: SessionStore,
    request: AgenticTaskRequest,
) -> Result<AgenticTaskReport> {
    let timeout_secs = request.timeout_secs.max(1);
    let session_id = fresh_session_id();

    // 执行根绑定:闭包只认本次会话 id,其余会话解析结果不变。
    let bound_workspace = request.workspace.clone();
    let matched_session = session_id.clone();
    let resolver: ExecutionRootResolver = Arc::new(move |id: &str| {
        (id == matched_session)
            .then(|| bound_workspace.clone())
            .flatten()
    });
    let mut pool = pool;
    pool.bridge.set_execution_root_resolver(resolver.clone());
    store.set_execution_root_resolver(resolver);
    let runtime = EnginePoolRuntime::new(Arc::new(pool));

    let outcome = run_turn(&runtime, &session_id, request, timeout_secs).await;

    // 无论成败都回收:引擎资源走 eval 清理通道,持久化会话删除,模型
    // suite pin 由 guard Drop 归还。
    runtime.schedule_eval_cleanup(&session_id);
    let _ = runtime.close_eval_session_result(&session_id).await;
    outcome
}

async fn run_turn(
    runtime: &EnginePoolRuntime,
    session_id: &str,
    request: AgenticTaskRequest,
    timeout_secs: u64,
) -> Result<AgenticTaskReport> {
    let guard = runtime
        .capture_eval_suite_model()
        .context("active evaluation model is not configured")?;
    let selection = guard.derive_case_selection()?;
    runtime
        .prepare(&SessionSpec {
            session_id: session_id.to_owned(),
            model_selection: Some(selection),
        })
        .await
        .context("prepare agentic session")?;
    let handle = runtime
        .submit(&TurnInput {
            session_id: session_id.to_owned(),
            content: request.prompt,
            mode: AppMode::Yolo,
            restrict_tools: false,
            eval_tool_policy: None,
        })
        .await
        .context("submit agentic turn")?;
    drop(guard);

    let mut timed_out = false;
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    while runtime.is_turn_active(session_id) {
        if Instant::now() >= deadline {
            timed_out = true;
            runtime.cancel(session_id).await;
            let settle = Instant::now() + Duration::from_secs(CANCEL_SETTLE_SECS);
            while runtime.is_turn_active(session_id) && Instant::now() < settle {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // `wait_for_completion` 内部是"轮次活跃即继续轮询"的无界等待;超时路径
    // 上取消可能无法让引擎真正停下,这里以沉淀窗口为界截断:取消后轮次仍
    // 活跃就放弃完整 TurnResult,直接产出超时报告,绝不无限等待。
    let turn_result = if timed_out && runtime.is_turn_active(session_id) {
        None
    } else {
        runtime.wait_for_completion(&handle).await.ok()
    };
    match turn_result {
        Some(turn) => Ok(AgenticTaskReport {
            session_id: session_id.to_owned(),
            status: if timed_out {
                "timeout".to_string()
            } else {
                turn.status
            },
            timed_out,
            assistant_text: turn.assistant_text,
            tool_events: turn
                .tool_events
                .into_iter()
                .map(|event| AgenticToolEvent {
                    name: event.name,
                    failed: event.failed,
                })
                .collect(),
            usage: turn.usage.map(|usage| AgenticUsageReport {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_hit_tokens: usage.cache_hit_tokens,
                cache_miss_tokens: usage.cache_miss_tokens,
                cache_write_tokens: usage.cache_write_tokens,
                reasoning_tokens: usage.reasoning_tokens,
                context_window: usage.context_window,
            }),
            error: turn.error,
        }),
        None => Ok(AgenticTaskReport {
            session_id: session_id.to_owned(),
            status: "timeout".to_string(),
            timed_out,
            assistant_text: String::new(),
            tool_events: Vec::new(),
            usage: None,
            error: Some("agent turn did not settle after cancel".to_string()),
        }),
    }
}

fn fresh_session_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "agentic_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::{
        AgenticTaskReport, AgenticTaskRequest, AgenticToolEvent, DEFAULT_TIMEOUT_SECS,
    };

    #[test]
    fn request_defaults_timeout_and_workspace() {
        let request: AgenticTaskRequest =
            serde_json::from_str(r#"{"prompt":"do it"}"#).unwrap();
        assert_eq!(request.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert!(request.workspace.is_none());

        let request: AgenticTaskRequest = serde_json::from_str(
            r#"{"prompt":"p","workspace":"/tmp/task","timeout_secs":42}"#,
        )
        .unwrap();
        assert_eq!(request.timeout_secs, 42);
        assert_eq!(
            request.workspace,
            Some(std::path::PathBuf::from("/tmp/task"))
        );
    }

    #[test]
    fn report_roundtrips_without_leaking_tool_payloads() {
        let report = AgenticTaskReport {
            session_id: "agentic_1_0".to_string(),
            status: "ok".to_string(),
            timed_out: false,
            assistant_text: "done".to_string(),
            tool_events: vec![AgenticToolEvent {
                name: "Bash".to_string(),
                failed: false,
            }],
            usage: None,
            error: None,
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"name\":\"Bash\""));
        assert!(!json.contains("secret"));
        let parsed: AgenticTaskReport = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, report);
    }
}
