//! Smoke test: load real prompts/ directory and print discovered agents.
//!
//! Usage:
//!   cargo run --example agent_smoke
//!
//! Expects to run from the workspace root.

use pinvou_platform::agent_registry::AgentRegistry;
use pinvou_platform::combined_planner::CombinedPlanner;

fn main() {
    let reg = AgentRegistry::from_directory("prompts")
        .expect("failed to load prompts/ directory");

    println!("Loaded {} agents:", reg.len());
    for a in reg.iter() {
        let emoji = a.emoji.as_deref().unwrap_or("•");
        let body_len = a.body.chars().count();
        println!(
            "  {} {} ({}): {} [body: {} chars]",
            emoji, a.id, a.name, a.description, body_len
        );
    }

    println!("\n--- planner agent list (rendered for LLM) ---");
    println!("{}", reg.render_for_planner());

    println!("\n--- combined planner prompt sample (no tools available) ---");
    let prompt = CombinedPlanner::build_prompt("帮我写本周周报", &reg, &[]);
    println!("{}", prompt);

    println!("\n--- combined planner prompt sample (request_user_input only) ---");
    let tools = vec!["request_user_input".to_string()];
    let prompt = CombinedPlanner::build_prompt("帮我写本周周报", &reg, &tools);
    println!("{}", prompt);
}
