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
use deepseek_tui::memory;
use deepseek_tui::prompts::{self, PromptSessionContext};
use deepseek_tui::tui::app::AppMode;
use deepseek_tui::tui::approval::ApprovalMode;
use pinvou3_lib::features::assistant::platform::bridge::Pinvou3Bridge;

fn main() -> Result<()> {
    // session id 走临时值,避免污染真实 sessions/
    let sid = "__dump_system_prompt__";

    let bridge = Pinvou3Bridge::boot()?;
    let cfg = bridge.build_engine_config_for_session(sid);

    // 复刻 Engine::new (core/engine.rs:454-475) 的入参装配
    let user_memory_block = memory::compose_block(cfg.memory_enabled, &cfg.memory_path);

    let session_ctx = PromptSessionContext {
        user_memory_block: user_memory_block.as_deref(),
        goal_objective: None, // Engine::new 里通过 goal_objective_for_prompt 算,新 session 无 goal => None
        project_context_pack_enabled: cfg.project_context_pack_enabled,
        locale_tag: &cfg.locale_tag,
        translation_enabled: cfg.translation_enabled,
        model_id: &cfg.model,
        show_thinking: cfg.show_thinking,
        // v0.8.57:上游把 allow_shell 从 PromptSessionContext 移除(#2949,decouple
        // 静态前缀,allow_shell 改走 per-turn <runtime_prompt> tag),dump 同步去掉。
        // v0.8.60 上游新增字段:
        //   context_window_override: None = 用 model_id 派生的默认窗口(qwen36 → 256K)。
        //   verbosity: None = GUI 不用 concise 输出模式(与 bridge Op::SendMessage 一致)。
        context_window_override: None,
        verbosity: None,
        // v0.8.65 上游新增(C5):skills 扫描 CodeWhale-only 开关。pinvou3 #41 已把扫描
        // 收窄成只 ~/.pinvou3/bundle/skills(技能市场私有区,两 mode 行为一致),此值无
        // 实际影响,取 false(Compatible)。
        skills_scan_codewhale_only: false,
    };

    // v0.8.57:上游把 system prompt 改成 **mode-independent**(mode/approval 移出静态前缀,
    // 函数签名去掉这两个参数)。pinvou3 的 composer 在 apply_static_prompt_composer 里以
    // 常量 Yolo/Auto 构造宽 ctx → dump 出来的就是生产 Yolo(Execute) 静态 prompt,与 `plan`/
    // `agent` 参数无关(仅用于下方 meta 展示)。
    let (mode, approval) = match std::env::args().nth(1).as_deref() {
        Some("plan") => (AppMode::Plan, ApprovalMode::Never),
        Some("agent") => (AppMode::Agent, ApprovalMode::Suggest),
        _ => (AppMode::Yolo, ApprovalMode::Auto),
    };
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
    eprintln!("show_thinking= {}", cfg.show_thinking);
    eprintln!("byte_len     = {}", text.len());
    eprintln!("line_count   = {}", text.lines().count());
    eprintln!("─────────────────────────────");
    println!("{text}");
    Ok(())
}
