//! 专家面具池（卡片池）：移植自 pinvou2 的 AgentPool（agent-market）。
//!
//! 设计要点：
//! - **只读静态数据**：1078 张专家卡是 immutable 资源，编译期 `include_str!` 内嵌，
//!   首次访问时一次性解析进 `OnceLock`，不落盘、不解包到 `~/.pinvou3/`。
//! - **加持 = persona（WHO），非 skill（HOW）**：选中一张卡 → per-session 存 persona_id →
//!   `build_send_message_op` 每 turn 把 [`equip_reminder`] 注入 `<system-reminder>`，
//!   让主 agent 以这位专家的视角持续回应，直到用户摘下面具。
//!   这是粘性身份，故走 per-turn 注入（同 super_permission），不走 skill 的一次性 prepend。
//! - **数据瘦身**：pinvou2 原始 agent-market.v0.3.json 2.5MB（含 embeddings 引用、
//!   dim_scores 等），这里只保留前端卡片 + 详情 + 加持 reminder 需要的字段（~950KB）。

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// 编译期内嵌的专家卡数据（瘦身版）。源自 pinvou2 exec/executors/data/agent-market.v0.3.json。
const PERSONAS_JSON: &str = include_str!("../resources/bundle/personas/personas.v0.3.json");

/// 单张专家卡。字段对齐前端卡片网格 / facet 过滤 / 详情 modal / 加持 reminder 所需。
/// 所有 string 字段在生成瘦身 json 时已把 null 归一成 ""，故可用裸 String + serde(default)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaCard {
    pub id: String,
    pub name: String,
    pub cn_name: String,
    pub emoji: String,
    pub color: String,
    #[serde(default)]
    pub vibe: String,
    pub short_desc: String,
    #[serde(default)]
    pub description: String,
    /// 一级领域 code（engineering/business/design/marketing/quality/product/ai-data/games）。
    pub l1: String,
    #[serde(default)]
    pub l2: String,
    #[serde(default)]
    pub l3: String,
    /// 档位 A/B/C。
    pub tier: String,
    pub final_score: f64,
    #[serde(default)]
    pub strengths: Vec<String>,
    #[serde(default)]
    pub unique_selling_point: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PersonaPoolFile {
    agents: Vec<PersonaCard>,
}

static POOL: OnceLock<Vec<PersonaCard>> = OnceLock::new();

/// 全部专家卡（首次调用时解析内嵌 json，之后零成本）。
pub fn all() -> &'static [PersonaCard] {
    POOL.get_or_init(|| {
        serde_json::from_str::<PersonaPoolFile>(PERSONAS_JSON)
            .map(|f| f.agents)
            .unwrap_or_else(|e| {
                eprintln!("[pinvou3-app] persona pool 解析失败: {e}");
                Vec::new()
            })
    })
}

/// 按 id 查一张卡。
pub fn get(id: &str) -> Option<&'static PersonaCard> {
    all().iter().find(|c| c.id == id)
}

/// 一级领域 code → 中文标签。用于加持 reminder（避免把英文 slug `product/...`
/// 写进 prompt 让小模型误当成文件路径——实测 Qwen3.6 会去 read_file 找它）。
fn l1_label(code: &str) -> &str {
    match code {
        "engineering" => "工程",
        "business" => "商业",
        "design" => "设计",
        "marketing" => "市场",
        "quality" => "质量",
        "product" => "产品",
        "ai-data" => "AI·数据",
        "games" => "游戏",
        _ => "通用",
    }
}

/// 每 turn 注入 `<system-reminder>` 的专家面具人设文案。
///
/// 命中率优先于优雅（Qwen3.6 友好）：命令式、短、点明身份 + 视角 + 持续性。
/// 由 [`crate::bridge::Pinvou3Bridge::build_send_message_op`] 拼进 reminder_body。
///
/// **不写 l1/l2 英文 slug**：`product/product-strategy` 这种带斜杠的 slug 会被
/// 小模型当成工作区文件路径去 read_file（实测 bug）。改用中文领域标签、无斜杠。
pub fn equip_reminder(card: &PersonaCard) -> String {
    let mut s = format!(
        "你现在戴着【{cn}】专家面具——一位{domain}领域的 {tier} 档专家(评分 {score:.0})。\
         {desc}。\n\
         本 turn 起,以这位专家的视角、专业判断与语气回应 Boss——遇到相关任务发挥其专长。\
         这是持续身份,会一直挂着直到用户摘下面具,不要因为话题转移就丢掉这个视角。",
        cn = card.cn_name,
        domain = l1_label(&card.l1),
        tier = card.tier,
        score = card.final_score,
        desc = card.short_desc,
    );
    if !card.strengths.is_empty() {
        s.push_str(&format!("\n专长:{}。", card.strengths.join("、")));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_parses_and_is_nonempty() {
        let cards = all();
        assert!(cards.len() > 1000, "应解析出 1000+ 张专家卡, 实际 {}", cards.len());
    }

    #[test]
    fn every_card_has_core_fields() {
        for c in all() {
            assert!(!c.id.is_empty());
            assert!(!c.cn_name.is_empty());
            assert!(!c.tier.is_empty());
            assert!(!c.l1.is_empty());
        }
    }

    #[test]
    fn get_by_id_roundtrips() {
        let first = &all()[0];
        let found = get(&first.id).expect("应能按 id 查到");
        assert_eq!(found.id, first.id);
    }

    #[test]
    fn equip_reminder_mentions_identity_and_persistence() {
        let card = &all()[0];
        let r = equip_reminder(card);
        assert!(r.contains(&card.cn_name), "reminder 必须点名专家");
        assert!(r.contains("摘下面具"), "reminder 必须说明这是持续身份");
    }

    /// 回归: reminder 绝不能含 `l1/l2` 英文 slug(带斜杠)——会被小模型当成
    /// 工作区文件路径去 read_file(实测 product/product-strategy bug)。
    #[test]
    fn equip_reminder_has_no_pathlike_slug() {
        for c in all() {
            let r = equip_reminder(c);
            if !c.l2.is_empty() {
                let slug = format!("{}/{}", c.l1, c.l2);
                assert!(
                    !r.contains(&slug),
                    "reminder 不能含路径状 slug {slug:?}(会被误当文件路径), 卡={}",
                    c.id
                );
            }
            // l1 英文 code 也不应裸出现(用中文领域标签替代)
            assert!(
                !r.contains(&format!("{}/", c.l1)),
                "reminder 不应含 `{}/` 形式的 slug, 卡={}",
                c.l1,
                c.id
            );
        }
    }
}
