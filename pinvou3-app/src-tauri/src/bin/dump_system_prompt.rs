//! 一次性 dump: 复现 pinvou3 新建 session 时,底座 `Engine::new` 在
//! `core/engine.rs:458` 拼出来的真实 system prompt。
//!
//! 不 spawn engine(避免连 vLLM/起 turn loop),只复刻 prompt 拼装环节:
//!   1. bridge.boot()                                  -> ensure_dirs + load prefs
//!   2. bridge.build_engine_config_for_session(sid)    -> EngineConfig(含 inline instructions)
//!   3. prompts::system_prompt_for_mode_with_context_skills_session_and_approval(...)
//!
//! 跑法:
//!   cargo run --bin dump_system_prompt \
//!     --manifest-path pinvou3-app/src-tauri/Cargo.toml \
//!     > /tmp/pinvou3_system_prompt.txt

use anyhow::Result;
use deepseek_tui::prompts::{self, PromptSessionContext};
use deepseek_tui::tui::app::AppMode;
use deepseek_tui::tui::approval::ApprovalMode;
use pinvou3_lib::features::assistant::platform::bridge::Pinvou3Bridge;

fn main() -> Result<()> {
    // session id 走临时值,避免污染真实 sessions/
    let sid = "__dump_system_prompt__";

    let bridge = Pinvou3Bridge::boot()?;
    let cfg = bridge.build_engine_config_for_session(sid);

    let (mode, approval) = match std::env::args().nth(1).as_deref() {
        Some("plan") => (AppMode::Plan, ApprovalMode::Never),
        Some("agent") => (AppMode::Agent, ApprovalMode::Suggest),
        _ => (AppMode::Yolo, ApprovalMode::Auto),
    };

    // CodeWhale 的原生记忆装配已收口到 Engine 内部。Pinvou 当前明确关闭该能力；
    // 若未来启用，必须先为此诊断工具提供与 Engine 等价的公开装配入口，避免静默漏项。
    anyhow::ensure!(
        !cfg.memory_enabled,
        "dump_system_prompt 尚不支持 CodeWhale 原生记忆，请先同步 Engine 的公开 prompt 装配入口"
    );

    let session_ctx = PromptSessionContext {
        user_memory_block: None,
        goal_objective: None, // Engine::new 里通过 goal_objective_for_prompt 算,新 session 无 goal => None
        project_context_pack_enabled: cfg.project_context_pack_enabled,
        locale_tag: &cfg.locale_tag,
        translation_enabled: cfg.translation_enabled,
        model_id: &cfg.model,
        // v0.8.57:上游把 allow_shell 从 PromptSessionContext 移除(#2949,decouple
        // 静态前缀,allow_shell 改走 per-turn <runtime_prompt> tag),dump 同步去掉。
        // context_window_override 当前不输出到 prompt；此工具不构造 API provider，故取 None。
        context_window_override: None,
        verbosity: cfg.verbosity.as_deref(),
        skills_scan_codewhale_only: cfg.skills_scan_codewhale_only,
        plugin_registry: cfg.plugin_registry.as_deref(),
        mode,
    };

    // CodeWhale 当前要求稳定前缀上下文携带 mode，因此保持 CLI 选择与上下文一致。
    // Pinvou 的静态 composer 目前仍统一输出 Execute 层，Plan 约束另由 per-turn reminder 注入。
    let prompt = prompts::system_prompt_for_mode_with_context_skills_session_and_approval(
        &cfg.workspace,
        None,
        Some(&cfg.skills_dir),
        Some(&cfg.instructions),
        session_ctx,
    );
    let text = prompts::system_prompt_flat_text(&prompt);

    // 打 dump 元数据 + body。v0.9.0 生产路径返回 Blocks，统一扁平化便于跨版本 diff。
    eprintln!("───────── dump meta ─────────");
    eprintln!("workspace    = {}", cfg.workspace.display());
    eprintln!("skills_dir   = {}", cfg.skills_dir.display());
    eprintln!("instructions = {:?}", cfg.instructions);
    eprintln!("locale_tag   = {}", cfg.locale_tag);
    eprintln!("model_id     = {}", cfg.model);
    eprintln!("approval     = {approval:?}");
    eprintln!("mode         = {mode:?}");
    eprintln!("byte_len     = {}", text.len());
    eprintln!("line_count   = {}", text.lines().count());
    eprintln!("─────────────────────────────");
    println!("{text}");
    Ok(())
}
