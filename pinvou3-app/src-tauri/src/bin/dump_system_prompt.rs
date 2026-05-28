//! 一次性 dump: 复现 pinvou3 新建 session 时,底座 `Engine::new` 在
//! `core/engine.rs:458` 拼出来的真实 system prompt。
//!
//! 不 spawn engine(避免连 vLLM/起 turn loop),只复刻 prompt 拼装环节:
//!   1. bridge.boot()                                  -> ensure_dirs + load prefs
//!   2. bridge.write_session_instructions(sid)         -> 写 sessions/<sid>/instructions.md
//!   3. bridge.build_engine_config_for_session(sid)    -> EngineConfig (instructions/workspace/...)
//!   4. prompts::system_prompt_for_mode_with_context_skills_session_and_approval(...)
//!
//! 跑法:
//!   cargo run --bin dump_system_prompt \
//!     --manifest-path pinvou3-app/src-tauri/Cargo.toml \
//!     > /tmp/pinvou3_system_prompt.txt

use anyhow::Result;
use deepseek_tui::memory;
use deepseek_tui::models::SystemPrompt;
use deepseek_tui::prompts::{
    self, PromptSessionContext,
};
use deepseek_tui::tui::app::AppMode;
use deepseek_tui::tui::approval::ApprovalMode;
use pinvou3_lib::bridge::Pinvou3Bridge;

fn main() -> Result<()> {
    // session id 走临时值,避免污染真实 sessions/
    let sid = "__dump_system_prompt__";

    let bridge = Pinvou3Bridge::boot()?;
    bridge.write_session_instructions(sid)?;
    let cfg = bridge.build_engine_config_for_session(sid);

    // 复刻 Engine::new (core/engine.rs:454-475) 的入参装配
    let user_memory_block =
        memory::compose_block(cfg.memory_enabled, &cfg.memory_path);

    let session_ctx = PromptSessionContext {
        user_memory_block: user_memory_block.as_deref(),
        goal_objective: None, // Engine::new 里通过 goal_objective_for_prompt 算,新 session 无 goal => None
        project_context_pack_enabled: cfg.project_context_pack_enabled,
        locale_tag: &cfg.locale_tag,
        translation_enabled: cfg.translation_enabled,
        model_id: &cfg.model,
        show_thinking: cfg.show_thinking,
    };

    // session.approval_mode 默认 = Suggest (跟 default_approval_mode_for_mode(Agent) 一致)
    let prompt =
        prompts::system_prompt_for_mode_with_context_skills_session_and_approval(
            AppMode::Agent,
            &cfg.workspace,
            None,
            Some(&cfg.skills_dir),
            Some(&cfg.instructions),
            session_ctx,
            ApprovalMode::Suggest,
        );

    match prompt {
        SystemPrompt::Text(text) => {
            // 打 dump 元数据 + body
            eprintln!("───────── dump meta ─────────");
            eprintln!("workspace    = {}", cfg.workspace.display());
            eprintln!("skills_dir   = {}", cfg.skills_dir.display());
            eprintln!("instructions = {:?}", cfg.instructions);
            eprintln!("locale_tag   = {}", cfg.locale_tag);
            eprintln!("model_id     = {}", cfg.model);
            eprintln!("approval     = Suggest (Agent 默认)");
            eprintln!("mode         = Agent");
            eprintln!("show_thinking= {}", cfg.show_thinking);
            eprintln!("byte_len     = {}", text.len());
            eprintln!("line_count   = {}", text.lines().count());
            eprintln!("─────────────────────────────");
            println!("{text}");
        }
        other => {
            eprintln!("unexpected SystemPrompt variant: {other:?}");
            std::process::exit(2);
        }
    }
    Ok(())
}
