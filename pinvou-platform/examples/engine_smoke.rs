//! Phase 2 原型：验证 pinvou-platform 能 headless 调起 DeepSeek-TUI engine。
//!
//! 用法（先 export DEEPSEEK_* env 或 source run-local.sh 的 env 部分）:
//!   cargo run --example engine_smoke -- "你是谁"
//!
//! 期望：能拿到 vLLM 流式响应，5 秒左右出完整答案。
//! 如果失败：原因 = 重构有阻塞，要解决后才能继续 Phase 3。

use std::time::Duration;

use deepseek_tui::config::{ApiProvider, Config, ProvidersConfig};
use deepseek_tui::core::engine::{spawn_engine, EngineConfig};
use deepseek_tui::core::events::Event;
use deepseek_tui::core::ops::Op;
use deepseek_tui::tui::app::AppMode;
use deepseek_tui::tui::approval::ApprovalMode;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let user_message = std::env::args().nth(1).unwrap_or_else(|| "你是谁".to_string());

    // === 1. 构造 deepseek-tui Config（同 pinvou_platform::engine_factory 路径） ===
    let mut api_config = Config::default();
    if let Ok(p) = std::env::var("DEEPSEEK_PROVIDER") {
        api_config.provider = Some(p);
    }
    if let Ok(k) = std::env::var("DEEPSEEK_API_KEY") {
        api_config.api_key = Some(k);
    }
    if let Ok(k) = std::env::var("OPENAI_API_KEY") {
        // openai provider 读 OPENAI_API_KEY；保险起见也设
        api_config.api_key.get_or_insert(k);
    }
    if let Ok(url) = std::env::var("DEEPSEEK_BASE_URL") {
        let providers = api_config
            .providers
            .get_or_insert_with(ProvidersConfig::default);
        match api_config
            .provider
            .as_deref()
            .and_then(ApiProvider::parse)
            .unwrap_or(ApiProvider::Deepseek)
        {
            ApiProvider::Vllm => providers.vllm.base_url = Some(url),
            ApiProvider::Openai => providers.openai.base_url = Some(url),
            _ => api_config.base_url = Some(url),
        }
    }
    let model_name = std::env::var("DEEPSEEK_MODEL").unwrap_or_else(|_| "/model".to_string());
    api_config.default_text_model = Some(model_name.clone());

    eprintln!(
        "[smoke] api_provider={:?} base_url={} model={}",
        api_config.api_provider(),
        api_config.deepseek_base_url(),
        model_name
    );

    // === 2. 构造 EngineConfig（最小可行：workspace + model） ===
    let workspace = std::env::current_dir()?;
    let engine_config = EngineConfig {
        model: model_name.clone(),
        workspace: workspace.clone(),
        ..EngineConfig::default()
    };

    eprintln!("[smoke] workspace={}", workspace.display());

    // === 3. spawn engine ===
    let handle = spawn_engine(engine_config, &api_config);
    eprintln!("[smoke] engine spawned");

    // === 4. 发用户消息 ===
    let reasoning_effort = std::env::var("DEEPSEEK_REASONING_EFFORT").ok();
    let send_op = Op::SendMessage {
        content: user_message.clone(),
        mode: AppMode::Agent,
        model: model_name.clone(),
        goal_objective: None,
        reasoning_effort: reasoning_effort.clone(),
        reasoning_effort_auto: false,
        auto_model: false,
        allow_shell: false,
        trust_mode: false,
        auto_approve: true,
        approval_mode: ApprovalMode::Auto,
    };
    eprintln!(
        "[smoke] sending message: {:?} reasoning_effort={:?}",
        user_message, reasoning_effort
    );
    handle.send(send_op).await?;

    // === 5. 收事件 ===
    let mut rx = handle.rx_event.write().await;
    let t0 = std::time::Instant::now();
    let mut text_chars: usize = 0;
    let mut got_first_text_at: Option<Duration> = None;
    loop {
        let event = match tokio::time::timeout(Duration::from_secs(60), rx.recv()).await {
            Ok(Some(ev)) => ev,
            Ok(None) => {
                eprintln!("[smoke] event channel closed");
                break;
            }
            Err(_) => {
                eprintln!("[smoke] timed out waiting for event (60s)");
                break;
            }
        };
        let dt = t0.elapsed();
        match event {
            Event::MessageStarted { index } => {
                eprintln!("[smoke +{:?}] MessageStarted index={index}", dt);
            }
            Event::MessageDelta { content, .. } => {
                if got_first_text_at.is_none() {
                    got_first_text_at = Some(dt);
                    eprintln!("[smoke +{:?}] first MessageDelta", dt);
                }
                text_chars += content.chars().count();
                print!("{content}");
                use std::io::Write;
                std::io::stdout().flush().ok();
            }
            Event::MessageComplete { .. } => {
                eprintln!("\n[smoke +{:?}] MessageComplete", dt);
            }
            Event::ThinkingStarted { .. } => {
                eprintln!("[smoke +{:?}] ThinkingStarted", dt);
            }
            Event::ThinkingDelta { content, .. } => {
                eprintln!(
                    "[smoke +{:?}] ThinkingDelta (suppressed) chars={}",
                    dt,
                    content.chars().count()
                );
            }
            Event::ThinkingComplete { .. } => {
                eprintln!("[smoke +{:?}] ThinkingComplete", dt);
            }
            Event::ToolCallStarted { id, name, input } => {
                eprintln!(
                    "[smoke +{:?}] ToolCallStarted id={id} name={name} input={}",
                    dt,
                    serde_json::to_string(&input).unwrap_or_default()
                );
            }
            Event::ToolCallComplete { id, name, result } => {
                let summary = match &result {
                    Ok(r) => format!("OK len={}", r.content.len()),
                    Err(e) => format!("ERR {e:?}"),
                };
                eprintln!(
                    "[smoke +{:?}] ToolCallComplete id={id} name={name} {summary}",
                    dt
                );
            }
            Event::TurnStarted { turn_id } => {
                eprintln!("[smoke +{:?}] TurnStarted {turn_id}", dt);
            }
            Event::TurnComplete { usage, status, error } => {
                eprintln!(
                    "[smoke +{:?}] TurnComplete status={:?} error={:?} usage={{input={} output={}}}",
                    dt, status, error, usage.input_tokens, usage.output_tokens
                );
                eprintln!(
                    "[smoke] TOTAL: {:?}, first_text_at={:?}, text_chars={}",
                    dt, got_first_text_at, text_chars
                );
                break;
            }
            Event::Error { envelope, .. } => {
                eprintln!("[smoke +{:?}] Error envelope={:?}", dt, envelope);
                break;
            }
            other => {
                eprintln!("[smoke +{:?}] (other event: {:?})", dt, std::mem::discriminant(&other));
            }
        }
    }

    // 让 engine 优雅退出
    let _ = handle.send(Op::Shutdown).await;
    Ok(())
}
