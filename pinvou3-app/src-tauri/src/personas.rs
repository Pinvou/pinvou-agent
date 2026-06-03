//! 专家面具池（卡片池）—— **Side B: agency-agents-zh 全正文版**。
//!
//! 与 Side A(pinvou2 agent-market 1078 元数据卡)的本质区别:
//! - 数据源 = jnMetaCode/agency-agents-zh(MIT, 201 个 agent),每个带 ~6K 字**完整人设正文**
//!   (职责/工作流/规则/交付物/沟通风格),不是摘要。
//! - **加持机制改造**:正文太长不能每 turn 灌。改成「加持时一次性注入完整 body
//!   (仿 skill 的 pending_instruction,首条消息 prepend) + 每 turn 只注入一句轻锚点」。
//!   见 [`equip_body_injection`](one-time) 与 [`equip_anchor`](per-turn)。
//! - 没有 tier/score/strengths 等结构化元数据(agency-agents-zh 不提供),facet 改部门过滤。
//!
//! License: agency-agents.json 数据 MIT(Michael Sitarzewski 原版 + jnMetaCode 中文化),
//! 见 resources/bundle/personas/AGENCY-AGENTS-LICENSE。

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// 编译期内嵌的 agency-agents-zh 数据(含完整 body)。
const PERSONAS_JSON: &str = include_str!("../resources/bundle/personas/agency-agents.json");

/// 单张专家卡(全正文版)。`body` 是完整人设 markdown(加持时注入)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaCard {
    pub id: String,
    /// 部门(academic/design/engineering/.../testing),前端 facet 按这个分组。
    pub dept: String,
    pub name: String,
    pub description: String,
    pub emoji: String,
    pub color: String,
    /// 完整人设正文(markdown)。list 时不下发给前端(太大),加持/详情时按需取。
    #[serde(default)]
    pub body: String,
}

/// 不含 body 的轻量摘要,给前端卡片网格用(list_personas 返回它,避免 1.2MB body 全量下发)。
#[derive(Debug, Clone, Serialize)]
pub struct PersonaSummary {
    pub id: String,
    pub dept: String,
    pub name: String,
    pub description: String,
    pub emoji: String,
    pub color: String,
}

impl PersonaCard {
    pub fn summary(&self) -> PersonaSummary {
        PersonaSummary {
            id: self.id.clone(),
            dept: self.dept.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            emoji: self.emoji.clone(),
            color: self.color.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct PersonaPoolFile {
    agents: Vec<PersonaCard>,
}

static POOL: OnceLock<Vec<PersonaCard>> = OnceLock::new();

/// 全部专家卡(首次调用解析内嵌 json，之后零成本)。
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

/// 全部卡的轻量摘要(list_personas 用)。
pub fn all_summaries() -> Vec<PersonaSummary> {
    all().iter().map(|c| c.summary()).collect()
}

/// 按 id 查一张卡(含 body)。
pub fn get(id: &str) -> Option<&'static PersonaCard> {
    all().iter().find(|c| c.id == id)
}

/// **一次性**注入的完整人设(加持后首条消息 prepend 一次,仿 skill pending_instruction)。
/// 把 agency-agents-zh 的完整 body 框起来,明确这是固定身份。
pub fn equip_body_injection(card: &PersonaCard) -> String {
    format!(
        "【你被加持了一张专家面具:{name}】\n\
         从这一刻起,你严格扮演下面这位专家——这是你的固定身份与行为准则,一直有效直到用户摘下面具:\n\n\
         ====== 专家人设开始 ======\n\
         {body}\n\
         ====== 专家人设结束 ======\n\n\
         以上是你的身份。回应 Boss 时始终基于这位专家的视角、方法论与沟通风格。\
         注意:人设正文里若出现示例代码、模板、路径,那是给你参考的范式,不是要你去读取的真实文件。",
        name = card.name,
        body = card.body,
    )
}

/// **每 turn**注入的轻锚点(短,放 `<system-reminder>`,防小模型长对话脱戏)。
/// 完整人设已在加持首条消息一次性给过,这里只重申身份,不重复 body。
pub fn equip_anchor(card: &PersonaCard) -> String {
    format!(
        "你仍戴着【{name}】专家面具——保持这位专家的身份、专业判断与沟通风格,\
         不要因话题转移而脱离角色。完整人设你已在加持时收到,按那个角色行事。",
        name = card.name,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_parses_and_is_nonempty() {
        let cards = all();
        assert!(cards.len() > 150, "应解析出 150+ 张 agency 专家卡, 实际 {}", cards.len());
    }

    #[test]
    fn every_card_has_core_fields_and_body() {
        for c in all() {
            assert!(!c.id.is_empty());
            assert!(!c.name.is_empty());
            assert!(!c.dept.is_empty());
            assert!(!c.body.is_empty(), "agency 卡必须有完整 body, 卡={}", c.id);
        }
    }

    #[test]
    fn summary_drops_body() {
        let s = serde_json::to_string(&all()[0].summary()).unwrap();
        assert!(!s.contains("\"body\""), "summary 不应含 body 字段");
    }

    #[test]
    fn body_injection_contains_full_body_and_framing() {
        let card = &all()[0];
        let inj = equip_body_injection(card);
        assert!(inj.contains(&card.name), "注入必须点名专家");
        assert!(inj.contains(&card.body), "一次性注入必须含完整 body");
        assert!(inj.contains("摘下面具"), "必须说明这是持续身份");
    }

    #[test]
    fn anchor_is_short_and_no_body() {
        let card = &all()[0];
        let a = equip_anchor(card);
        assert!(a.contains(&card.name), "锚点必须点名");
        assert!(!a.contains(&card.body), "per-turn 锚点不能重复 body");
        let chars = a.chars().count();
        assert!(chars < 120, "锚点应短(<120 字), 实际 {chars} 字");
    }
}
