//! 一次性实证 harness(非 CI,#[ignore]):量化两条压缩线的实际交叉点。
//!
//! 背景见 docs/context-compaction-设计.md §6。两条线用不同的尺:
//!   - T(should_compact):对「可摘要子集」用 raw 尺(bytes÷4,无放大)
//!   - E(emergency):对「全量」用 conservative 尺(raw×1.5 + system÷3 + framing)
//! 本 harness 程序化对拍两把尺,定 T 公式常数 k/S/R/framing,不依赖真机/vLLM。
//!
//! 跑法:
//!   cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml \
//!       --test compaction_probe -- --ignored --nocapture --test-threads=1
//!
//! 前提:pinvou 关 thinking(会话无 Thinking block),故 should_compact 内部子集
//! 的 no-thinking 估算 == estimate_tokens(子集)(thinking-aware,仅带 tool_use 且
//! 含 Thinking block 时才有差),harness 用后者代替私有 estimate_tokens_for_message。

use deepseek_tui::compaction::{
    CompactionConfig, KEEP_RECENT_MESSAGES, estimate_input_tokens_conservative, estimate_tokens,
    plan_compaction, should_compact,
};
use deepseek_tui::models::{ContentBlock, Message, SystemPrompt};

fn text_block(bytes: usize) -> ContentBlock {
    ContentBlock::Text {
        text: "x".repeat(bytes),
        cache_control: None,
    }
}

fn msg(role: &str, blocks: Vec<ContentBlock>) -> Message {
    Message {
        role: role.to_string(),
        content: blocks,
    }
}

/// 一个真实形态轮单元(无 thinking):
///   user 提问(600B) + assistant(前言 400B + tool_use) + tool_result(截断后 8000B)
/// 底座对每个 tool_result 硬压 ≤12,000 字符,取典型 8,000。
fn push_turn(msgs: &mut Vec<Message>, i: usize) {
    msgs.push(msg("user", vec![text_block(600)]));
    msgs.push(msg(
        "assistant",
        vec![
            text_block(400),
            ContentBlock::ToolUse {
                id: format!("call_{i}"),
                name: "read_file".into(),
                input: serde_json::json!({"path": format!("data_{i}")}),
                caller: None,
            },
        ],
    ));
    msgs.push(msg(
        "tool",
        vec![ContentBlock::ToolResult {
            tool_use_id: format!("call_{i}"),
            content: "y".repeat(8000),
            is_error: None,
            content_blocks: None,
        }],
    ));
}

fn raw_of(msgs: &[Message], idxs: &[usize]) -> usize {
    let gathered: Vec<Message> = idxs.iter().map(|&i| msgs[i].clone()).collect();
    estimate_tokens(&gathered)
}

/// O = clamp(业务需求 24,576, 下限 6,144, 上界 W/4)
fn output_reservation(w: u32) -> u32 {
    24_576u32.min(w / 4).max(6_144)
}

const SYSTEM_BYTES: usize = 36_000; // 代表性 system prompt(→ conservative 12,000);真机 dump 后校准

/// 列出 pinvou 各云端 preset 默认模型名经 `context_window_for_model` 得到的窗口。
/// 云端不探测,derive 就用这个值;返回 None 的 → derive 兜底 128000。
#[test]
#[ignore]
fn cloud_model_windows() {
    use deepseek_tui::models::context_window_for_model;
    // (preset, 默认模型名)—— 见 bridge::model() 的 ModelPreset 分支
    let models = [
        ("Deepseek", "deepseek-v4-pro"),
        ("Kimi", "kimi-k2.6"),
        ("OpenaiCompatible", "gpt-4o"),
        ("Qwen", "qwen-max"),
        ("Doubao", "doubao-pro-256k"),
        ("Minimax", "abab6.5s-chat"),
        ("Glm", "glm-4-plus"),
        ("Mimo", "mimo-v2-flash"),
        ("LocalVllm(对照)", "qwen36_35b_256k"),
    ];
    eprintln!("\n==== 云端模型窗口(context_window_for_model)====");
    for (preset, m) in models {
        let w = context_window_for_model(m);
        let effective = w.unwrap_or(128_000);
        eprintln!(
            "  {preset:20} {m:20} → {:>10}  {}",
            w.map(|v| v.to_string()).unwrap_or_else(|| "None".into()),
            if w.is_none() {
                format!("→ derive 兜底 {effective}")
            } else {
                String::new()
            }
        );
    }
}

#[test]
#[ignore]
fn probe_crossover_constants() {
    let system = SystemPrompt::Text("s".repeat(SYSTEM_BYTES));
    let system_conservative = SYSTEM_BYTES / 3;

    eprintln!("\n==== 交叉点常数实证 (system_conservative={system_conservative}) ====");
    for &w in &[262_144u32, 131_072, 65_536] {
        let o = output_reservation(w);
        let e = (w - o - 1024) as usize;

        let mut msgs: Vec<Message> = Vec::new();
        for i in 0..5000 {
            push_turn(&mut msgs, i);
            let conservative = estimate_input_tokens_conservative(&msgs, Some(&system));
            if conservative >= e {
                let plan = plan_compaction(&msgs, None, KEEP_RECENT_MESSAGES, None, None);
                let subset = raw_of(&msgs, &plan.summarize_indices);
                let pinned = raw_of(
                    &msgs,
                    &plan.pinned_indices.iter().copied().collect::<Vec<_>>(),
                );
                let full_raw = estimate_tokens(&msgs);
                let n = msgs.len();
                let framing = n * 12 + 48;
                let k_eff = (conservative as f64 - system_conservative as f64 - framing as f64)
                    / full_raw as f64;
                eprintln!("\n──── W={w}  O={o}  E={e} ────");
                eprintln!("  到达 E 时: N={n} 条, 全量conservative={conservative}(≈E)");
                eprintln!("  全量 raw = {full_raw}");
                eprintln!("  system_conservative={system_conservative}  framing={framing}");
                eprintln!("  反推 k_eff = (E - S - framing)/全量raw = {k_eff:.3}");
                eprintln!("  可摘要子集 raw = {subset}  <= T 设此值则与 E 同时触发");
                eprintln!("  pinned(recent+query) raw = {pinned}  <= R");
                eprintln!("  建议 T ≈ {} (子集raw 留 15K margin)", subset.saturating_sub(15_000));
                if w == 262_144 {
                    eprintln!(
                        "  ▶ 190,000 判定: 子集raw={subset} {} 190000 → 写死190K {}",
                        if subset < 190_000 { "<" } else { ">=" },
                        if subset < 190_000 {
                            "在256K机从未在正常路径触发(坐实)"
                        } else {
                            "可触发"
                        }
                    );
                }
                break;
            }
        }
    }
}

#[test]
#[ignore]
fn probe_trigger_order() {
    let system = SystemPrompt::Text("s".repeat(SYSTEM_BYTES));

    eprintln!("\n==== 触发顺序实证(真实 should_compact) ====");
    for &w in &[262_144u32, 131_072] {
        let o = output_reservation(w);
        let e = (w - o - 1024) as usize;
        eprintln!("\n──── W={w}  E={e} ────");
        for t in [190_000usize, 130_000, 120_000, 90_000, 45_000] {
            if t as u32 >= w {
                continue;
            }
            let cfg = CompactionConfig {
                enabled: true,
                token_threshold: t,
                ..Default::default()
            };
            let mut msgs: Vec<Message> = Vec::new();
            let mut sc_at: Option<(usize, usize)> = None;
            let mut em_at: Option<(usize, usize)> = None;
            for i in 0..5000 {
                push_turn(&mut msgs, i);
                let conservative = estimate_input_tokens_conservative(&msgs, Some(&system));
                if sc_at.is_none() && should_compact(&msgs, &cfg, None, None, None) {
                    sc_at = Some((msgs.len(), conservative));
                }
                if em_at.is_none() && conservative > e {
                    em_at = Some((msgs.len(), conservative));
                }
                if sc_at.is_some() && em_at.is_some() {
                    break;
                }
            }
            let verdict = match (sc_at, em_at) {
                (Some((ns, _)), Some((ne, _))) if ns < ne => "✅ 正常线先(nice 活)",
                (Some(_), Some(_)) => "❌ 紧急线先(倒置)",
                (Some(_), None) => "✅ 只正常线触发",
                _ => "? 都没触发",
            };
            eprintln!("  T={t:>7}: should_compact 首触={sc_at:?}  emergency 首触={em_at:?}  {verdict}");
        }
    }
}
