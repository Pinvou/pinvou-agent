//! L1 真 vLLM dialog harness。
//!
//! 直接用 bridge + engine 跑端到端对话，断言 LLM 工具调用 / 落盘文件 / 输出关键词,
//! 防本轮 INSTRUCTIONS_MD / bridge / blocklist 修改后 quality 静默回归。
//!
//! 所有 scenario 标 `#[ignore]`,默认 `cargo test` 不跑 (不阻塞 PR)。跑法:
//!
//! ```text
//! cargo test --test l1_dialog_harness -- --ignored --test-threads=1
//! ```
//!
//! pre-flight 健康探针:vLLM `/v1/models` 200 OK 才执行 scenario,
//! 不在线 → eprintln + return (退出码 0,nightly 不告警)。
//!
//! 设计与决策见 `docs/自动化测试方案.md` §3。

#![allow(dead_code)] // 框架辅助函数会在后续 scenario 里逐步消化

use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use deepseek_tui::core::events::Event;
use deepseek_tui::tui::app::AppMode;
use pinvou3_lib::bridge::mode_state::PlanPhase;
use pinvou3_lib::bridge::Pinvou3Bridge;
use pinvou3_lib::engine::AppEngine;

/// 每个 Expect 自动套这组拒绝词,防 LLM "我无法/抱歉" 这类伪绿场景被关键词
/// 正向匹配蒙混过关。
const DEFAULT_OUTPUT_NEVER: &[&str] = &[
    "我无法",
    "抱歉",
    "I cannot",
    "I'm sorry",
    "我不会",
    "无法获取",
    "无法访问",
    "Sorry, I",
];

const DEFAULT_VLLM_BASE_URL: &str = "http://10.214.74.113:8000/v1";

/// 健康探针:vLLM `/v1/models` 3s 内 200 OK 才视为在线。
async fn vllm_alive() -> bool {
    let base = std::env::var("DEEPSEEK_BASE_URL").unwrap_or_else(|_| DEFAULT_VLLM_BASE_URL.into());
    let probe = format!("{}/models", base.trim_end_matches('/'));
    match tokio::time::timeout(Duration::from_secs(3), reqwest::get(&probe)).await {
        Ok(Ok(resp)) => resp.status().is_success(),
        _ => false,
    }
}

/// pre-flight 包装:vLLM 不在线时 eprintln + return,scenario 当作 skip 处理。
/// 返回 true 表示在线,scenario 应继续。
async fn require_vllm(scenario_name: &str) -> bool {
    if vllm_alive().await {
        return true;
    }
    eprintln!(
        "SKIP {scenario_name}: vLLM endpoint unreachable (set DEEPSEEK_BASE_URL or check {DEFAULT_VLLM_BASE_URL})",
    );
    false
}

/// 隔离 scenario 用的 tempdir:`/tmp/pinvou3-l1-<ns>-<scenario>/`。
/// 用纳秒时间戳确保并发不冲突 (即便 --test-threads=1)。
fn make_scenario_tempdir(scenario: &str) -> PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("pinvou3-l1-{ns}-{scenario}"));
    std::fs::create_dir_all(&p).expect("create scenario tempdir");
    p
}

/// 收集 scenario 一次 turn 完整事件流,直到 `Event::TurnComplete` 出现。
/// 返回 (timeline=(t_sec, event)*, elapsed, timed_out)。
/// t_sec 是相对 turn start 的秒数,judge transcript 渲染要用。
async fn collect_turn_events(
    engine: &AppEngine,
    timeout: Duration,
) -> (Vec<(f64, Event)>, Duration, bool) {
    let start = Instant::now();
    let mut timeline = Vec::new();
    let mut rx = engine.handle.rx_event.write().await;
    let mut timed_out = false;
    let mut closed = false;
    loop {
        let remaining = timeout.checked_sub(start.elapsed()).unwrap_or_default();
        if remaining.is_zero() {
            timed_out = true;
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(event)) => {
                let t = start.elapsed().as_secs_f64();
                // ApprovalRequired:headless harness 没有 event_forwarder 的
                // auto-approve task,需要在这里主动调 approve_tool_call。
                // 上游 trust_mode/auto_approve 不旁路 await_tool_approval(已知
                // bug,见 engine.rs:298-300 注释)。
                if let Event::ApprovalRequired { ref id, .. } = event {
                    let h = engine.handle.clone();
                    let id_clone = id.clone();
                    tokio::spawn(async move {
                        if let Err(e) = h.approve_tool_call(id_clone).await {
                            eprintln!("[harness] approve_tool_call failed: {e:?}");
                        }
                    });
                }
                if matches!(event, Event::Error { .. }) {
                    eprintln!("[harness +{t:.1}s] engine Error event: {:?}", event);
                }
                let is_done = matches!(event, Event::TurnComplete { .. } | Event::Error { .. });
                timeline.push((t, event));
                if is_done {
                    break;
                }
            }
            Ok(None) => {
                closed = true;
                break;
            }
            Err(_) => {
                timed_out = true;
                break;
            }
        }
    }
    if closed {
        eprintln!("[harness] rx_event channel closed (engine task exited?)");
    }
    (timeline, start.elapsed(), timed_out)
}

fn event_kind(e: &Event) -> &'static str {
    match e {
        Event::MessageDelta { .. } => "MessageDelta",
        Event::ThinkingDelta { .. } => "ThinkingDelta",
        Event::ToolCallStarted { .. } => "ToolCallStarted",
        Event::ToolCallComplete { .. } => "ToolCallComplete",
        Event::TurnComplete { .. } => "TurnComplete",
        Event::Error { .. } => "Error",
        Event::ApprovalRequired { .. } => "ApprovalRequired",
        Event::UserInputRequired { .. } => "UserInputRequired",
        Event::CompactionStarted { .. } => "CompactionStarted",
        Event::CompactionCompleted { .. } => "CompactionCompleted",
        Event::CompactionFailed { .. } => "CompactionFailed",
        _ => "OtherEvent",
    }
}

/// 单 scenario 期望项:工具计数 / 文件落地 / 输出关键词 / 时长上限。
#[derive(Default)]
struct Expect {
    /// 工具名 → 允许的调用次数区间 (闭区间,inclusive)
    tool_use_counts: HashMap<&'static str, RangeInclusive<usize>>,
    /// 永不调用的工具
    tools_never: Vec<&'static str>,
    /// 必须落盘存在的路径
    files_exist: Vec<PathBuf>,
    /// 输出 (concat 全部 MessageDelta) 含任一关键词即 pass
    output_contains_any: Vec<&'static str>,
    /// 调用方追加的 NEVER 词;DEFAULT_OUTPUT_NEVER 已自动套上
    output_never_extra: Vec<&'static str>,
    /// turn 上限秒
    max_duration_s: f64,
}

/// scenario 跑完后聚合结果:文本 / 工具调用直方图 / 时长。
struct TurnSummary {
    /// 所有 MessageDelta.content 串起来 (LLM 的纯文本输出)
    full_text: String,
    /// 工具名 → 成功完成的调用次数
    tool_call_counts: HashMap<String, usize>,
    elapsed: Duration,
    timed_out: bool,
}

fn summarize(timeline: &[(f64, Event)], elapsed: Duration, timed_out: bool) -> TurnSummary {
    let mut full_text = String::new();
    let mut tool_call_counts: HashMap<String, usize> = HashMap::new();
    for (_t, e) in timeline {
        match e {
            Event::MessageDelta { content, .. } => full_text.push_str(content),
            Event::ToolCallComplete { name, result, .. } => {
                if result.is_ok() {
                    *tool_call_counts.entry(name.clone()).or_insert(0) += 1;
                }
            }
            _ => {}
        }
    }
    TurnSummary {
        full_text,
        tool_call_counts,
        elapsed,
        timed_out,
    }
}

/// 验证 Expect。失败 panic 让 #[test] fail。
fn verify_expect(summary: &TurnSummary, expect: &Expect, scenario: &str) {
    assert!(
        !summary.timed_out,
        "[{scenario}] turn 超时,可能 vLLM 慢/卡死 (elapsed={:?})",
        summary.elapsed
    );

    // tool_use_counts
    for (tool, range) in &expect.tool_use_counts {
        let count = summary.tool_call_counts.get(*tool).copied().unwrap_or(0);
        assert!(
            range.contains(&count),
            "[{scenario}] tool {tool} 调用次数 {count} 不在期望区间 {range:?}, \
             histogram={:?}",
            summary.tool_call_counts
        );
    }

    // tools_never
    for tool in &expect.tools_never {
        let count = summary.tool_call_counts.get(*tool).copied().unwrap_or(0);
        assert_eq!(
            count, 0,
            "[{scenario}] tool {tool} 不应被调用,实际 {count} 次, histogram={:?}",
            summary.tool_call_counts
        );
    }

    // files_exist
    for p in &expect.files_exist {
        assert!(p.is_file(), "[{scenario}] 期望文件不存在: {}", p.display());
    }

    // output_contains_any
    if !expect.output_contains_any.is_empty() {
        let hit = expect
            .output_contains_any
            .iter()
            .any(|kw| summary.full_text.contains(kw));
        assert!(
            hit,
            "[{scenario}] output 不含任何关键词 {:?}, output 前 200 字: {}",
            expect.output_contains_any,
            summary.full_text.chars().take(200).collect::<String>()
        );
    }

    // 拒绝词兜底 (DEFAULT + extra)
    for never in DEFAULT_OUTPUT_NEVER
        .iter()
        .chain(expect.output_never_extra.iter())
    {
        assert!(
            !summary.full_text.contains(never),
            "[{scenario}] output 含拒绝词 {never:?} (LLM 拒答?), output 前 200 字: {}",
            summary.full_text.chars().take(200).collect::<String>()
        );
    }

    // max_duration_s
    if expect.max_duration_s > 0.0 {
        let actual = summary.elapsed.as_secs_f64();
        assert!(
            actual <= expect.max_duration_s,
            "[{scenario}] 耗时 {actual:.1}s 超过上限 {:.1}s",
            expect.max_duration_s
        );
    }
}

/// 跑一轮对话 + 落 transcript + 验证。出错 panic (transcript 已先落档可复盘)。
async fn run_turn(
    engine: &AppEngine,
    user: &str,
    mode: AppMode,
    phase: PlanPhase,
    expect: &Expect,
    scenario: &str,
    turn_timeout: Duration,
) {
    engine
        .send_user_message(user.to_string(), mode, phase)
        .await
        .expect("send_user_message");
    let (timeline, elapsed, timed_out) = collect_turn_events(engine, turn_timeout).await;
    let summary = summarize(&timeline, elapsed, timed_out);
    eprintln!(
        "[{scenario}] elapsed={:.1}s tools={:?} text_len={}",
        summary.elapsed.as_secs_f64(),
        summary.tool_call_counts,
        summary.full_text.chars().count(),
    );
    // 先落 transcript 再 verify_expect:即便断言失败,judge 也能复盘
    let path = record_transcript(scenario, user, mode, phase, &timeline, &summary);
    eprintln!("[{scenario}] transcript → {}", path.display());
    verify_expect(&summary, expect, scenario);
}

/// 同一次 `cargo test` 跑下所有 scenario 共享一个 ts 子目录。
static RUN_TS: OnceLock<String> = OnceLock::new();
fn run_ts() -> &'static str {
    RUN_TS.get_or_init(|| {
        let s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        format!("{s}")
    })
}

fn transcript_dir() -> PathBuf {
    let dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"))
        .join("l1-runs")
        .join(run_ts());
    std::fs::create_dir_all(&dir).expect("create transcript dir");
    dir
}

/// 把 scenario 一次 turn 的完整 transcript 落 markdown,供 judge (Claude) 离线评分。
/// 路径:`<target>/l1-runs/<ts>/<scenario>.md`。
/// 跟 cargo test PASS/FAIL 完全解耦——质量评估是另一回事。
fn record_transcript(
    scenario: &str,
    user: &str,
    mode: AppMode,
    phase: PlanPhase,
    timeline: &[(f64, Event)],
    summary: &TurnSummary,
) -> PathBuf {
    let path = transcript_dir().join(format!("{scenario}.md"));
    let mut md = String::new();
    md.push_str(&format!("# L1 scenario: `{scenario}`\n\n"));
    md.push_str("## meta\n\n");
    md.push_str(&format!("- mode: `{mode:?}` / phase: `{phase:?}`\n"));
    md.push_str(&format!(
        "- elapsed: **{:.1}s**\n",
        summary.elapsed.as_secs_f64()
    ));
    md.push_str(&format!("- timed_out: {}\n", summary.timed_out));
    md.push_str(&format!(
        "- tool_call_histogram: `{:?}`\n",
        summary.tool_call_counts
    ));
    md.push_str(&format!(
        "- text_chars: {}\n\n",
        summary.full_text.chars().count()
    ));

    md.push_str("## user prompt\n\n```text\n");
    md.push_str(user);
    if !user.ends_with('\n') {
        md.push('\n');
    }
    md.push_str("```\n\n");

    md.push_str("## tool / event timeline\n\n");
    let rendered = render_timeline(timeline);
    if rendered.is_empty() {
        md.push_str("_(no tool/event activity)_\n\n");
    } else {
        md.push_str(&rendered);
        md.push('\n');
    }

    md.push_str("## assistant final text\n\n");
    if summary.full_text.is_empty() {
        md.push_str("_(empty)_\n");
    } else {
        md.push_str("```\n");
        md.push_str(summary.full_text.trim_end());
        md.push_str("\n```\n");
    }

    std::fs::write(&path, md).expect("write transcript md");
    path
}

fn render_timeline(timeline: &[(f64, Event)]) -> String {
    let mut s = String::new();
    for (t, e) in timeline {
        match e {
            Event::ToolCallStarted { id, name, input } => {
                let args = abbreviate(&format!("{input:?}"), 200);
                s.push_str(&format!(
                    "- `[+{t:.1}s]` **tool_start** `{name}` id=`{id}` args=`{args}`\n"
                ));
            }
            Event::ToolCallComplete { id, name, result } => {
                let (status, body) = match result {
                    Ok(r) => ("ok", abbreviate(&r.content, 200)),
                    Err(e) => ("err", abbreviate(&format!("{e:?}"), 200)),
                };
                s.push_str(&format!(
                    "- `[+{t:.1}s]` **tool_end** `{name}` id=`{id}` → **{status}** `{body}`\n"
                ));
            }
            Event::ApprovalRequired { id, tool_name, .. } => {
                s.push_str(&format!(
                    "- `[+{t:.1}s]` approval_required `{tool_name}` id=`{id}` (harness auto-approve)\n"
                ));
            }
            Event::UserInputRequired { id, .. } => {
                s.push_str(&format!(
                    "- `[+{t:.1}s]` user_input_required id=`{id}` (headless harness 不处理)\n"
                ));
            }
            Event::TurnComplete {
                usage,
                status,
                error,
            } => {
                let extra = error
                    .as_ref()
                    .map(|e| format!(" error={e}"))
                    .unwrap_or_default();
                s.push_str(&format!(
                    "- `[+{t:.1}s]` **turn_complete** status={status:?} usage=in:{}/out:{}{extra}\n",
                    usage.input_tokens, usage.output_tokens
                ));
            }
            Event::Error { envelope, .. } => {
                s.push_str(&format!(
                    "- `[+{t:.1}s]` **ERROR** {}: {}\n",
                    envelope.code, envelope.message
                ));
            }
            Event::CompactionStarted { message, auto, .. } => {
                s.push_str(&format!(
                    "- `[+{t:.1}s]` compaction start auto={auto} {message}\n"
                ));
            }
            Event::CompactionCompleted {
                messages_before,
                messages_after,
                ..
            } => {
                s.push_str(&format!(
                    "- `[+{t:.1}s]` compaction done {messages_before:?}→{messages_after:?}\n"
                ));
            }
            Event::CompactionFailed { message, .. } => {
                s.push_str(&format!("- `[+{t:.1}s]` compaction failed: {message}\n"));
            }
            // MessageDelta / ThinkingDelta 不入 timeline (累积在 full_text)
            _ => {}
        }
    }
    s
}

fn abbreviate(s: &str, max: usize) -> String {
    let total = s.chars().count();
    if total <= max {
        s.replace('`', "´").replace('\n', "⏎")
    } else {
        let head: String = s.chars().take(max).collect();
        format!(
            "{}…[{total} chars total]",
            head.replace('`', "´").replace('\n', "⏎")
        )
    }
}

/// 启动 scenario engine:tempdir workspace + headless engine。
async fn spawn_for_scenario(scenario: &str) -> (AppEngine, PathBuf) {
    ensure_runtime_env();
    let ws = make_scenario_tempdir(scenario);
    let bridge = Pinvou3Bridge::boot_with_workspace(ws.clone()).expect("boot bridge");
    let engine = AppEngine::spawn_headless(bridge)
        .await
        .expect("spawn engine");
    (engine, ws)
}

/// 设置 deepseek-tui engine 起跑所需的 env (复制 run-dev.sh 的关键变量)。
/// 用 `set_var_if_unset` 让外部 export 优先,允许 CI/本地切换 endpoint。
fn ensure_runtime_env() {
    set_var_if_unset("DEEPSEEK_PROVIDER", "vllm");
    set_var_if_unset("DEEPSEEK_API_KEY", "local-no-auth");
    set_var_if_unset("DEEPSEEK_BASE_URL", DEFAULT_VLLM_BASE_URL);
    set_var_if_unset("DEEPSEEK_MODEL", "/model");
    set_var_if_unset("DEEPSEEK_REASONING_EFFORT", "off");
    // 关键:vLLM 在 10.214.74.113 不是 loopback,底座默认拒绝非 loopback HTTP
    set_var_if_unset("DEEPSEEK_ALLOW_INSECURE_HTTP", "1");
    set_var_if_unset("DEEPSEEK_FORCE_HTTP1", "1");
    set_var_if_unset("DEEPSEEK_MAX_OUTPUT_TOKENS", "16384");
    set_var_if_unset("DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS", "90");
}

fn set_var_if_unset(k: &str, v: &str) {
    if std::env::var_os(k).is_none() {
        std::env::set_var(k, v);
    }
}

// ============================================================================
// Scenarios
// ============================================================================

/// Harness sanity:vLLM 探针 + bridge/engine 能 boot。不调 LLM,~1s。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "L1 真 vLLM 端到端,默认不跑"]
async fn l1_health_and_boot() {
    if !require_vllm("l1_health_and_boot").await {
        return;
    }
    let (_engine, ws) = spawn_for_scenario("health_and_boot").await;
    assert!(ws.is_dir());
    eprintln!("[health_and_boot] ws={} OK", ws.display());
}

/// MVP 1: 简单翻译任务,LLM 必须**纯文本回答,不调任何工具**。
/// 防 INSTRUCTIONS_MD 引导过激,让 AI 把"翻译这句话"也理解成"先 list_dir 探环境"。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "L1 真 vLLM 端到端,默认不跑"]
async fn translate_no_tool() {
    let scenario = "translate_no_tool";
    if !require_vllm(scenario).await {
        return;
    }
    let (engine, _ws) = spawn_for_scenario(scenario).await;

    let mut expect = Expect::default();
    expect.tools_never = vec![
        "list_dir",
        "read_file",
        "write_file",
        "exec_shell",
        "web_search",
        "code_execution",
    ];
    // 期望含 local 或 AI 这种翻译常见词 (Qwen3.6 翻译质量稳定)
    expect.output_contains_any = vec!["local", "AI", "Local", "test"];
    expect.max_duration_s = 30.0;

    run_turn(
        &engine,
        "把这句话翻译成英文,只回译文,不要解释:我们正在测试一个本地部署的 AI 助手。",
        AppMode::Yolo,
        PlanPhase::None,
        &expect,
        scenario,
        Duration::from_secs(40),
    )
    .await;
}

/// MVP 2: 一 turn 内连续 7 次 write_file。
/// 防 OpenAI streaming batch tool_calls bug 回归 (单 slot current_tool_index
/// 被覆盖,导致 7 个 tool_use 只剩 1 进 messages,产物面板少 6 个卡片)。
/// 详见 docs/自动化测试方案.md §3.4 + PR #1686。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "L1 真 vLLM 端到端,默认不跑"]
async fn batch_create_7_files() {
    let scenario = "batch_create_7_files";
    if !require_vllm(scenario).await {
        return;
    }
    let (engine, ws) = spawn_for_scenario(scenario).await;
    let ws_str = ws.to_string_lossy().to_string();

    let mut expect = Expect::default();
    // 7 次成功 write_file (允许 7..=10,LLM 可能多调一次列目录等)
    expect.tool_use_counts.insert("write_file", 7..=10);
    // 7 个文件都必须落盘
    for i in 1..=7 {
        expect.files_exist.push(ws.join(format!("{i}.md")));
    }
    // LLM 可能在最后说"完成"等,这里不强制关键词
    expect.max_duration_s = 180.0;

    let user = format!(
        "在目录 {ws_str} 下创建 7 个 markdown 文件,文件名分别是 1.md 到 7.md。\
         每个文件内容只有一行:它的文件名 (例如 1.md 的内容是 `1.md`)。\
         **必须用 write_file 工具一次完成全部 7 个文件,不要分多轮**,\
         也不要先调 list_dir/exec_shell 探目录,目录已经存在。"
    );

    run_turn(
        &engine,
        &user,
        AppMode::Yolo,
        PlanPhase::None,
        &expect,
        scenario,
        Duration::from_secs(200),
    )
    .await;
}

/// MVP 3: Plan 模式调 list_dir 跨 workspace 边界 (`/tmp` 不在 session
/// workspace 内)。
/// 防 trust_mode=false 引发 PathEscape 报错回归 (P1 修复点)。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "L1 真 vLLM 端到端,默认不跑"]
async fn plan_mode_list_dir() {
    let scenario = "plan_mode_list_dir";
    if !require_vllm(scenario).await {
        return;
    }
    let (engine, _ws) = spawn_for_scenario(scenario).await;

    let mut expect = Expect::default();
    expect.tool_use_counts.insert("list_dir", 1..=10);
    // Plan 模式禁写工具,sandbox + 工具集双重保险
    expect.tools_never = vec!["write_file", "edit_file"];
    // 关键:不能出现 PathEscape / permission denied 这类 trust_mode 拒绝词
    expect.output_never_extra = vec!["PathEscape", "permission denied"];
    expect.max_duration_s = 180.0;

    run_turn(
        &engine,
        "我想了解 /tmp 目录里有什么。先用 list_dir 工具列一下,然后用 update_plan \
         给我一个简短的整理方案 (3-5 步即可)。",
        AppMode::Plan,
        PlanPhase::Planning,
        &expect,
        scenario,
        Duration::from_secs(200),
    )
    .await;
}

/// MVP 4: 让 AI 写到 `/tmp/<unique>.md`,验证落盘成功。
/// 防 deepseek-tui 端 trust_mode/sandbox 配置或 INSTRUCTIONS_MD workspace 引导
/// 把 AI "锁死"在某个特定子目录回归(A 方案放宽允许 /tmp 等用户授权位置)。
/// validate_user_path 自身的边界由 L2 commands::tests 覆盖。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "L1 真 vLLM 端到端,默认不跑"]
async fn save_to_tmp_no_validate_fail() {
    let scenario = "save_to_tmp_no_validate_fail";
    if !require_vllm(scenario).await {
        return;
    }
    let (engine, _ws) = spawn_for_scenario(scenario).await;
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let target = std::env::temp_dir().join(format!("pinvou3-l1-tmp-save-{ns}.md"));
    // 清理可能的残留
    let _ = std::fs::remove_file(&target);

    let mut expect = Expect::default();
    expect.tool_use_counts.insert("write_file", 1..=3);
    expect.files_exist = vec![target.clone()];
    expect.max_duration_s = 120.0;

    let prompt = format!(
        "用 write_file 工具创建文件 {} ,内容是 `# pinvou3 测试`(只这一行)。\
         不要先 list_dir 探目录,目录 /tmp 已经存在。",
        target.display()
    );

    run_turn(
        &engine,
        &prompt,
        AppMode::Yolo,
        PlanPhase::None,
        &expect,
        scenario,
        Duration::from_secs(150),
    )
    .await;

    // cleanup
    let _ = std::fs::remove_file(&target);
}

/// MVP 5: 简单单 turn 必须 < 15s (LLM 没工具调用,不应该 thinking)。
/// 防 reasoning_effort=off 失效或 prefill 变长拖慢响应回归。
/// thinking 没关时 Qwen3.6 单 turn 可达 30s+,差 2 倍以上易判别。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "L1 真 vLLM 端到端,默认不跑"]
async fn reasoning_off_speed() {
    let scenario = "reasoning_off_speed";
    if !require_vllm(scenario).await {
        return;
    }
    let (engine, _ws) = spawn_for_scenario(scenario).await;

    let mut expect = Expect::default();
    expect.tools_never = vec![
        "list_dir",
        "read_file",
        "write_file",
        "exec_shell",
        "code_execution",
        "web_search",
    ];
    expect.max_duration_s = 15.0;

    run_turn(
        &engine,
        "用一句话回答:Python 列表去重最简单的方式是什么?",
        AppMode::Yolo,
        PlanPhase::None,
        &expect,
        scenario,
        Duration::from_secs(30),
    )
    .await;
}
