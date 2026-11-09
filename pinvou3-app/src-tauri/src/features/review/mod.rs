//! Pinvou v4 召唤式检阅。
//!
//! Boss 主动召唤 → 从 session messages 投影出「需求 vs 应对」→ 单次独立 LLM
//! 审查 → 返回 personas/issues。Pinvou 只检阅、不替 Boss 决策。
//!
//! 设计与实证：`docs/品悟v4-召唤式评审设计.md`（§6 上下文策略、§7 实证）。
//! 上下文策略（§6，实测背书）：能全喂就全喂（实测 1.2 万 token 级高噪音
//! 会话不崩、还能事实核查），超过 `FULL_FEED_CHAR_LIMIT` 才降级到确定性投影。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use deepseek_tui::config::ApiProvider;
use deepseek_tui::models::{ContentBlock, Message};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::features::assistant::platform::bridge::{Pinvou3Bridge, prefs::ModelPreset};

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

/// 核账模式 prompt（§5，sim 验证收敛）：对同一产出物的复审，只核账、禁新增、终态。
/// 治复审不收敛——暴增/翻案/永久挂账/无终态。
const RECONCILE_PROMPT: &str = r#"你是 Pinvou，Boss 身边的独立检阅顾问。这是对**同一产出物**的复审（核账模式），不是重新自由批评。
给你【上轮账目】（上次立的问题）+【当前产物】（已修订版本）。严格按规则核账：
1. 逐条核对账目对照产物：**已改好的不要再列进 issues**（视为闭合）；只把**没改/没改对**的留在 issues 里，说明还差什么。
2. **禁止新增问题**。唯一例外：本次修订在它改动的段落内新引入的错误（issue 里注明"修订引入"）。不要提上轮没提过的新角度。
3. 已结的账不要翻，除非有新证据。
4. 外部事实（交通/票价/签证政策）你无外部知识，只能 kind="needs_verify" 标「需核实」、severity≤medium，绝不标 high 硬伤、绝不断言"不存在/没有"。
5. **终态**：所有账目都闭合（issues 为空）→ verdict="pass"。**pass 时 trace 必须逐条点名每笔账目的核对结论**（如"①交通：已改为巴黎直飞苏黎世✓ ②日期：已锁定5月中下旬✓ ③签证：已补主停留国原则✓"），让 Boss 看到你逐条对过产物、不是空泛放行；还有没闭合的 → verdict="continue"。
输出只能是 JSON：{"verdict":"pass|continue","trace":"pass 时逐条交代各账目核对结论，continue 时一句话","issues":[{"severity":"low|medium|high","kind":"...","text":"...","suggestion":"..."}],"risk":"low|medium|high"}"#;

/// 覆盖镜头 prompt（§2 悟镜头，多场景 sim 实测背书）：不挑错，查"全不全"。领域无关——
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

/// 本地审查实测 5–19s，旧版 5s 必挂；30s 又被云端思考模型击穿（k3/Kimi
/// Coding Plan 2026-11-06 实测悟镜头 63s 才返回，30s 必超时），放宽到 180s
/// 给思考模型留余量，同时不让挂起的请求把弹窗锁死太久。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);
// 编译期钉：30s 墙正是本常量修掉的回归（k3 实测 45–63s），静默回退必须编译红
// 而不是用户红。
const _: () = assert!(DEFAULT_TIMEOUT.as_secs() == 180);

/// 全喂阈值（字符数；中英混 ÷1.7 ≈ token）。实测全喂 2 万字级长会话
/// （≈1.2 万 token）不崩，所以 24000 字以内直接全喂，超过才投影降级（§6）。
const FULL_FEED_CHAR_LIMIT: usize = 24_000;

/// 产物上限（字符）。产物真相在 workspace 文件，整篇读、整篇喂——本地 256k 扛得住
/// （实测 2 万字全喂稳）。常规产物（文档/邮件/代码）都在此内整篇喂；超此才截断+标注，
/// 真超大产物的「不漏」解是 P1 map-reduce 分块全审。
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

/// Boss 决策点的推荐（§2 recommendations 各进各篮）。topic=待决策点，pick=推荐项，why=理由。
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

/// guard 后返回给前端的审查结果（§3 弹窗裁决）。
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
    /// 核账模式终态：pass=通过可交付 / continue=还有未结账目（首轮模式为 None）。
    pub verdict: Option<String>,
    /// 这次审的产出物 path，存进 sidecar 供下次召唤核账匹配同一产出物。
    pub artifact_path: Option<String>,
    /// guard diagnostic log: consumed by neither production code nor the frontend, only by tests asserting guard routing;
    /// builds other than tests do not compile this field.
    #[cfg(test)]
    #[serde(skip_serializing)]
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
    pub coverage: Vec<PinvouGap>,
    verdict: Option<String>,
}

/// 召唤入口：从 session messages + workspace 审查。同一产出物的复审走核账模式（§5）。
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
    // 覆盖体检模式(§2 悟镜头,独立入口):查产物"全不全"。复用 build_context(全喂需求+产物文件)，
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

/// 需求/事实/对话脉络走全喂或投影（§6）；**产物单独读 workspace 文件真实内容**附在
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

fn artifact_paths_from_input(input: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    let mut push = |path: &str| {
        let path = path.trim();
        if !path.is_empty() && !paths.iter().any(|existing| existing == path) {
            paths.push(path.to_string());
        }
    };
    for key in ["path", "file_path", "filename"] {
        if let Some(path) = input.get(key).and_then(Value::as_str) {
            push(path);
        }
    }
    for key in ["replace", "changes"] {
        for change in input
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            for path_key in ["path", "file_path", "filename"] {
                if let Some(path) = change.get(path_key).and_then(Value::as_str) {
                    push(path);
                }
            }
        }
    }
    if let Some(patch) = input.get("patch").and_then(Value::as_str) {
        for line in patch.lines() {
            if let Some(path) = ["*** Add File:", "*** Update File:", "*** Delete File:"]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
            {
                push(path);
            } else if let Some(path) = line.strip_prefix("+++ ") {
                let path = path.strip_prefix("b/").unwrap_or(path);
                if path != "/dev/null" {
                    push(path);
                }
            }
        }
    }
    paths
}

/// 最后被 write_file/edit_file/File.write/File.edit/File.patch/present_artifact 碰过的产物 path。
/// **含 edit 是修 bug 的关键**：旧逻辑只认 write_file，长 session 用 edit_file 迭代
/// 就看不到最新态；v0.9.5 起写操作统一走 canonical `File` action=write/edit。
fn last_artifact_path(messages: &[Message]) -> Option<String> {
    let mut last = None;
    for m in messages {
        for b in &m.content {
            if let ContentBlock::ToolUse { name, input, .. } = b {
                let is_file_write = (name.as_str() == "File"
                    && input
                        .get("action")
                        .and_then(Value::as_str)
                        .is_some_and(|action| matches!(action, "write" | "edit" | "patch")))
                    || matches!(name.as_str(), "write_file" | "edit_file" | "apply_patch");
                if is_file_write || name.as_str() == "present_artifact" {
                    if let Some(path) = artifact_paths_from_input(input).into_iter().last() {
                        last = Some(path);
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
    // read_to_string 对二进制产物(.docx/.pdf/.xlsx 等)静默失败返回 None——该类
    // session 退化成只凭对话流审查。文本抽取是后续功能候选(P1),当前形态刻意
    // 如此(2026-11-09 审:把已知限制写进代码,别只活在 PR 描述里)。
    let content = std::fs::read_to_string(&abs).ok()?;
    let n = content.chars().count();
    let bounded = if n > ARTIFACT_CHAR_LIMIT {
        let head: String = content.chars().take(ARTIFACT_CHAR_LIMIT).collect();
        format!(
            "{head}\n\n…（产物共 {n} 字，这里是前 {ARTIFACT_CHAR_LIMIT} 字；后半未展示，如需审让 Boss 指定。P1 改 map-reduce 分块全审）"
        )
    } else {
        content
    };
    Some((path, bounded))
}

/// 账目是否已被 Boss 结清、核账不再核（§5）：accept=缺陷接受现状、confirmed=需核实但 Boss
/// 已确认没问题。其余(modify/verify/adopt/ask/pending)都保留进核账。kind 分流后新增的 confirmed
/// 也得跳过，否则 Boss 已确认的账会被重核→震荡。
fn is_ledger_closed(resolution: Option<&str>) -> bool {
    matches!(resolution, Some("accept") | Some("confirmed"))
}

/// 读 sidecar(`pinvou_reviews.json`)里针对同一产出物的最近一轮账目(issues)。有则进
/// 核账模式（§5，激活原未做项「连续召唤接续」）。sidecar 由前端 recordPinvouReview 落盘，后端只读。
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
    // 最近一轮【品】entry。**跳过【悟】的 coverage entry**(issues 空+无 verdict)
    // ——否则悟 record 的 entry 会当 latest、让 current_pos 守门失效(悟非 pass→
    // 永远不触发重新立账,品又读不到 pass 后的增量)。
    // 序选择:r4 审 m5——entry 带 `seq`(召唤序号,recordPinvouReview 写入)时取
    // **seq 最大的**,不是从后往前第一个。dismiss→再召唤的并发窗口里新 settle
    // 先落、被顶掉的旧 settle 后落,纯"最后一条"会把 Boss 没看过的旧审查当
    // 最近一轮,核账对着过期 verdict 审。老 entry(无 seq)按 0 计,稳定性不受影响。
    let latest = arr
        .iter()
        .filter(|e| {
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
                    .is_some_and(|a| !a.is_empty())
        })
        .max_by_key(|e| e.get("seq").and_then(Value::as_u64).unwrap_or(0))?;
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
    // pass 轮 issues 空会被跳过;accept/confirmed 已结的过滤(§5,is_ledger_closed)。
    arr.iter().rev().find_map(|entry| {
        let rv = entry.get("review")?;
        if rv.get("artifact_path").and_then(Value::as_str)? != artifact_path {
            return None;
        }
        if rv
            .get("issues")
            .and_then(Value::as_array)
            .is_none_or(|a| a.is_empty())
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

/// 核账上下文：上轮账目 + 当前产物文件真实内容（§5）。核账只对账，不喂全会话。
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

/// guard 空 trace 兜底文案,跟随 locale。模型正常会按 output_language_directive 返回本地化 trace,
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
    // Local vLLM: correct the model name against the server's served list
    // (resolve_served_model) — after a server-side --served-model-name change
    // the configured name would 404 model_not_found (review issues its own
    // HTTP calls and does not go through the engine's fresh_bridge_for
    // correction). A configured name the server lists must be kept verbatim
    // (LM Studio/Ollama list every downloaded model; the first entry is
    // unrelated to the configuration). On probe failure fall back to the
    // configured value; cloud providers are not probed.
    let provider = bridge.provider();
    let preset = review_model_preset(bridge);
    let model_name = if provider == "vllm" || provider == "tachyon" {
        // The served-name probe uses an inference-same-origin key:
        // authenticated vLLM 401s on /v1/models.
        crate::features::monitor::resolve_served_model(
            &base_url,
            Some(bridge.api_key().as_str()),
            &bridge.model(),
        )
        .await
        .0
    } else {
        bridge.model()
    };
    // 非中文 locale 追加输出语言指令(覆盖 prompt 里的「中文」措辞)。
    let prompt = match output_language_directive(bridge.locale_tag()) {
        Some(suffix) => format!("{prompt}{suffix}"),
        None => prompt.to_string(),
    };
    // 输出预算与主对话同一算法(2026-11-06 产品决策):走
    // `main_wire_max_output_tokens`(底座 route-effective cap,含 explicit
    // override/兼容上限/route 事实/窗口裁剪),不再硬编码——思考常开后预算必须
    // 容得下 reasoning tokens,硬编码值会随模型漂移。
    let max_tokens = bridge.main_wire_max_output_tokens(&model_name);
    // Anthropic 官方端点是 Messages 协议（x-api-key 鉴权，system 独立字段，
    // 无 response_format），走原生直连；其余 preset 仍走 OpenAI chat/completions。
    // 已知缺口（"永不关闭"口径未覆盖，见 apply_review_thinking_controls 文档尾）：
    // 这里不发思考字段——省略即思考关（fable/mythos/opus-5/sonnet-5/haiku-5 之外
    // 无 adaptive 默认）。底座 Messages adapter 的 low 档 shaper 未公开；宿主侧
    // 手抄型号启发会再造漂移面，待底座暴露后补齐。
    if preset == ModelPreset::Anthropic {
        let content = crate::core::model_endpoint::post_anthropic_messages(
            &client,
            &base_url,
            &bridge.api_key(),
            &model_name,
            &prompt,
            user_content,
            max_tokens,
            bridge.opencode_conversation_key("model-review"),
        )
        .await?
        .text;
        return parse_model_review(&content).context("parse Pinvou review");
    }
    // 路由身份与主对话同一解析：engine cfg.provider 正是 `provider()` 的产物
    // （engine_route_provider 除 Responses 改名外恒等），底座 `api_provider()`
    // 按 ApiProvider::parse（大小写不敏感 + 别名表 + deepseek-cn 特例）解析，
    // 不可解析兜底 Deepseek。审查按同一 ApiProvider 分类，raw 字符串臂全部
    //退役——规范 slug/大小写/别名拼写不再与引擎劈开（2026-11-07 r3 审 M3/M7）。
    let api = bridge.build_dt_config().api_provider();
    // Token 上限字段同样镜像引擎（review_max_tokens_field 文档注）：换名集合里的
    // 路由发 max_completion_tokens，其余保持 max_tokens。
    let max_tokens_field = review_max_tokens_field(api, &base_url, &model_name);
    let mut body = review_request_body(
        &model_name,
        &prompt,
        user_content,
        max_tokens,
        max_tokens_field,
    );
    apply_review_thinking_controls(&mut body, api, preset, &base_url, &model_name);
    let resp = crate::core::model_endpoint::with_opencode_session_header(
        client.post(url).bearer_auth(bridge.api_key()),
        &base_url,
        bridge.opencode_conversation_key("model-review"),
    )
    .json(&body)
    .send()
    .await
    .context("post chat/completions")?;
    let resp =
        crate::core::model_endpoint::error_for_status_with_body(resp, "chat/completions status")
            .await?;
    let value: Value = resp.json().await.context("parse chat/completions json")?;
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .cloned()
        .unwrap_or_default();
    let content = choice
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let finish_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .map(str::to_owned);
    parse_model_review_with_finish(content, finish_reason.as_deref()).context("parse Pinvou review")
}

/// One-shot Chat Completions body for model review. Field set mirrors the
/// main-session engine wire (`model`/`messages`/`max_tokens`/`stream`, and —
/// pinned by foundation tests — **no `temperature`**): a hard-coded 0 400s on
/// gateways that pin sampling server-side (Kimi Coding Plan, live-probed
/// 2026-09-30: "invalid temperature: only 1 is allowed for this model"; the
/// image-capability probe hit the same constraint on 2026-08). Single source
/// so the wire-drift budget stays testable. `max_tokens` is the caller's
/// route-effective budget (`bridge::main_wire_max_output_tokens`, the same
/// algorithm as the main turn — a hard-coded constant drifted the moment a
/// thinking-on budget was needed: the old hard-coded cap was consumed entirely
/// by k3 reasoning tokens, content empty, finish_reason=length, 2026-11-06).
/// The literal digits are kept out of this doc comment on purpose: the call-site
/// region pin asserts the region never mentions them (a re-inlined constant
/// would otherwise satisfy a contains-only pin).
/// `max_tokens_field` is the caller's `review_max_tokens_field` decision —
/// the engine swaps the field to `max_completion_tokens` on four route
/// classes and the review must not mismatch them. Thinking controls (never
/// disable; lowest tier) are layered on by `apply_review_thinking_controls`.
fn review_request_body(
    model: &str,
    prompt: &str,
    user_content: &str,
    max_tokens: u32,
    max_tokens_field: &'static str,
) -> Value {
    json!({
        "model": model,
        "messages": [
            { "role": "system", "content": prompt },
            { "role": "user", "content": user_content }
        ],
        (max_tokens_field): max_tokens,
        "stream": false,
        // 根治偶发非法 JSON(qwen36 长输出漏逗号/字符串内未转义引号,实测 line N col M 解析炸):
        // vLLM guided decoding token 级保证输出合法 JSON 语法,不靠事后 find('{')..rfind('}') 补救。
        // 后端实测 json_object/guided_json/json_schema 四种约束均支持,取最通用的 json_object;
        // 三个 prompt 均含「输出只能是 JSON」字样,满足 json mode 前置要求。
        // Deliberate additive drift from the engine wire (the engine sends
        // none): vLLM guided decoding verified by the team when this landed;
        // Kimi Coding Plan live-verified accepted 2026-09-30.
        "response_format": { "type": "json_object" }
    })
}

fn review_model_preset(bridge: &Pinvou3Bridge) -> ModelPreset {
    bridge
        .effective_model_owned()
        .map(|m| m.preset)
        .unwrap_or_else(|| bridge.prefs.advanced.model_preset.unwrap_or_default())
}

/// Byte-for-byte mirror of the foundation's exact-route predicate
/// `is_exact_https_route`（CodeWhale crates/config/src/provider.rs）: scheme
/// and host compare ASCII case-insensitively, the path is case-sensitive, and
/// exactly one trailing slash is insignificant（端口属于 authority，故
/// `ollama.com:443` 是另一 authority）。
fn is_exact_https_host_path(base_url: &str, host: &str, path: &str) -> bool {
    let trimmed = base_url.trim();
    let trimmed = trimmed.strip_suffix('/').unwrap_or(trimmed);
    trimmed
        .split_once("://")
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("https"))
        .and_then(|(_, rest)| rest.split_once('/'))
        .is_some_and(|(url_host, url_path)| url_host.eq_ignore_ascii_case(host) && url_path == path)
}

/// Moonshot 的两个 K3 路由（底座 config.rs 守卫逐字镜像）：Kimi Code 会员端点
/// 是精确 `api.kimi.com/coding/v1`；直连按量平台是精确 `api.moonshot.ai|cn /v1`。
fn is_exact_kimi_code_route(base_url: &str) -> bool {
    is_exact_https_host_path(base_url, "api.kimi.com", "coding/v1")
}

fn is_exact_moonshot_platform_route(base_url: &str) -> bool {
    is_exact_https_host_path(base_url, "api.moonshot.ai", "v1")
        || is_exact_https_host_path(base_url, "api.moonshot.cn", "v1")
}

/// 底座 `modelstudio_model_is_thinking_only`（client/chat.rs）名单逐字镜像：
/// 这些 Model Studio 托管 id 没有 enable/disable 控制——"at best ignored and at
/// worst a 400"。底座按 Modelstudio 身份路由限定；第一方 Kimi 身份（Moonshot
/// kind）上通用臂照发 `thinking: enabled`，审查同一照发（2026-11-07 r3 审 m2：
/// 抑制收窄到身份门控，与底座同形）。
fn modelstudio_model_is_thinking_only(model: &str) -> bool {
    let lowered = model.trim().to_ascii_lowercase();
    matches!(
        lowered.as_str(),
        "qwen3.8-max"
            | "qwen3.8-max-preview"
            | "kimi-k2.7-code"
            | "kimi/kimi-k2.7-code"
            | "kimi/kimi-k2.7-code-highspeed"
    )
}

/// 底座 `modelstudio_model_is_hybrid`（client/chat.rs）逐字镜像：有
/// enable_thinking 开关的 hybrid 名单（前缀族）。
fn modelstudio_model_is_hybrid(model: &str) -> bool {
    let lowered = model.trim().to_ascii_lowercase();
    lowered.starts_with("qwen3.7-")
        || lowered.starts_with("qwen3.6-")
        || lowered.starts_with("qwen3.5-")
        || lowered.starts_with("qwen3-")
        || lowered.starts_with("deepseek-v4")
        || lowered.starts_with("deepseek-v3.2")
        || lowered.starts_with("deepseek-v3.1")
        || lowered.starts_with("kimi-k2.6")
        || matches!(lowered.as_str(), "kimi/kimi-k2.6")
        || lowered.starts_with("kimi-k2.5")
        || lowered.starts_with("glm-")
}

/// 底座 `modelstudio_model_supports_preserve_thinking`（client/chat.rs）逐字镜像。
fn modelstudio_model_supports_preserve_thinking(model: &str) -> bool {
    let lowered = model.trim().to_ascii_lowercase();
    matches!(
        lowered.as_str(),
        "qwen3.7-max"
            | "qwen3.7-max-us"
            | "qwen3.7-max-2026-05-17"
            | "qwen3.7-max-2026-05-20"
            | "qwen3.7-max-2026-06-08"
            | "qwen3.7-max-preview"
            | "qwen3.7-plus"
            | "qwen3.7-plus-us"
            | "qwen3.7-plus-2026-05-26"
            | "qwen3.6-max-preview"
            | "qwen3.6-plus"
            | "qwen3.6-plus-2026-04-02"
            | "qwen3.6-flash"
            | "qwen3.6-flash-2026-04-16"
            | "kimi-k2.6"
            | "kimi-k2.7-code"
            | "kimi/kimi-k2.6"
            | "kimi/kimi-k2.7-code"
            | "kimi/kimi-k2.7-code-highspeed"
    )
}

/// 底座 `modelstudio_model_supports_reasoning_effort`（client/chat.rs）逐字镜像。
fn modelstudio_model_supports_reasoning_effort(model: &str) -> bool {
    let lowered = model.trim().to_ascii_lowercase();
    lowered.starts_with("deepseek-v4")
        || matches!(lowered.as_str(), "glm-5.2" | "glm-5.1" | "glm-5")
}

/// 底座 `modelstudio_reasoning_effort_for_model` 的 low 档结果：审查恒定钉最低档
/// low，而 Model Studio 把 low/medium 折叠为 high——镜像折叠，不发 low。
const MODELSTUDIO_LOW_WIRE_EFFORT: &str = "high";

/// 底座 `is_exact_modelstudio_chat_route`（client/chat.rs）逐字镜像：只认
/// Alibaba 自家 OpenAI 兼容 Chat 端点（Token Plan 默认与 `{workspace}.<region>`
/// 工作区 .maas 主机、Coding Plan intl 主机、经典 dashscope/intl compatible-mode）。
/// 其余（代理/自建网关/拼错）一律 fail-close。
fn is_exact_modelstudio_chat_route(base_url: &str) -> bool {
    let trimmed = base_url.trim().trim_end_matches('/').to_ascii_lowercase();
    let Some((host, path)) = trimmed
        .strip_prefix("https://")
        .and_then(|rest| rest.split_once('/'))
    else {
        return false;
    };
    let token_plan_chat = host.ends_with(".maas.aliyuncs.com") && path == "compatible-mode/v1";
    let coding_plan_chat = host == "coding-intl.dashscope.aliyuncs.com" && path == "v1";
    let classic_dashscope_chat = matches!(
        host,
        "dashscope.aliyuncs.com" | "dashscope-intl.aliyuncs.com"
    ) && path == "compatible-mode/v1";
    token_plan_chat || coding_plan_chat || classic_dashscope_chat
}

/// 底座 `is_exact_zai_chat_route`（config/provider.rs:577-582）逐字镜像：三条
/// 精确第一方 z.ai/bigmodel Chat 端点。
fn is_exact_zai_chat_route(base_url: &str) -> bool {
    is_exact_https_host_path(base_url, "api.z.ai", "api/coding/paas/v4")
        || is_exact_https_host_path(base_url, "api.z.ai", "api/paas/v4")
        || is_exact_https_host_path(base_url, "open.bigmodel.cn", "api/paas/v4")
}

/// 底座 GLM 常数（config/provider_defaults.rs:144-148）镜像。forced-thinking =
/// GLM-5.3/-Flash（`thinking:{disabled}` 被拒、档位只有 low/high/max，漏发档位 =
/// API 文档默认 max）；tiered = 5.2 与 forced 族；known = tiered ∪ 5.1 ∪ 5-Turbo。
fn zai_model_is(model: &str, expected: &str) -> bool {
    model.trim().eq_ignore_ascii_case(expected)
}

fn zai_model_is_forced_thinking(model: &str) -> bool {
    zai_model_is(model, "GLM-5.3") || zai_model_is(model, "GLM-5.3-Flash")
}

fn zai_model_is_known_reasoning(model: &str) -> bool {
    zai_model_is_forced_thinking(model)
        || zai_model_is(model, "GLM-5.2")
        || zai_model_is(model, "GLM-5.1")
        || zai_model_is(model, "GLM-5-Turbo")
}

/// 底座 `is_exact_google_chat_route`（client/chat.rs）逐字镜像：Google 官方
/// OpenAI 兼容端点。`google.thinking_config.thinking_level` 只写在该路由。
fn is_exact_google_chat_route(base_url: &str) -> bool {
    is_exact_https_host_path(
        base_url,
        "generativelanguage.googleapis.com",
        "v1beta/openai",
    )
}

/// 底座 `canonical_minimax_model_id`（tui config.rs:1316-1347）的 M3 收敛分支
/// 镜像：`minimax`/`minimax-m3`/`minimax-m-3`/`minimax-m-3-thinking` 都收敛到
/// 规范 id `MiniMax-M3`。引擎的换名/路由判定吃规范化后的线材型号，审查同形
/// （2026-11-07 r3 审 M8：原文比较让手写别名拼写劈开字段决策）。
fn canonical_minimax_model_is_m3(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized.replace(['_', ' '], "-");
    matches!(
        normalized.as_str(),
        "minimax" | "minimax-m3" | "minimax-m-3" | "minimax-m-3-thinking"
    )
}

/// K3.1 附加条件与型号名单：Moonshot 身份（vendor kimi|moonshot、Kimi preset、
/// env `kimi` 经别名归一）或官方 Kimi 路由 URL × K3.1 拼写（仅容忍 `[1m]` 声明
/// 后缀）。型号名单与 `prefs::is_moonshot_k31_route` 逐字相同；附加条件是**刻意
/// 的拓宽**，不是逐字镜像——prefs 按 vendor 戳（kimi|moonshot）或官方 URL 附加，
/// 这里按 Moonshot 身份或官方 URL 附加（2026-11-09 审 m2 澄清，旧注释的"镜像"
/// 说法不实）。分歧面：无 kimi|moonshot 戳的中转记录若经 Kimi preset 解析出
/// Moonshot 身份，引擎走通用 Moonshot 臂发 `thinking:{enabled}`（bridge 注已标记
/// 该形在 K3.1 契约之外），审查发零字段。零字段是 K3.1 always-reasoning 契约的
/// 文档有效形态（深度走顶层 `reasoning_effort`，`thinking:{disabled}` 被拒，
/// 引擎 send-path 刻意把 K3.1 effort 解析为 None，omission 即文档有效形态）——
/// 审查取契约形态而非复刻越界线材（2026-11-07 r3 审 M2）。
fn review_model_is_moonshot_k31(api: ApiProvider, base_url: &str, model: &str) -> bool {
    let attached = api == ApiProvider::Moonshot
        || is_exact_kimi_code_route(base_url)
        || is_exact_moonshot_platform_route(base_url);
    if !attached {
        return false;
    }
    let normalized = model.trim().to_ascii_lowercase();
    let bare = normalized
        .strip_suffix("[1m]")
        .unwrap_or(normalized.as_str());
    matches!(
        bare,
        "kimi-k3.1-pro" | "kimi-k3.1-pro-highspeed" | "k3.1-pro" | "k3.1-pro-highspeed"
    )
}

/// 底座 `model_is_openai_reasoning_family`（config/models.rs）逐字镜像：
/// gpt-5.5[-pro]（含日期快照）、gpt-5.6 系、codex 名单。引擎的
/// `apply_openai_reasoning_effort` 恰好对这批型号在 OpenAI chat 线写
/// `reasoning_effort`。
fn openai_reasoning_family_model(model_lower: &str) -> bool {
    fn has_date_snapshot_suffix(model_lower: &str, prefix: &str) -> bool {
        let Some(rest) = model_lower.strip_prefix(prefix) else {
            return false;
        };
        let bytes = rest.as_bytes();
        bytes.len() == 10
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes
                .iter()
                .enumerate()
                .all(|(idx, byte)| idx == 4 || idx == 7 || byte.is_ascii_digit())
    }
    matches!(model_lower, "gpt-5.5" | "gpt-5.5-pro")
        || has_date_snapshot_suffix(model_lower, "gpt-5.5-")
        || has_date_snapshot_suffix(model_lower, "gpt-5.5-pro-")
        || matches!(
            model_lower,
            "gpt-5.6" | "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna"
        )
        || matches!(
            model_lower,
            "gpt-5-codex"
                | "gpt-5.1-codex"
                | "gpt-5.1-codex-mini"
                | "gpt-5.1-codex-max"
                | "gpt-5.2-codex"
                | "gpt-5.3-codex"
                | "codex-gpt-5.5"
                | "chatgpt-gpt-5.5"
                | "gpt-5.5-codex"
                | "gpt-5.5-codex-preview"
                | "codex-gpt-5.5-preview"
                | "chatgpt-gpt-5.5-preview"
        )
}

/// 引擎 Chat Completions 线的 token 上限字段换名（`apply_provider_token_limit`，
/// CodeWhale client/chat.rs）逐字镜像：四类路由发 `max_completion_tokens` 而非
/// `max_tokens`——XiaomiMimo、OpenAI 推理家族、精确 MiniMax-M3 第一方路由、直连
/// Moonshot K3。Kimi Code 会员端点（api.kimi.com/coding/v1）**不在**换名集合：
/// 2026-09-30 实测线材就是 `max_tokens`。审查镜像同一换名，一次性请求才与主对话
/// 逐字节同线（旧线在所有路由都发 `max_tokens`，对这些路由是静默错配）。
/// 身份按引擎同源 ApiProvider 分类（见 model_review 注）：`kimi`/`moonshotai`/
/// `MoonShot` 等拼写都归一 Moonshot kind，不再有原文比较劈开字段决策的洞
/// （2026-11-07 r3 审 M7）；MiniMax 类先做底座同款型号规范化
/// （`canonical_minimax_model_id` M3 收敛分支，M8）。
fn review_max_tokens_field(api: ApiProvider, base_url: &str, model: &str) -> &'static str {
    let model_lower = model.trim().to_ascii_lowercase();
    let direct_moonshot_k3 = api == ApiProvider::Moonshot
        && is_exact_moonshot_platform_route(base_url)
        && model_lower == "kimi-k3";
    let exact_minimax_m3 = api == ApiProvider::Minimax
        && (is_exact_https_host_path(base_url, "api.minimax.io", "v1")
            || is_exact_https_host_path(base_url, "api.minimaxi.com", "v1"))
        && canonical_minimax_model_is_m3(model);
    if api == ApiProvider::XiaomiMimo
        || (api == ApiProvider::Openai && openai_reasoning_family_model(&model_lower))
        || direct_moonshot_k3
        || exact_minimax_m3
    {
        "max_completion_tokens"
    } else {
        "max_tokens"
    }
}

/// 品/悟审查的思考档位写入（2026-11-06 产品决策；2026-11-07 r3 审按引擎同源
/// 身份重建全臂）：**思考永不关闭**，与本地路由同一处理
/// （`bridge::request_reasoning_effort` + `core::always_thinking`）——取线材能表达
/// 的最低思考档；只有开/关的家族显式选开。身份按引擎主对话同一解析的
/// [`ApiProvider`] 分类（`model_review` 里 `build_dt_config().api_provider()`：
/// ApiProvider::parse 大小写不敏感 + 别名表 + deepseek-cn 特例，不可解析兜底
/// Deepseek）——规范 slug/大小写/别名拼写与引擎不再劈开。线材形状对齐底座两层
/// （`apply_reasoning_effort` 通用层 + `apply_route_reasoning_controls` 路由层，
/// CodeWhale crates/tui/src/client.rs、client/chat.rs）；偏离处均有注释并逐字注明
/// 底座依据，不是"逐字翻译"：
/// - vLLM/Tachyon/本地：`chat_template_kwargs.enable_thinking=true` +
///   `reasoning_effort=low`（引擎本地路由同款）；always-thinking 知识表 NoControl
///   模型（qwen3-thinking/deepseek-r1/kimi-k2.7/minimax-m2）native 思考，不发字段。
/// - Ollama：本地 `think=true`（boolean think 线，最低思考档=开；对无思考模型是硬
///   400，引擎本地路由已按 #622 接受同一取舍）。托管 `https://ollama.com/v1` 与
///   `DEEPSEEK_PROVIDER=ollama-cloud` 在引擎侧都是 OllamaCloud 身份 →
///   `reasoning_effort=low` 线（api_provider 升级 + 通用层 low 档）。
/// - DeepSeek/DeepseekCN：`reasoning_effort=low` + `thinking.enabled`
///   （deepseek_effort 表 Low 档，shared DeepSeek table 同臂）。注意
///   `response_format=json_object` × thinking 常开这一组合未实测（旧线是
///   thinking 关 + json_object；k3 的组合不迁移）；若网关拒绝，以 HTTP 错误浮出。
/// - K3 双路由特例（先于通用 Kimi 臂；底座用路由层在通用臂之后改写同一线材，
///   这里直接归类、字段只写一次，线材等价）：
///   Kimi Code 会员端点（精确 api.kimi.com/coding/v1 × k3|k3-256k）发嵌套
///   `thinking:{enabled,effort:low}`（`apply_kimi_code_k3_reasoning_effort`）；
///   直连按量平台（精确 api.moonshot.ai|cn /v1 × kimi-k3）always-thinking、只接受
///   顶层 `reasoning_effort`，发 `reasoning_effort=low` 且不发 `thinking`
///   （`apply_direct_moonshot_k3_reasoning_effort`："a generic Moonshot `thinking`
///   payload is not truthful for this route"；服务端缺省档是 max，漏发档位 = 档位
///   漂移到 max）。K3.1 家族（kimi-k3.1-pro[-highspeed]/k3.1-pro[-highspeed]，含
///   `[1m]` 声明后缀）在通用臂**前**发零字段：引擎 send-path 刻意把 K3.1 effort
///   解析为 None，omission 即文档有效形态（prefs/mod.rs K3.1 契约；
///   `review_model_is_moonshot_k31` 镜像同一附加条件与型号名单）。
/// - Kimi 全家族（Moonshot kind / Kimi preset / moonshot.cn|.ai、api.kimi.com
///   URL 嗅探——路由方言跟端点走，身份无关的已记载拓宽）：`thinking.enabled`——
///   官方接入要求思考常开（底座 #3024、image_capability 名单同口径）。底座通用
///   Moonshot 臂对 dashscope URL 照发 enabled（thinking-only 名单抑制按身份门控
///   在 Modelstudio 臂），审查同形，不再自行按 URL 抑制。
/// - Model Studio（Modelstudio* 四 kind，底座 `apply_modelstudio_route_reasoning_controls`
///   镜像）：非精确 Alibaba Chat 端点 fail-close 零字段；精确端点上 thinking-only
///   名单（qwen3.8-max[+preview]、kimi-k2.7-code 系）无开关不发字段，hybrid 名单
///   发 `enable_thinking=true`，preserve 名单发 `preserve_thinking=true`，
///   deepseek-v4*/glm-5.2|5.1|5 发 `reasoning_effort=high`（底座把 low 折叠为
///   high）。`wire="openai"` 的 Modelstudio*Anthropic 变体（TokenPlan/CodingPlan
///   Anthropic 两个 kind）底座路由门 `is_exact_modelstudio_chat_route` 明确
///   "speak the Messages dialect and are never verified here"——路由层对四个
///   Modelstudio kind 一律先剥离全部思考字段、只对非 Anthropic 两个 kind 回写，
///   所以引擎线材对 *Anthropic 恒零字段；审查同形：这两个 kind 直接 fail-close
///   零字段，不进名单逻辑（2026-11-09 审 M1：旧线让 *Anthropic 混进名单臂、在
///   精确 Alibaba 端点上发 `enable_thinking`，恰是底座注释点名的 unguarded
///   400 风险）。
/// - Qwen preset / 非 Modelstudio 身份的 dashscope URL：**零字段**——引擎落
///   Openai 身份，通用层无写入、路由层按身份门控，主对话线材不含任何思考字段；
///   hybrid 型号 native 缺省思考开，thinking-only 名单无开关，经典 id 无思考
///   （2026-11-07 r3 审 M4：旧线发 `enable_thinking=true` 是引擎不发的字段，
///   在经典 id/非阿里网关上至坏 400）。
/// - Doubao/Volces（Volcengine kind）：`reasoning_effort=high` + `thinking.enabled`
///   （底座把 low/medium 折叠为 high——自家档位线材未验证，保守沿用同一折叠）。
/// - GLM/z.ai（Zai kind，底座 `apply_zai_route_reasoning_controls` fail-close
///   镜像）：仅精确第一方 z.ai/bigmodel Chat 端点 × 已知推理型号有控制——
///   GLM-5.3/-Flash（forced-thinking）发 `thinking:{enabled,clear_thinking:false}` +
///   `reasoning_effort=low`（底座 `apply_zai_forced_thinking_effort` low 档同款；
///   漏发档位 = API 文档默认 max，2026-11-07 r3 审 M1）；GLM-5.2/5.1/5-Turbo 仅
///   generic toggle；未知型号与非第一方网关零字段（"A compatible gateway owns its
///   own request dialect... fail closed"，r3 审 m1 同步闭合）。
/// - Mimo（XiaomiMimo kind）：`thinking.enabled`。
/// - Minimax（Minimax kind）：`reasoning_split=true`；精确第一方 M3 路由
///   （规范型号 `MiniMax-M3`，`canonical_minimax_model_id` 收敛镜像）另发
///   `thinking:{type:"adaptive"}`（底座 low 档同款）。split 全线保留是
///   **审查线特例**，不是底座主对话线材的逐字翻译：审查是非流式 + json 解析，
///   不 split 时 M2 的 reasoning 会混进 content 破坏解析。
/// - OpenRouter 系（Openrouter/Orcarouter/Novita/Together kind）：
///   `reasoning_effort=low` + `thinking.enabled`（底座 low 档同款）。**例外**
///   `thinkingmachines/inkling` 仅在 Together kind 上：底座
///   `apply_inkling_reasoning_effort` 只对 `provider == Together` 生效并明确
///   `object.remove("thinking")`——Inkling 不吃 Together 通用 thinking 扩展，
///   只发顶层 effort；OpenRouter 等其他聚合 kind × inkling 底座走通用臂照发
///   effort+thinking，审查同形（2026-11-09 审 m1 收敛例外范围）。
/// - SiliconFlow 系（Siliconflow/SiliconflowCn/Sglang/Deepinfra/Atlascloud kind，
///   底座同一折叠臂）：`reasoning_effort=high` + `thinking.enabled`。
/// - env-pin 家族（kind 归一后与底座同名臂同款）：arcee/huggingface
///   `reasoning_effort=low`；fireworks `reasoning_effort=high`；nvidia-nim
///   （规范 slug，`nim`/`nvidia`/`nvidia_nim` 拼写经 parse 同样命中）
///   `chat_template_kwargs{thinking:true,reasoning_effort:"high"}`。
/// - Meta（Meta kind × `muse-spark*`）：`reasoning_effort=low`（底座
///   `apply_openai_reasoning_effort` 的 muse 分支 low 档同款；其余 Meta id 底座
///   通用层无写入 → 零字段）。
/// - Google（Google kind × 精确 generativelanguage OpenAI 兼容端点）：
///   `google.thinking_config.thinking_level="low"`（底座 `apply_google_thinking_level`
///   low 档同款；off/low/medium 都映射 low）。
/// - OpenAI 推理家族（Openai kind × gpt-5.5[+日期快照]/gpt-5.6 系/codex 名单）：
///   `reasoning_effort=low`（底座 `apply_openai_reasoning_effort` low 档同款；名单
///   逐字镜像 `model_is_openai_reasoning_family`）。非推理家族（gpt-4o、gpt-6-sol
///   等）与未知网关：线材不表达档位，思考 native 开 → 不发字段。
/// - xAI：grok-4.5|4.6 在精确 api.x.ai/v1 端点收顶层档位 → `reasoning_effort=low`
///   （底座 `apply_xai_grok_4_6_reasoning_effort`；grok-4.7——Xai preset 默认——
///   不收档位，两端都不发；非第一方网关两端也都不发）。
/// - 其余 kind（Anthropic/DeepseekAnthropic/MinimaxAnthropic/Openmodel、Custom、
///   Stepfun/Sakana/LongCat/OpencodeGo/Zen、Mistral（low 档底座实发为空）、
///   Telecomjs/Edenai/Concentrate、Antigravity 墓碑等）：底座 low 档零字段 →
///   审查零字段。
/// - **Anthropic preset 例外**：preset Anthropic 在 `model_review` 里走 Messages
///   直连，不经本函数、当前不发思考字段——主对话 Messages adapter 会在 low 档写
///   `thinking:{enabled,budget_tokens}`/`{adaptive}`（client/anthropic.rs），省略
///   即思考关。底座 shaper 未公开、宿主侧手抄型号启发等于再造漂移面；按"永不关闭"
///   口径补齐它属后续底座暴露工作（见 PR body 披露），本函数不撒谎称已覆盖。
fn apply_review_thinking_controls(
    body: &mut Value,
    api: ApiProvider,
    preset: ModelPreset,
    base_url: &str,
    model: &str,
) {
    let base = base_url.trim().trim_end_matches('/').to_ascii_lowercase();
    // 本地路由：与引擎本地处理完全一致——本地 ollama 只有 boolean think；托管
    // ollama.com/v1 与 `DEEPSEEK_PROVIDER=ollama-cloud` 在引擎侧都是 OllamaCloud
    // （reasoning_effort 线）；vLLM/Tachyon 走 always-thinking 知识表（NoControl
    // 不发字段，其余最低档 low）。
    if matches!(api, ApiProvider::Vllm | ApiProvider::Tachyon) || preset == ModelPreset::LocalVllm {
        match crate::core::always_thinking::always_thinking_spec(model) {
            Some(crate::core::always_thinking::AlwaysThinkingSpec::NoControl) => return,
            _ => {
                body["chat_template_kwargs"] = json!({ "enable_thinking": true });
                body["reasoning_effort"] = json!("low");
                return;
            }
        }
    }
    if api == ApiProvider::Ollama {
        // 底座 config provider.rs `is_exact_ollama_cloud_route`/`is_exact_https_route`
        // 的逐字镜像判定：http 降级、邻近路径、端口变体都不算托管云（api_provider
        // 已按官方 /v1 端点把 ollama 升级为 OllamaCloud，这里兜 Openai 身份直指
        // ollama.com 的已记载拓宽）。
        if is_exact_https_host_path(base_url, "ollama.com", "v1") {
            body["reasoning_effort"] = json!("low");
        } else {
            body["think"] = json!(true);
        }
        return;
    }
    if api == ApiProvider::OllamaCloud {
        body["reasoning_effort"] = json!("low");
        return;
    }
    if matches!(api, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        || preset == ModelPreset::Deepseek
        || base.contains("api.deepseek.com")
    {
        body["reasoning_effort"] = json!("low");
        body["thinking"] = json!({ "type": "enabled" });
        return;
    }
    // K3 双路由特例：底座在通用臂之后以路由层剥离/改写同一线材，这里先归类、
    // 字段只写一次。归族条件与下方通用 Kimi 臂一致（含 URL 嗅探——路由方言跟
    // 端点走，与 preset/provider 身份无关）。
    let kimi_family = api == ApiProvider::Moonshot
        || preset == ModelPreset::Kimi
        || base.contains("moonshot.cn")
        || base.contains("moonshot.ai")
        || base.contains("api.kimi.com");
    if kimi_family {
        // K3.1 家族：引擎刻意零字段（omission=文档有效形态，见 helper 注），
        // 先于一切 thinking 写入。
        if review_model_is_moonshot_k31(api, base_url, model) {
            return;
        }
        if is_exact_kimi_code_route(base_url)
            && matches!(model.trim().to_ascii_lowercase().as_str(), "k3" | "k3-256k")
        {
            body["thinking"] = json!({ "type": "enabled", "effort": "low" });
            return;
        }
        if is_exact_moonshot_platform_route(base_url)
            && model.trim().eq_ignore_ascii_case("kimi-k3")
        {
            body["reasoning_effort"] = json!("low");
            return;
        }
        // 底座通用 Moonshot 臂照发 enabled（thinking-only 名单抑制按身份门控在
        // Modelstudio 臂）——Moonshot 身份指向 dashscope 也不例外。
        body["thinking"] = json!({ "type": "enabled" });
        return;
    }
    // Model Studio（底座 `apply_modelstudio_route_reasoning_controls` 镜像：
    // 路由 fail-close、名单门控、档位折叠）。*Anthropic 两个 kind 底座路由门
    // 不认（Messages 方言，"never verified here"）——路由层剥离后不回写，引擎
    // 线材恒零字段；这里同样不进名单臂，落函数尾零字段（2026-11-09 审 M1）。
    if matches!(
        api,
        ApiProvider::ModelstudioTokenPlan | ApiProvider::ModelstudioCodingPlan
    ) {
        if !is_exact_modelstudio_chat_route(base_url) {
            return;
        }
        let thinking_only = modelstudio_model_is_thinking_only(model);
        if !thinking_only && !modelstudio_model_is_hybrid(model) {
            return;
        }
        // low 档 thinking 恒开（modelstudio_effort_disables_thinking(low)=false）。
        if !thinking_only {
            body["enable_thinking"] = json!(true);
        }
        if modelstudio_model_supports_preserve_thinking(model) {
            // 一次性审查无 assistant 历史，preserve 无回放效应；镜像引擎线材保同形。
            body["preserve_thinking"] = json!(true);
        }
        if !thinking_only && modelstudio_model_supports_reasoning_effort(model) {
            body["reasoning_effort"] = json!(MODELSTUDIO_LOW_WIRE_EFFORT);
        }
        return;
    }
    // 以下 Doubao/z.ai/Mimo/Minimax（及上方 Deepseek 臂）的 URL 嗅探是**已记载
    // 拓宽**，与 Kimi 家族同性质：路由方言跟端点走，与身份/preset 无关——
    // OpenaiCompatible preset 直指官方端点（无 vendor 戳）时身份落 Openai，
    // 底座零字段，但官方端点真实方言就是思考字段（底座注释"URL 嗅探变体"测试
    // 同口径钉住，2026-11-09 审 m3 补记载）。嗅探含子串匹配的固有近似
    // （lookalike 主机/路径片段可提前命中本臂）；底座 heritage
    // `reasoning_dialect_from_base_url` 同语义。
    if preset == ModelPreset::Doubao
        || api == ApiProvider::Volcengine
        || base.contains("volces.com")
        || base.contains("volcengine")
        || base.contains("byteplus.com")
    {
        body["reasoning_effort"] = json!("high");
        body["thinking"] = json!({ "type": "enabled" });
        return;
    }
    if api == ApiProvider::Zai
        || preset == ModelPreset::Glm
        || base.contains("bigmodel.cn")
        || base.contains("z.ai")
    {
        // 底座路由层 fail-close：非精确第一方端点剥离两字段（"A compatible
        // gateway owns its own request dialect"）；精确端点上未知 GLM id 也剥离
        // （is_exact_known_zai_reasoning_route）。GLM-5.3/-Flash forced-thinking
        // 漏发档位 = API 文档默认 max（M1）。
        if is_exact_zai_chat_route(base_url) && zai_model_is_known_reasoning(model) {
            body["thinking"] = json!({ "type": "enabled", "clear_thinking": false });
            if zai_model_is_forced_thinking(model) {
                body["reasoning_effort"] = json!("low");
            }
        }
        return;
    }
    if api == ApiProvider::XiaomiMimo
        || preset == ModelPreset::Mimo
        || base.contains("xiaomimimo.com")
    {
        body["thinking"] = json!({ "type": "enabled" });
        return;
    }
    if api == ApiProvider::Minimax
        || preset == ModelPreset::Minimax
        || base.contains("minimax.chat")
        || base.contains("minimaxi.com")
    {
        // split 全线保留 = 审查线特例（非流式 + json 解析，见块注）；精确第一方
        // M3 路由另发底座 low 档的 adaptive（规范化型号判定，M8）。
        body["reasoning_split"] = json!(true);
        if api == ApiProvider::Minimax
            && (is_exact_https_host_path(base_url, "api.minimax.io", "v1")
                || is_exact_https_host_path(base_url, "api.minimaxi.com", "v1"))
            && canonical_minimax_model_is_m3(model)
        {
            body["thinking"] = json!({ "type": "adaptive" });
        }
        return;
    }
    // 聚合商家族：底座 low 档实发字段（client.rs OpenRouter/Novita/Together 臂
    // `reasoning_effort`+`thinking enabled`）——审查若发零控制，主对话思考而审查
    // 静默关思考，违背"永不关闭"。
    if matches!(
        api,
        ApiProvider::Openrouter
            | ApiProvider::Orcarouter
            | ApiProvider::Novita
            | ApiProvider::Together
    ) {
        // Inkling 例外：底座 `apply_inkling_reasoning_effort` 只在 **Together
        // 身份**上生效（chat.rs `provider != ApiProvider::Together`
        // 提前返回），明确 `object.remove("thinking")`（"It does not use
        // Together's generic `thinking` extension"）——只发顶层 effort。
        // OpenRouter/Orcarouter/Novita × inkling 底座走各自通用臂照发
        // effort+thinking，审查同形（2026-11-09 审 m1：旧线把例外扩大到全部
        // 四 slug，是与底座的未记载分歧）。
        if api == ApiProvider::Together
            && model
                .trim()
                .eq_ignore_ascii_case("thinkingmachines/inkling")
        {
            body["reasoning_effort"] = json!("low");
            return;
        }
        body["reasoning_effort"] = json!("low");
        body["thinking"] = json!({ "type": "enabled" });
        return;
    }
    // SiliconFlow 与底座同一折叠组（client.rs 同一臂：Siliconflow/SiliconflowCn/
    // Sglang/Deepinfra/Atlascloud → low/medium 折叠为 high+enabled）。
    if matches!(
        api,
        ApiProvider::Siliconflow
            | ApiProvider::SiliconflowCn
            | ApiProvider::Sglang
            | ApiProvider::Deepinfra
            | ApiProvider::Atlascloud
    ) {
        body["reasoning_effort"] = json!("high");
        body["thinking"] = json!({ "type": "enabled" });
        return;
    }
    if matches!(api, ApiProvider::Arcee | ApiProvider::Huggingface) {
        body["reasoning_effort"] = json!("low");
        return;
    }
    if api == ApiProvider::Fireworks {
        body["reasoning_effort"] = json!("high");
        return;
    }
    if api == ApiProvider::NvidiaNim {
        body["chat_template_kwargs"] = json!({ "thinking": true, "reasoning_effort": "high" });
        return;
    }
    // Meta：底座 `apply_openai_reasoning_effort` 的 muse 分支（muse-spark* 收
    // 顶层档位，low→"low"）；其余 Meta id 底座通用层无写入。
    if api == ApiProvider::Meta {
        let model_lower = model.trim().to_ascii_lowercase();
        if model_lower == "muse-spark" || model_lower.starts_with("muse-spark-") {
            body["reasoning_effort"] = json!("low");
        }
        return;
    }
    // Google：底座 `apply_google_thinking_level`（精确官方 OpenAI 兼容端点；
    // off/low/medium 都映射 low——审查恒定 low）。
    if api == ApiProvider::Google && is_exact_google_chat_route(base_url) {
        body["google"]["thinking_config"]["thinking_level"] = json!("low");
        return;
    }
    // OpenAI 推理家族（见 helper 注）：底座对这批型号在 chat 线写 effort，审查
    // 发零控制即静默档位漂移（非 never-off 违背——native 缺省开——但违背"最低档"
    // 契约）。OpenaiCompatible preset 默认型号 gpt-5.6-terra 正在此名单。
    if api == ApiProvider::Openai
        && openai_reasoning_family_model(&model.trim().to_ascii_lowercase())
    {
        body["reasoning_effort"] = json!("low");
        return;
    }
    // xAI grok-4.5|4.6 精确第一方端点（底座同款守卫）；grok-4.7 不收档位。
    if (api == ApiProvider::Xai || preset == ModelPreset::Xai)
        && is_exact_https_host_path(base_url, "api.x.ai", "v1")
        && matches!(
            model.trim().to_ascii_lowercase().as_str(),
            "grok-4.5" | "grok-4.6"
        )
    {
        body["reasoning_effort"] = json!("low");
        return;
    }
}

fn parse_model_review(content: &str) -> Result<ModelReview> {
    parse_model_review_with_finish(content, None)
}

/// `finish_reason=length` 是预算被推理吃满/输出截断的信号：空 content 或截断
/// JSON 报干巴巴的"没有 JSON"会把人引去查提示词，而病根在输出预算。带上
/// finish_reason 的解释（2026-11-09 审 m9；k3 旧线上正是这个形态——旧上限全被
/// reasoning 吃掉、content 空、finish_reason=length）。
fn parse_model_review_with_finish(
    content: &str,
    finish_reason: Option<&str>,
) -> Result<ModelReview> {
    parse_model_review_inner(content).map_err(|err| {
        if finish_reason == Some("length") {
            err.context(
                "model hit the route's output-token cap before emitting \
                 parseable review JSON (finish_reason=length) — the budget \
                 went to reasoning tokens or the JSON was cut off; the wire \
                 already carries the engine's main-turn budget, so this \
                 points at the route, not at the review",
            )
        } else {
            err
        }
    })
}

fn parse_model_review_inner(content: &str) -> Result<ModelReview> {
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
            if let ContentBlock::ToolUse {
                id, name, input, ..
            } = b
            {
                let semantic = match (name.as_str(), input.get("action").and_then(Value::as_str)) {
                    ("Web", Some("search")) => "web_search",
                    ("Web", Some("fetch")) => "fetch_url",
                    _ => name.as_str(),
                };
                map.insert(id.as_str(), semantic);
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

/// Section-header prefixes of B1 transfer messages (resolvePinvouReview: after
/// the Pinvou review dialog checks "let AI fix" and similar actions, both
/// frontend bridges assemble the message in the UI language — the three
/// BT_TABLE blocks are identical across the tauri/web bridges — and send it
/// back to the main session as the Boss). The message is composed of up to
/// five sections, one per checked action, and **any of them can lead**
/// (checking only verify makes the message start with "以下几条涉及外部事实" /
/// "The items below involve external facts" / "以下の項目は外部事実に関わります"),
/// so matching must cover all prefixes instead of a single starts_with, in
/// every UI language. Keep in sync with resolvePinvouReview in
/// `pinvou3-app/src/platform/tauri/bridge/chat.js` and
/// `pinvou3-app/src/platform/web/bridge.js`; the
/// `b1_transfer_prefixes_stay_in_sync_with_bridge_sources` test fails when a
/// bridge copy changes without this table following. The old zh-only prefix
/// found in historical comments ("请参考下面 Pinvou 的检阅意见") has never been
/// sent by any frontend since the open-source baseline (e34024e6) — both
/// bridges always used "请按下面的检阅意见" — so no compatibility branch is
/// needed beyond the zh entries below.
const B1_TRANSFER_PREFIXES: &[&str] = &[
    // zh fix section header (zh entries kept for historical sessions)
    "请按下面的检阅意见",
    // zh verify section header
    "以下几条涉及外部事实",
    // zh adopt section header
    "以下事项我已拍板",
    // zh ask section header
    "以下待定项请用 request_user_input 正式问我",
    // zh fill section header
    "以下维度产物还缺",
    // en fix section header
    "Apply the review notes below",
    // en verify section header
    "The items below involve external facts",
    // en adopt section header
    "The following decisions are final",
    // en ask section header
    "For the pending items below, ask me formally",
    // en fill section header
    "The artifact is still missing the dimensions below",
    // ja fix section header
    "下のレビュー意見に従い",
    // ja verify section header
    "以下の項目は外部事実に関わります",
    // ja adopt section header
    "以下の事項は確定しました",
    // ja ask section header
    "以下の未確定項目については",
    // ja fill section header
    "成果物には以下の観点が不足しています",
];

/// 确定性投影（§6，超长降级用）：Boss 原话全留 / request_user_input 决策 /
/// Web(action=search) 事实截断；丢 checklist、thinking、tool 细节。**产物不在这里**——由
/// build_context 读 workspace 文件真实内容（修 edit_file bug，§6）。
fn project(messages: &[Message]) -> String {
    let names = tool_name_map(messages);
    let mut boss_says = Vec::new();
    let mut decisions = Vec::new();
    let mut facts = Vec::new();
    for m in messages {
        let is_user = m.role == "user";
        for b in &m.content {
            match b {
                // B1 transfer messages (fixed section headers assembled by
                // resolvePinvouReview in the UI language) are the Pinvou
                // review feedback from the previous round, not the Boss's
                // original request — project them out, otherwise stale numbers
                // quoted there get read as "the AI's current state"; adopted
                // decisions already live in the artifact files (the artifacts
                // are the truth). The frontend (identical in the tauri/web
                // bridges) composes one section per checked action and any of
                // the five headers can lead, so all must be recognized in
                // every language; see the B1_TRANSFER_PREFIXES docs.
                ContentBlock::Text { text, .. }
                    if is_user
                        && !B1_TRANSFER_PREFIXES
                            .iter()
                            .any(|prefix| text.starts_with(prefix)) =>
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

/// guard：issues[].persona 必须在 personas∪alternates；空 trace 给默认。
/// MVP 不校验 persona id 是否预注册（人格是"视角参数"）。
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
    #[cfg(test)]
    let mut guard_reasons = Vec::new();
    let issues = raw
        .issues
        .into_iter()
        .map(|mut it| {
            if !it.persona.is_empty() && !valid.contains(&it.persona) {
                #[cfg(test)]
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
        verdict: raw.verdict,
        artifact_path: None,
        #[cfg(test)]
        guard_reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试辅助：与 `model_review` 同源的身份解析——ApiProvider::parse（大小写
    /// 不敏感 + 别名表 + deepseek-cn 特例），不可解析兜底 Deepseek（引擎
    /// api_provider 的同一兜底尾；custom-table/antigravity 边缘不在此表）。
    /// 测试直接传现场字符串（含别名/大小写拼写），把归一行为一并钉住。
    fn review_api(provider: &str) -> ApiProvider {
        ApiProvider::parse(provider).unwrap_or(ApiProvider::Deepseek)
    }

    /// 思考永不关闭（2026-11-06 产品决策）：各家族落到最低思考档/显式开。
    /// 线材形状逐字对齐底座 `apply_reasoning_effort` 的 low 档翻译；本地路由与
    /// 引擎本地处理（always_thinking 归一）一致。
    #[test]
    fn review_thinking_controls_pick_lowest_tier_never_off() {
        // Kimi Code 会员 K3（精确 api.kimi.com/coding/v1 × k3|k3-256k）：嵌套
        // effort 线（底座 apply_kimi_code_k3_reasoning_effort），无顶层档位；
        // 尾斜杠按底座语义忽略。
        let mut k3 = json!({});
        apply_review_thinking_controls(
            &mut k3,
            review_api("moonshot"),
            ModelPreset::OpenaiCompatible,
            "https://api.kimi.com/coding/v1/",
            "k3",
        );
        assert_eq!(
            k3["thinking"],
            json!({ "type": "enabled", "effort": "low" }),
            "k3: {k3}"
        );
        assert!(
            k3.get("reasoning_effort").is_none(),
            "Kimi Code K3 用嵌套 effort,不吃顶层档位: {k3}"
        );

        // 直连按量平台 K3（精确 api.moonshot.cn|ai /v1 × kimi-k3）：always-thinking
        // 路由只收顶层 reasoning_effort → low，且**不发 thinking**（底座
        // apply_direct_moonshot_k3_reasoning_effort；Kimi preset 默认路由走这里）。
        let mut k3direct = json!({});
        apply_review_thinking_controls(
            &mut k3direct,
            review_api("moonshot"),
            ModelPreset::OpenaiCompatible,
            "https://api.moonshot.cn/v1",
            "kimi-k3",
        );
        assert_eq!(k3direct["reasoning_effort"], json!("low"), "{k3direct}");
        assert!(
            k3direct.get("thinking").is_none(),
            "直连 K3 不吃 thinking 字段: {k3direct}"
        );
        // scheme/host 大小写不敏感（底座 is_exact_https_route 同款）仍是直连 K3。
        let mut k3direct_case = json!({});
        apply_review_thinking_controls(
            &mut k3direct_case,
            review_api("moonshot"),
            ModelPreset::OpenaiCompatible,
            "HTTPS://API.Moonshot.CN/v1",
            "kimi-k3",
        );
        assert_eq!(
            k3direct_case["reasoning_effort"],
            json!("low"),
            "{k3direct_case}"
        );
        // 同平台 URL × "k3"（非平台型号名）：落通用 Kimi 臂显式开（底座直连守卫
        // 同样只认 kimi-k3）。
        let mut k3platform_nonk3 = json!({});
        apply_review_thinking_controls(
            &mut k3platform_nonk3,
            review_api("moonshot"),
            ModelPreset::OpenaiCompatible,
            "https://api.moonshot.cn/v1",
            "k3",
        );
        assert_eq!(
            k3platform_nonk3["thinking"],
            json!({ "type": "enabled" }),
            "{k3platform_nonk3}"
        );

        // Kimi 身份 × dashscope：底座通用 Moonshot 臂对 dashscope URL 照发
        // enabled（thinking-only 名单抑制按身份门控在 Modelstudio 臂）——审查同
        // 形照发（r3 审 m2：不再按 URL 自行抑制）。
        let mut kimi_dashscope = json!({});
        apply_review_thinking_controls(
            &mut kimi_dashscope,
            review_api("moonshot"),
            ModelPreset::Kimi,
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "kimi-k2.7-code",
        );
        assert_eq!(
            kimi_dashscope["thinking"],
            json!({ "type": "enabled" }),
            "Moonshot 身份 dashscope 照发 enabled: {kimi_dashscope}"
        );
        // 补集（r3 审 M6 钉）：同路由可控 id 不得跌回零字段。
        let mut kimi_dashscope_complement = json!({});
        apply_review_thinking_controls(
            &mut kimi_dashscope_complement,
            review_api("moonshot"),
            ModelPreset::Kimi,
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "kimi-k2.6",
        );
        assert_eq!(
            kimi_dashscope_complement["thinking"],
            json!({ "type": "enabled" }),
            "dashscope 可控 id 必须显式开: {kimi_dashscope_complement}"
        );

        // URL-only 归族不能漏：api.kimi.com 会员端点在无 vendor/preset 线索时
        // 仍走嵌套 effort 线（k3 首报路由的纯 URL 形态）。
        let mut k3_url_only = json!({});
        apply_review_thinking_controls(
            &mut k3_url_only,
            review_api("openai"),
            ModelPreset::OpenaiCompatible,
            "https://api.kimi.com/coding/v1",
            "k3",
        );
        assert_eq!(
            k3_url_only["thinking"],
            json!({ "type": "enabled", "effort": "low" }),
            "{k3_url_only}"
        );
        // moonshot.cn URL-only（provider/preset 都不带家族信息时）归 Kimi 臂。
        let mut moonshot_cn_url = json!({});
        apply_review_thinking_controls(
            &mut moonshot_cn_url,
            review_api("openai"),
            ModelPreset::OpenaiCompatible,
            "https://api.moonshot.cn/v1",
            "kimi-k2.6",
        );
        assert_eq!(
            moonshot_cn_url["thinking"],
            json!({ "type": "enabled" }),
            "{moonshot_cn_url}"
        );

        // Kimi 官方端点 k2.6（旧逻辑被显式关思考）：现在同样显式开。
        let mut k26 = json!({});
        apply_review_thinking_controls(
            &mut k26,
            review_api("moonshot"),
            ModelPreset::Kimi,
            ModelPreset::Kimi.default_base_url(),
            "kimi-k2.6",
        );
        assert_eq!(k26["thinking"], json!({ "type": "enabled" }), "k2.6: {k26}");

        // Tachyon 本地（probed local kind）：可控模型最低档同 vLLM 形状。
        let mut tachyon = json!({});
        apply_review_thinking_controls(
            &mut tachyon,
            review_api("tachyon"),
            ModelPreset::OpenaiCompatible,
            "http://127.0.0.1:8718/v1",
            "qwen36_35b_256k",
        );
        assert_eq!(
            tachyon["chat_template_kwargs"],
            json!({ "enable_thinking": true }),
            "{tachyon}"
        );
        assert_eq!(tachyon["reasoning_effort"], json!("low"), "{tachyon}");

        // vLLM 本地可控模型：enable_thinking=true + reasoning_effort=low（引擎本地同款）。
        let mut vllm = json!({});
        apply_review_thinking_controls(
            &mut vllm,
            review_api("vllm"),
            ModelPreset::LocalVllm,
            ModelPreset::LocalVllm.default_base_url(),
            "qwen36_35b_256k",
        );
        assert_eq!(
            vllm["chat_template_kwargs"],
            json!({ "enable_thinking": true }),
            "vLLM 最低档=思考开+low: {vllm}"
        );
        assert_eq!(vllm["reasoning_effort"], json!("low"));
        assert!(
            vllm.get("thinking").is_none(),
            "vLLM 不吃 Anthropic 形状: {vllm}"
        );

        // 本地 kimi-k3（Tiers）：最低档 low，同 vLLM 形状。
        let mut k3local = json!({});
        apply_review_thinking_controls(
            &mut k3local,
            review_api("vllm"),
            ModelPreset::LocalVllm,
            ModelPreset::LocalVllm.default_base_url(),
            "kimi-k3",
        );
        assert_eq!(k3local["reasoning_effort"], json!("low"), "{k3local}");
        assert_eq!(
            k3local["chat_template_kwargs"]["enable_thinking"],
            json!(true),
            "{k3local}"
        );

        // 本地 always-thinking NoControl（qwen3-thinking）：native 思考，不发字段。
        let mut nocontrol = json!({});
        apply_review_thinking_controls(
            &mut nocontrol,
            review_api("vllm"),
            ModelPreset::LocalVllm,
            ModelPreset::LocalVllm.default_base_url(),
            "qwen3-thinking-2507",
        );
        assert!(
            nocontrol.as_object().expect("body object").is_empty(),
            "NoControl 不发任何思考字段: {nocontrol}"
        );

        // Ollama：boolean think 线，最低思考档=开。
        let mut ollama = json!({});
        apply_review_thinking_controls(
            &mut ollama,
            review_api("ollama"),
            ModelPreset::OpenaiCompatible,
            "http://127.0.0.1:11434/v1",
            "qwen3:8b",
        );
        assert_eq!(ollama["think"], json!(true), "{ollama}");

        // Ollama 托管云（精确 https://ollama.com/v1）：引擎升级为 OllamaCloud，
        // 用 reasoning_effort 线而非 think（config provider.rs is_exact_ollama_cloud_route）。
        let mut ollama_cloud = json!({});
        apply_review_thinking_controls(
            &mut ollama_cloud,
            review_api("ollama"),
            ModelPreset::OpenaiCompatible,
            "https://ollama.com/v1/",
            "qwen3:8b",
        );
        assert_eq!(
            ollama_cloud["reasoning_effort"],
            json!("low"),
            "{ollama_cloud}"
        );
        assert!(
            ollama_cloud.get("think").is_none(),
            "托管云不吃本地 boolean think: {ollama_cloud}"
        );

        // 底座 is_exact_https_route 的 path 大小写敏感："V1" 不是托管云 → 本地 think 线。
        let mut ollama_lookalike = json!({});
        apply_review_thinking_controls(
            &mut ollama_lookalike,
            review_api("ollama"),
            ModelPreset::OpenaiCompatible,
            "https://ollama.com/V1",
            "qwen3:8b",
        );
        assert_eq!(ollama_lookalike["think"], json!(true), "{ollama_lookalike}");
        assert!(
            ollama_lookalike.get("reasoning_effort").is_none(),
            "{ollama_lookalike}"
        );

        // 底座 is_exact_https_route 的 scheme/host 大小写不敏感："HTTPS://" + 大写
        // 主机仍是托管云（旧实现的 strip_prefix("https://") 在此漂移为本地线）。
        let mut ollama_cloud_case = json!({});
        apply_review_thinking_controls(
            &mut ollama_cloud_case,
            review_api("ollama"),
            ModelPreset::OpenaiCompatible,
            "HTTPS://OLLAMA.COM/v1",
            "qwen3:8b",
        );
        assert_eq!(
            ollama_cloud_case["reasoning_effort"],
            json!("low"),
            "{ollama_cloud_case}"
        );
        assert!(
            ollama_cloud_case.get("think").is_none(),
            "{ollama_cloud_case}"
        );

        // DeepSeek：档位表 Low 档（reasoning_effort=low + thinking enabled）。
        let mut deepseek = json!({});
        apply_review_thinking_controls(
            &mut deepseek,
            review_api("deepseek"),
            ModelPreset::Deepseek,
            ModelPreset::Deepseek.default_base_url(),
            "deepseek-v4-pro",
        );
        assert_eq!(deepseek["reasoning_effort"], json!("low"), "{deepseek}");
        assert_eq!(
            deepseek["thinking"],
            json!({ "type": "enabled" }),
            "{deepseek}"
        );
        assert!(deepseek.get("chat_template_kwargs").is_none());

        // Qwen preset：引擎落 Openai 身份（vendor/preset 归一 "openai"），通用层
        // 无写入、路由层按身份门控 → 主对话线材零思考字段，审查同零（r3 审 M4：
        // 旧 enable_thinking=true 是引擎不发的字段）。hybrid 型号 native 缺省开。
        let mut qwen = json!({});
        apply_review_thinking_controls(
            &mut qwen,
            review_api("openai"),
            ModelPreset::Qwen,
            ModelPreset::Qwen.default_base_url(),
            "qwen3.6-plus",
        );
        assert!(
            qwen.as_object().expect("body object").is_empty(),
            "Qwen preset 引擎身份 Openai → 零字段: {qwen}"
        );

        // Qwen thinking-only 型号（底座 modelstudio_model_is_thinking_only 名单，
        // qwen3.8-max 是 Qwen preset 默认）：引擎身份 Openai 本就零字段；Modelstudio
        // 身份下的名单抑制由 modelstudio 臂单独钉。
        let mut qwen_thinking_only = json!({});
        apply_review_thinking_controls(
            &mut qwen_thinking_only,
            review_api("openai"),
            ModelPreset::Qwen,
            ModelPreset::Qwen.default_base_url(),
            "qwen3.8-max",
        );
        assert!(
            qwen_thinking_only
                .as_object()
                .expect("body object")
                .is_empty(),
            "thinking-only qwen 拒绝开关字段: {qwen_thinking_only}"
        );

        // Doubao/Volces：底座把 low 折叠为 high + enabled。
        let mut doubao = json!({});
        apply_review_thinking_controls(
            &mut doubao,
            review_api("openai"),
            ModelPreset::Doubao,
            ModelPreset::Doubao.default_base_url(),
            ModelPreset::Doubao.default_model(),
        );
        assert_eq!(doubao["reasoning_effort"], json!("high"), "{doubao}");
        assert_eq!(doubao["thinking"], json!({ "type": "enabled" }), "{doubao}");
        let mut volces = json!({});
        apply_review_thinking_controls(
            &mut volces,
            review_api("openai"),
            ModelPreset::OpenaiCompatible,
            "https://ark.cn-beijing.volces.com/api/v3",
            "doubao-pro",
        );
        assert_eq!(volces["reasoning_effort"], json!("high"), "{volces}");
        assert_eq!(volces["thinking"], json!({ "type": "enabled" }), "{volces}");

        // GLM/z.ai、Mimo：显式 enabled（GLM 带 clear_thinking=false）。
        let mut glm = json!({});
        apply_review_thinking_controls(
            &mut glm,
            review_api("openai"),
            ModelPreset::OpenaiCompatible,
            "https://open.bigmodel.cn/api/paas/v4",
            "glm-5.1",
        );
        assert_eq!(
            glm["thinking"],
            json!({ "type": "enabled", "clear_thinking": false }),
            "{glm}"
        );
        let mut mimo = json!({});
        apply_review_thinking_controls(
            &mut mimo,
            review_api("openai"),
            ModelPreset::Mimo,
            ModelPreset::Mimo.default_base_url(),
            ModelPreset::Mimo.default_model(),
        );
        assert_eq!(mimo["thinking"], json!({ "type": "enabled" }), "{mimo}");

        // Minimax：M2 always-thinking 无控制 → native 开；reasoning_split 保持内容洁净。
        let mut minimax = json!({});
        apply_review_thinking_controls(
            &mut minimax,
            review_api("openai"),
            ModelPreset::Minimax,
            ModelPreset::Minimax.default_base_url(),
            "MiniMax-M3",
        );
        assert_eq!(minimax["reasoning_split"], json!(true), "{minimax}");
        assert!(
            minimax.get("thinking").is_none(),
            "Minimax 不发关字段: {minimax}"
        );

        // OpenAI 官方 chat 线与未知网关：线材不表达档位 → 不发字段。
        for (base_url, model) in [
            ("https://api.openai.com/v1", "gpt-4o"),
            ("https://api.openai.com/v1", "gpt-6-sol"),
            ("https://gateway.example/v1", "some-model"),
        ] {
            let mut plain = json!({});
            apply_review_thinking_controls(
                &mut plain,
                review_api("openai"),
                ModelPreset::OpenaiCompatible,
                base_url,
                model,
            );
            assert!(
                plain.as_object().expect("body object").is_empty(),
                "{base_url}/{model} 不应发思考字段: {plain}"
            );
        }

        // Custom Responses preset 只切主会话路由；引擎身份仍是 Openai → 零字段
        // （与 qwen preset 同一 parity）。
        let mut responses_qwen = json!({});
        apply_review_thinking_controls(
            &mut responses_qwen,
            review_api("openai"),
            ModelPreset::OpenaiResponses,
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "qwen3.6-flash",
        );
        assert!(
            responses_qwen.as_object().expect("body object").is_empty(),
            "OpenaiResponses 引擎身份 Openai → 零字段: {responses_qwen}"
        );

        // Vendor 归一家族：provider 字符串直接归臂（OpenaiCompatible preset +
        // 非可嗅探网关也要与底座同线材，review/mod.rs 各臂 provider 匹配）。
        // Zai 身份 × 非第一方网关：底座路由层 fail-close（"A compatible gateway
        // owns its own request dialect"）剥离 thinking → 审查同零（r3 审 m1 闭合）。
        let mut zai_vendor = json!({});
        apply_review_thinking_controls(
            &mut zai_vendor,
            review_api("zai"),
            ModelPreset::OpenaiCompatible,
            "https://gateway.example/v1",
            "glm-5.1",
        );
        assert!(
            zai_vendor.as_object().expect("body object").is_empty(),
            "zai 非第一方网关 fail-close: {zai_vendor}"
        );
        let mut volcengine_vendor = json!({});
        apply_review_thinking_controls(
            &mut volcengine_vendor,
            review_api("volcengine"),
            ModelPreset::OpenaiCompatible,
            "https://gateway.example/v1",
            "doubao-pro",
        );
        assert_eq!(
            volcengine_vendor["reasoning_effort"],
            json!("high"),
            "{volcengine_vendor}"
        );
        assert_eq!(
            volcengine_vendor["thinking"],
            json!({ "type": "enabled" }),
            "{volcengine_vendor}"
        );
        let mut minimax_vendor = json!({});
        apply_review_thinking_controls(
            &mut minimax_vendor,
            review_api("minimax"),
            ModelPreset::OpenaiCompatible,
            "https://gateway.example/v1",
            "MiniMax-M3",
        );
        assert_eq!(
            minimax_vendor["reasoning_split"],
            json!(true),
            "{minimax_vendor}"
        );
        let mut mimo_vendor = json!({});
        apply_review_thinking_controls(
            &mut mimo_vendor,
            review_api("xiaomi-mimo"),
            ModelPreset::OpenaiCompatible,
            "https://gateway.example/v1",
            "mimo-pro",
        );
        assert_eq!(
            mimo_vendor["thinking"],
            json!({ "type": "enabled" }),
            "{mimo_vendor}"
        );

        // 聚合商家族：底座 low 档实发（OpenRouter 臂 effort low+enabled；
        // SiliconFlow 臂 low→high+enabled），审查发零控制=静默关思考。
        let mut openrouter = json!({});
        apply_review_thinking_controls(
            &mut openrouter,
            review_api("openrouter"),
            ModelPreset::OpenaiCompatible,
            "https://openrouter.ai/api/v1",
            "deepseek/deepseek-chat-v4",
        );
        assert_eq!(openrouter["reasoning_effort"], json!("low"), "{openrouter}");
        assert_eq!(
            openrouter["thinking"],
            json!({ "type": "enabled" }),
            "{openrouter}"
        );
        let mut siliconflow = json!({});
        apply_review_thinking_controls(
            &mut siliconflow,
            review_api("siliconflow-cn"),
            ModelPreset::OpenaiCompatible,
            "https://api.siliconflow.cn/v1",
            "deepseek-ai/DeepSeek-V4",
        );
        assert_eq!(
            siliconflow["reasoning_effort"],
            json!("high"),
            "{siliconflow}"
        );
        assert_eq!(
            siliconflow["thinking"],
            json!({ "type": "enabled" }),
            "{siliconflow}"
        );

        // 底座同一折叠组的其余成员（sglang/deepinfra/atlascloud，env-pin 可达）：
        // low 同样折叠为 high+enabled。
        for provider in ["sglang", "deepinfra", "atlascloud"] {
            let mut collapse = json!({});
            apply_review_thinking_controls(
                &mut collapse,
                review_api(provider),
                ModelPreset::OpenaiCompatible,
                "https://gateway.example/v1",
                "some-model",
            );
            assert_eq!(
                collapse["reasoning_effort"],
                json!("high"),
                "{provider}: {collapse}"
            );
            assert_eq!(
                collapse["thinking"],
                json!({ "type": "enabled" }),
                "{provider}: {collapse}"
            );
        }

        // env-pin 家族（底座 client.rs 同名臂同款）：arcee/huggingface 只发
        // effort；fireworks 只发 high；nvidia-nim 走 chat_template_kwargs。
        let mut arcee = json!({});
        apply_review_thinking_controls(
            &mut arcee,
            review_api("arcee"),
            ModelPreset::OpenaiCompatible,
            "https://gateway.example/v1",
            "some-model",
        );
        assert_eq!(arcee["reasoning_effort"], json!("low"), "{arcee}");
        assert!(arcee.get("thinking").is_none(), "{arcee}");
        let mut fireworks = json!({});
        apply_review_thinking_controls(
            &mut fireworks,
            review_api("fireworks"),
            ModelPreset::OpenaiCompatible,
            "https://gateway.example/v1",
            "some-model",
        );
        assert_eq!(fireworks["reasoning_effort"], json!("high"), "{fireworks}");
        assert!(fireworks.get("thinking").is_none(), "{fireworks}");
        let mut nim = json!({});
        apply_review_thinking_controls(
            &mut nim,
            review_api("nim"),
            ModelPreset::OpenaiCompatible,
            "https://gateway.example/v1",
            "some-model",
        );
        assert_eq!(
            nim["chat_template_kwargs"],
            json!({ "thinking": true, "reasoning_effort": "high" }),
            "{nim}"
        );

        // OpenAI 推理家族（OpenaiCompatible preset 默认型号 gpt-5.6-terra 正在
        // 名单）：effort=low，无 thinking 字段；codex 名单一并钉住。
        for model in ["gpt-5.6-terra", "gpt-5.5-codex", "gpt-5.5-2026-01-01"] {
            let mut openai_reasoning = json!({});
            apply_review_thinking_controls(
                &mut openai_reasoning,
                review_api("openai"),
                ModelPreset::OpenaiCompatible,
                "https://api.openai.com/v1",
                model,
            );
            assert_eq!(
                openai_reasoning["reasoning_effort"],
                json!("low"),
                "{model}: {openai_reasoning}"
            );
            assert!(
                openai_reasoning.get("thinking").is_none(),
                "{model}: {openai_reasoning}"
            );
        }

        // xAI grok-4.5|4.6 在精确第一方端点收档位；grok-4.7（Xai preset 默认）
        // 与非第一方网关两端都不发。
        let mut grok45 = json!({});
        apply_review_thinking_controls(
            &mut grok45,
            review_api("xai"),
            ModelPreset::Xai,
            "https://api.x.ai/v1",
            "grok-4.5",
        );
        assert_eq!(grok45["reasoning_effort"], json!("low"), "{grok45}");
        assert!(grok45.get("thinking").is_none(), "{grok45}");
        let mut grok46 = json!({});
        apply_review_thinking_controls(
            &mut grok46,
            review_api("xai"),
            ModelPreset::OpenaiCompatible,
            "https://api.x.ai/v1",
            "grok-4.6",
        );
        assert_eq!(grok46["reasoning_effort"], json!("low"), "{grok46}");
        let mut grok47 = json!({});
        apply_review_thinking_controls(
            &mut grok47,
            review_api("xai"),
            ModelPreset::Xai,
            "https://api.x.ai/v1",
            ModelPreset::Xai.default_model(),
        );
        assert!(
            grok47.as_object().expect("body object").is_empty(),
            "grok-4.7 不收档位: {grok47}"
        );
        let mut grok_gateway = json!({});
        apply_review_thinking_controls(
            &mut grok_gateway,
            review_api("xai"),
            ModelPreset::Xai,
            "https://gateway.example/v1",
            "grok-4.5",
        );
        assert!(
            grok_gateway.as_object().expect("body object").is_empty(),
            "非第一方 xAI 网关不发 grok 字段: {grok_gateway}"
        );

        // URL 嗅探变体（provider/preset 都不带家族信息时归臂不能漏）。
        let mut deepseek_url = json!({});
        apply_review_thinking_controls(
            &mut deepseek_url,
            review_api("openai"),
            ModelPreset::OpenaiCompatible,
            "https://api.deepseek.com/v1",
            "deepseek-v4-pro",
        );
        assert_eq!(
            deepseek_url["reasoning_effort"],
            json!("low"),
            "{deepseek_url}"
        );
        assert_eq!(
            deepseek_url["thinking"],
            json!({ "type": "enabled" }),
            "{deepseek_url}"
        );
        let mut moonshot_ai_url = json!({});
        apply_review_thinking_controls(
            &mut moonshot_ai_url,
            review_api("openai"),
            ModelPreset::OpenaiCompatible,
            "https://api.moonshot.ai/v1",
            "kimi-k2.6",
        );
        assert_eq!(
            moonshot_ai_url["thinking"],
            json!({ "type": "enabled" }),
            "{moonshot_ai_url}"
        );
        let mut minimaxi_url = json!({});
        apply_review_thinking_controls(
            &mut minimaxi_url,
            review_api("openai"),
            ModelPreset::OpenaiCompatible,
            "https://api.minimaxi.com/v1",
            "MiniMax-M2.5",
        );
        assert_eq!(
            minimaxi_url["reasoning_split"],
            json!(true),
            "{minimaxi_url}"
        );
        let mut byteplus_url = json!({});
        apply_review_thinking_controls(
            &mut byteplus_url,
            review_api("openai"),
            ModelPreset::OpenaiCompatible,
            "https://ark.ap-southeast.byteplus.com/api/v3",
            "doubao-pro",
        );
        assert_eq!(
            byteplus_url["reasoning_effort"],
            json!("high"),
            "{byteplus_url}"
        );
        assert_eq!(
            byteplus_url["thinking"],
            json!({ "type": "enabled" }),
            "{byteplus_url}"
        );
    }

    /// r3 审修复波新增的身份/路由钉：引擎同源 ApiProvider 归一（别名/大小写/
    /// 规范 slug）、K3.1 零字段、GLM forced-thinking 档位、Modelstudio shaper
    /// 镜像、聚合商四 slug、Inkling 例外、canonical env-pin 家族、Meta/Google、
    /// Minimax 精确路由 adaptive。
    #[test]
    fn review_thinking_controls_identity_and_route_pins() {
        // K3.1 家族（Moonshot 身份或官方路由附加）：引擎刻意零字段（omission=
        // 文档有效形态；thinking:{disabled} 被 400），审查镜像 omission。
        for (provider, url, model) in [
            ("moonshot", "https://api.moonshot.cn/v1", "kimi-k3.1-pro"),
            (
                "moonshot",
                "https://api.kimi.com/coding/v1",
                "k3.1-pro-highspeed",
            ),
            (
                "moonshot",
                "https://api.moonshot.ai/v1",
                "kimi-k3.1-pro[1m]",
            ),
            // r4 审突变实证：四个拼写只钉两个，删掉未钉拼写全测试保持绿。
            (
                "moonshot",
                "https://api.moonshot.cn/v1",
                "kimi-k3.1-pro-highspeed",
            ),
            ("moonshot", "https://api.kimi.com/coding/v1", "k3.1-pro"),
        ] {
            let mut k31 = json!({});
            apply_review_thinking_controls(
                &mut k31,
                review_api(provider),
                ModelPreset::OpenaiCompatible,
                url,
                model,
            );
            assert!(
                k31.as_object().expect("body object").is_empty(),
                "K3.1 ({model}) 必须零字段: {k31}"
            );
        }
        // 非附加组合（Openai 身份 + 非官方 URL）：同样不进 K3.1/kimi 臂，零字段。
        let mut k31_unattached = json!({});
        apply_review_thinking_controls(
            &mut k31_unattached,
            review_api("openai"),
            ModelPreset::OpenaiCompatible,
            "https://gateway.example/v1",
            "kimi-k3.1-pro",
        );
        assert!(
            k31_unattached.as_object().expect("body object").is_empty(),
            "非附加 K3.1 组合零字段: {k31_unattached}"
        );

        // GLM forced-thinking（r3 审 M1 头条钉）：GLM preset 默认路由
        // （open.bigmodel.cn/api/paas/v4 × glm-5.3）必须带 reasoning_effort=low
        // ——漏发档位 = API 文档默认 max。
        let mut glm53 = json!({});
        apply_review_thinking_controls(
            &mut glm53,
            review_api("zai"),
            ModelPreset::Glm,
            "https://open.bigmodel.cn/api/paas/v4",
            ModelPreset::Glm.default_model(),
        );
        assert_eq!(
            glm53["thinking"],
            json!({ "type": "enabled", "clear_thinking": false }),
            "{glm53}"
        );
        assert_eq!(glm53["reasoning_effort"], json!("low"), "{glm53}");
        // GLM-5.3-Flash 同 forced-thinking 族；GLM-5.2 tiered 仅 toggle；
        // 精确路由 × 未知 GLM id fail-close 零字段。
        let mut glm53flash = json!({});
        apply_review_thinking_controls(
            &mut glm53flash,
            review_api("zai"),
            ModelPreset::OpenaiCompatible,
            "https://api.z.ai/api/paas/v4",
            "GLM-5.3-Flash",
        );
        assert_eq!(glm53flash["reasoning_effort"], json!("low"), "{glm53flash}");
        let mut glm52 = json!({});
        apply_review_thinking_controls(
            &mut glm52,
            review_api("zai"),
            ModelPreset::OpenaiCompatible,
            "https://open.bigmodel.cn/api/paas/v4",
            "GLM-5.2",
        );
        assert_eq!(
            glm52["thinking"],
            json!({ "type": "enabled", "clear_thinking": false }),
            "{glm52}"
        );
        assert!(
            glm52.get("reasoning_effort").is_none(),
            "GLM-5.2 low 档仅 generic toggle: {glm52}"
        );
        let mut glm_unknown = json!({});
        apply_review_thinking_controls(
            &mut glm_unknown,
            review_api("zai"),
            ModelPreset::OpenaiCompatible,
            "https://open.bigmodel.cn/api/paas/v4",
            "glm-5.4-preview",
        );
        assert!(
            glm_unknown.as_object().expect("body object").is_empty(),
            "精确路由未知 GLM id fail-close: {glm_unknown}"
        );

        // Modelstudio shaper 镜像（env-pin "modelstudio-token-plan" 经 parse 归一）：
        // hybrid → enable_thinking=true；thinking-only 名单 → 零字段；
        // deepseek-v4 → effort 折叠 high；非精确端点 fail-close。
        let ms_url = "https://dashscope.aliyuncs.com/compatible-mode/v1";
        let mut ms_hybrid = json!({});
        apply_review_thinking_controls(
            &mut ms_hybrid,
            review_api("modelstudio-token-plan"),
            ModelPreset::OpenaiCompatible,
            ms_url,
            "qwen3.6-flash",
        );
        assert_eq!(ms_hybrid["enable_thinking"], json!(true), "{ms_hybrid}");
        assert_eq!(
            ms_hybrid["preserve_thinking"],
            json!(true),
            "qwen3.6-flash 在 preserve 名单: {ms_hybrid}"
        );
        let mut ms_thinking_only = json!({});
        apply_review_thinking_controls(
            &mut ms_thinking_only,
            review_api("modelstudio-coding-plan"),
            ModelPreset::OpenaiCompatible,
            ms_url,
            "qwen3.8-max",
        );
        assert!(
            ms_thinking_only
                .as_object()
                .expect("body object")
                .is_empty(),
            "thinking-only 名单零字段: {ms_thinking_only}"
        );
        let mut ms_effort = json!({});
        apply_review_thinking_controls(
            &mut ms_effort,
            review_api("modelstudio-token-plan"),
            ModelPreset::OpenaiCompatible,
            ms_url,
            "deepseek-v4",
        );
        assert_eq!(
            ms_effort["reasoning_effort"],
            json!("high"),
            "Model Studio low 折叠 high: {ms_effort}"
        );
        let mut ms_gateway = json!({});
        apply_review_thinking_controls(
            &mut ms_gateway,
            review_api("modelstudio-token-plan"),
            ModelPreset::OpenaiCompatible,
            "https://gateway.example/v1",
            "qwen3.6-flash",
        );
        assert!(
            ms_gateway.as_object().expect("body object").is_empty(),
            "Modelstudio 非精确端点 fail-close: {ms_gateway}"
        );
        // r4 审 M1：*Anthropic 两个 kind 底座路由门不认（Messages 方言，
        // "never verified here"——路由层剥离后不回写），引擎线材恒零字段。
        // 审查同形 fail-close，不进名单臂（精确 Alibaba 端点 × hybrid 也零字段）。
        for kind in [
            "modelstudio-token-plan-anthropic",
            "modelstudio-coding-plan-anthropic",
        ] {
            let mut ms_anthropic = json!({});
            apply_review_thinking_controls(
                &mut ms_anthropic,
                review_api(kind),
                ModelPreset::OpenaiCompatible,
                ms_url,
                "qwen3.6-flash",
            );
            assert!(
                ms_anthropic.as_object().expect("body object").is_empty(),
                "{kind} 恒零字段（底座剥离不回写）: {ms_anthropic}"
            );
        }

        // 聚合商四 slug 全钉（r3 审 M6：此前只钉 openrouter，缩到单 slug 突变绿）
        // + 大小写变体（parse 归一）+ Inkling 例外。
        for slug in [
            "openrouter",
            "orcarouter",
            "novita",
            "together",
            "ORCAROUTER",
        ] {
            let mut agg = json!({});
            apply_review_thinking_controls(
                &mut agg,
                review_api(slug),
                ModelPreset::OpenaiCompatible,
                "https://gateway.example/v1",
                "deepseek/deepseek-chat-v4",
            );
            assert_eq!(agg["reasoning_effort"], json!("low"), "{slug}: {agg}");
            assert_eq!(
                agg["thinking"],
                json!({ "type": "enabled" }),
                "{slug}: {agg}"
            );
        }
        let mut inkling = json!({});
        apply_review_thinking_controls(
            &mut inkling,
            review_api("together"),
            ModelPreset::OpenaiCompatible,
            "https://api.together.xyz/v1",
            "thinkingmachines/inkling",
        );
        assert_eq!(inkling["reasoning_effort"], json!("low"), "{inkling}");
        // r4 审 m1：底座 `apply_inkling_reasoning_effort` 只对 Together 身份生效
        // （chat.rs `provider != ApiProvider::Together` 提前返回）——其他聚合
        // kind × inkling 走各自通用臂照发 effort+thinking，审查同形。
        for slug in ["openrouter", "orcarouter", "novita"] {
            let mut inkling_other = json!({});
            apply_review_thinking_controls(
                &mut inkling_other,
                review_api(slug),
                ModelPreset::OpenaiCompatible,
                "https://gateway.example/v1",
                "thinkingmachines/inkling",
            );
            assert_eq!(
                inkling_other["reasoning_effort"],
                json!("low"),
                "{slug} inkling: {inkling_other}"
            );
            assert_eq!(
                inkling_other["thinking"],
                json!({ "type": "enabled" }),
                "{slug} inkling 走通用臂: {inkling_other}"
            );
        }
        assert!(
            inkling.get("thinking").is_none(),
            "Inkling 不吃 Together 通用 thinking 扩展: {inkling}"
        );

        // huggingface（r3 审 M6 漏钉）+ canonical env-pin slug（r3 审 M3：
        // 规范拼写此前零命中）+ 大小写变体 + 不可解析兜底 Deepseek（引擎同尾）。
        let mut hf = json!({});
        apply_review_thinking_controls(
            &mut hf,
            review_api("huggingface"),
            ModelPreset::OpenaiCompatible,
            "https://gateway.example/v1",
            "some-model",
        );
        assert_eq!(hf["reasoning_effort"], json!("low"), "{hf}");
        assert!(hf.get("thinking").is_none(), "{hf}");
        let mut nim = json!({});
        apply_review_thinking_controls(
            &mut nim,
            review_api("nvidia-nim"),
            ModelPreset::OpenaiCompatible,
            "https://integrate.api.nvidia.com/v1",
            "some-model",
        );
        assert_eq!(
            nim["chat_template_kwargs"],
            json!({ "thinking": true, "reasoning_effort": "high" }),
            "{nim}"
        );
        let mut ollama_cloud = json!({});
        apply_review_thinking_controls(
            &mut ollama_cloud,
            review_api("ollama-cloud"),
            ModelPreset::OpenaiCompatible,
            "https://ollama.com/v1",
            "some-model",
        );
        assert_eq!(
            ollama_cloud["reasoning_effort"],
            json!("low"),
            "ollama-cloud 规范 slug: {ollama_cloud}"
        );
        let mut deepseek_cn = json!({});
        apply_review_thinking_controls(
            &mut deepseek_cn,
            review_api("deepseek-cn"),
            ModelPreset::OpenaiCompatible,
            "https://gateway.example/v1",
            "deepseek-v4-pro",
        );
        assert_eq!(
            deepseek_cn["reasoning_effort"],
            json!("low"),
            "{deepseek_cn}"
        );
        assert_eq!(
            deepseek_cn["thinking"],
            json!({ "type": "enabled" }),
            "{deepseek_cn}"
        );
        let mut sf_case = json!({});
        apply_review_thinking_controls(
            &mut sf_case,
            review_api("SiliconFlow-CN"),
            ModelPreset::OpenaiCompatible,
            "https://gateway.example/v1",
            "some-model",
        );
        assert_eq!(
            sf_case["reasoning_effort"],
            json!("high"),
            "大小写归一 SiliconFlow-CN: {sf_case}"
        );
        let mut moonshot_case = json!({});
        apply_review_thinking_controls(
            &mut moonshot_case,
            review_api("MoonShot"),
            ModelPreset::OpenaiCompatible,
            "https://gateway.example/v1",
            "kimi-k2.6",
        );
        assert_eq!(
            moonshot_case["thinking"],
            json!({ "type": "enabled" }),
            "大小写+别名归一 MoonShot: {moonshot_case}"
        );
        let mut unparseable = json!({});
        apply_review_thinking_controls(
            &mut unparseable,
            review_api("my-custom-relay"),
            ModelPreset::OpenaiCompatible,
            "https://gateway.example/v1",
            "some-model",
        );
        assert_eq!(
            unparseable["reasoning_effort"],
            json!("low"),
            "不可解析 pin 兜底 Deepseek（引擎 api_provider 同尾）: {unparseable}"
        );
        assert_eq!(
            unparseable["thinking"],
            json!({ "type": "enabled" }),
            "{unparseable}"
        );

        // Meta muse-spark（r3 审新臂）+ Google thinking_level + 非精确负例。
        let mut muse = json!({});
        apply_review_thinking_controls(
            &mut muse,
            review_api("meta"),
            ModelPreset::OpenaiCompatible,
            "https://gateway.example/v1",
            "muse-spark-1.3",
        );
        assert_eq!(muse["reasoning_effort"], json!("low"), "{muse}");
        let mut google = json!({});
        apply_review_thinking_controls(
            &mut google,
            review_api("google"),
            ModelPreset::OpenaiCompatible,
            "https://generativelanguage.googleapis.com/v1beta/openai",
            "gemini-3.6-flash",
        );
        assert_eq!(
            google["google"]["thinking_config"]["thinking_level"],
            json!("low"),
            "{google}"
        );
        let mut google_gateway = json!({});
        apply_review_thinking_controls(
            &mut google_gateway,
            review_api("google"),
            ModelPreset::OpenaiCompatible,
            "https://gateway.example/v1",
            "gemini-3.6-flash",
        );
        assert!(
            google_gateway.as_object().expect("body object").is_empty(),
            "Google 非精确端点 fail-close: {google_gateway}"
        );

        // Minimax 精确路由 adaptive（规范型号收敛，r3 审 M8：minimax-m-3 手写
        // 别名此前不命中）；网关仅 split（审查线特例，见块注）。
        let mut m3_alias = json!({});
        apply_review_thinking_controls(
            &mut m3_alias,
            review_api("minimax"),
            ModelPreset::OpenaiCompatible,
            "https://api.minimax.io/v1",
            "minimax-m-3",
        );
        assert_eq!(m3_alias["reasoning_split"], json!(true), "{m3_alias}");
        assert_eq!(
            m3_alias["thinking"],
            json!({ "type": "adaptive" }),
            "规范收敛 M3 精确路由 adaptive: {m3_alias}"
        );
    }

    #[test]
    fn review_request_body_mirrors_engine_wire_no_temperature() {
        let body = review_request_body("k3", "sys prompt", "user content", 8192, "max_tokens");
        assert!(
            body.get("temperature").is_none(),
            "aux review body must mirror the engine wire (no temperature): a \
             hard-coded 0 400s on sampling-pinned gateways — Kimi Coding Plan \
             live 2026-09-30: only 1 is allowed for this model"
        );
        assert_eq!(body["model"], "k3");
        assert_eq!(
            body["max_tokens"],
            json!(8192),
            "预算透传 max_tokens 参数(主对话同款算法值,由 \
             main_wire_max_output_tokens 计算),不得在 builder 内硬编码"
        );
        assert!(
            body.get("max_completion_tokens").is_none(),
            "换名集合之外的路由必须仍发 max_tokens(Kimi Code 会员端点 \
             2026-09-30 实测线材): {body}"
        );
        assert_eq!(body["stream"], false);
        assert_eq!(body["response_format"]["type"], "json_object");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "sys prompt");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"], "user content");

        // 换名集合路由：字段名换为 max_completion_tokens 且不再发 max_tokens
        // (引擎 apply_provider_token_limit 同款移除语义)。
        let swapped = review_request_body(
            "kimi-k3",
            "sys prompt",
            "user content",
            8192,
            "max_completion_tokens",
        );
        assert_eq!(swapped["max_completion_tokens"], json!(8192), "{swapped}");
        assert!(
            swapped.get("max_tokens").is_none(),
            "换名路由不得同时携带旧字段名: {swapped}"
        );
    }

    /// 引擎 token 上限字段换名判定逐字钉（`apply_provider_token_limit` 四类：
    /// XiaomiMimo / OpenAI 推理家族 / 精确 MiniMax-M3 路由 / 直连 Moonshot K3）；
    /// Kimi Code 会员端点（api.kimi.com/coding/v1）明确**不**换——那是
    /// 2026-09-30 实测过的 `max_tokens` 线。
    #[test]
    fn review_max_tokens_field_mirrors_engine_swap() {
        assert_eq!(
            review_max_tokens_field(
                review_api("moonshot"),
                "https://api.moonshot.cn/v1",
                "kimi-k3"
            ),
            "max_completion_tokens"
        );
        assert_eq!(
            review_max_tokens_field(
                review_api("moonshot"),
                "https://api.moonshot.cn/v1",
                "kimi-k2.6"
            ),
            "max_tokens"
        );
        assert_eq!(
            review_max_tokens_field(
                review_api("moonshot"),
                "https://api.kimi.com/coding/v1",
                "k3"
            ),
            "max_tokens",
            "Kimi Code 会员端点不换字段名(实测线材钉)"
        );
        assert_eq!(
            review_max_tokens_field(
                review_api("openai"),
                "https://api.openai.com/v1",
                "gpt-5.6-terra"
            ),
            "max_completion_tokens"
        );
        assert_eq!(
            review_max_tokens_field(review_api("openai"), "https://api.openai.com/v1", "gpt-4o"),
            "max_tokens"
        );
        assert_eq!(
            review_max_tokens_field(
                review_api("xiaomi-mimo"),
                "https://api.xiaomimimo.com/v1",
                "mimo-pro"
            ),
            "max_completion_tokens"
        );
        assert_eq!(
            review_max_tokens_field(
                review_api("minimax"),
                "https://api.minimaxi.com/v1",
                "MiniMax-M3"
            ),
            "max_completion_tokens"
        );
        assert_eq!(
            review_max_tokens_field(
                review_api("minimax"),
                "https://gateway.example/v1",
                "MiniMax-M3"
            ),
            "max_tokens",
            "MiniMax 换名只在精确第一方路由"
        );
        // r3 审 M7：引擎按 ApiProvider::parse（大小写不敏感 + 别名表）归一身份，
        // `kimi`/`MOONSHOT` 拼写在引擎侧换名 → 审查必须同换。
        assert_eq!(
            review_max_tokens_field(review_api("kimi"), "https://api.moonshot.ai/v1", "kimi-k3"),
            "max_completion_tokens",
            "`kimi` 是 Moonshot kind 的注册别名——换名判定按 kind 命中"
        );
        assert_eq!(
            review_max_tokens_field(
                review_api("MOONSHOT"),
                "https://api.moonshot.cn/v1",
                "kimi-k3"
            ),
            "max_completion_tokens",
            "大小写归一后换名"
        );
        assert_eq!(
            review_max_tokens_field(
                review_api("moonshotai"),
                "https://api.moonshot.cn/v1",
                "kimi-k3"
            ),
            "max_completion_tokens",
            "别名归一后换名"
        );
        // r3 审 M8：引擎换名判定吃规范化后的线材型号
        // （canonical_minimax_model_id 把 minimax-m-3 收敛到 MiniMax-M3），
        // 审查同形——手写别名拼写不再劈开字段决策。
        assert_eq!(
            review_max_tokens_field(
                review_api("minimax"),
                "https://api.minimax.io/v1",
                "minimax-m-3"
            ),
            "max_completion_tokens",
            "minimax-m-3 规范收敛到 MiniMax-M3 → 换名"
        );
        assert_eq!(
            review_max_tokens_field(
                review_api("minimax"),
                "https://api.minimax.io/v1",
                "MINIMAX_M_3"
            ),
            "max_completion_tokens",
            "下划线/大小写别名同样收敛换名"
        );
        assert_eq!(
            review_max_tokens_field(
                review_api("minimax"),
                "https://api.minimax.io/v1",
                "MiniMax-M2.7"
            ),
            "max_tokens",
            "非 M3 规范型号不换"
        );
    }

    /// Pin the call site, not just the builder: the builder pin above goes
    /// red when `review_request_body` itself changes, but a re-inlined body
    /// inside `model_review` would bypass it silently. The request region
    /// must build through the shared builder and must not carry a sampling
    /// parameter (the Kimi Coding Plan gateway rejects client temperature).
    #[test]
    fn model_review_call_site_builds_body_through_builder() {
        let source = include_str!("mod.rs");
        let start = source
            .find("async fn model_review")
            .expect("model_review definition present");
        let region = &source[start
            ..source[start..]
                .find("fn review_request_body")
                .expect("review_request_body definition present")
                + start];
        assert!(
            region.contains("review_request_body("),
            "model_review must build its one-shot body via review_request_body"
        );
        assert!(
            region.contains("main_wire_max_output_tokens("),
            "the review max_tokens must come from the bridge's main-wire budget \
             (the same algorithm as the main conversation); re-inlining a \
             constant here drifts from the engine — the old hard-coded 1600 \
             was consumed entirely by k3 reasoning tokens (2026-11-06)"
        );
        assert!(
            !region.contains("\"temperature\""),
            "the review request region must not re-inline a temperature field"
        );
        // r3 审 M5：上面的 pin 只护 builder 与预算来源；两条引擎对齐接线本身
        // （思考控制 + 换名判定）此前可被整体删除而全测试保持绿（突变实证）。
        // 一行 contains 断言把删除变成红。
        assert!(
            region.contains("apply_review_thinking_controls("),
            "model_review must layer the never-off thinking controls on the \
             one-shot body; deleting the call silently reverts to \
             thinking-off/zero-controls while every unit pin stays green"
        );
        assert!(
            region.contains("review_max_tokens_field("),
            "model_review must decide the token-limit field via \
             review_max_tokens_field (the engine's max_completion_tokens \
             swap); a literal here silently mismatches the swapped routes"
        );
        assert!(
            region.contains("post_anthropic_messages("),
            "preset Anthropic must keep riding the native Messages branch \
             (dropping the early return would send an OpenAI chat wire with \
             thinking fields to Anthropic endpoints)"
        );
        assert!(
            region.contains(".timeout(DEFAULT_TIMEOUT)"),
            "the review client must keep the compile-time-pinned timeout; \
             dropping the builder timeout leaves the const assert green \
             while the request runs unbounded"
        );
        // r4 审 m4：上面的 contains 钉挡不住**旁路**——region 内追加
        // request 级 `.timeout(Duration::from_secs(30))` 会静默压过 client 的
        // 180s（reqwest request timeout 优先），builder 后改写
        // `body["max_tokens"]` 也满足全部 contains 钉。两条负向断言把旁路变红。
        assert_eq!(
            region.matches(".timeout(").count(),
            1,
            "the request region must carry exactly one timeout — the pinned \
             builder timeout; a second request-level .timeout() silently \
             overrides it (reqwest request timeout wins over client timeout) \
             and a silent revert would pass every contains pin"
        );
        assert!(
            !region.contains("from_secs("),
            "no raw Duration::from_secs in the request region — an inline \
             request-level timeout would silently override the pinned \
             DEFAULT_TIMEOUT"
        );
        assert!(
            !region.contains("1600"),
            "the retired hard-coded token cap must not reappear inside the \
             request region — a post-build body[\"max_tokens\"] override \
             would satisfy every contains pin while silently shrinking the \
             engine-parity budget"
        );
    }

    /// 核账跳过哪些动作的契约：只有 accept(接受现状)/confirmed(需核实已确认)算已结，其余
    /// 都保留核。漏掉 confirmed 会让 Boss 已确认的账被重核→震荡。
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

    /// r4 审 m5:dismiss→再召唤的并发窗口里,新 settle(seq=2)先落、被顶掉的旧
    /// settle(seq=1)后落——纯"从后往前第一个"会把 Boss 没看过的旧审查当最近
    /// 一轮。可观察后果:旧 settle 是 pass(pos 偏早)时,数组序判定触发
    /// current_pos 守门 → 返回 None 重新立账,而真实最近一轮(seq2)已立了 issues
    /// 该进核账。带 seq 的 entry 按 seq 最大者选;老 entry(无 seq)按 0 计,
    /// 同为 0 时取数组序最后一条=旧行为原样保留。
    #[test]
    fn ledger_latest_picks_max_seq_not_array_order() {
        // 落序:seq2(新,issues)先落,seq1(旧,pass)后落——stale 在数组尾。
        let arr = vec![
            serde_json::json!({"pos":20,"seq":2,"review":{"artifact_path":"/p.md","issues":[
                {"text":"签证错","severity":"high","kind":"quality","resolution":"modify"}
            ]}}),
            serde_json::json!({"pos":10,"seq":1,"review":{"artifact_path":"/p.md","verdict":"pass","issues":[]}}),
        ];
        // 正确:latest=seq2(issues)→passed=false→读最近立账=签证错。
        // 若按数组序选 latest=seq1 pass(pos=10)→current_pos=20>10→None 重新立账,
        // 把 Boss 正在裁决的 seq2 账目甩掉重审——这就是突变要红的差异。
        let kept = ledger_from_entries(&arr, "/p.md", 20).expect("seq2 立账该进核账,不重新立账");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].text, "签证错", "latest 按 seq 取,不按数组序");
        // 兼容:无 seq 的老 entry 都按 0 计,同分取数组序最后一条=旧行为。
        let legacy = vec![
            serde_json::json!({"pos":10,"review":{"artifact_path":"/p.md","verdict":"pass","issues":[]}}),
            serde_json::json!({"pos":12,"review":{"artifact_path":"/p.md","issues":[
                {"text":"交通错","severity":"high","kind":"quality","resolution":"modify"}
            ]}}),
        ];
        let kept_legacy = ledger_from_entries(&legacy, "/p.md", 12).expect("无 seq 保持旧行为");
        assert_eq!(kept_legacy[0].text, "交通错", "老 entry(无 seq)latest 仍按数组序");
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
        assert!(
            output_language_directive("ja")
                .unwrap()
                .contains("Japanese")
        );
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
                thought_signature: None,
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
    fn project_keeps_canonical_web_search_facts() {
        let messages = vec![
            user_text("核对当前报价"),
            assistant_tool("w1", "Web", json!({"action": "search", "query": "报价"})),
            tool_result("w1", "当前报价：100 元"),
        ];
        let projected = project(&messages);
        assert!(projected.contains("当前报价：100 元"), "{projected}");
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
    fn last_artifact_path_picks_last_file_from_canonical_patch() {
        let messages = vec![assistant_tool(
            "p1",
            "File",
            json!({
                "action": "patch",
                "patch": "*** Begin Patch\n*** Update File: plan.md\n@@\n-old\n+new\n*** Add File: appendix.md\n+extra\n*** End Patch"
            }),
        )];
        assert_eq!(
            last_artifact_path(&messages),
            Some("appendix.md".to_string())
        );
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
        // Both frontend bridges (tauri/chat.js, web/bridge.js) compose
        // resolvePinvouReview sections per checked action and any of the five
        // headers can lead — build one case per real prefix (strings taken
        // verbatim from the bridge sources). Since the trilingual copy audit
        // the bridges assemble the sections in the UI language, so every
        // localized header variant must be recognized too.
        let real_prefixes = [
            (
                "fix",
                "请按下面的检阅意见，**只定向修改对应段落，不要全文重写**：",
            ),
            (
                "verify",
                "以下几条涉及外部事实，**先查证再改、标明依据，别凭记忆直接改**：",
            ),
            ("adopt", "以下事项我已拍板，按此更新产物："),
            (
                "ask",
                "以下待定项请用 request_user_input 正式问我，别自己猜：",
            ),
            (
                "fill",
                "以下维度产物还缺，请补充进去（保留其余、只增不改）：",
            ),
            (
                "fix/en",
                "Apply the review notes below — **revise only the targeted sections, do not rewrite the whole document**:",
            ),
            (
                "verify/en",
                "The items below involve external facts — **verify first, cite your sources, and don't edit from memory**:",
            ),
            (
                "adopt/en",
                "The following decisions are final — update the artifacts accordingly:",
            ),
            (
                "ask/en",
                "For the pending items below, ask me formally via request_user_input instead of guessing:",
            ),
            (
                "fill/en",
                "The artifact is still missing the dimensions below — add them (keep everything else; add only, don't rewrite):",
            ),
            (
                "fix/ja",
                "下のレビュー意見に従い、**該当するセクションのみを修正してください。全文の書き直しはしないでください**：",
            ),
            (
                "verify/ja",
                "以下の項目は外部事実に関わります。**必ず検証してから修正し、根拠を示してください（記憶に頼った編集はしないでください）**：",
            ),
            (
                "adopt/ja",
                "以下の事項は確定しました。この通り成果物を更新してください：",
            ),
            (
                "ask/ja",
                "以下の未確定項目については、推測せず request_user_input で正式に私に質問してください：",
            ),
            (
                "fill/ja",
                "成果物には以下の観点が不足しています。補足してください（既存部分は保持し、追記のみで書き換えないでください）：",
            ),
        ];
        for (kind, prefix) in real_prefixes {
            let messages = vec![
                user_text("帮我规划欧洲游，预算 2 万"),
                user_text(&format!(
                    "{prefix}\n- 【预算】采纳 8.5w（国庆 5-7w 难实现）"
                )),
            ];
            let p = project(&messages);
            assert!(
                p.contains("帮我规划欧洲游"),
                "Boss's own words kept ({kind}): {p}"
            );
            assert!(
                !p.contains("5-7w"),
                "transfer message led by the {kind} header (with last round's stale numbers) must be excluded: {p}"
            );
        }
    }

    #[test]
    fn b1_transfer_prefixes_stay_in_sync_with_bridge_sources() {
        // The five section headers are owned by the frontend bridges
        // (resolvePinvouReview, en/ja/zh BT_TABLE blocks). When a bridge copy
        // changes its wording, this fails until B1_TRANSFER_PREFIXES follows;
        // the projection test above covers the reverse direction.
        let tauri_bridge = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../src/platform/tauri/bridge.js"
        ));
        let web_bridge = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../src/platform/web/bridge.js"
        ));
        for prefix in B1_TRANSFER_PREFIXES {
            assert!(
                tauri_bridge.contains(prefix),
                "prefix absent from the tauri bridge source: {prefix}"
            );
            assert!(
                web_bridge.contains(prefix),
                "prefix absent from the web bridge source: {prefix}"
            );
        }
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
                    suggestion: String::new(),
                },
            ],
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
