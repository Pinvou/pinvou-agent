//! Pinvou v4 召唤式检阅。
//!
//! Boss 主动召唤 → 从 session messages 投影出「需求 vs 应对」→ 单次独立 LLM
//! 审查 → 返回 personas/issues。Pinvou 只检阅、不替 Boss 决策。
//!
//! 设计与实证：`docs/品悟v4-常驻检阅助手设计.md` / `docs/品悟v4-召唤式实测报告.md`。
//! 上下文策略（§4.1，实测背书）：能全喂就全喂（真实 1.2 万 token、94% 噪音不崩、
//! 还能事实核查），超过 `FULL_FEED_CHAR_LIMIT` 才降级到确定性投影。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use deepseek_tui::models::{ContentBlock, Message};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::features::assistant::platform::bridge::{prefs::ModelPreset, Pinvou3Bridge};

const PROMPT: &str = r#"你是 Pinvou，Boss 身边的独立检阅顾问，召之即来。

Boss 刚刚召唤你，让你检阅前面主 AI 的工作。给你的材料分两部分：
- 【Boss 需求】：Boss 在整个过程里说过的意图、约束、做过的选择。这是你的立场起点。
- 【AI 应对】：主 AI 给出的产物、论述或计划。这是你要审的对象。

你站在 Boss 一侧，把发现分成**两类、各进各的篮子，不重叠**：
A. issues = 产物的**缺陷**：AI 改得动的错误、遗漏、与 Boss 需求冲突或偏移的地方。你是批判者，不附和 AI——这类 AI 自己能修。
B. recommendations = 需 **Boss 拿主意**的决策点/缺信息：选项、偏好、预算/日期/目的地/同行人这类**没有客观标准答案、AI 替不了 Boss 定**的待定项。你替 Boss 消化、给推荐+理由，但拍板权永远归 Boss。

**铁律：同一件事只进一个篮子。**"日期没定""同行人没确认"这种要 Boss 补的，只进 recommendations 给推荐，**绝不**再作为 issue 重复列一遍——把"要 Boss 定的事"伪装成"AI 能改的缺陷"，会让 Boss 收到自相矛盾的两份处置。

硬规则：
1. 先判断领域，以最相关的领域顾问视角审查（如"旅行规划""商业顾问""法务""技术架构"）。横跨多领域时选一个 primary，其余放 alternates。
2. 紧扣 Boss 的需求和约束。AI 应对里和 Boss 约束冲突、或忽略 Boss 提过的东西，必须在 issues 指出。
3. 只要 AI 把选择权交回 Boss（列多个方案让选、问偏好、问预算/日期/目的地等无标准答案的待定项），或产物里其实悬而未决、需 Boss 定的，就在 recommendations 里逐项给推荐：推荐哪个 + 一句为什么。别只说"逻辑清晰没问题"就完事。信息不足以推荐时在 why 说清还差什么。**涉及外部事实（航班/票价/政策）的推荐，别把没核实的当确定**——推荐已知稳妥项、把待核实的放次选并在 why 标"需核实"，绝不用"优先选[未核实项]"这种确信措辞。
4. 涉及钱、不可逆、隐私、人际、法律/医疗/金融，必须标风险。
5. **外部事实（交通班次/票价/营业时间/签证政策）你没有外部知识**：发现可疑只能 kind="needs_verify" 标「需核实」、severity≤medium，**绝不标 high 硬伤、绝不断言"实际上没有/不存在"**——确信的外部事实断言是已实证的事故源（如把本有 Thalys 直达的"巴黎→阿姆"误断成"无直达"，会把对的方案打掉）。
6. issues 要克制：**最多挑 3–5 条最重要的、按 severity 排序**；没实质风险/遗漏就返回 []。recommendations 不受此限——只要 Boss 面临选择就给。
7. 你只给意见和推荐，不替 Boss 操作、不替 Boss 最终确认。

输出只能是 JSON，不要 Markdown，不要解释：
{
  "personas": [{"id":"领域英文短id","label":"领域顾问中文名","primary":true}],
  "alternates": ["其他相关领域id"],
  "trace": "给 Boss 看的一句话总结，像微信，不要列表腔",
  "recommendations": [{"topic":"待决策点（如'方案选择'/'出发日期'）","pick":"推荐选哪个","why":"一句理由"}],
  "issues": [{"severity":"low|medium|high","kind":"risk|irreversible|quality|intent_drift|needs_verify","persona":"领域id","text":"缺陷(AI 改得动的)","suggestion":"怎么改"}],
  "risk": "low|medium|high",
  "confidence": 0.0
}"#;

/// 核账模式 prompt（§3，sim 验证收敛）：对同一产出物的复审，只核账、禁新增、终态。
/// 治 tejz7cxrd5jd0 的不收敛——暴增/翻案/永久挂账/无终态。
const RECONCILE_PROMPT: &str = r#"你是 Pinvou，Boss 身边的独立检阅顾问。这是对**同一产出物**的复审（核账模式），不是重新自由批评。
给你【上轮账目】（上次立的问题）+【当前产物】（已修订版本）。严格按规则核账：
1. 逐条核对账目对照产物：**已改好的不要再列进 issues**（视为闭合）；只把**没改/没改对**的留在 issues 里，说明还差什么。
2. **禁止新增问题**。唯一例外：本次修订在它改动的段落内新引入的错误（issue 里注明"修订引入"）。不要提上轮没提过的新角度。
3. 已结的账不要翻，除非有新证据。
4. 外部事实（交通/票价/签证政策）你无外部知识，只能 kind="needs_verify" 标「需核实」、severity≤medium，绝不标 high 硬伤、绝不断言"不存在/没有"。
5. **终态**：所有账目都闭合（issues 为空）→ verdict="pass"。**pass 时 trace 必须逐条点名每笔账目的核对结论**（如"①交通：已改为巴黎直飞苏黎世✓ ②日期：已锁定5月中下旬✓ ③签证：已补主停留国原则✓"），让 Boss 看到你逐条对过产物、不是空泛放行；还有没闭合的 → verdict="continue"。
输出只能是 JSON：{"verdict":"pass|continue","trace":"pass 时逐条交代各账目核对结论，continue 时一句话","issues":[{"severity":"low|medium|high","kind":"...","text":"...","suggestion":"..."}],"risk":"low|medium|high"}"#;

/// 覆盖镜头 prompt（§coverage，多场景 sim 4abe9ae 背书）：不挑错，查"全不全"。领域无关——
/// 让模型自判专家身份临场列该类产物的完整性维度框架，再标缺/薄弱维度。收敛靠框架有限。
const COVERAGE_PROMPT: &str = r#"你是 Pinvou，Boss 身边的独立检阅顾问。这次专做【覆盖度检查】——不挑已有内容的对错，只看产物"全不全"。

给你 Boss 的需求 + 主 AI 的产物。两步走：
1. 以你**自判的领域专家身份**，先想清楚：**这一类产物**要算完整、合格、能交付，行业惯例本该覆盖哪些维度？列出这个领域的完整性维度框架（贴合行业惯例，别硬套别的领域）。
2. 对照产物，逐个维度看覆盖度。只把**薄弱/缺失**的维度列进 coverage，说明缺什么、建议补什么；已经齐的不用列。

硬规则：
- 维度框架必须贴合这类产物自己的行业惯例。
- 核心维度（该有必须有的）缺失 → severity="high"；加分维度（锦上添花）缺失 → severity="low"。
- 克制：最多列 5–7 个最重要的缺口，按 severity 排序。
- 只指"缺哪些维度"，不挑"已写内容对不对"（那是 issues 镜头的事）。
- 外部事实（如某地必备某证件）拿不准就在 suggestion 里写"需核实"，别硬断言。

输出只能是 JSON，不要解释：
{
  "personas": [{"id":"领域英文短id","label":"领域顾问中文名","primary":true}],
  "trace": "给 Boss 的一句话总结（像微信，说整体覆盖如何、缺哪几块）",
  "framework": ["这类产物完整该覆盖的维度，逐个列"],
  "coverage": [{"dimension":"维度名","coverage":"weak|missing","severity":"high|medium|low","text":"缺/薄弱在哪","suggestion":"建议补什么"}]
}"#;

/// 本地审查实测 5–19s，旧版 5s 必挂；放宽到 30s（设计 §9）。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// 全喂阈值（字符数；中英混 ÷1.7 ≈ token）。实测全喂巴黎 session 20117 字
/// （≈1.2 万 token）不崩，所以 24000 字以内直接全喂，超过才投影降级（§4.1）。
const FULL_FEED_CHAR_LIMIT: usize = 24_000;

/// 产物上限（字符）。产物真相在 workspace 文件，整篇读、整篇喂——本地 256k 扛得住
/// （实测 2 万字全喂稳）。常规产物（文档/邮件/代码）都在此内整篇喂；超此才截断+标注，
/// 真超大产物的「不漏」解是 P1 map-reduce 分块全审（§10.10）。
const ARTIFACT_CHAR_LIMIT: usize = 30_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinvouPersona {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinvouIssue {
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub persona: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub suggestion: String,
}

/// Boss 决策点的推荐（§B 助决策）。topic=待决策点，pick=推荐项，why=理由。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinvouRecommendation {
    #[serde(default)]
    pub topic: String,
    #[serde(default)]
    pub pick: String,
    #[serde(default)]
    pub why: String,
}

/// 覆盖镜头(coverage)的缺口：dimension=缺的维度，coverage=weak|missing，
/// severity=high(核心维度缺=不完整)/low(加分维度)。和 issues(挑错)是两套镜头。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinvouGap {
    #[serde(default)]
    pub dimension: String,
    #[serde(default)]
    pub coverage: String,
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub suggestion: String,
}

/// guard 后返回给前端的审查结果（§7 协议）。
#[derive(Debug, Clone, Serialize)]
pub struct PinvouReview {
    pub personas: Vec<PinvouPersona>,
    pub alternates: Vec<String>,
    pub trace: String,
    pub recommendations: Vec<PinvouRecommendation>,
    pub issues: Vec<PinvouIssue>,
    /// 覆盖镜头:这类产物的完整性维度框架(供展示)。空=没做覆盖体检。
    #[serde(default)]
    pub framework: Vec<String>,
    /// 覆盖镜头:产物缺/薄弱的维度。
    #[serde(default)]
    pub coverage: Vec<PinvouGap>,
    pub risk: Option<String>,
    pub confidence: Option<f64>,
    /// 核账模式终态：pass=通过可交付 / continue=还有未结账目（首轮模式为 None）。
    pub verdict: Option<String>,
    /// 这次审的产出物 path，存进 sidecar 供下次召唤核账匹配同一产出物。
    pub artifact_path: Option<String>,
    pub guard_reasons: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ModelReview {
    #[serde(default)]
    personas: Vec<PinvouPersona>,
    #[serde(default)]
    alternates: Vec<String>,
    #[serde(default)]
    trace: String,
    #[serde(default)]
    recommendations: Vec<PinvouRecommendation>,
    #[serde(default)]
    issues: Vec<PinvouIssue>,
    #[serde(default)]
    framework: Vec<String>,
    #[serde(default)]
    coverage: Vec<PinvouGap>,
    risk: Option<String>,
    confidence: Option<f64>,
    verdict: Option<String>,
}

/// 召唤入口：从 session messages + workspace 审查。同一产出物的复审走核账模式（§3）。
pub async fn summon(
    bridge: &Pinvou3Bridge,
    messages: &[Message],
    workspace: &Path,
    session_id: &str,
    focus: Option<&str>,
    mode: Option<&str>,
) -> Result<PinvouReview> {
    // focus = 就近图标锚定的产出物 path（召唤自带作用域，§1）；否则取最后修改的产出物。
    let artifact_path = focus
        .map(str::to_string)
        .or_else(|| last_artifact_path(messages));
    // 覆盖体检模式(§coverage,独立入口):查产物"全不全"。复用 build_context(全喂需求+产物文件)，
    // COVERAGE_PROMPT 让模型临场列完整性框架+缺口。不走核账——体检是 Boss 主动的一次性深度动作。
    if mode == Some("coverage") {
        let raw =
            model_review(bridge, COVERAGE_PROMPT, &build_context(messages, workspace)).await?;
        let mut review = apply_guard(raw, bridge.locale_tag());
        review.artifact_path = artifact_path;
        return Ok(review);
    }
    // 核账模式：该产出物之前召唤过(sidecar 有账目) → 注入上轮账目，只核账、禁新增、可终态。
    // 有该产物的上轮记录 → 核账模式（即使账目全已结，也核账输出 pass，而非重新自由批评）。
    let prior = artifact_path
        .as_deref()
        .and_then(|p| read_prior_ledger(session_id, p, messages.len() as u64));
    let (prompt, context) = match &prior {
        Some(ledger) => (
            RECONCILE_PROMPT,
            build_reconcile_context(ledger, messages, workspace),
        ),
        None => (PROMPT, build_context(messages, workspace)),
    };
    let raw = model_review(bridge, prompt, &context).await?;
    let mut review = apply_guard(raw, bridge.locale_tag());
    review.artifact_path = artifact_path;
    Ok(review)
}

/// 需求/事实/对话脉络走全喂或投影（§4.1）；**产物单独读 workspace 文件真实内容**附在
/// 末尾——修 edit_file 被忽略 + diff 流难拼最终态（产物真相在文件，不在工具调用历史）。
fn build_context(messages: &[Message], workspace: &Path) -> String {
    let full = full_transcript(messages);
    let mut ctx = if full.chars().count() <= FULL_FEED_CHAR_LIMIT {
        format!("下面是我和主 AI 的完整对话记录，帮我检阅：\n\n{full}")
    } else {
        project(messages)
    };
    if let Some((path, content)) = latest_artifact_file(messages, workspace) {
        ctx.push_str(&format!(
            "\n\n【AI 应对·最新产物 `{path}` 的当前完整内容（以此为准）】\n{content}"
        ));
    }
    ctx
}

/// 最后被 write_file/edit_file/present_artifact 碰过的产物 path。**含 edit_file 是修
/// bug 的关键**：旧逻辑只认 write_file，长 session 用 edit_file 迭代就看不到最新态。
fn last_artifact_path(messages: &[Message]) -> Option<String> {
    let mut last = None;
    for m in messages {
        for b in &m.content {
            if let ContentBlock::ToolUse { name, input, .. } = b {
                if matches!(
                    name.as_str(),
                    "write_file" | "edit_file" | "present_artifact"
                ) {
                    if let Some(p) = input.get("path").and_then(Value::as_str) {
                        last = Some(p.to_string());
                    }
                }
            }
        }
    }
    last
}

/// 读最新产物的 workspace 文件当前内容（bounded）。超 ARTIFACT_CHAR_LIMIT 截断+标注，
/// 让 pinvou 知道没看全、不对截断部分误报遗漏（silent truncation 反模式）。
fn latest_artifact_file(messages: &[Message], workspace: &Path) -> Option<(String, String)> {
    let path = last_artifact_path(messages)?;
    let abs = if Path::new(&path).is_absolute() {
        std::path::PathBuf::from(&path)
    } else {
        workspace.join(&path)
    };
    // 安全：只读 workspace 内的文件，别把 ~/.ssh 等喂给外部 LLM
    let in_ws = abs
        .canonicalize()
        .ok()
        .zip(workspace.canonicalize().ok())
        .map(|(a, w)| a.starts_with(&w))
        .unwrap_or(false);
    if !in_ws {
        return None;
    }
    let content = std::fs::read_to_string(&abs).ok()?;
    let n = content.chars().count();
    let bounded = if n > ARTIFACT_CHAR_LIMIT {
        let head: String = content.chars().take(ARTIFACT_CHAR_LIMIT).collect();
        format!("{head}\n\n…（产物共 {n} 字，这里是前 {ARTIFACT_CHAR_LIMIT} 字；后半未展示，如需审让 Boss 指定。P1 改 map-reduce 分块全审）")
    } else {
        content
    };
    Some((path, bounded))
}

/// 账目是否已被 Boss 结清、核账不再核（§3）：accept=缺陷接受现状、confirmed=需核实但 Boss
/// 已确认没问题。其余(modify/verify/adopt/ask/pending)都保留进核账。kind 分流后新增的 confirmed
/// 也得跳过，否则 Boss 已确认的账会被重核→震荡（pkx4clhny5jd0 实测暴露）。
fn is_ledger_closed(resolution: Option<&str>) -> bool {
    matches!(resolution, Some("accept") | Some("confirmed"))
}

/// 读 sidecar(`pinvou_reviews.json`)里针对同一产出物的最近一轮账目(issues)。有则进
/// 核账模式（§3，激活原未做项「连续召唤接续」）。sidecar 由前端 recordPinvouReview 落盘，后端只读。
fn read_prior_ledger(
    session_id: &str,
    artifact_path: &str,
    current_pos: u64,
) -> Option<Vec<PinvouIssue>> {
    let path = crate::platform::paths::session_pinvou_reviews(session_id);
    let txt = std::fs::read_to_string(path).ok()?;
    let arr: Vec<Value> = serde_json::from_str(&txt).ok()?;
    ledger_from_entries(&arr, artifact_path, current_pos)
}

/// 从 sidecar entries 选【首轮立账】的账目(抽出便于单测)。读首个该产物、issues 非空的 entry,
/// 不是最近一轮——否则 pass 轮 issues 空,下次核账读到空账目就退化成自由批评、重新挑错,导致
/// pass↔continue 震荡(实测连点品三态循环)。固定读首轮那批,核账每次都核同一批+当前产物→收敛。
fn ledger_from_entries(
    arr: &[Value],
    artifact_path: &str,
    current_pos: u64,
) -> Option<Vec<PinvouIssue>> {
    // 最近一轮【品】entry(rev=从后往前第一个;有 verdict 或 issues 非空)。**跳过【悟】的 coverage
    // entry**(issues 空+无 verdict)——否则悟 record 的 entry 会当 latest、让 current_pos 守门失效
    // (悟非 pass→永远不触发重新立账,品又读不到 pass 后的增量)。
    let latest = arr.iter().rev().find(|e| {
        let r = match e.get("review") {
            Some(r) => r,
            None => return false,
        };
        if r.get("artifact_path").and_then(Value::as_str) != Some(artifact_path) {
            return false;
        }
        r.get("verdict").and_then(Value::as_str).is_some()
            || r.get("issues")
                .and_then(Value::as_array)
                .map_or(false, |a| !a.is_empty())
    })?;
    let latest_pos = latest.get("pos").and_then(Value::as_u64).unwrap_or(0);
    let passed = latest
        .get("review")
        .and_then(|r| r.get("verdict"))
        .and_then(Value::as_str)
        == Some("pass");
    // 最近一轮 pass(结案) + 之后产物又被改过(当前 messages 比审查那时长,说明悟/AI 动过产物)
    // → None,品【重新立账】审 AI 这次的增量修改(补的内容可能有错)。连点品(产物没变,
    // current_pos==latest_pos)→ 不重新立账,走下面核【最近一轮立账】→稳定 pass、不震荡。
    // 这样既能审增量(产物真变了就重审),又不会因连点品在 pass↔挑错 间横跳(关键=current_pos 守门)。
    if passed && current_pos > latest_pos {
        return None;
    }
    // 读【最近一轮立账】(最近 issues 非空 entry)的账目——支持多轮立账(首轮品 + 悟后重新立账各立一批)。
    // pass 轮 issues 空会被跳过;accept/confirmed 已结的过滤(§3,is_ledger_closed)。
    arr.iter().rev().find_map(|entry| {
        let rv = entry.get("review")?;
        if rv.get("artifact_path").and_then(Value::as_str)? != artifact_path {
            return None;
        }
        if rv
            .get("issues")
            .and_then(Value::as_array)
            .map_or(true, |a| a.is_empty())
        {
            return None;
        }
        let kept = rv
            .get("issues")?
            .as_array()?
            .iter()
            .filter(|i| !is_ledger_closed(i.get("resolution").and_then(Value::as_str)))
            .filter_map(|i| serde_json::from_value::<PinvouIssue>(i.clone()).ok())
            .collect::<Vec<_>>();
        Some(kept)
    })
}

/// 核账上下文：上轮账目 + 当前产物文件真实内容（§3）。核账只对账，不喂全会话。
fn build_reconcile_context(
    prior: &[PinvouIssue],
    messages: &[Message],
    workspace: &Path,
) -> String {
    let mut out = String::from("【上轮账目】\n");
    for (i, it) in prior.iter().enumerate() {
        out.push_str(&format!("{i}. [{}] {}\n", it.severity, it.text));
    }
    match latest_artifact_file(messages, workspace) {
        Some((path, content)) => out.push_str(&format!("\n【当前产物 `{path}`】\n{content}")),
        None => out.push_str("\n（产物文件读不到，按账目对照 Boss 需求核账）"),
    }
    out
}

/// 非中文 locale 的输出语言指令,追加到三个 prompt 末尾。三个 prompt 本体留中文(按收敛性精调过,
/// 见设计文档,逐句翻译会引入行为漂移),只让模型把**自然语言字段值**切到目标语言——JSON key、枚举值
/// (severity/kind/verdict/coverage/primary)、persona id slug 保持英文。zh-Hans 及未知 → None(no-op)。
fn output_language_directive(locale_tag: &str) -> Option<String> {
    // zh-Hans:即使被审查的产物/代码是英文,finding 也强制简体中文——实测 Qwen3.6 在英文产物
    // 上下文下会漂成英文 finding(中文 prompt 本体不够硬,需要一条显式输出语言指令兜住)。
    if locale_tag == "zh-Hans" {
        return Some(
            "\n\n## 输出语言(强制)\n\
             JSON 里所有自然语言字段值(trace / label / topic / pick / why / text / suggestion / \
             dimension / framework 各项)必须用简体中文,即使被审查的产物/代码是英文也别跟着写英文。\
             JSON 的 key、枚举值(severity / kind / verdict / coverage / primary)、persona id slug \
             保持原样 ASCII。"
                .to_string(),
        );
    }
    let lang = match locale_tag {
        "en" => "English",
        "ja" => "Japanese (日本語)",
        _ => return None, // 未知 locale → prompt 原样中文
    };
    Some(format!(
        "\n\n## Output Language (HARD override)\n\
         Write EVERY natural-language value in your JSON output in {lang}: \
         `trace`, `label`, `topic`, `pick`, `why`, `text`, `suggestion`, `dimension`, \
         and every item in `framework`. This OVERRIDES any wording above that asks for \
         Chinese (e.g. 「领域顾问中文名」「像微信」「给 Boss 看的一句话」). Keep all JSON keys \
         and enum values (`severity`, `kind`, `verdict`, `coverage`, `primary`) plus the \
         persona `id` slug exactly as specified — those stay ASCII/English."
    ))
}

/// guard 空 trace 兜底文案,跟随 locale(§7)。模型正常会按 output_language_directive 返回本地化 trace,
/// 这里只是它返回空时的保底,也得跟 UI 语言一致。
fn default_trace(locale_tag: &str, clean: bool) -> String {
    match (locale_tag, clean) {
        ("en", true) => "Looked it over — no problems.",
        ("en", false) => "I've reviewed it; a few points for you to confirm.",
        ("ja", true) => "確認しました。問題ありません。",
        ("ja", false) => "確認しました。いくつか確認したい点があります。",
        (_, true) => "看过了，没问题。",
        (_, false) => "我看过了，有几个点你确认下。",
    }
    .to_string()
}

async fn model_review(
    bridge: &Pinvou3Bridge,
    prompt: &str,
    user_content: &str,
) -> Result<ModelReview> {
    let client = Client::builder()
        .timeout(DEFAULT_TIMEOUT)
        .build()
        .context("build reqwest client")?;
    let base_url = bridge.base_url();
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    // 本地 vLLM：用实际 served name 发请求——vLLM 端改了 --served-model-name 后，配置里
    // bridge.model() 可能仍是旧名（品悟走独立 HTTP，不经 engine 的 fresh_bridge_for 探测），
    // 直接用会 404 model_not_found。探测失败回退配置值；云端 provider 不探测。
    let provider = bridge.provider();
    let preset = review_model_preset(bridge);
    let model_name = if provider == "vllm" {
        crate::features::monitor::probe_vllm_model_info(&base_url)
            .await
            .0
            .unwrap_or_else(|| bridge.model())
    } else {
        bridge.model()
    };
    // 非中文 locale 追加输出语言指令(覆盖 prompt 里的「中文」措辞)。
    let prompt = match output_language_directive(bridge.locale_tag()) {
        Some(suffix) => format!("{prompt}{suffix}"),
        None => prompt.to_string(),
    };
    let mut body = json!({
        "model": model_name,
        "messages": [
            { "role": "system", "content": prompt },
            { "role": "user", "content": user_content }
        ],
        "temperature": 0,
        "max_tokens": 1600,
        "stream": false,
        // 根治偶发非法 JSON(qwen36 长输出漏逗号/字符串内未转义引号,实测 line N col M 解析炸):
        // vLLM guided decoding token 级保证输出合法 JSON 语法,不靠事后 find('{')..rfind('}') 补救。
        // 后端实测 json_object/guided_json/json_schema 四种约束均支持,取最通用的 json_object;
        // 三个 prompt 均含「输出只能是 JSON」字样,满足 json mode 前置要求。
        "response_format": { "type": "json_object" }
    });
    apply_review_reasoning_controls(&mut body, preset, &provider, &base_url, &model_name);
    let resp = client
        .post(url)
        .bearer_auth(bridge.api_key())
        .json(&body)
        .send()
        .await
        .context("post chat/completions")?
        .error_for_status()
        .context("chat/completions status")?;
    let value: Value = resp.json().await.context("parse chat/completions json")?;
    let content = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    parse_model_review(content).context("parse Pinvou review")
}

fn review_model_preset(bridge: &Pinvou3Bridge) -> ModelPreset {
    bridge
        .effective_model_owned()
        .map(|m| m.preset)
        .unwrap_or_else(|| bridge.prefs.advanced.model_preset.unwrap_or_default())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewReasoningDialect {
    None,
    ThinkingDisabled,
    QwenEnableThinking,
    VllmChatTemplate,
    Minimax,
}

fn apply_review_reasoning_controls(
    body: &mut Value,
    preset: ModelPreset,
    provider: &str,
    base_url: &str,
    model: &str,
) {
    match review_reasoning_dialect(preset, provider, base_url, model) {
        ReviewReasoningDialect::ThinkingDisabled => {
            body["thinking"] = json!({ "type": "disabled" });
        }
        ReviewReasoningDialect::QwenEnableThinking => {
            body["enable_thinking"] = json!(false);
        }
        ReviewReasoningDialect::VllmChatTemplate => {
            body["chat_template_kwargs"] = json!({ "enable_thinking": false });
        }
        ReviewReasoningDialect::Minimax => {
            body["thinking"] = json!({ "type": "disabled" });
            body["reasoning_split"] = json!(true);
        }
        ReviewReasoningDialect::None => {}
    }
}

fn review_reasoning_dialect(
    preset: ModelPreset,
    provider: &str,
    base_url: &str,
    model: &str,
) -> ReviewReasoningDialect {
    if provider == "vllm" || preset == ModelPreset::LocalVllm {
        return ReviewReasoningDialect::VllmChatTemplate;
    }
    if provider == "deepseek" || preset == ModelPreset::Deepseek {
        return ReviewReasoningDialect::ThinkingDisabled;
    }

    match preset {
        ModelPreset::Kimi => {
            if kimi_supports_disabled_thinking(model) {
                ReviewReasoningDialect::ThinkingDisabled
            } else {
                ReviewReasoningDialect::None
            }
        }
        ModelPreset::Qwen => ReviewReasoningDialect::QwenEnableThinking,
        ModelPreset::Doubao | ModelPreset::Glm | ModelPreset::Mimo => {
            ReviewReasoningDialect::ThinkingDisabled
        }
        ModelPreset::Minimax => ReviewReasoningDialect::Minimax,
        ModelPreset::OpenaiCompatible | ModelPreset::LocalVllm | ModelPreset::Deepseek => {
            review_reasoning_dialect_from_base_url(base_url, model)
        }
    }
}

fn review_reasoning_dialect_from_base_url(base_url: &str, model: &str) -> ReviewReasoningDialect {
    let normalized = base_url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches("/chat/completions")
        .trim_end_matches("/v1")
        .to_ascii_lowercase();

    if normalized.contains("api.deepseek.com") || normalized.contains("api.deepseeki.com") {
        ReviewReasoningDialect::ThinkingDisabled
    } else if normalized.contains("dashscope.aliyuncs.com") {
        ReviewReasoningDialect::QwenEnableThinking
    } else if normalized.contains("moonshot.cn") || normalized.contains("moonshot.ai") {
        if kimi_supports_disabled_thinking(model) {
            ReviewReasoningDialect::ThinkingDisabled
        } else {
            ReviewReasoningDialect::None
        }
    } else if normalized.contains("volces.com")
        || normalized.contains("volcengine")
        || normalized.contains("byteplus.com")
    {
        ReviewReasoningDialect::ThinkingDisabled
    } else if normalized.contains("minimax.chat") || normalized.contains("minimaxi.com") {
        ReviewReasoningDialect::Minimax
    } else if normalized.contains("bigmodel.cn") || normalized.contains("z.ai") {
        ReviewReasoningDialect::ThinkingDisabled
    } else if normalized.contains("xiaomimimo.com") {
        ReviewReasoningDialect::ThinkingDisabled
    } else {
        ReviewReasoningDialect::None
    }
}

fn kimi_supports_disabled_thinking(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    (model.contains("kimi-k2.5") || model.contains("kimi-k2.6"))
        && !model.contains("thinking")
        && !model.contains("k2.7")
}

fn parse_model_review(content: &str) -> Result<ModelReview> {
    if let Ok(v) = serde_json::from_str::<ModelReview>(content.trim()) {
        return Ok(v);
    }
    if let Some(part) = content
        .split("```")
        .find(|p| p.trim_start().starts_with('{'))
    {
        let candidate = part.trim().trim_start_matches("json").trim();
        if let Ok(v) = serde_json::from_str::<ModelReview>(candidate) {
            return Ok(v);
        }
    }
    if let (Some(start), Some(end)) = (content.find('{'), content.rfind('}')) {
        if start < end {
            return serde_json::from_str::<ModelReview>(&content[start..=end])
                .context("parse extracted json");
        }
    }
    anyhow::bail!("no JSON review in model output")
}

// ───────────────────────── 上下文投影 ─────────────────────────

fn tool_name_map(messages: &[Message]) -> HashMap<&str, &str> {
    let mut map = HashMap::new();
    for m in messages {
        for b in &m.content {
            if let ContentBlock::ToolUse { id, name, .. } = b {
                map.insert(id.as_str(), name.as_str());
            }
        }
    }
    map
}

/// 全喂：把多轮 messages 转成可读 transcript（thinking/image 等略）。
fn full_transcript(messages: &[Message]) -> String {
    let names = tool_name_map(messages);
    let mut lines = Vec::new();
    for m in messages {
        let who = if m.role == "user" { "Boss" } else { "AI" };
        for b in &m.content {
            match b {
                ContentBlock::Text { text, .. } => lines.push(format!("[{who}] {text}")),
                ContentBlock::ToolUse { name, input, .. } => {
                    let s = truncate_chars(&serde_json::to_string(input).unwrap_or_default(), 400);
                    lines.push(format!("[AI 调用 {name}] {s}"));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    let nm = names.get(tool_use_id.as_str()).copied().unwrap_or("?");
                    lines.push(format!("[工具结果·{nm}] {content}"));
                }
                _ => {}
            }
        }
    }
    lines.join("\n")
}

/// 确定性投影（§4.2，超长降级用）：Boss 原话全留 / request_user_input 决策 /
/// web_search 事实截断；丢 checklist、thinking、tool 细节。**产物不在这里**——由
/// build_context 读 workspace 文件真实内容（修 edit_file bug，§10.10）。
fn project(messages: &[Message]) -> String {
    let names = tool_name_map(messages);
    let mut boss_says = Vec::new();
    let mut decisions = Vec::new();
    let mut facts = Vec::new();
    for m in messages {
        let is_user = m.role == "user";
        for b in &m.content {
            match b {
                // B1 转交消息(applyPinvouReview 固定前缀)是 pinvou 上轮审阅回传,不是 Boss
                // 原始需求——投影排除,否则其中引用的旧数字会被当成"AI 当前状态"误导;采纳的
                // 决策已落在产物文件里(产物才是真相)。
                ContentBlock::Text { text, .. }
                    if is_user && !text.starts_with("请参考下面 Pinvou 的检阅意见") =>
                {
                    boss_says.push(text.clone())
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => match names.get(tool_use_id.as_str()).copied().unwrap_or("?") {
                    "request_user_input" => {
                        if let Ok(v) = serde_json::from_str::<Value>(content) {
                            if let Some(ans) = v.get("answers").and_then(Value::as_array) {
                                for a in ans {
                                    let id = a.get("id").and_then(Value::as_str).unwrap_or("");
                                    let label =
                                        a.get("label").and_then(Value::as_str).unwrap_or("");
                                    decisions.push(format!("{id}={label}"));
                                }
                            }
                        }
                    }
                    "web_search" => facts.push(truncate_chars(content, 200)),
                    _ => {}
                },
                _ => {}
            }
        }
    }
    let mut out = String::from("【Boss 需求】\n");
    for s in boss_says.iter().filter(|s| !s.trim().is_empty()) {
        out.push_str(&format!("- {s}\n"));
    }
    if !decisions.is_empty() {
        out.push_str("【Boss 已确认的选择】\n");
        for d in &decisions {
            out.push_str(&format!("- {d}\n"));
        }
    }
    if !facts.is_empty() {
        out.push_str("\n\n【相关事实(搜索摘录)】\n");
        for f in facts.iter().take(4) {
            out.push_str(&format!("- {f}\n"));
        }
    }
    out
}

fn truncate_chars(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if t.chars().count() < s.chars().count() {
        format!("{t}…")
    } else {
        t
    }
}

// ───────────────────────── runtime guard ─────────────────────────

/// §7 guard：issues[].persona 必须在 personas∪alternates；空 trace 给默认。
/// MVP 不校验 persona id 是否预注册（人格是"视角参数"，见设计 §7/§10.2）。
fn apply_guard(raw: ModelReview, locale_tag: &str) -> PinvouReview {
    let valid: HashSet<String> = raw
        .personas
        .iter()
        .map(|p| p.id.clone())
        .chain(raw.alternates.iter().cloned())
        .collect();
    let fallback = raw
        .personas
        .first()
        .map(|p| p.id.clone())
        .unwrap_or_default();
    let mut guard_reasons = Vec::new();
    let issues = raw
        .issues
        .into_iter()
        .map(|mut it| {
            if !it.persona.is_empty() && !valid.contains(&it.persona) {
                guard_reasons.push(format!(
                    "issue persona '{}' not in personas/alternates → {}",
                    it.persona, fallback
                ));
                it.persona = fallback.clone();
            }
            it
        })
        .collect::<Vec<_>>();
    let trace = if raw.trace.trim().is_empty() {
        default_trace(locale_tag, issues.is_empty())
    } else {
        raw.trace
    };
    PinvouReview {
        personas: raw.personas,
        alternates: raw.alternates,
        trace,
        recommendations: raw.recommendations,
        issues,
        framework: raw.framework,
        coverage: raw.coverage,
        risk: raw.risk,
        confidence: raw.confidence,
        verdict: raw.verdict,
        artifact_path: None,
        guard_reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_reasoning_controls_match_provider() {
        let mut deepseek = json!({});
        apply_review_reasoning_controls(
            &mut deepseek,
            ModelPreset::Deepseek,
            "deepseek",
            ModelPreset::Deepseek.default_base_url(),
            "deepseek-v4-pro",
        );
        assert_eq!(
            deepseek["thinking"],
            json!({ "type": "disabled" }),
            "DeepSeek official API needs the top-level thinking disable field"
        );
        assert!(deepseek.get("chat_template_kwargs").is_none());

        let mut vllm = json!({});
        apply_review_reasoning_controls(
            &mut vllm,
            ModelPreset::LocalVllm,
            "vllm",
            ModelPreset::LocalVllm.default_base_url(),
            "qwen36_35b_256k",
        );
        assert_eq!(
            vllm["chat_template_kwargs"],
            json!({ "enable_thinking": false }),
            "vLLM needs chat_template_kwargs to disable thinking"
        );
        assert!(vllm.get("thinking").is_none());
    }

    #[test]
    fn review_reasoning_controls_cover_builtin_presets() {
        let mut qwen = json!({});
        apply_review_reasoning_controls(
            &mut qwen,
            ModelPreset::Qwen,
            "openai",
            ModelPreset::Qwen.default_base_url(),
            "qwen3.6-plus",
        );
        assert_eq!(qwen["enable_thinking"], json!(false));

        for preset in [ModelPreset::Doubao, ModelPreset::Glm, ModelPreset::Mimo] {
            let mut body = json!({});
            apply_review_reasoning_controls(
                &mut body,
                preset,
                "openai",
                preset.default_base_url(),
                preset.default_model(),
            );
            assert_eq!(body["thinking"], json!({ "type": "disabled" }));
        }

        let mut minimax = json!({});
        apply_review_reasoning_controls(
            &mut minimax,
            ModelPreset::Minimax,
            "openai",
            ModelPreset::Minimax.default_base_url(),
            "MiniMax-M3",
        );
        assert_eq!(minimax["thinking"], json!({ "type": "disabled" }));
        assert_eq!(minimax["reasoning_split"], json!(true));

        let mut kimi = json!({});
        apply_review_reasoning_controls(
            &mut kimi,
            ModelPreset::Kimi,
            "moonshot",
            ModelPreset::Kimi.default_base_url(),
            "kimi-k2.6",
        );
        assert_eq!(kimi["thinking"], json!({ "type": "disabled" }));

        let mut kimi_thinking_only = json!({});
        apply_review_reasoning_controls(
            &mut kimi_thinking_only,
            ModelPreset::Kimi,
            "moonshot",
            ModelPreset::Kimi.default_base_url(),
            "kimi-k2.7-code",
        );
        assert!(kimi_thinking_only.get("thinking").is_none());
    }

    #[test]
    fn review_reasoning_controls_infer_known_openai_compatible_base_urls() {
        assert_eq!(
            review_reasoning_dialect(
                ModelPreset::OpenaiCompatible,
                "openai",
                "https://dashscope.aliyuncs.com/compatible-mode/v1",
                "qwen3.6-flash",
            ),
            ReviewReasoningDialect::QwenEnableThinking
        );
        assert_eq!(
            review_reasoning_dialect(
                ModelPreset::OpenaiCompatible,
                "openai",
                "https://open.bigmodel.cn/api/paas/v4",
                "glm-5.1",
            ),
            ReviewReasoningDialect::ThinkingDisabled
        );
        assert_eq!(
            review_reasoning_dialect(
                ModelPreset::OpenaiCompatible,
                "openai",
                "https://api.openai.com/v1",
                "gpt-4o",
            ),
            ReviewReasoningDialect::None
        );
    }

    /// 核账跳过哪些动作的契约：只有 accept(接受现状)/confirmed(需核实已确认)算已结，其余
    /// 都保留核。漏掉 confirmed 会让 Boss 已确认的账被重核→震荡(pkx4clhny5jd0 实测暴露)。
    #[test]
    fn ledger_closed_only_for_accept_and_confirmed() {
        assert!(is_ledger_closed(Some("accept")));
        assert!(is_ledger_closed(Some("confirmed")));
        for open in ["modify", "verify", "adopt", "ask", "pending"] {
            assert!(!is_ledger_closed(Some(open)), "{open} 不该算已结");
        }
        assert!(!is_ledger_closed(None));
    }

    /// 连点品(产物没变,current_pos==最近 pass 的 pos)→ 核最近立账、稳定 pass,不重新立账=不震荡。
    #[test]
    fn ledger_repeat_pin_stable_no_relist() {
        let arr = vec![
            serde_json::json!({"pos":10,"review":{"artifact_path":"/p.md","issues":[
                {"text":"交通错","severity":"high","kind":"quality","resolution":"modify"}
            ]}}),
            serde_json::json!({"pos":12,"review":{"artifact_path":"/p.md","verdict":"pass","issues":[]}}),
        ];
        let kept = ledger_from_entries(&arr, "/p.md", 12).expect("连点品核最近立账");
        assert_eq!(kept.len(), 1, "连点品不重新立账,核最近立账");
        assert_eq!(kept[0].text, "交通错");
    }

    /// 最近一轮 pass 之后产物又被改(current_pos > 那轮 pos,说明悟/AI 动过)→ None,品重新立账审增量。
    #[test]
    fn ledger_relist_after_pass_when_product_changed() {
        let arr = vec![
            serde_json::json!({"pos":10,"review":{"artifact_path":"/p.md","issues":[
                {"text":"交通错","severity":"high","kind":"quality","resolution":"modify"}
            ]}}),
            serde_json::json!({"pos":12,"review":{"artifact_path":"/p.md","verdict":"pass","issues":[]}}),
        ];
        assert!(
            ledger_from_entries(&arr, "/p.md", 20).is_none(),
            "产物改过→重新立账审增量"
        );
    }

    /// 多轮立账:读【最近一轮立账】(悟后重新立的签证账),不是首轮(交通)。
    #[test]
    fn ledger_reads_latest_round_not_first() {
        let arr = vec![
            serde_json::json!({"pos":10,"review":{"artifact_path":"/p.md","issues":[
                {"text":"交通错","severity":"high","kind":"quality","resolution":"modify"}
            ]}}),
            serde_json::json!({"pos":12,"review":{"artifact_path":"/p.md","verdict":"pass","issues":[]}}),
            serde_json::json!({"pos":20,"review":{"artifact_path":"/p.md","issues":[
                {"text":"签证错","severity":"high","kind":"quality","resolution":"modify"}
            ]}}),
        ];
        let kept = ledger_from_entries(&arr, "/p.md", 20).expect("读最近立账");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].text, "签证错", "读最近立账(签证),不是首轮(交通)");
    }

    /// 悟 record 的 coverage entry(issues 空+无 verdict)绝不能当 latest 让守门失效:pass→悟→AI改,
    /// 品仍应重新立账审增量(latest 跳过悟、认最近的品 pass)。
    #[test]
    fn ledger_skips_coverage_entry_for_relist() {
        let arr = vec![
            serde_json::json!({"pos":10,"review":{"artifact_path":"/p.md","issues":[
                {"text":"交通错","severity":"high","kind":"quality","resolution":"modify"}
            ]}}),
            serde_json::json!({"pos":12,"review":{"artifact_path":"/p.md","verdict":"pass","issues":[]}}),
            // 悟:coverage 非空、issues 空、无 verdict(pos 同 pass,悟召唤不发消息)
            serde_json::json!({"pos":12,"review":{"artifact_path":"/p.md","issues":[],"coverage":[
                {"dimension":"签证","severity":"high"}
            ]}}),
        ];
        assert!(
            ledger_from_entries(&arr, "/p.md", 20).is_none(),
            "悟 entry 被跳过,pass 后改产物→重新立账审增量"
        );
    }

    /// 覆盖镜头:framework + coverage 必须从 ModelReview 透传进 PinvouReview,不被 guard 丢。
    #[test]
    fn guard_preserves_framework_and_coverage() {
        let raw = ModelReview {
            framework: vec!["签证".into(), "保险".into()],
            coverage: vec![PinvouGap {
                dimension: "签证".into(),
                coverage: "missing".into(),
                severity: "high".into(),
                text: "缺".into(),
                suggestion: "补".into(),
            }],
            ..Default::default()
        };
        let review = apply_guard(raw, "zh-Hans");
        assert_eq!(review.framework, vec!["签证", "保险"]);
        assert_eq!(review.coverage.len(), 1);
        assert_eq!(review.coverage[0].dimension, "签证");
    }

    /// 非中文 locale 追加输出语言指令(覆盖 prompt 的中文措辞);zh-Hans/未知 → None(no-op)。
    #[test]
    fn output_language_directive_per_locale() {
        let en = output_language_directive("en").expect("en 应有指令");
        assert!(en.contains("English") && en.contains("OVERRIDES"));
        assert!(output_language_directive("ja")
            .unwrap()
            .contains("Japanese"));
        assert!(
            output_language_directive("zh-Hans")
                .unwrap()
                .contains("简体中文"),
            "中文也要强制输出语言(实测在英文产物上会漂英文)"
        );
        assert!(output_language_directive("fr").is_none(), "未知回退 no-op");
    }

    /// 空 trace 兜底跟随 locale。
    #[test]
    fn default_trace_follows_locale() {
        assert_eq!(default_trace("zh-Hans", true), "看过了，没问题。");
        assert!(default_trace("en", true).starts_with("Looked"));
        assert!(default_trace("en", false).contains("confirm"));
        assert!(default_trace("ja", true).contains("問題ありません"));
    }

    fn user_text(t: &str) -> Message {
        Message {
            role: "user".into(),
            content: vec![ContentBlock::Text {
                text: t.into(),
                cache_control: None,
            }],
        }
    }

    fn assistant_tool(id: &str, name: &str, input: Value) -> Message {
        Message {
            role: "assistant".into(),
            content: vec![ContentBlock::ToolUse {
                id: id.into(),
                name: name.into(),
                input,
                caller: None,
            }],
        }
    }

    fn tool_result(tid: &str, content: &str) -> Message {
        Message {
            role: "user".into(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tid.into(),
                content: content.into(),
                is_error: None,
                content_blocks: None,
            }],
        }
    }

    #[test]
    fn project_keeps_boss_words_and_facts_drops_noise() {
        let messages = vec![
            user_text("帮我做巴黎行程，预算 2 万，照顾老人"),
            assistant_tool("a1", "web_search", json!({"query": "巴黎"})),
            tool_result("a1", "人均报价：拓展 800 元……"),
            assistant_tool("a2", "checklist_update", json!({"id": 1})),
            tool_result("a2", "Todo #1 done"),
        ];
        let p = project(&messages);
        assert!(p.contains("预算 2 万"), "Boss 原话要留: {p}");
        assert!(!p.contains("Todo #1"), "checklist 噪音要丢: {p}");
        assert!(p.contains("人均报价"), "web_search 事实要留: {p}");
        // 产物不在 project（由 build_context 读 workspace 文件负责），这里只管需求+事实
    }

    #[test]
    fn last_artifact_path_picks_latest_including_edit_file() {
        let messages = vec![
            assistant_tool(
                "a1",
                "write_file",
                json!({"path": "plan.md", "content": "v1"}),
            ),
            assistant_tool(
                "a2",
                "edit_file",
                json!({"path": "plan.md", "old_string": "v1", "new_string": "v2"}),
            ),
            assistant_tool(
                "a3",
                "edit_file",
                json!({"path": "plan.md", "old_string": "v2", "new_string": "v3"}),
            ),
        ];
        // 修 bug 关键：edit_file 也算产物，取最后一次的 path（旧逻辑只认 write_file 会漏）
        assert_eq!(last_artifact_path(&messages), Some("plan.md".to_string()));
    }

    #[test]
    fn reconcile_context_injects_prior_ledger() {
        let prior = vec![PinvouIssue {
            severity: "high".into(),
            kind: "quality".into(),
            persona: "travel".into(),
            text: "预算自相矛盾".into(),
            suggestion: "对齐".into(),
        }];
        let messages = vec![user_text("规划行程")];
        let ctx = build_reconcile_context(&prior, &messages, std::path::Path::new("/tmp"));
        assert!(ctx.starts_with("【上轮账目】"), "核账注入账目: {ctx}");
        assert!(ctx.contains("预算自相矛盾"), "账目内容在: {ctx}");
    }

    #[test]
    fn project_excludes_b1_transfer_messages() {
        let messages = vec![
            user_text("帮我规划欧洲游，预算 2 万"),
            user_text("请参考下面 Pinvou 的检阅意见，修改前面的内容：\n- 【预算】采纳 8.5w（国庆 5-7w 难实现）"),
        ];
        let p = project(&messages);
        assert!(p.contains("帮我规划欧洲游"), "Boss 原话保留: {p}");
        assert!(!p.contains("5-7w"), "转交消息(含上轮旧数字)要排除: {p}");
    }

    #[test]
    fn project_extracts_request_user_input_decisions() {
        let messages = vec![
            user_text("订行程"),
            assistant_tool("q1", "request_user_input", json!({})),
            tool_result("q1", r#"{"answers":[{"id":"transport","label":"火车"}]}"#),
        ];
        let p = project(&messages);
        assert!(p.contains("transport=火车"), "Boss 选择要进需求: {p}");
    }

    #[test]
    fn build_context_full_feeds_short_session() {
        let messages = vec![user_text("一句短需求")];
        let ctx = build_context(&messages, std::path::Path::new("/tmp"));
        assert!(ctx.contains("完整对话记录"), "短 session 应全喂: {ctx}");
        assert!(ctx.contains("[Boss] 一句短需求"));
    }

    #[test]
    fn build_context_projects_when_over_limit() {
        let big = "约束".repeat(FULL_FEED_CHAR_LIMIT); // 远超阈值
        let messages = vec![user_text(&big)];
        let ctx = build_context(&messages, std::path::Path::new("/tmp"));
        assert!(
            ctx.starts_with("【Boss 需求】"),
            "超长应投影: 头部={}",
            &ctx[..30.min(ctx.len())]
        );
    }

    #[test]
    fn guard_remaps_unknown_issue_persona_to_primary() {
        let raw = ModelReview {
            personas: vec![PinvouPersona {
                id: "travel".into(),
                label: "旅行规划".into(),
                primary: true,
            }],
            alternates: vec!["budget".into()],
            trace: "有问题".into(),
            recommendations: vec![PinvouRecommendation {
                topic: "预算".into(),
                pick: "中档".into(),
                why: "稳妥".into(),
            }],
            issues: vec![
                PinvouIssue {
                    severity: "high".into(),
                    kind: "risk".into(),
                    persona: "legal".into(), // 不在 personas∪alternates
                    text: "x".into(),
                    suggestion: "y".into(),
                },
                PinvouIssue {
                    severity: "low".into(),
                    kind: "quality".into(),
                    persona: "budget".into(), // 在 alternates，保留
                    text: "z".into(),
                    suggestion: "".into(),
                },
            ],
            risk: Some("high".into()),
            confidence: Some(0.8),
            verdict: None,
            framework: vec![],
            coverage: vec![],
        };
        let r = apply_guard(raw, "zh-Hans");
        assert_eq!(r.issues[0].persona, "travel", "未知 persona 归到 primary");
        assert_eq!(r.issues[1].persona, "budget", "合法 persona 保留");
        assert!(r.guard_reasons.iter().any(|g| g.contains("legal")));
        assert_eq!(r.recommendations.len(), 1, "recommendations 透传保留");
    }

    #[test]
    fn guard_fills_empty_trace_for_clean_review() {
        let raw = ModelReview {
            personas: vec![PinvouPersona {
                id: "writing".into(),
                label: "文字编辑".into(),
                primary: true,
            }],
            trace: "  ".into(),
            issues: vec![],
            ..Default::default()
        };
        let r = apply_guard(raw, "zh-Hans");
        assert_eq!(r.trace, "看过了，没问题。");
    }
}
