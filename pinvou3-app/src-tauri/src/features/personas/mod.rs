//! 专家面具池（卡牌池）—— **Side B: agency-agents-zh 全正文版 + 用户自创卡**。
//!
//! 三个数据源（[`all_summaries`] / [`get`] 合并）:
//! 1. **agency 内嵌**（`source="builtin"`）: jnMetaCode/agency-agents-zh(MIT, 201 个),
//!    每个带 ~6K 字完整人设正文。编译期 `include_str!`，OnceLock 缓存。
//! 2. **pinvou3 内置卡**（`source="builtin"`）: 目前只有「卡牌制造专家」(见 [`builtin_extra`])。
//! 3. **用户自创卡**（`source="user"`）: 扫 `~/.pinvou3/user/personas/<id>.json`,
//!    可增删改，永不被 bundle 覆写。改动后 [`reload_user`] 刷新内存缓存。
//!
//! **加持机制**: 正文太长不能每 turn 灌。加持时一次性注入完整 body
//! ([`equip_body_injection`]) + 每 turn 轻锚点 ([`equip_anchor`])。
//!
//! License: agency-agents.json 数据 MIT，见 resources/common/bundle/personas/AGENCY-AGENTS-LICENSE。

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// 编译期内嵌的 agency-agents-zh 数据(含完整 body)。
const PERSONAS_JSON: &str =
    include_str!("../../../resources/common/bundle/personas/agency-agents.json");

fn default_source() -> String {
    "builtin".to_string()
}

/// 单张专家卡(全正文版)。`body` 是完整人设 markdown(加持时注入)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaCard {
    pub id: String,
    /// 部门(engineering/design/.../tool),前端 facet 按这个分组。
    pub dept: String,
    pub name: String,
    pub description: String,
    pub emoji: String,
    pub color: String,
    /// 完整人设正文(markdown)。list 时不下发给前端(太大),加持/详情时按需取。
    #[serde(default)]
    pub body: String,
    /// "builtin"(内嵌/pinvou3 内置) | "user"(用户自创)。前端据此着色 + 决定可否编辑/删除。
    #[serde(default = "default_source")]
    pub source: String,
    /// 内部行为标记(不入 json、不下发前端):加持这张卡时**本轮清空工具表**——模型看
    /// 不到 write_file / present_artifact 等任何工具。用于"产出是结构化数据而非文件"的
    /// 元卡(目前只有卡牌制造专家),防小模型误走写文件路径、产出无法被前端识别收藏的卡。
    /// 默认 false(普通领域卡照常带全量工具)。判定每 turn 实时读 active_persona,
    /// 戴上即限 / 卸下即恢复 / 换卡按新卡走,无持久状态。
    #[serde(skip)]
    pub conversational_only: bool,
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
    pub source: String,
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
            source: self.source.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct PersonaPoolFile {
    agents: Vec<PersonaCard>,
}

// ── 内嵌源(agency + pinvou3 内置卡): OnceLock 缓存,只读 ──────────────
static EMBEDDED: OnceLock<Vec<PersonaCard>> = OnceLock::new();
// ── 用户自创卡: RwLock 缓存,create/update/delete 后 reload_user 刷新 ──
static USER: OnceLock<RwLock<Vec<PersonaCard>>> = OnceLock::new();
/// 用户专家池内存版本；多智能体全局名册以它做增量缓存失效。
static USER_REVISION: AtomicU64 = AtomicU64::new(0);

fn embedded() -> &'static [PersonaCard] {
    EMBEDDED.get_or_init(|| {
        let mut v = serde_json::from_str::<PersonaPoolFile>(PERSONAS_JSON)
            .map(|f| f.agents)
            .unwrap_or_else(|e| {
                eprintln!("[pinvou3-app] persona pool 解析失败: {e}");
                Vec::new()
            });
        // agency 数据 source 默认已是 "builtin"。
        v.extend(builtin_extra());
        v
    })
}

fn user_lock() -> &'static RwLock<Vec<PersonaCard>> {
    USER.get_or_init(|| RwLock::new(load_user_cards()))
}

/// 扫 `~/.pinvou3/user/personas/<id>.json`,解析成卡(source 强制 "user")。
fn load_user_cards() -> Vec<PersonaCard> {
    let dir = crate::platform::paths::user_personas_dir();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(txt) => match serde_json::from_str::<PersonaCard>(&txt) {
                Ok(mut c) => {
                    c.source = "user".to_string();
                    out.push(c);
                }
                Err(e) => eprintln!("[pinvou3-app] 用户卡 {} 解析失败: {e}", path.display()),
            },
            Err(e) => eprintln!("[pinvou3-app] 读用户卡 {} 失败: {e}", path.display()),
        }
    }
    out
}

/// 重新从磁盘加载用户卡（create/update/delete 后调，让 list/get 立即看到）。
pub fn reload_user() {
    *user_lock().write().expect("user persona lock poisoned") = load_user_cards();
    // 卡池内容先发布，再推进版本；Acquire 读取到新版本时必然能看到新卡。
    USER_REVISION.fetch_add(1, Ordering::Release);
}

/// 当前可执行专家池版本。内嵌卡在进程内不可变，因此只需跟踪用户卡变更。
#[must_use]
pub fn executable_revision() -> u64 {
    // 确保 USER 首次从磁盘初始化发生在版本读取之前；否则第一次 capture 可能
    // 在旧缓存与惰性初始化之间缺少明确的发布点。
    let _ = user_lock();
    USER_REVISION.load(Ordering::Acquire)
}

/// 全部卡的轻量摘要(list_personas 用)。内嵌 + 用户,user 在后。
pub fn all_summaries() -> Vec<PersonaSummary> {
    let mut out: Vec<PersonaSummary> = embedded().iter().map(|c| c.summary()).collect();
    out.extend(
        user_lock()
            .read()
            .expect("user lock")
            .iter()
            .map(|c| c.summary()),
    );
    out
}

/// 可承担工具任务的专家卡完整快照。
///
/// 多智能体名册与当轮候选提醒必须从同一次读取生成，不能先取摘要、再逐张
/// [`get`]：用户在两次读取之间编辑或删除卡片时，会让模型看到的 profile id
/// 与实际可派名册错位。这里一次持有用户卡读锁并克隆完整集合，调用方随后可
/// 在不持锁的情况下构造底座配置和轻量候选索引。
pub fn executable_cards() -> Vec<PersonaCard> {
    let mut out: Vec<PersonaCard> = embedded()
        .iter()
        .filter(|card| !card.conversational_only)
        .cloned()
        .collect();
    out.extend(
        user_lock()
            .read()
            .expect("user lock")
            .iter()
            .filter(|card| !card.conversational_only)
            .cloned(),
    );
    out
}

/// 按 id 查一张卡(含 body),返回 owned clone。先查内嵌(缓存),再查用户卡。
pub fn get(id: &str) -> Option<PersonaCard> {
    if let Some(c) = embedded().iter().find(|c| c.id == id) {
        return Some(c.clone());
    }
    user_lock()
        .read()
        .expect("user lock")
        .iter()
        .find(|c| c.id == id)
        .cloned()
}

// ── 用户卡 CRUD ────────────────────────────────────────────────────

/// id 只允许 ascii 字母数字和 `-`（防路径穿越）。
fn id_is_safe(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn slugify(name: &str) -> String {
    let mut s = String::new();
    let mut prev_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            s.push('-');
            prev_dash = true;
        }
    }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "card".to_string()
    } else {
        s
    }
}

fn gen_user_id(name: &str) -> String {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() % 1_000_000)
        .unwrap_or(0);
    format!("user-{}-{suffix}", slugify(name))
}

fn write_card(card: &PersonaCard) -> Result<(), String> {
    if !id_is_safe(&card.id) {
        return Err(format!("非法卡 id: {}", card.id));
    }
    let dir = crate::platform::paths::user_personas_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("建目录失败: {e}"))?;
    let path = dir.join(format!("{}.json", card.id));
    let json = serde_json::to_string_pretty(card).map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("写卡失败: {e}"))
}

/// 新建用户卡。生成 `user-<slug>-<nanos>` id,写盘,刷新缓存,返回摘要。
pub fn create_user_persona(mut card: PersonaCard) -> Result<PersonaSummary, String> {
    if card.name.trim().is_empty() {
        return Err("卡牌名称不能为空".to_string());
    }
    if card.dept.trim().is_empty() {
        card.dept = "specialized".to_string();
    }
    card.id = gen_user_id(&card.name);
    card.source = "user".to_string();
    write_card(&card)?;
    reload_user();
    Ok(card.summary())
}

/// 更新用户卡(只能改 user- 前缀的自制卡)。
pub fn update_user_persona(mut card: PersonaCard) -> Result<PersonaSummary, String> {
    if !card.id.starts_with("user-") || !id_is_safe(&card.id) {
        return Err("只能编辑自制卡".to_string());
    }
    if card.name.trim().is_empty() {
        return Err("卡牌名称不能为空".to_string());
    }
    let path = crate::platform::paths::user_personas_dir().join(format!("{}.json", card.id));
    if !path.exists() {
        return Err("卡牌不存在".to_string());
    }
    if card.dept.trim().is_empty() {
        card.dept = "specialized".to_string();
    }
    card.source = "user".to_string();
    write_card(&card)?;
    reload_user();
    Ok(card.summary())
}

/// 删除用户卡(只能删 user- 前缀的自制卡)。
pub fn delete_user_persona(id: &str) -> Result<(), String> {
    if !id.starts_with("user-") || !id_is_safe(id) {
        return Err("只能删除自制卡".to_string());
    }
    let path = crate::platform::paths::user_personas_dir().join(format!("{id}.json"));
    let _ = std::fs::remove_file(&path);
    reload_user();
    Ok(())
}

// ── 加持注入 ───────────────────────────────────────────────────────

/// **一次性**注入完整人设（加持后的首条消息 prepend 一次）。
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
pub fn equip_anchor(card: &PersonaCard) -> String {
    format!(
        "你仍戴着【{name}】专家面具——保持这位专家的身份、专业判断与沟通风格,\
         不要因话题转移而脱离角色。完整人设你已在加持时收到,按那个角色行事。",
        name = card.name,
    )
}

// ── pinvou3 内置卡: 卡牌制造专家 ────────────────────────────────────

/// pinvou3 自己的内置卡(不来自 agency 数据)。目前只有「卡牌制造专家」。
fn builtin_extra() -> Vec<PersonaCard> {
    vec![PersonaCard {
        id: "pinvou-card-creator".to_string(),
        dept: "tool".to_string(),
        name: "卡牌制造专家".to_string(),
        description: "对话式帮你设计一张新的专家卡牌,产出后一键存入卡牌池".to_string(),
        emoji: "🃏".to_string(),
        color: "#7C3AED".to_string(),
        source: "builtin".to_string(),
        body: CARD_CREATOR_BODY.to_string(),
        // 纯对话元卡:产出是内联 persona-card JSON 块(前端抠出→存入卡牌池),绝不写文件。
        // 加持期间清空工具表,从工具层杜绝小模型误走 write_file/present_artifact 路径。
        conversational_only: true,
    }]
}

const CARD_CREATOR_BODY: &str = r##"# 卡牌制造专家

你是「卡牌制造专家」。专门帮 Boss 设计可加持给 AI 的**专家卡牌**。每张卡牌本质是一份"专家能力档案"——加持后 AI 会严格按这份档案的视角和方法论干活。所以你设计的卡质量直接决定加持效果。

## 工作流程
1. **先做 2-3 轮追问理清需求,再出卡**(硬性流程:哪怕 Boss 一上来就说得挺具体,也要问够 2-3 轮、把意图磨清楚再产卡。绝不在第一条消息就直接出卡)。
   - **一次只问一个问题**:挑当前最关键、最影响卡质量的那个,问完等 Boss 回答,再问下一个。像聊天一样一来一回,**不要一次甩一串问题**。
   - **问题从这四个维度里挑**(Boss 已经说清的别重复问,只问还没定的):① 领域 / 行业细分;② 核心技能 / 方法论 / 工具栈;③ 典型任务 / 使用场景;④ 目标 / 交付物形式 / 语气风格。
   - 追问简短直接,可带一句"为什么问"(如"不同赛道的爆款逻辑差很多")。
   - **这一轮的问题若有明确可选项**(如"聚焦哪个角色?"有内容/运营/电商几种)→ 用一个 `card-question` 块给出问题和选项(格式见下),前端会渲染成**可点击的选项卡**,Boss 点一下即作答;**开放式问题**(如"目标客单价大概多少?")就正常用文字问。
   - 一般问到第 2-3 轮、信息够做出一张"真能干活、不假大空"的卡就停,进入出卡;Boss 中途说"你帮我定 / 随便"时别纠缠,按常理补默认值直接出卡。
2. **设计这张卡**,正文 body 要写全这几块(这是高质量专家卡的结构):
   - **身份与定位**:这是谁、擅长什么、什么底色
   - **核心职责**:3-5 条这位专家负责的事
   - **工作流程**:拿到任务后一步步怎么做
   - **关键规则 / 质量标准**:必须遵守什么、什么算合格
   - **典型交付物**:产出什么、什么格式
   - **沟通风格**:怎么跟用户说话
3. **选部门 dept**(英文 code):engineering / design / product / marketing / finance / sales / testing / project-management / paid-media / support / academic / game-development / spatial-computing / supply-chain / hr / legal / specialized。挑最贴的一个。
4. **产出成品**:把设计好的卡以一个 `persona-card` 代码块输出,里面是严格 JSON:

```persona-card
{
  "name": "专家中文名",
  "dept": "部门英文code",
  "emoji": "一个emoji",
  "color": "#十六进制色",
  "description": "一句话简介(15字内)",
  "body": "完整人设markdown正文,用 \\n 换行"
}
```

## 追问选项的格式
某轮追问**有明确可选项**时,用一个 `card-question` 代码块给出问题和选项(前端渲染成可点击选项卡):

```card-question
{
  "question": "你希望这张卡聚焦哪个角色?",
  "options": ["内容创作型 —— 写爆款笔记/起标题/配图", "运营增长型 —— 账号定位/选题/数据复盘", "电商变现型 —— 带货/商品笔记/投放", "复合型 —— 以上都覆盖"]
}
```
- `question` 一句话;`options` 2-5 个,每个写成"短标签 —— 简短说明"(Boss 点选时只发短标签)。
- 必须以 ```card-question 字面标签起头;JSON 合法。
- 块外只写一句简短引导(如"先确认一个关键点:"),**别再用文字把选项重复列一遍**。
- 一条消息最多一个 `card-question` 块;开放式问题不用这个块,直接文字问。

## 硬规则
- **绝不调用任何文件或命令工具**(`File` / `Bash` 等)。卡牌不是文件,**直接在回复正文里输出代码块**就行,不要写盘、不要产出 .txt/.md 文件。
- 代码块**必须以 ```persona-card 这个字面标签起头**(不是 ```json,不是无标签)。前端靠这个识别成可保存的卡。
- **body 要详实**(至少几百字),是真能指导 AI 干活的方法论,不能是空话套话。
- **一次只产一张卡**的 `persona-card` 块。块以外可以正常跟 Boss 对话/确认。
- JSON 必须能被解析:body 里的换行用 `\n`,引号转义。
- 输出 `persona-card` 块后,告诉 Boss:**点卡片下方的「存入卡牌池」按钮即可保存**,保存前还能在编辑器里改。
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_parses_and_has_builtin() {
        let cards = embedded();
        assert!(
            cards.len() > 150,
            "应解析出 150+ 张卡, 实际 {}",
            cards.len()
        );
        assert!(
            cards.iter().any(|c| c.id == "pinvou-card-creator"),
            "内置卡牌制造专家必须在内嵌源里"
        );
    }

    #[test]
    fn every_embedded_card_has_core_fields_and_body() {
        for c in embedded() {
            assert!(!c.id.is_empty());
            assert!(!c.name.is_empty());
            assert!(!c.dept.is_empty());
            assert!(!c.body.is_empty(), "卡必须有 body, 卡={}", c.id);
            assert_eq!(
                c.source, "builtin",
                "内嵌卡 source 应为 builtin, 卡={}",
                c.id
            );
        }
    }

    #[test]
    fn summary_keeps_source_drops_body() {
        let s = serde_json::to_string(&embedded()[0].summary()).unwrap();
        assert!(!s.contains("\"body\""), "summary 不应含 body");
        assert!(s.contains("\"source\""), "summary 应含 source");
    }

    #[test]
    fn get_finds_builtin_creator() {
        let c = get("pinvou-card-creator").expect("应能查到内置卡");
        assert_eq!(c.name, "卡牌制造专家");
        assert!(
            c.body.contains("persona-card"),
            "创作卡 body 必须引导输出 persona-card 块"
        );
    }

    #[test]
    fn card_creator_body_is_valid_in_raw_string() {
        // 确保 r##".."## 原始串没被 "#色 提前终止 —— body 完整。
        let c = get("pinvou-card-creator").unwrap();
        assert!(c.body.contains("硬规则"));
        assert!(c.body.ends_with("改。\n") || c.body.contains("存入卡牌池"));
    }

    #[test]
    fn slugify_handles_chinese_and_ascii() {
        assert_eq!(slugify("Finance Advisor"), "finance-advisor");
        assert_eq!(slugify("财务顾问"), "card"); // 纯中文 → 兜底
        assert_eq!(slugify("  !!  "), "card");
    }

    #[test]
    fn gen_user_id_is_safe_and_prefixed() {
        let id = gen_user_id("我的专家");
        assert!(id.starts_with("user-"));
        assert!(id_is_safe(&id), "生成的 id 必须只含安全字符: {id}");
    }

    #[test]
    fn user_persona_crud_roundtrip() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let tmp = format!(
            "/tmp/pinvou3-persona-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::env::set_var("PINVOU3_HOME", &tmp);

        let mk = |id: &str, name: &str| PersonaCard {
            id: id.to_string(),
            dept: "finance".to_string(),
            name: name.to_string(),
            description: "d".to_string(),
            emoji: "💰".to_string(),
            color: "#10B981".to_string(),
            body: "正文方法论".to_string(),
            source: "builtin".to_string(),
            conversational_only: false,
        };

        // create
        let sum = create_user_persona(mk("", "测试财务顾问")).expect("create");
        assert!(sum.id.starts_with("user-"));
        assert_eq!(sum.source, "user");
        let got = get(&sum.id).expect("get after create");
        assert_eq!(got.name, "测试财务顾问");
        assert_eq!(got.source, "user");
        assert!(
            all_summaries().iter().any(|s| s.id == sum.id),
            "list 应含新卡"
        );

        // update
        let usum = update_user_persona(mk(&sum.id, "改名后")).expect("update");
        assert_eq!(usum.name, "改名后");
        assert_eq!(get(&sum.id).unwrap().name, "改名后");

        // 内置卡不能被 update/delete
        assert!(update_user_persona(mk("pinvou-card-creator", "x")).is_err());
        assert!(delete_user_persona("pinvou-card-creator").is_err());

        // delete
        delete_user_persona(&sum.id).expect("delete");
        assert!(get(&sum.id).is_none(), "删后查不到");

        // cleanup
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        reload_user();
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn body_injection_and_anchor_contract() {
        let card = &embedded()[0];
        let inj = equip_body_injection(card);
        assert!(inj.contains(&card.name) && inj.contains(&card.body) && inj.contains("摘下面具"));
        let a = equip_anchor(card);
        assert!(a.contains(&card.name) && !a.contains(&card.body));
        assert!(a.chars().count() < 120);
    }
}
