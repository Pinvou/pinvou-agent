//! SDAN 工作流图执行器 (Harness Loop) —— 工作流无关,数据来自 bundle/workflow/<wf>/
//!
//! 每次 LLM turn 结束后，engine.rs 调 harness。完整执行图：
//!
//! ```text
//! TurnComplete
//!   ↓
//! 有 running_role?
//!   ├─ 有 → 该角色的 gate 类型?
//!   │       ├─ auto → 跑 gate_runner.py
//!   │       │         ├─ PASS → scheduler --complete → scheduler --next → dispatch
//!   │       │         └─ FAIL → rollback_scope?
//!   │       │                   ├─ local → inject 修复 prompt (重试 ≤ max_retries)
//!   │       │                   └─ structural → scheduler --rollback → scheduler --next
//!   │       └─ human → emit gate_approval → 暂停等用户
//!   │                   用户确认 → scheduler --complete → scheduler --next
//!   │                   用户拒绝 → inject 重做 prompt
//!   └─ 无 → scheduler --next → dispatch 第一个角色
//! ```

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::platform::paths;

/// 按 UTF-8 char 边界向下取整截断 `s` 到 ≤ `max_bytes` 字节，返回前缀切片。
/// 直接 `&s[..max_bytes]` 在 max_bytes 落在多字节字符(中文)中间时会 panic——
/// 角色产出几乎全是中文,曾导致 build_review_prompt 在 spawn_blocking 里 panic
/// → turn 崩溃 → engine busy flag 永不复位 → 卡死(P0 根因)。所有按字节切中文的
/// 地方都必须走这里。
fn truncate_on_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ── 调度器 JSON 结构 ──
// 这些 struct 镜像 Python scheduler.py 产出的 JSON schema;部分字段(all_actionable、
// failed_roles、output_path)由 Python 端产出但 Rust 当前未读取,保留是为了显式记录
// 契约,避免日后 Python 改字段时 Rust 端无感知。
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct SchedulerDecision {
    pub action: String,
    #[serde(default)]
    pub role_id: Option<String>,
    #[serde(default)]
    pub role_name: Option<String>,
    #[serde(default)]
    pub all_actionable: Option<Vec<String>>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub waiting_roles: Option<Vec<String>>,
    #[serde(default)]
    pub failed_roles: Option<Vec<String>>,
    /// [per_page] dispatch_batch：fan-out 节点拆出的 N 个 per-page 子任务。
    #[serde(default)]
    pub tasks: Option<Vec<SchedulerTask>>,
}

/// [per_page] scheduler 展开的单页任务（scheduler.py build_tasks_for 产出）。
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct SchedulerTask {
    pub page: u32,
    pub task_id: String,
    #[serde(default)]
    pub output_path: String,
    /// [per_page·多产物] 本实例全部产物绝对路径（illustrator 一页多槽时多张图）。
    /// 空 = 单产物角色（slide_writer），realness 走 output_template 反推。
    #[serde(default)]
    pub outputs: Vec<String>,
    /// 本页版式（如 "L01"）。per_page 派发据此把 [STATIC] 段裁到【只该页模板】，
    /// 避免单页 agent 把全量 11 个静态资产(63KB)读进上下文撞 SSE 超时。缺省空=不裁。
    #[serde(default)]
    pub layout: String,
    /// 单页任务交代，拼到该页 SubAgent prompt 末尾。
    pub addendum: String,
}

#[derive(Debug, Deserialize)]
struct GateResult {
    #[serde(default)]
    verdict: String,
    #[serde(default)]
    findings: Vec<GateFinding>,
}

#[derive(Debug, Deserialize)]
struct GateFinding {
    #[serde(default)]
    severity: String,
    #[serde(default)]
    rule: String,
    #[serde(default)]
    rollback_scope: String,
    #[serde(default)]
    fix_hint: String,
    #[serde(default)]
    message: String,
    /// 结构化回滚信号（SDAN/02·06）：gate_runner.py 对 structural finding
    /// 显式产出，取值 ∈ route_table.rollback_dispatch 键。空 = 未分类，
    /// 由 find_rollback_rule 显式告警兜底，不静默猜。
    #[serde(default)]
    violation_type: String,
}

// ── Harness 返回值 ──

#[derive(Debug)]
pub enum HarnessAction {
    /// Harness 派发真 SubAgent。forwarder 发 `Op::SpawnSubAgent`，
    /// 用 registry 的 `allowed_tools` 白名单 + `max_steps` 在独立上下文执行。
    /// `prompt` = registry role prompt + 文件铁律 + 品悟交代句（+ 修复指令）。
    SpawnAgent {
        role_id: String,
        role_name: String,
        prompt: String,
        allowed_tools: Vec<String>,
        max_steps: Option<u32>,
        /// [pinvou3-fork] 结构化产出 schema(registry.output_schema)。`Some` 时
        /// SubAgent 会被强制走 submit_output 提交(见 docs/SDAN/12-structured-output.md)。
        output_schema: Option<serde_json::Value>,
        /// [pinvou3-fork] 写文件型角色:registry.outputs 非空且无结构化 output_schema。
        expects_file_output: bool,
    },
    /// [per_page] 纵向 fan-out：一个逻辑节点拆成 N 个 per-page SubAgent 并发派发。
    /// engine 并发发 N 个 `Op::SpawnSubAgent`（role=`{base_role}#pNN`），收齐 N 个
    /// `AgentComplete` 后对**单一逻辑节点** `base_role` 调一次 `step_after_role`。
    SpawnAgentBatch {
        base_role: String,
        role_name: String,
        tasks: Vec<SubAgentTask>,
    },
    /// 需要用户审批
    WaitForHuman {
        role_id: String,
        role_name: String,
        description: String,
    },
    AllDone,
    Blocked {
        message: String,
    },
    NotApplicable,
    Error(String),
}

/// [per_page] 单页 SubAgent 派发单元（prompt 已拼好，含单页交代）。
#[derive(Debug)]
pub struct SubAgentTask {
    /// `{base_role}#pNN`，engine 据此关联 join；前缀 `#` 前是逻辑节点名。
    pub agent_role: String,
    pub page: u32,
    /// 本实例全部产物绝对路径（空=按 output_template 反推，见 SchedulerTask.outputs）。
    pub outputs: Vec<String>,
    pub prompt: String,
    pub allowed_tools: Vec<String>,
    pub max_steps: Option<u32>,
    pub output_schema: Option<serde_json::Value>,
    pub expects_file_output: bool,
}

// ── 路径工具 ──
// [工作流分离] 引擎脚本/数据路径都按"工作流名"解析:bundle/workflow/<wf>/...
// <wf> 由 scenario(项目)或 role_id(角色所属)推出,走 WorkflowRegistry
// (扫各 workflow.json 的 scenarios / 各 agent_registry.json 的 agents)。

/// 兜底工作流:第一个已发现的(老项目缺 scenario / 角色反查不中时用,
/// 解析出的路径至少存在,失败也走有日志的脚本报错而不是 panic)。
fn fallback_workflow() -> String {
    crate::features::workflow::workflow_registry::discover()
        .into_iter()
        .map(|w| w.id)
        .next()
        .unwrap_or_else(|| "sansheng-liubu".to_string())
}

/// scenario → 工作流名。查不到回落 fallback(兼容缺 scenario 的老项目)。
pub(crate) fn workflow_name_for_scenario(scenario: &str) -> String {
    crate::features::workflow::workflow_registry::by_scenario(scenario)
        .map(|w| w.id)
        .unwrap_or_else(fallback_workflow)
}

/// 项目 → 所属工作流名(读 _state 的 scenario)。
pub(crate) fn workflow_of_project(project: &Path) -> String {
    workflow_name_for_scenario(&read_scenario(project).unwrap_or_default())
}

/// 角色 id → 所属工作流名(角色跨工作流不重叠)。差事节点(`libu~1`)/分页实例
/// (`slide_writer#3`)先剥后缀取基角色,再扫各工作流 registry 命中;查不到回落 fallback。
pub(crate) fn workflow_of_role(role_id: &str) -> String {
    let base = role_id.split(['~', '#']).next().unwrap_or(role_id);
    for wf in crate::features::workflow::workflow_registry::discover() {
        if read_registry_for(&wf.id)
            .get("agents")
            .and_then(|a| a.get(base))
            .is_some()
        {
            return wf.id;
        }
    }
    fallback_workflow()
}

/// 工作流根目录(解包后绝对路径,含 scripts/ 数据文件)。SubAgent prompt 注入此路径,
/// 让其用绝对路径调脚本 / 读资源(templates/base.css、reference/design_tokens.md 等)。
pub(crate) fn workflow_root_for(workflow: &str) -> PathBuf {
    paths::bundle_workflow_dir().join(workflow)
}

pub(crate) fn scheduler_path_for(workflow: &str) -> PathBuf {
    workflow_root_for(workflow)
        .join("scripts")
        .join("scheduler.py")
}
fn gate_runner_path_for(workflow: &str) -> PathBuf {
    workflow_root_for(workflow)
        .join("scripts")
        .join("gate_runner.py")
}
fn deliverable_validator_path_for(workflow: &str) -> PathBuf {
    workflow_root_for(workflow)
        .join("scripts")
        .join("validate_deliverable.py")
}
fn warmup_check_path_for(workflow: &str) -> PathBuf {
    workflow_root_for(workflow)
        .join("scripts")
        .join("warmup_check.py")
}

/// 读 agent_registry.json（能力真相源）。失败返回空 Object。
fn read_registry_for(workflow: &str) -> serde_json::Value {
    std::fs::read_to_string(workflow_root_for(workflow).join("agent_registry.json"))
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_else(|| serde_json::Value::Object(Default::default()))
}

/// 读 route_table.json（调度真相源）。失败返回空 Object。
fn read_route_table_for(workflow: &str) -> serde_json::Value {
    std::fs::read_to_string(workflow_root_for(workflow).join("route_table.json"))
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_else(|| serde_json::Value::Object(Default::default()))
}

/// 读取全量 agent 状态快照（给前端初始化 + 每次推进后刷新）。
/// 返回 JSON: {"roles": {"role_id": {status, name, depends_on, last_gate_verdict?, outputs_present?, last_run_ts?}, ...}, "project_dir": "...", ...}
///
/// scheduler.py --status 返回基础字段；本函数额外 enrich：
/// - `last_gate_verdict`: 最近一次 gate report 的 verdict（PASS/FAIL/WARN）
/// - `outputs_present`: agent_registry.outputs glob 后实际存在的文件数
/// - `last_run_ts`: flow_log.jsonl / agent_log.jsonl 里该 role 最近一条事件的时间戳
/// - `project_dir`（顶层）: 项目目录绝对路径，前端调 get_role_outputs / get_gate_report 时用
pub fn read_full_agent_state(workspace: &Path) -> Option<serde_json::Value> {
    let project = find_project_dir(workspace)?;
    let scenario = read_scenario(&project).unwrap_or_else(|| "solution_deck".to_string());
    let json = run_scheduler(&project, &["--scenario", &scenario, "--status"]).ok()?;
    let mut v: serde_json::Value = serde_json::from_str(&json).ok()?;

    // Enrich roles 字段
    if let Some(roles) = v.get_mut("roles").and_then(|r| r.as_object_mut()) {
        for (role_id, role_obj) in roles.iter_mut() {
            if let Some(obj) = role_obj.as_object_mut() {
                if let Some(verdict) = read_last_gate_verdict(&project, role_id) {
                    obj.insert(
                        "last_gate_verdict".into(),
                        serde_json::Value::String(verdict),
                    );
                }
                obj.insert(
                    "outputs_present".into(),
                    serde_json::Value::from(count_outputs_present(&project, role_id)),
                );
                if let Some(ts) = read_last_run_ts(&project, role_id) {
                    obj.insert("last_run_ts".into(), serde_json::Value::String(ts));
                }
            }
        }
    }

    // 顶层附 project_dir + scenario（前端 attachRun 恢复 run 态时用）
    // + workflow_id/ui（前端泳道/表单/标题全数据驱动,见 workflow.json 的 ui 块）
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "project_dir".into(),
            serde_json::Value::String(project.to_string_lossy().to_string()),
        );
        obj.insert(
            "scenario".into(),
            serde_json::Value::String(scenario.clone()),
        );
        let workflow = workflow_name_for_scenario(&scenario);
        obj.insert(
            "workflow_id".into(),
            serde_json::Value::String(workflow.clone()),
        );
        if let Some(wf) = crate::features::workflow::workflow_registry::discover()
            .into_iter()
            .find(|w| w.id == workflow)
        {
            obj.insert("ui".into(), wf.ui);
        }
        if let Some(stop) = stop_info_for_project(&project) {
            obj.insert("stopped".into(), serde_json::Value::Bool(true));
            if let Some(value) = stop.get("stopped_at") {
                obj.insert("stopped_at".into(), value.clone());
            }
            if let Some(value) = stop.get("reason") {
                obj.insert("stop_reason".into(), value.clone());
            }
        } else if let Some(reason) = workflow_failure_reason_for_project(&project) {
            obj.insert("blocked".into(), serde_json::Value::Bool(true));
            obj.insert("blocked_reason".into(), serde_json::Value::String(reason));
        } else if let Ok(report) = read_warmup_report(&project) {
            if report.get("status").and_then(|value| value.as_str()) == Some("blocked") {
                obj.insert("blocked".into(), serde_json::Value::Bool(true));
                if let Some(reason) = warmup_block_reason(&report) {
                    obj.insert("blocked_reason".into(), serde_json::Value::String(reason));
                }
                obj.insert("warmup_report".into(), report);
            }
        }
    }

    Some(v)
}

fn read_last_gate_verdict(project: &Path, role_id: &str) -> Option<String> {
    let dir = project.join("_state").join("gate_reports");
    if !dir.exists() {
        return None;
    }
    let prefix = format!("{role_id}_");
    let mut latest: Option<(u64, PathBuf)> = None;
    let entries = std::fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.starts_with(&prefix) || !name.ends_with(".json") {
            continue;
        }
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        match &latest {
            Some((m, _)) if *m >= mtime => {}
            _ => latest = Some((mtime, p)),
        }
    }
    let (_, path) = latest?;
    let content = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    v.get("verdict").and_then(|v| v.as_str()).map(String::from)
}

fn count_outputs_present(project: &Path, role_id: &str) -> usize {
    let registry_path =
        workflow_root_for(&workflow_of_project(project)).join("agent_registry.json");
    let content = match std::fs::read_to_string(&registry_path) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let registry: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    let outputs = match registry
        .get("agents")
        .and_then(|a| a.get(role_id))
        .and_then(|r| r.get("outputs"))
        .and_then(|o| o.as_array())
    {
        Some(o) => o.clone(),
        None => return 0,
    };
    let mut count = 0;
    for pat in outputs {
        let Some(pat) = pat.as_str() else { continue };
        let abs = project.join(pat);
        if abs.to_string_lossy().contains('*') {
            let parent = match abs.parent() {
                Some(p) => p.to_path_buf(),
                None => continue,
            };
            let file_pat = match abs.file_name() {
                Some(f) => f.to_string_lossy().to_string(),
                None => continue,
            };
            if !parent.exists() {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(parent) else {
                continue;
            };
            for entry in entries.flatten() {
                let p = entry.path();
                if !p.is_file() {
                    continue;
                }
                let name = p
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                if simple_glob_match(&file_pat, &name) {
                    count += 1;
                }
            }
        } else if abs.exists() && abs.is_file() {
            count += 1;
        }
    }
    count
}

fn simple_glob_match(pattern: &str, name: &str) -> bool {
    if let Some(idx) = pattern.find('*') {
        let prefix = &pattern[..idx];
        let suffix = &pattern[idx + 1..];
        name.starts_with(prefix)
            && name.ends_with(suffix)
            && name.len() >= prefix.len() + suffix.len()
    } else {
        pattern == name
    }
}

fn read_last_run_ts(project: &Path, role_id: &str) -> Option<String> {
    let mut latest_ts: Option<String> = None;
    for file_name in ["flow_log.jsonl", "agent_log.jsonl", "workflow_flow.log"] {
        let log_path = project.join("_state").join(file_name);
        let Ok(content) = std::fs::read_to_string(&log_path) else {
            continue;
        };
        for line in content.lines() {
            let rec: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if rec.get("role_id").and_then(|r| r.as_str()) != Some(role_id) {
                continue;
            }
            let Some(ts) = rec
                .get("timestamp")
                .or_else(|| rec.get("ts"))
                .and_then(|t| t.as_str())
            else {
                continue;
            };
            if latest_ts.as_deref().is_none_or(|latest| ts > latest) {
                latest_ts = Some(ts.to_string());
            }
        }
    }
    latest_ts
}

/// Dump gate 结果到 `_state/gate_reports/<role>_<ts>.json`，供前端 `get_gate_report` 命令读取。
/// 设计文档 plan § 1 详情 Drawer "Gate Report" Tab 的数据源。
fn write_gate_report(project: &Path, role_id: &str, layer: u8, gate: &GateResult) {
    use chrono::Utc;
    let dir = project.join("_state").join("gate_reports");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let ts = Utc::now().format("%Y%m%dT%H%M%S").to_string();
    let path = dir.join(format!("{role_id}_{ts}.json"));
    let findings: Vec<serde_json::Value> = gate
        .findings
        .iter()
        .map(|f| {
            serde_json::json!({
                "severity": f.severity,
                "rule": f.rule,
                "rollback_scope": f.rollback_scope,
                "fix_hint": f.fix_hint,
                "message": f.message,
                "violation_type": f.violation_type,
            })
        })
        .collect();
    let report = serde_json::json!({
        "role_id": role_id,
        "layer": layer,
        "verdict": gate.verdict,
        "findings": findings,
        "ts": ts,
    });
    if let Ok(content) = serde_json::to_string_pretty(&report) {
        let _ = std::fs::write(&path, content);
    }
}

pub fn find_project_dir(workspace: &Path) -> Option<PathBuf> {
    let progress = workspace.join("_state").join("workflow_progress.json");
    if progress.exists() {
        return Some(workspace.to_path_buf());
    }
    // 遍历 workspace 子目录，选 `_state/workflow_progress.json` mtime 最新的。
    // 之前是返回第一个匹配 → 老项目（按字典序）总是被选中，新建项目永远拿不到。
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    if let Ok(entries) = std::fs::read_dir(workspace) {
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            if p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            let progress = p.join("_state").join("workflow_progress.json");
            if !progress.exists() {
                continue;
            }
            let mtime = progress
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .unwrap_or(std::time::UNIX_EPOCH);
            match &best {
                Some((m, _)) if *m >= mtime => {}
                _ => best = Some((mtime, p)),
            }
        }
    }
    best.map(|(_, p)| p)
}

/// 初始化一个新的工作流项目目录。
///
/// 在 `workspace` 下创建 `wf-<ts>-<scenario>/`，写入：
/// - `_state/workflow_progress.json`（scenario + 创建时间，roles 由 scheduler.py 首次 --next 时 populate）
/// - `_state/brief.json`（scenario + 合并 brief_init 字段）
/// - 目录骨架 `_research/` `_audit/` `HTML_Deck/` `配套材料/`
///
/// 设计文档 docs/SDAN/02-router.md（on_start）+ 11-validation（项目预检）。
/// 返回创建的项目目录绝对路径。
pub fn init_project(
    workspace: &Path,
    scenario: &str,
    brief_init: &serde_json::Value,
) -> Result<PathBuf, String> {
    use chrono::Utc;

    // 合法场景 = 各 workflow.json 认领的 scenarios 并集(基座不硬编码场景名)
    let valid_scenarios: Vec<String> = crate::features::workflow::workflow_registry::discover()
        .into_iter()
        .flat_map(|w| w.scenarios)
        .collect();
    if !valid_scenarios.iter().any(|s| s == scenario) {
        return Err(format!(
            "invalid scenario {scenario:?}, must be one of {valid_scenarios:?}"
        ));
    }

    let ts = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    // 中性前缀 wf-。项目路径会进每个角色提示词("项目目录: …"),旧前缀 ppt-
    // 实测污染规划:中书省看见目录名就把交付形态脑补成 PPT。旧目录不重命名:
    // find_project_dir 按 _state/workflow_progress.json 找(与前缀无关),
    // workflow_migrate 两种前缀都认。
    let project = workspace.join(format!("wf-{ts}-{scenario}"));

    for sub in ["_state", "_research", "_audit", "配套材料"] {
        std::fs::create_dir_all(project.join(sub)).map_err(|e| format!("create {sub}: {e}"))?;
    }

    let progress = serde_json::json!({
        "scenario": scenario,
        "created_at": ts,
        "version": 1,
    });
    std::fs::write(
        project.join("_state").join("workflow_progress.json"),
        serde_json::to_string_pretty(&progress).map_err(|e| format!("serialize progress: {e}"))?,
    )
    .map_err(|e| format!("write workflow_progress.json: {e}"))?;

    let mut brief = serde_json::Map::new();
    brief.insert(
        "scenario".into(),
        serde_json::Value::String(scenario.into()),
    );
    brief.insert("_init_at".into(), serde_json::Value::String(ts.clone()));
    if let Some(obj) = brief_init.as_object() {
        for (k, v) in obj {
            brief.insert(k.clone(), v.clone());
        }
    }
    std::fs::write(
        project.join("_state").join("brief.json"),
        serde_json::to_string_pretty(&serde_json::Value::Object(brief))
            .map_err(|e| format!("serialize brief: {e}"))?,
    )
    .map_err(|e| format!("write brief.json: {e}"))?;

    log_flow(
        &project,
        "project_initialized",
        &[
            ("scenario", scenario),
            ("project_dir", &project.to_string_lossy()),
        ],
    );

    // scheduler.py --status 验证项目结构合法。目录已经初始化，失败不在这里删除项目；
    // 但必须把完整 stderr 持久化，后续 kick 的 --next 会将同一错误返回给前端。
    if let Err(e) = run_scheduler(&project, &["--scenario", scenario, "--status"]) {
        eprintln!("[harness::init_project] scheduler --status warning: {e}");
        record_runtime_failure_for_project(&project, "", "scheduler_status", &e);
    }

    Ok(project)
}

fn read_scenario(project: &Path) -> Option<String> {
    let content =
        std::fs::read_to_string(project.join("_state").join("workflow_progress.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    v.get("scenario")?.as_str().map(String::from)
}

const WORKFLOW_STOP_MARKER: &str = "workflow_stopped.json";

fn stop_marker_path(project: &Path) -> PathBuf {
    project.join("_state").join(WORKFLOW_STOP_MARKER)
}

fn stop_info_for_project(project: &Path) -> Option<serde_json::Value> {
    let content = std::fs::read_to_string(stop_marker_path(project)).ok()?;
    serde_json::from_str(&content).ok()
}

/// Whether the latest workflow project in `workspace` was explicitly stopped
/// by the user. Late SubAgent completion events consult this boundary before
/// they are allowed to advance the scheduler.
pub fn workflow_is_stopped(workspace: &Path) -> bool {
    find_project_dir(workspace)
        .as_deref()
        .is_some_and(|project| stop_marker_path(project).is_file())
}

/// Persist an irreversible stop marker for the current run and return the
/// original brief so the UI can prefill “edit and restart”. A stopped run is
/// never resumed in place; restarting creates a fresh run/session.
pub fn stop_workflow(
    workspace: &Path,
    session_id: &str,
    reason: &str,
) -> Result<serde_json::Value, String> {
    let project = find_project_dir(workspace).ok_or_else(|| "no project found".to_string())?;
    let marker_path = stop_marker_path(&project);
    let existing = stop_info_for_project(&project);
    let stopped_at = existing
        .as_ref()
        .and_then(|value| value.get("stopped_at"))
        .and_then(|value| value.as_str())
        .map(String::from)
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
    let marker = serde_json::json!({
        "stopped": true,
        "stopped_at": stopped_at,
        "reason": reason,
        "session_id": session_id,
    });
    if existing.is_none() {
        let tmp_path = marker_path.with_extension("json.tmp");
        std::fs::write(
            &tmp_path,
            serde_json::to_string_pretty(&marker)
                .map_err(|e| format!("serialize stop marker: {e}"))?,
        )
        .map_err(|e| format!("write stop marker: {e}"))?;
        std::fs::rename(&tmp_path, &marker_path).map_err(|e| format!("commit stop marker: {e}"))?;
    }
    log_flow(
        &project,
        "workflow_stopped",
        &[("reason", reason), ("session_id", session_id)],
    );
    batch_clear_session(session_id);

    let brief = std::fs::read_to_string(project.join("_state").join("brief.json"))
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    Ok(serde_json::json!({
        "ok": true,
        "session_id": session_id,
        "project_dir": project.to_string_lossy(),
        "scenario": read_scenario(&project),
        "brief": brief,
        "stopped_at": stopped_at,
        "reason": reason,
    }))
}

fn read_role_gate_type(role_id: &str) -> String {
    // gate 是能力字段，真相源 = agent_registry.json（route_table 不重复定义）。
    read_registry_for(&workflow_of_role(role_id))
        .get("agents")
        .and_then(|a| a.get(role_id))
        .and_then(|r| r.get("gate"))
        .and_then(|g| g.as_str())
        .unwrap_or("auto")
        .to_string()
}

/// [B2] 差事节点 id(<bu>~<seq>)→ 所属部(查 registry 能力/gate 用);非差事原样返回。
/// 分隔符用 `~` 而非 `#`——`#` 是 per_page 页实例(<role>#pNN)的约定,复用会被
/// engine 的 per_page 完成逻辑劫持(差事结果误记到静态部节点)。
fn bu_of(role_id: &str) -> String {
    match role_id.split_once('~') {
        Some((bu, _)) => bu.to_string(),
        None => role_id.to_string(),
    }
}

/// [B2] 尚书省派完单 → 调 dispatch_graph.py 编译差事图,写 _state/dynamic_routes.json。
/// 成功 Ok;编译失败(派单不合法,wave/bu 校验等)→ Err(stderr)。
fn materialize_dispatch_graph(project: &Path) -> Result<(), String> {
    let script = workflow_root_for(&workflow_of_project(project))
        .join("scripts")
        .join("dispatch_graph.py");
    if !script.exists() {
        return Err(format!("dispatch_graph.py not found: {}", script.display()));
    }
    let scripts_dir = script.parent().unwrap_or(project);
    let args = [script.to_string_lossy().to_string(),
        project.to_string_lossy().to_string()];
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    run_python(&arg_refs, scripts_dir).map(|_| ())
}

/// 解析角色 gate 类型。真相源 = agent_registry.json 的 gate 字段(auto / human)。
/// (legacy-ppt-workflow 专属的 human_if_custom/visual_theme 收敛随该工作流 2026-06-11 存档移除)
fn resolve_gate_type(role_id: &str, project: &Path) -> String {
    let _ = project;
    read_role_gate_type(&bu_of(role_id))
}

// ── 日志 ──

fn log_flow(project: &Path, event: &str, extra: &[(&str, &str)]) {
    let mut record = serde_json::Map::new();
    record.insert(
        "timestamp".into(),
        serde_json::Value::String(chrono::Local::now().to_rfc3339()),
    );
    record.insert("layer".into(), serde_json::Value::String("flow".into()));
    record.insert("event".into(), serde_json::Value::String(event.into()));
    for (k, v) in extra {
        record.insert((*k).into(), serde_json::Value::String((*v).into()));
    }
    let state_dir = project.join("_state");
    let path = state_dir.join("flow_log.jsonl");
    let result = std::fs::create_dir_all(&state_dir).and_then(|_| {
        let mut line = serde_json::to_string(&record).unwrap_or_default();
        line.push('\n');
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| file.write_all(line.as_bytes()))
    });
    if let Err(error) = result {
        eprintln!(
            "[harness] write workflow log failed ({}): {error}",
            path.display()
        );
    }
}

pub(crate) fn record_runtime_failure(workspace: &Path, role_id: &str, stage: &str, error: &str) {
    let Some(project) = find_project_dir(workspace) else {
        eprintln!("[harness] runtime failure without workflow project: {stage}: {error}");
        return;
    };
    record_runtime_failure_for_project(&project, role_id, stage, error);
}

fn record_runtime_failure_for_project(project: &Path, role_id: &str, stage: &str, error: &str) {
    let detail = failure_detail(error);
    let reason = failure_summary(&detail);
    let category = failure_category(&detail);
    log_flow(
        project,
        "runtime_failure",
        &[
            ("role_id", role_id),
            ("stage", stage),
            ("category", category),
            ("reason", &reason),
            ("detail", &detail),
        ],
    );
}

// ── Subprocess ──

// [2026-06-04] 30→120s:gate_runner --layer 1 对 23 页 deck 实测 31.8s,30s 差 2 秒
// 被杀 → designer 后推进链静默断头(MegaBook run 实锤)。对齐 audit_format 工具的
// 120s。深层债:layer1 耗时该优化(31s 大头在逐页结构审计)。
const SUBPROCESS_TIMEOUT_SECS: u64 = 120;
const WARMUP_TIMEOUT_SECS: u64 = 300;

fn run_python(args: &[&str], cwd: &Path) -> Result<String, String> {
    run_python_with_timeout(args, cwd, SUBPROCESS_TIMEOUT_SECS)
}

fn run_python_with_timeout(args: &[&str], cwd: &Path, timeout_secs: u64) -> Result<String, String> {
    // warmup 只做本地前置条件检查，不再请求模型接口。仅轻量读取模型 base_url，
    // 禁止为每个 Python 子进程重新 boot bridge（会重复解包 bundle、同步凭据和清理文件）。
    // API Key 不暴露给 Python 调度/验收子进程。
    let base_url = std::env::var("PINVOU3_MODEL_BASE_URL")
        .unwrap_or_else(|_| crate::features::monitor::vllm_base_url());
    let mut command = crate::platform::process::python_command();
    let program = command.get_program().to_string_lossy().into_owned();
    command
        .args(args)
        .current_dir(cwd)
        .env("PYTHONPATH", cwd)
        .env("PYTHONIOENCODING", "utf-8") // Windows stdout 默认 GBK，中文 print 会 UnicodeEncodeError
        .env("PINVOU3_MODEL_BASE_URL", base_url);
    // 平台层负责隐藏控制台、持续排空管道与超时回收，业务层只解释协议输出。
    let output = crate::platform::process::output_with_timeout(
        command,
        std::time::Duration::from_secs(timeout_secs),
    )
    .map_err(|error| {
        let message = if error.starts_with("spawn ") {
            format!("工作流 Python 启动失败 (interpreter={program}): {error}")
        } else if error.contains("timed out") {
            format!(
                "工作流 Python 执行超时 (interpreter={program}, timeout={timeout_secs}s): {error}"
            )
        } else {
            format!("工作流 Python 执行失败 (interpreter={program}): {error}")
        };
        eprintln!("[harness] {message}");
        message
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() && !stderr.is_empty() {
        eprintln!(
            "[harness] {program} exit={} stderr={}",
            output.status,
            truncate_on_char_boundary(&stderr, 300)
        );
    }

    if !output.status.success() {
        let detail = subprocess_failure_detail(&stdout, &stderr);
        return Err(format!(
            "工作流 Python 执行失败 (interpreter={program}, exit={}): {detail}",
            output.status
        ));
    }

    Ok(stdout)
}

/// 保留调度器 stderr（含 Python traceback）供 flow_log.jsonl 持久化；极少数程序只把
/// 错误写 stdout，因此 stderr 为空时回退 stdout。两者都有时同时记录，避免误诊。
fn subprocess_failure_detail(stdout: &str, stderr: &str) -> String {
    let stdout = stdout.trim();
    let stderr = stderr.trim();
    let detail = match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => "无错误输出".to_string(),
        (true, false) => stderr.to_string(),
        (false, true) => stdout.to_string(),
        (false, false) => format!("stderr:\n{stderr}\nstdout:\n{stdout}"),
    };
    truncate_on_char_boundary(&detail, 4000).to_string()
}

fn run_scheduler(project: &Path, args: &[&str]) -> Result<String, String> {
    let script = scheduler_path_for(&workflow_of_project(project));
    if !script.exists() {
        return Err(format!("scheduler.py not found: {}", script.display()));
    }
    let scripts_dir = script.parent().unwrap_or(project);
    let mut full_args = vec![
        script.to_string_lossy().to_string(),
        project.to_string_lossy().to_string(),
    ];
    full_args.extend(args.iter().map(|s| s.to_string()));
    let arg_refs: Vec<&str> = full_args.iter().map(|s| s.as_str()).collect();
    run_python(&arg_refs, scripts_dir)
}

fn run_warmup(project: &Path) -> Result<serde_json::Value, String> {
    let script = warmup_check_path_for(&workflow_of_project(project));
    if !script.exists() {
        return Err(format!("warmup_check.py not found: {}", script.display()));
    }
    let scripts_dir = script.parent().unwrap_or(project);
    let args = [script.to_string_lossy().to_string(),
        project.to_string_lossy().to_string()];
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    match run_python_with_timeout(&arg_refs, scripts_dir, WARMUP_TIMEOUT_SECS) {
        Ok(stdout) => match serde_json::from_str(&stdout) {
            Ok(v) => Ok(v),
            Err(_) => read_warmup_report(project),
        },
        Err(e) => {
            // 进程失败 (含超时被 kill)：尝试读已写盘的报告
            match read_warmup_report(project) {
                Ok(report) => {
                    if report.get("status").and_then(|v| v.as_str()) == Some("blocked") {
                        Err(report.to_string())
                    } else {
                        Ok(report)
                    }
                }
                // 报告也读不出来 → 真正的 blocked（无 ground truth 默认从严）
                Err(report_err) => Err(serde_json::json!({
                    "status": "blocked",
                    "error": e,
                    "report_error": report_err,
                })
                .to_string()),
            }
        }
    }
}

fn read_warmup_report(project: &Path) -> Result<serde_json::Value, String> {
    let path = project.join("_state").join("warmup_report.json");
    if !path.exists() {
        return Err(format!("warmup_report.json missing: {}", path.display()));
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("read warmup_report.json: {e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("parse warmup_report.json: {e}"))
}

/// 从 warmup 报告提取一条适合直接展示给用户的阻断原因。优先返回具体检查项；
/// 解释器未启动、来不及生成检查项时回退底层错误，避免把整份 JSON 塞进界面。
pub(crate) fn warmup_block_reason(report: &serde_json::Value) -> Option<String> {
    let check_reason = report
        .get("checks")
        .and_then(|checks| checks.as_object())
        .and_then(|checks| {
            checks.values().find(|check| {
                check.get("status").and_then(|value| value.as_str()) == Some("blocked")
            })
        })
        .and_then(|check| check.get("details").and_then(|value| value.as_str()))
        .map(str::trim)
        .filter(|details| !details.is_empty())
        .map(String::from);

    check_reason.or_else(|| {
        report
            .get("error")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|error| !error.is_empty())
            .map(String::from)
    })
}

/// 识别真实模型请求返回的不可重试鉴权/授权错误。底座会把 HTTP 401/403 格式化为
/// `Authentication failed` / `Authorization failed`；兼容旧错误文本中的显式状态码。
/// 只在 `AgentComplete.failed=true` 后调用，不把普通角色文本中的数字误判为故障。
pub(crate) fn model_auth_failure_reason(error: &str) -> Option<String> {
    let lower = error.to_ascii_lowercase();
    let is_auth_failure = [
        "authentication failed",
        "authorization failed",
        "http 401",
        "http 403",
        "401 unauthorized",
        "403 forbidden",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    if !is_auth_failure {
        return None;
    }

    error
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| truncate_on_char_boundary(line, 400).to_string())
}

/// 把 SubAgent 的失败信封整理成可持久化、可展示的诊断文本。
/// 底座会在正文后附 `<codewhale:...>` 完成哨兵；它只用于协议同步，不应污染日志。
fn failure_detail(error: &str) -> String {
    let detail = error
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("<codewhale:"))
        .take(24)
        .collect::<Vec<_>>()
        .join("\n");
    let detail = if detail.is_empty() {
        "SubAgent 未返回具体错误信息".to_string()
    } else {
        detail
    };
    truncate_on_char_boundary(&detail, 4000).to_string()
}

fn failure_summary(detail: &str) -> String {
    let summary = detail.lines().take(6).collect::<Vec<_>>().join(" | ");
    truncate_on_char_boundary(&summary, 800).to_string()
}

fn failure_category(detail: &str) -> &'static str {
    let lower = detail.to_ascii_lowercase();
    if model_auth_failure_reason(detail).is_some() {
        "model_auth"
    } else if ["timed out", "timeout", "deadline exceeded"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        "timeout"
    } else if ["http 429", "rate limit", "too many requests"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        "rate_limit"
    } else if [
        "not permitted",
        "permission denied",
        "access denied",
        "read-only",
        "readonly",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        "permission"
    } else if ["tool", "mcp", "command failed", "exit code"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        "tool"
    } else if [
        "connection",
        "dns",
        "request failed",
        "sse stream",
        "http 500",
        "http 502",
        "http 503",
        "http 504",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        "network"
    } else if lower.contains("model") {
        "model"
    } else {
        "unknown"
    }
}

fn workflow_failure_reason_for_project(project: &Path) -> Option<String> {
    let content =
        std::fs::read_to_string(project.join("_state").join("workflow_progress.json")).ok()?;
    let state: serde_json::Value = serde_json::from_str(&content).ok()?;
    let roles = state.get("roles")?.as_object()?;
    roles.iter().find_map(|(role_id, value)| {
        let status = value.get("status").and_then(serde_json::Value::as_str)?;
        if !matches!(status, "failed" | "blocked") {
            return None;
        }
        let error = value
            .get("error")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|message| !message.is_empty());
        Some(match error {
            Some(message) => format!("{role_id}: {message}"),
            None => format!("{role_id} 执行失败"),
        })
    })
}

/// 公开入口版本(预留 API,待 command 层接入);当前生产路径直接调
/// workflow_failure_reason_for_project。保留 pub(crate) 签名以便未来暴露。
#[allow(dead_code)]
pub(crate) fn workflow_failure_reason(workspace: &Path) -> Option<String> {
    workflow_failure_reason_for_project(&find_project_dir(workspace)?)
}

// [tool 化 2026-06-06] run_ghost_deck_step 已删——框架实例化(含 base.css)是 designer
// SubAgent 的业务,经 compose_deck tool 完成(SDAN/02 Router 四不:Router 不跑业务脚本)。
// generate_ghost_deck.py 仍是排版核心库,但只由 compose_deck.py 内部 import,不再被 harness 直调。

fn run_gate_runner(project: &Path) -> Result<GateResult, String> {
    let script = gate_runner_path_for(&workflow_of_project(project));
    if !script.exists() {
        return Ok(GateResult {
            verdict: "PASS".into(),
            findings: vec![],
        });
    }
    let deck_dir = project.join("HTML_Deck");
    if !deck_dir.exists() {
        return Ok(GateResult {
            verdict: "PASS".into(),
            findings: vec![],
        });
    }
    let scripts_dir = script.parent().unwrap_or(project);
    let args = [script.to_string_lossy().to_string(),
        deck_dir.to_string_lossy().to_string(),
        "--layer".to_string(),
        "1".to_string()];
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let out = run_python(&arg_refs, scripts_dir)?;
    serde_json::from_str(&out).map_err(|e| format!("parse gate_runner: {e}"))
}

fn run_deliverable_check(project: &Path, role_id: &str) -> Result<GateResult, String> {
    let script = deliverable_validator_path_for(&workflow_of_project(project));
    if !script.exists() {
        return Ok(GateResult {
            verdict: "PASS".into(),
            findings: vec![],
        });
    }
    let scripts_dir = script.parent().unwrap_or(project);
    let args = [script.to_string_lossy().to_string(),
        project.to_string_lossy().to_string(),
        role_id.to_string()];
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let out = run_python(&arg_refs, scripts_dir)?;
    let v: serde_json::Value =
        serde_json::from_str(&out).map_err(|e| format!("parse deliverable check: {e}"))?;

    let verdict = v
        .get("verdict")
        .and_then(|v| v.as_str())
        .unwrap_or("PASS")
        .to_string();
    let findings: Vec<GateFinding> = v
        .get("findings")
        .and_then(|f| f.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    Some(GateFinding {
                        severity: item.get("severity")?.as_str()?.to_string(),
                        rule: item
                            .get("rule")
                            .and_then(|r| r.as_str())
                            .unwrap_or("")
                            .to_string(),
                        rollback_scope: item
                            .get("rollback_scope")
                            .and_then(|r| r.as_str())
                            .unwrap_or("local")
                            .to_string(),
                        fix_hint: item
                            .get("fix_hint")
                            .and_then(|r| r.as_str())
                            .unwrap_or("")
                            .to_string(),
                        message: item
                            .get("message")
                            .and_then(|r| r.as_str())
                            .unwrap_or("")
                            .to_string(),
                        violation_type: item
                            .get("violation_type")
                            .and_then(|r| r.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // 如果是 slide_writer 或 qa_inspector，追加 gate_runner HTML 全套检查
    if (role_id == "slide_writer" || role_id == "qa_inspector") && verdict != "FAIL" {
        let html_result = run_gate_runner(project);
        if let Ok(html_gate) = html_result {
            if html_gate.verdict == "FAIL" {
                let mut combined_findings = findings;
                combined_findings.extend(html_gate.findings);
                return Ok(GateResult {
                    verdict: "FAIL".into(),
                    findings: combined_findings,
                });
            }
        }
    }

    Ok(GateResult { verdict, findings })
}

fn parse_decision(json: &str) -> Result<SchedulerDecision, String> {
    serde_json::from_str(json).map_err(|e| {
        format!(
            "parse scheduler: {e}\nraw: {}",
            truncate_on_char_boundary(json, 200)
        )
    })
}

// ── 核心逻辑 ──

/// turn 完成后，没有正在执行的角色 → 推进到下一个
pub fn step_fresh(workspace: &Path) -> HarnessAction {
    let project = match find_project_dir(workspace) {
        Some(p) => p,
        None => return HarnessAction::NotApplicable,
    };
    if stop_marker_path(&project).is_file() {
        return HarnessAction::Blocked {
            message: "工作流已由用户停止，请修改需求后新建任务".to_string(),
        };
    }
    // warm-up gating: 判 status == "pass" 而非文件存在
    // 防止 blocked 报告也写盘导致下次启动跳过检查
    //
    // 兜底开关 PINVOU3_SKIP_WARMUP=1: 彻底跳过本地 warmup gating，直接进入
    // scheduler --next dispatch。正常场景保留依赖、脚本和项目目录等本地检查。
    let skip_warmup = std::env::var("PINVOU3_SKIP_WARMUP")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let needs_warmup = if skip_warmup {
        false
    } else {
        match read_warmup_report(&project) {
            Ok(report) => report.get("status").and_then(|v| v.as_str()) != Some("pass"),
            Err(_) => true, // 文件不存在 / parse 失败 → 重跑
        }
    };
    if needs_warmup {
        match run_warmup(&project) {
            Ok(report) => {
                if report.get("status").and_then(|v| v.as_str()) != Some("pass") {
                    return HarnessAction::Blocked {
                        message: report.to_string(),
                    };
                }
            }
            Err(report_or_error) => {
                return HarnessAction::Blocked {
                    message: report_or_error,
                }
            }
        }
    }

    let scenario = read_scenario(&project).unwrap_or_else(|| "solution_deck".to_string());

    let json = match run_scheduler(&project, &["--scenario", &scenario, "--next"]) {
        Ok(j) => j,
        Err(e) => return HarnessAction::Error(e),
    };
    let decision = match parse_decision(&json) {
        Ok(d) => d,
        Err(e) => return HarnessAction::Error(e),
    };

    dispatch_or_wait(decision, &project, &scenario)
}

/// turn 完成后，有正在执行的角色 → 判断它完成没，跑 gate，再推进
pub fn step_after_role(workspace: &Path, running_role: &str) -> HarnessAction {
    let project = match find_project_dir(workspace) {
        Some(p) => p,
        None => return HarnessAction::NotApplicable,
    };
    if stop_marker_path(&project).is_file() {
        return HarnessAction::NotApplicable;
    }
    let scenario = read_scenario(&project).unwrap_or_else(|| "solution_deck".to_string());
    let gate_type = resolve_gate_type(running_role, &project);

    match gate_type.as_str() {
        "human" => {
            // human gate → 先检查产出文件是否存在，有了才弹审批
            // 没有产出说明 LLM 还在执行，返回 NotApplicable 让它继续
            let check = run_deliverable_check(&project, running_role);
            let has_output = match &check {
                Ok(r) => r.verdict != "FAIL" || r.findings.iter().all(|f| f.severity != "CRITICAL"),
                Err(_) => false,
            };
            if has_output {
                let _ = run_scheduler(
                    &project,
                    &["--scenario", &scenario, "--gate-wait", running_role],
                );
                HarnessAction::WaitForHuman {
                    role_id: running_role.to_string(),
                    role_name: running_role.to_string(),
                    description: format!("{running_role} 执行完毕，等待你确认"),
                }
            } else {
                // 产出还没就绪，不打断 LLM
                HarnessAction::NotApplicable
            }
        }
        _ => {
            // auto gate → 先跑 validate_deliverable 检查交付物结构
            let gate_result = match run_deliverable_check(&project, running_role) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("[harness] deliverable check error: {e}, treating as PASS");
                    GateResult {
                        verdict: "PASS".into(),
                        findings: vec![],
                    }
                }
            };

            // L1 报告写盘（_state/gate_reports/<role>_<ts>.json）
            write_gate_report(&project, running_role, 1, &gate_result);

            if gate_result.verdict == "PASS" || gate_result.verdict == "WARN" {
                // [拆对话线 C] L1 hard 通过即推进。不再注入品悟 L2 对话型评审
                // (SDAN/05:soft 不阻断、不打回;唯一硬门 = hard 代码 + 用户)。
                // 内容质量待 soft 裁决模块(出建议卡片、不阻断)接管,见 docs/SDAN/05。
                log_flow(
                    &project,
                    "gate_pass",
                    &[("role_id", running_role), ("verdict", &gate_result.verdict)],
                );
                // [tool 化 2026-06-06] 框架实例化(含 base.css 挂载)已迁到 designer 的
                // compose_deck tool(SDAN/02 Router 四不:Router 不跑业务脚本)。harness 不再
                // 在 content_planner 后外部排版——框架由 designer SubAgent 经 compose_deck 出。
                let _ = run_scheduler(
                    &project,
                    &["--scenario", &scenario, "--complete", running_role],
                );
                // [B2] 尚书省派完单 → 编译差事图(写 dynamic_routes.json),下一次 --next 才能
                // 看到差事节点(<bu>#<seq>)。编译失败=派单不合法,记日志后降级(无 dynamic_routes
                // → WorkflowState 走静态六部兜底,不致命)。
                if running_role == "shangshu" {
                    match materialize_dispatch_graph(&project) {
                        Ok(_) => log_flow(&project, "dispatch_graph_compiled", &[]),
                        Err(e) => {
                            eprintln!("[harness] 差事图编译失败(降级静态六部): {e}");
                            log_flow(&project, "dispatch_graph_failed", &[("error", &e)]);
                        }
                    }
                }
                let json = match run_scheduler(&project, &["--scenario", &scenario, "--next"]) {
                    Ok(j) => j,
                    Err(e) => return HarnessAction::Error(e),
                };
                let decision = match parse_decision(&json) {
                    Ok(d) => d,
                    Err(e) => return HarnessAction::Error(e),
                };
                dispatch_or_wait(decision, &project, &scenario)
            } else {
                // FAIL → 分析 rollback_scope。
                // [白浪决策 2026-06-03] 只有【显式分类】(violation_type 非空,∈ rollback_dispatch
                // 键)的 structural finding 才真回滚到上游节点；【未分类】的 structural finding
                // (如 pagenum_mismatch/font_size_off 这类 slide 级瑕疵,taxonomy 没覆盖)默认
                // 走下方 local 分支 = 重派【该节点本身】(per_page 整批重派带 fix hints),
                // 绝不兜底成 density_violation 去回滚 content_planner 重做策划+改 outline。
                let structural = gate_result
                    .findings
                    .iter()
                    .any(|f| f.rollback_scope == "structural" && !f.violation_type.is_empty());

                if structural {
                    let rule_id = find_rollback_rule(&gate_result.findings);
                    log_flow(
                        &project,
                        "rollback",
                        &[("role_id", running_role), ("rule_id", &rule_id)],
                    );
                    let _ = run_scheduler(
                        &project,
                        &[
                            "--scenario",
                            &scenario,
                            "--fail",
                            running_role,
                            "--reason",
                            "gate structural fail",
                        ],
                    );
                    let _ =
                        run_scheduler(&project, &["--scenario", &scenario, "--rollback", &rule_id]);

                    let json = match run_scheduler(&project, &["--scenario", &scenario, "--next"]) {
                        Ok(j) => j,
                        Err(e) => return HarnessAction::Error(e),
                    };
                    let decision = match parse_decision(&json) {
                        Ok(d) => d,
                        Err(e) => return HarnessAction::Error(e),
                    };
                    dispatch_or_wait(decision, &project, &scenario)
                } else {
                    // local → 先 fail（累加 retries），再看还能不能重试
                    let _ = run_scheduler(
                        &project,
                        &[
                            "--scenario",
                            &scenario,
                            "--fail",
                            running_role,
                            "--reason",
                            "gate local fail",
                        ],
                    );

                    // 检查 scheduler 状态：如果 retries >= max 则 status=failed，返回 Blocked
                    let status_json =
                        run_scheduler(&project, &["--scenario", &scenario, "--status"])
                            .unwrap_or_default();
                    let is_failed = status_json.contains(&format!("\"{}\"", "failed"));

                    if is_failed {
                        HarnessAction::Blocked {
                            message: format!("{} 重试次数用尽", running_role),
                        }
                    } else {
                        let fix_hints: Vec<String> = gate_result
                            .findings
                            .iter()
                            .filter(|f| f.severity == "CRITICAL" || f.severity == "WARNING")
                            .map(|f| format!("- {}: {} → {}", f.rule, f.message, f.fix_hint))
                            .collect();

                        // 重新派发同角色 SubAgent，附 L1 修复指令（Step C，信任根失败路径）。
                        let addendum = format!(
                            "## 上一轮 L1 检查未通过，请修复：\n\n{}\n\n修复后会重新检查。",
                            fix_hints.join("\n")
                        );
                        // [per_page] 整批重派（每页带修复指令）；build_batch_action 自带 --start。
                        if role_is_per_page(running_role) {
                            match fetch_batch_tasks(&project, &scenario, running_role) {
                                Ok(tasks) => build_batch_action(
                                    &project,
                                    &scenario,
                                    running_role,
                                    running_role,
                                    &tasks,
                                    &addendum,
                                ),
                                Err(e) => HarnessAction::Error(e),
                            }
                        } else {
                            let _ = run_scheduler(
                                &project,
                                &["--scenario", &scenario, "--start", running_role],
                            );
                            spawn_agent_or_error(&project, &scenario, running_role, &addendum)
                        }
                    }
                }
            }
        }
    }
}

/// 用户审批通过——先验证 deliverable 再 complete
pub fn approve_gate(workspace: &Path, role_id: &str) -> HarnessAction {
    let project = match find_project_dir(workspace) {
        Some(p) => p,
        None => return HarnessAction::NotApplicable,
    };
    if stop_marker_path(&project).is_file() {
        return HarnessAction::Blocked {
            message: "工作流已停止，不能继续审批".to_string(),
        };
    }
    let scenario = read_scenario(&project).unwrap_or_else(|| "solution_deck".to_string());

    let check = run_deliverable_check(&project, role_id);
    let has_critical = match &check {
        Ok(r) => r.findings.iter().any(|f| f.severity == "CRITICAL"),
        Err(e) => {
            eprintln!("[harness] deliverable check error in approve_gate: {e}");
            false
        }
    };
    if has_critical {
        let findings = check.unwrap_or(GateResult {
            verdict: "FAIL".into(),
            findings: vec![],
        });
        let issues: Vec<String> = findings
            .findings
            .iter()
            .filter(|f| f.severity == "CRITICAL")
            .map(|f| format!("- {}: {}", f.rule, f.message))
            .collect();
        log_flow(
            &project,
            "gate_fail",
            &[
                ("role_id", role_id),
                ("reason", "deliverable check failed on approve"),
            ],
        );
        return HarnessAction::Blocked {
            message: format!(
                "{} 的交付物不完整，无法通过审批：\n{}",
                role_id,
                issues.join("\n")
            ),
        };
    }

    log_flow(&project, "human_approve", &[("role_id", role_id)]);
    let _ = run_scheduler(&project, &["--scenario", &scenario, "--complete", role_id]);
    step_fresh(workspace)
}

/// [2026-06-06] SubAgent 执行失败（run_subagent 返回 Err：0 步即死/工具不可用/超时）。
/// 绝不走 gate——否则 validate_deliverable 拿【上一轮的陈旧产物】(存在+非空)放行，
/// 失败被洗成 PASS（实锤：web_search 不可用 → PM 秒死 → 旧 brief 过关）。
/// 语义对齐 gate local fail：--fail 计次 → 耗尽则 Blocked，否则带失败原因重派同角色。
pub fn agent_failed(workspace: &Path, role_id: &str, error: &str) -> HarnessAction {
    agent_failed_impl(workspace, role_id, None, error)
}

fn agent_failed_impl(
    workspace: &Path,
    role_id: &str,
    agent_id: Option<&str>,
    error: &str,
) -> HarnessAction {
    let project = match find_project_dir(workspace) {
        Some(p) => p,
        None => return HarnessAction::NotApplicable,
    };
    if stop_marker_path(&project).is_file() {
        return HarnessAction::NotApplicable;
    }
    let scenario = read_scenario(&project).unwrap_or_else(|| "solution_deck".to_string());
    // 失败信封常把真正原因放在第二行以后；保留多行详情进日志，同时生成单行摘要供
    // scheduler 状态、阻塞卡片和重派提示使用。协议哨兵由 failure_detail 过滤。
    let detail = failure_detail(error);
    let reason = failure_summary(&detail);
    let category = failure_category(&detail);
    let mut failure_fields = vec![
        ("role_id", role_id),
        ("category", category),
        ("reason", reason.as_str()),
        ("detail", detail.as_str()),
    ];
    if let Some(agent_id) = agent_id {
        failure_fields.push(("agent_id", agent_id));
    }
    log_flow(&project, "agent_failed", &failure_fields);
    if let Some(auth_reason) = model_auth_failure_reason(&detail) {
        let fatal_reason = format!("模型服务鉴权失败: {auth_reason}");
        if let Err(scheduler_error) = run_scheduler(
            &project,
            &[
                "--scenario",
                &scenario,
                "--fail-fatal",
                role_id,
                "--reason",
                &fatal_reason,
            ],
        ) {
            return HarnessAction::Error(format!("记录模型鉴权失败终态时出错: {scheduler_error}"));
        }
        log_flow(
            &project,
            "agent_failure_terminal",
            &[
                ("role_id", role_id),
                ("category", category),
                ("reason", &fatal_reason),
                ("retryable", "false"),
            ],
        );
        return HarnessAction::Blocked {
            message: format!("模型服务鉴权失败，已停止自动重试：{auth_reason}"),
        };
    }
    if let Err(scheduler_error) = run_scheduler(
        &project,
        &[
            "--scenario",
            &scenario,
            "--fail",
            role_id,
            "--reason",
            &format!("subagent 执行失败: {reason}"),
        ],
    ) {
        log_flow(
            &project,
            "scheduler_failure",
            &[
                ("role_id", role_id),
                ("stage", "record_agent_failure"),
                ("reason", &scheduler_error),
                ("original_failure", &reason),
            ],
        );
        return HarnessAction::Error(format!(
            "记录 {role_id} 执行失败时调度器出错: {scheduler_error}; 原始失败: {reason}"
        ));
    }

    // retries 耗尽 → scheduler 置 failed → Blocked（与 gate local fail 对称）
    let status_json = match run_scheduler(&project, &["--scenario", &scenario, "--status"]) {
        Ok(status) => status,
        Err(scheduler_error) => {
            log_flow(
                &project,
                "scheduler_failure",
                &[
                    ("role_id", role_id),
                    ("stage", "read_failure_status"),
                    ("reason", &scheduler_error),
                    ("original_failure", &reason),
                ],
            );
            return HarnessAction::Error(format!(
                "读取 {role_id} 失败状态时调度器出错: {scheduler_error}; 原始失败: {reason}"
            ));
        }
    };
    let status = serde_json::from_str::<serde_json::Value>(&status_json).unwrap_or_default();
    let persisted_progress =
        std::fs::read_to_string(project.join("_state").join("workflow_progress.json"))
            .ok()
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
            .unwrap_or_default();
    // workflow_progress.json 是重试计数和终态的持久真相源；scheduler --status
    // 可能只返回展示快照而省略 retries，优先读取持久状态，避免日志错误显示 0/2。
    let role_status = persisted_progress
        .get("roles")
        .and_then(|roles| roles.get(role_id))
        .or_else(|| status.get("roles").and_then(|roles| roles.get(role_id)));
    let is_failed = role_status
        .and_then(|role| role.get("status"))
        .and_then(serde_json::Value::as_str)
        == Some("failed");
    let retries = role_status
        .and_then(|role| role.get("retries"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
        .to_string();
    let max_retries = status
        .get("roles")
        .and_then(|roles| roles.get(role_id))
        .and_then(|role| role.get("effective_config"))
        .and_then(|config| config.get("max_retries"))
        .and_then(serde_json::Value::as_u64)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    if is_failed {
        log_flow(
            &project,
            "agent_failure_terminal",
            &[
                ("role_id", role_id),
                ("category", category),
                ("reason", &reason),
                ("retryable", "false"),
                ("attempt", &retries),
                ("max_retries", &max_retries),
            ],
        );
        return HarnessAction::Blocked {
            message: format!("{role_id} SubAgent 执行失败且重试耗尽: {reason}"),
        };
    }

    log_flow(
        &project,
        "agent_retry_scheduled",
        &[
            ("role_id", role_id),
            ("category", category),
            ("reason", &reason),
            ("attempt", &retries),
            ("max_retries", &max_retries),
        ],
    );

    let addendum = format!(
        "## 上一轮执行失败（非产出质量问题）\n\n失败原因：{reason}\n\n请重新执行本角色任务。"
    );
    if role_is_per_page(role_id) {
        match fetch_batch_tasks(&project, &scenario, role_id) {
            Ok(tasks) => {
                build_batch_action(&project, &scenario, role_id, role_id, &tasks, &addendum)
            }
            Err(e) => HarnessAction::Error(e),
        }
    } else {
        if let Err(scheduler_error) =
            run_scheduler(&project, &["--scenario", &scenario, "--start", role_id])
        {
            log_flow(
                &project,
                "scheduler_failure",
                &[
                    ("role_id", role_id),
                    ("stage", "restart_agent"),
                    ("reason", &scheduler_error),
                    ("original_failure", &reason),
                ],
            );
            return HarnessAction::Error(format!(
                "重启 {role_id} 时调度器出错: {scheduler_error}; 原始失败: {reason}"
            ));
        }
        spawn_agent_or_error(&project, &scenario, role_id, &addendum)
    }
}

/// 用户审批拒绝 → 让角色重做
pub fn reject_gate(workspace: &Path, role_id: &str, reason: &str) -> HarnessAction {
    let project = match find_project_dir(workspace) {
        Some(p) => p,
        None => return HarnessAction::NotApplicable,
    };
    if stop_marker_path(&project).is_file() {
        return HarnessAction::Blocked {
            message: "工作流已停止，不能继续打回".to_string(),
        };
    }
    let scenario = read_scenario(&project).unwrap_or_else(|| "solution_deck".to_string());
    let _ = run_scheduler(
        &project,
        &[
            "--scenario",
            &scenario,
            "--fail",
            role_id,
            "--reason",
            reason,
        ],
    );

    // 用户拒绝 → 角色重新 running，派发同角色 SubAgent 附拒绝原因（Step C）。
    let _ = run_scheduler(&project, &["--scenario", &scenario, "--start", role_id]);
    let addendum =
        format!("## 用户拒绝上一轮产出\n\n原因：{reason}\n\n请重新执行，重点关注以上反馈。");
    spawn_agent_or_error(&project, &scenario, role_id, &addendum)
}

// ── 辅助 ──

/// [pinvou3-fork] 从 agent_registry.json 读角色的工具白名单 + max_steps。
/// registry 是工具的唯一真像（route_table 引用 registry，不重复定义 tools）。Custom subagent 要求非空 tools。
fn read_role_registry_tools(
    role_id: &str,
) -> (Vec<String>, Option<u32>, Option<serde_json::Value>, bool) {
    // [B2] 差事节点(<bu>#<seq>)的工具/能力按所属部查;非差事原样。
    let bu = bu_of(role_id);
    let role_id = bu.as_str();
    let reg_path = workflow_root_for(&workflow_of_role(role_id)).join("agent_registry.json");
    let content = match std::fs::read_to_string(&reg_path) {
        Ok(c) => c,
        Err(_) => return (Vec::new(), None, None, false),
    };
    let reg: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return (Vec::new(), None, None, false),
    };
    let Some(agent) = reg.get("agents").and_then(|a| a.get(role_id)) else {
        return (Vec::new(), None, None, false);
    };
    let tools = agent
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let max_steps = agent
        .get("max_steps")
        .and_then(|m| m.as_u64())
        .map(|n| n as u32);
    // [pinvou3-fork] output_schema:只有当它是标准 JSON Schema(有 type/properties)时才透传,
    // 触发 submit_output 强制提交。旧的"字段→描述"扁平 map / 自由文本角色(无 schema)→ None,走原逻辑。
    let output_schema = agent.get("output_schema").and_then(|s| {
        // [codex MINOR 修] 必须 type=="object" 且有 properties,才是 submit_output 能用的标准 schema。
        let is_object = s.get("type").and_then(|t| t.as_str()) == Some("object");
        if is_object && s.get("properties").is_some() {
            Some(s.clone())
        } else {
            None
        }
    });
    let has_outputs = agent
        .get("outputs")
        .and_then(|outputs| outputs.as_array())
        .map(|outputs| !outputs.is_empty())
        .unwrap_or(false);
    let expects_file_output = has_outputs && output_schema.is_none();
    (tools, max_steps, output_schema, expects_file_output)
}

/// Step C 的 SubAgent prompt：[PRIORITY OVERRIDE] + 文件铁律 + registry role prompt + 附加段。
/// [资源模块·§0 寻址原则] 构造 Task 信封的 `[STATIC]` 段:按角色 reads_static 从
/// static_assets.json 筛出它能读的静态资产,给出**地址**(绝对路径)+ inline:summary 的
/// 附一段摘要。语义=Router 报文带地址,SubAgent 凭地址 read_file(见 docs/SDAN/08a)。
/// 读不到 static_assets.json / 角色无 reads_static → 返回空串(不影响)。
///
/// [per_page] `page_layout=Some("L01")` 时把 reads 裁到【该页必需的最小集】:只留
/// 该页模板 `tpl_L01` + `base_css`(仅 @import 引用、明示勿读内容) + `image_slot_protocol`
/// (配图页才读)，砍掉其余 4 个模板 + layout_modes/content_redlines/design_tokens 等
/// 大参考。根因：单页 agent 若读全量 11 资产(63KB≈2万token)，step2 上下文爆 → 4 并发
/// 砸单 vLLM → TTFT 破 110s SSE 超时。裁到 ~7KB 后 step2 不再爆。
fn build_static_section(role_id: &str, page_layout: Option<&str>) -> String {
    let wf = workflow_root_for(&workflow_of_role(role_id));
    // 读 registry 的 reads_static(角色视角:我要读啥)
    let reg_path = wf.join("agent_registry.json");
    let reg: serde_json::Value = match std::fs::read_to_string(&reg_path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
    {
        Some(v) => v,
        None => return String::new(),
    };
    let mut reads: Vec<String> = reg
        .get("agents")
        .and_then(|a| a.get(role_id))
        .and_then(|r| r.get("reads_static"))
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    // [per_page] 按该页版式裁剪：只保留 base_css / image_slot_protocol / 该页 tpl_LNN。
    let no_read_keys: std::collections::HashSet<&str> = if let Some(layout) = page_layout {
        let want_tpl = format!("tpl_{layout}"); // "tpl_L01"
        reads.retain(|k| k == "base_css" || k == "image_slot_protocol" || k == &want_tpl);
        // base_css 只用于 @import 路径引用，明示别 read_file 它（避免 10KB 进上下文）。
        ["base_css"].into_iter().collect()
    } else {
        std::collections::HashSet::new()
    };
    if reads.is_empty() {
        return String::new();
    }
    // 读 static_assets.json 拿每个 key 的 path/inline
    let sa_path = wf.join("static_assets.json");
    let sa: serde_json::Value = match std::fs::read_to_string(&sa_path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
    {
        Some(v) => v,
        None => return String::new(),
    };
    let assets = match sa.get("assets").and_then(|a| a.as_object()) {
        Some(m) => m,
        None => return String::new(),
    };
    let mut lines: Vec<String> = Vec::new();
    for key in &reads {
        let Some(entry) = assets.get(key) else {
            continue;
        };
        let Some(rel) = entry.get("path").and_then(|p| p.as_str()) else {
            continue;
        };
        let abs = wf.join(rel);
        // [per_page] 标记为"勿读"的资产(如 base_css)：只给 @import 路径，明示别 read_file，
        // 防止大文件进 step2 上下文。
        if no_read_keys.contains(key.as_str()) {
            lines.push(format!(
                "- `{key}`(**只在 HTML 里 `@import` 引用此路径，切勿 read_file 读它的内容**): {}",
                abs.display()
            ));
            continue;
        }
        let inline = entry.get("inline").and_then(|i| i.as_str());
        // inline:"summary" 的小资产读首 1200 字摘要内联;false 的只给地址。
        if inline == Some("summary") {
            let summary = std::fs::read_to_string(&abs)
                .map(|c| c.chars().take(1200).collect::<String>())
                .unwrap_or_default();
            lines.push(format!(
                "- `{key}`(只读规范): {}\n  摘要(完整版 read_file 上面路径):\n  {}",
                abs.display(),
                summary.replace('\n', "\n  ")
            ));
        } else {
            lines.push(format!("- `{key}`(只读资源): {}", abs.display()));
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "\n\n## [STATIC] 可读静态资源(只读,用 read_file 读,别想着生成它们)\n\n{}\n",
        lines.join("\n")
    )
}

fn render_output_section(outputs: Vec<String>, project: &Path) -> String {
    if outputs.is_empty() {
        return String::new();
    }
    let mut lines: Vec<String> = Vec::new();
    for rel in outputs {
        if rel.contains('*') {
            let (dir, pattern) = match rel.rsplit_once('/') {
                Some((dir, pattern)) => (dir, pattern),
                None => ("", rel.as_str()),
            };
            let dir_display = if dir.is_empty() {
                project.display().to_string()
            } else {
                project.join(dir).display().to_string()
            };
            lines.push(format!("- 目录 {dir_display},文件名按 {pattern}(可多个)"));
        } else {
            lines.push(format!("- {}", project.join(rel).display()));
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "\n\n## [产物地址] 你必须用 write_file 把产物写到以下绝对路径(只写这里,别写别处;不写文件 = 任务未完成)\n\n{}\n",
        lines.join("\n")
    )
}

fn build_output_section_from_agent(agent: &serde_json::Value, project: &Path) -> String {
    let is_submit_type = agent
        .get("output_schema")
        .and_then(|schema| schema.as_object())
        .and_then(|schema| schema.get("x-output-file"))
        .and_then(|output_file| output_file.as_str())
        .map(|output_file| !output_file.is_empty())
        .unwrap_or(false);
    if is_submit_type {
        return String::new();
    }
    let outputs: Vec<String> = agent
        .get("outputs")
        .and_then(|outputs| outputs.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    render_output_section(outputs, project)
}

fn build_output_section(role_id: &str, project: &std::path::Path) -> String {
    // [B2] 差事节点(<bu>~<seq>):产物固定 deliverables/<bu>_<seq>.md,由 id 直接派生。
    if let Some((bu, seq)) = role_id.split_once('~') {
        return render_output_section(vec![format!("deliverables/{bu}_{seq}.md")], project);
    }
    let wf = workflow_root_for(&workflow_of_project(project));
    let reg_path = wf.join("agent_registry.json");
    let reg: serde_json::Value = match std::fs::read_to_string(&reg_path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
    {
        Some(v) => v,
        None => return String::new(),
    };
    let Some(agent) = reg.get("agents").and_then(|a| a.get(role_id)) else {
        return String::new();
    };
    build_output_section_from_agent(agent, project)
}

/// 附加段 = 品悟交代句（首次 dispatch）或修复指令（gate/review/用户拒绝重做）。
fn build_spawn_prompt(
    project: &Path,
    scenario: &str,
    role_id: &str,
    addendum: &str,
    page_layout: Option<&str>,
) -> Result<String, String> {
    let raw_prompt = run_scheduler(project, &["--scenario", scenario, "--prompt", role_id])?;
    let static_section = build_static_section(role_id, page_layout);
    let output_section = build_output_section(role_id, project);
    let addendum_block = if addendum.trim().is_empty() {
        String::new()
    } else {
        format!("\n\n## 任务交代 / 上下文\n\n{addendum}")
    };
    Ok(format!(
        "你是工作流中的「{role_id}」角色，只执行下面的角色任务，完成产出后即停止、不自行进入下一步；若缺少必要信息，用 request_user_input 询问用户。\
         {static_section}{output_section}\n\n\
         ---\n\n\
         {raw_prompt}{addendum_block}",
    ))
}

/// [per_page] 记一页 SubAgent 完成（scheduler --page-done，join 计数在 State 模块）。
/// 返回 `true` = N 实例全到、可对单一逻辑节点验收。事件循环串行调用、无竞争。
/// 解析失败兜底 `true`（宁可早收尾也不卡死整批）。
pub fn record_page_done(workspace: &Path, base_role: &str, page: u32) -> bool {
    let project = match find_project_dir(workspace) {
        Some(p) => p,
        None => return true,
    };
    let scenario = read_scenario(&project).unwrap_or_else(|| "solution_deck".to_string());
    let json = run_scheduler(
        &project,
        &[
            "--scenario",
            &scenario,
            "--page-done",
            base_role,
            &page.to_string(),
        ],
    )
    .unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(&json)
        .ok()
        .and_then(|v| v.get("complete").and_then(|c| c.as_bool()))
        .unwrap_or(true)
}

// ── [per_page] 有界并发派发队列 ──────────────────────────────────────────────
//
// 为什么需要：底座 SubAgentManager 在 running_count >= max_agents 时【硬拒绝】新
// SpawnSubAgent(不排队，见 subagent/mod.rs:1366)。一个 N 页 fan-out 若一次性把 N 个
// SubAgent 全 send 出去、N>K，多出的页会被底座拒绝、永不执行 → batch join 永远凑不齐
// → 节点永不 completed。物理约束：本地单 vLLM，并发 prefill 越多 TTFT 越高(实测 10 并发
// 多数 step2 撞 110s SSE 超时)。所以【Router 运行时必须自己有界并发 + 排队】：先派 K 个，
// 每有一页完成再补派下一页，使在飞数稳定 = K，其余排队，全部最终执行。
//
// 单进程桌面应用 → 用进程级全局队列(按 session 键)。cli_server(初派)与 engine 事件循环
// (补派)同进程，共享此全局，无需把 Arc 穿过 bridge clone(bridge 无 Arc 字段、clone 不共享)。
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

#[allow(clippy::type_complexity)]
static PER_PAGE_QUEUE: OnceLock<Mutex<HashMap<String, VecDeque<SubAgentTask>>>> = OnceLock::new();

fn per_page_queue() -> &'static Mutex<HashMap<String, VecDeque<SubAgentTask>>> {
    PER_PAGE_QUEUE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// per_page 节点的在飞并发上限 K(其余页排队)。本地单 vLLM 友好默认 4；
/// `PINVOU3_PER_PAGE_CONCURRENCY` 可不重编调。clamp 到 [1, 12]。
pub fn per_page_concurrency() -> usize {
    std::env::var("PINVOU3_PER_PAGE_CONCURRENCY")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(4)
        .clamp(1, 12)
}

// ── [per_page] fan-out 可视化状态 ────────────────────────────────────────────
//
// 每页一个状态 queued|running|done|retrying，供前端工作流界面把 per_page 节点展开成 N 个
// SubAgent chip 实时显示并发。engine/commands 在每个转移点更新后 emit `workflow:fanout`。
#[allow(clippy::type_complexity)]
static PER_PAGE_FANOUT: OnceLock<Mutex<HashMap<String, std::collections::BTreeMap<u32, String>>>> =
    OnceLock::new();

fn per_page_fanout() -> &'static Mutex<HashMap<String, std::collections::BTreeMap<u32, String>>> {
    PER_PAGE_FANOUT.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fanout_lock(
) -> std::sync::MutexGuard<'static, HashMap<String, std::collections::BTreeMap<u32, String>>> {
    match per_page_fanout().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// [Codex 评审修复 2026-06-04] 批次状态键 = session+角色。旧实现只用 session，
/// 两个 per_page 角色（slide_writer 与 illustrator）同 session 会互踩队列/重试/fanout。
fn pk(session_id: &str, base_role: &str) -> String {
    format!("{session_id}|{base_role}")
}

/// 设某页 fan-out 状态(queued/running/done/retrying)。
pub fn fanout_mark(session_id: &str, base_role: &str, page: u32, status: &str) {
    let mut m = fanout_lock();
    m.entry(pk(session_id, base_role))
        .or_default()
        .insert(page, status.to_string());
}

fn fanout_clear(session_id: &str, base_role: &str) {
    fanout_lock().remove(&pk(session_id, base_role));
}

/// 快照为 `[{page,status}]`(按页号升序),供 emit。无状态 → 空数组。
pub fn fanout_snapshot(session_id: &str, base_role: &str) -> serde_json::Value {
    let m = fanout_lock();
    let arr: Vec<serde_json::Value> = m
        .get(&pk(session_id, base_role))
        .map(|pages| {
            pages
                .iter()
                .map(|(p, s)| serde_json::json!({ "page": p, "status": s }))
                .collect()
        })
        .unwrap_or_default();
    serde_json::Value::Array(arr)
}

/// [per_page·多产物] 批内每页产物登记表（realness 校验 image_file 型时按此查找）。
#[allow(clippy::type_complexity)]
static BATCH_OUTPUTS: OnceLock<Mutex<HashMap<String, HashMap<u32, Vec<String>>>>> = OnceLock::new();

fn batch_outputs() -> &'static Mutex<HashMap<String, HashMap<u32, Vec<String>>>> {
    BATCH_OUTPUTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn batch_outputs_record(session_id: &str, base_role: &str, page: u32, outs: &[String]) {
    if outs.is_empty() {
        return;
    }
    if let Ok(mut m) = batch_outputs().lock() {
        m.entry(pk(session_id, base_role))
            .or_default()
            .insert(page, outs.to_vec());
    }
}

pub fn batch_outputs_for(session_id: &str, base_role: &str, page: u32) -> Vec<String> {
    batch_outputs()
        .lock()
        .ok()
        .and_then(|m| {
            m.get(&pk(session_id, base_role))
                .and_then(|p| p.get(&page).cloned())
        })
        .unwrap_or_default()
}

/// 初派：把整批 `tasks` 灌入该 (session,角色) 的待派队列，返回**前 K 个**立即派发；
/// 余下 N-K 个留队，由 [`batch_pop_next`] 在每页完成时逐个补派。覆盖式 seed(重派整批
/// 时先清旧残留)。
pub fn batch_seed_and_take(
    session_id: &str,
    base_role: &str,
    tasks: Vec<SubAgentTask>,
    k: usize,
) -> Vec<SubAgentTask> {
    for t in &tasks {
        batch_outputs_record(session_id, base_role, t.page, &t.outputs);
    }
    let mut q = match per_page_queue().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let mut pending: VecDeque<SubAgentTask> = tasks.into();
    let take = k.min(pending.len());
    let first: Vec<SubAgentTask> = pending.drain(..take).collect();
    // fan-out 可视化初始化：前 K 个 running，其余 queued。
    for t in &first {
        fanout_mark(session_id, base_role, t.page, "running");
    }
    for t in &pending {
        fanout_mark(session_id, base_role, t.page, "queued");
    }
    q.insert(pk(session_id, base_role), pending);
    drop(q);
    page_retry_clear(session_id, base_role); // 新批次：清掉上一批的单页重试计数
    first
}

/// 补派：弹出该 (session,角色) 队列里的下一个待派页(无则 None)。每页完成调一次以维持在飞=K。
pub fn batch_pop_next(session_id: &str, base_role: &str) -> Option<SubAgentTask> {
    let mut q = match per_page_queue().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let t = q
        .get_mut(&pk(session_id, base_role))
        .and_then(|d| d.pop_front());
    if let Some(ref task) = t {
        fanout_mark(session_id, base_role, task.page, "running"); // 排队页被补派 → running
    }
    t
}

/// 清空该 (session,角色) 的待派队列(批次收齐 / 取消时调,防残留污染下批)。
pub fn batch_clear(session_id: &str, base_role: &str) {
    if let Ok(mut q) = per_page_queue().lock() {
        q.remove(&pk(session_id, base_role));
    }
    if let Ok(mut m) = batch_outputs().lock() {
        m.remove(&pk(session_id, base_role));
    }
    page_retry_clear(session_id, base_role);
    fanout_clear(session_id, base_role);
}

/// Clear every in-memory per-page queue/ledger belonging to one workflow
/// session. Stopping a run must not leave queued pages that can be dispatched
/// by a late completion event.
pub fn batch_clear_session(session_id: &str) {
    let prefix = format!("{session_id}|");
    if let Ok(mut q) = per_page_queue().lock() {
        q.retain(|key, _| !key.starts_with(&prefix));
    }
    if let Ok(mut m) = batch_outputs().lock() {
        m.retain(|key, _| !key.starts_with(&prefix));
    }
    if let Ok(mut m) = per_page_retry().lock() {
        m.retain(|key, _| !key.starts_with(&prefix));
    }
    fanout_lock().retain(|key, _| !key.starts_with(&prefix));
}

// ── [per_page] 单页产出校验 + 超时自动重试 ──────────────────────────────────
//
// 为什么需要：SubAgent SSE 超时/放弃时仍 emit AgentComplete，若无条件 record_page_done，
// 空壳页(ghost)会蒙混进 batch → gate 见到缺页/页号不符 → pagenum_mismatch → 误判回滚 →
// 重做策划+清真页的死循环。正解：每页完成先校验【真写成】(文件存在+够大+含正文标记)，
// 空壳页不计 done、自动重派该页(带重试上限)，batch 只在 N 页全真时 complete。

#[allow(clippy::type_complexity)]
static PER_PAGE_RETRY: OnceLock<Mutex<HashMap<String, HashMap<u32, u32>>>> = OnceLock::new();

fn per_page_retry() -> &'static Mutex<HashMap<String, HashMap<u32, u32>>> {
    PER_PAGE_RETRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 单页空壳重试上限。`PINVOU3_PER_PAGE_MAX_RETRY` 可调，默认 2，clamp [0,5]。
pub fn max_page_retry() -> u32 {
    std::env::var("PINVOU3_PER_PAGE_MAX_RETRY")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(2)
        .min(5)
}

/// 记一次某页重试，返回累计重试次数(从 1 起)。
pub fn page_retry_inc(session_id: &str, base_role: &str, page: u32) -> u32 {
    let mut m = match per_page_retry().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let e = m
        .entry(pk(session_id, base_role))
        .or_default()
        .entry(page)
        .or_insert(0);
    *e += 1;
    *e
}

fn page_retry_clear(session_id: &str, base_role: &str) {
    if let Ok(mut m) = per_page_retry().lock() {
        m.remove(&pk(session_id, base_role));
    }
}

/// per_page 节点该页产出文件的相对路径(读 registry dispatch.output_template,默认
/// `HTML_Deck/slides/p{page:02d}.html`)。
fn page_output_rel(base_role: &str, page: u32) -> String {
    let tpl = read_registry_for(&workflow_of_role(base_role))
        .get("agents")
        .and_then(|a| a.get(base_role))
        .and_then(|r| r.get("dispatch"))
        .and_then(|d| d.get("output_template"))
        .and_then(|t| t.as_str())
        .unwrap_or("HTML_Deck/slides/p{page:02d}.html")
        .to_string();
    // 支持 {page:02d}(补零两位)与 {page}(原值)两种占位。
    tpl.replace("{page:02d}", &format!("{page:02}"))
        .replace("{page}", &page.to_string())
}

/// 该实例产物是否【真写成】。规则由 registry `dispatch.realness` 声明（真相源），
/// runtime 只消费、不写角色知识 [Codex 评审 2026-06-04]：
/// - `html_page`(缺省)：output_template 反推路径；> min_bytes(900) 且含 contains_any 任一标记。
/// - `image_file`：按 `outputs`(批内登记的本页全部产物)逐一验 PNG 魔数 + ≥ min_bytes(100KB)，
///   全过才算真（挡 LLM 手搓伪图）；登记缺失视为未写成。
pub fn page_output_is_real(
    workspace: &Path,
    base_role: &str,
    page: u32,
    outputs: &[String],
) -> bool {
    let project = match find_project_dir(workspace) {
        Some(p) => p,
        None => return false,
    };
    let reg = read_registry_for(&workflow_of_project(&project));
    let realness = reg
        .get("agents")
        .and_then(|a| a.get(base_role))
        .and_then(|r| r.get("dispatch"))
        .and_then(|d| d.get("realness"));
    let rtype = realness
        .and_then(|r| r.get("type"))
        .and_then(|t| t.as_str())
        .unwrap_or("html_page");

    if rtype == "image_file" {
        let min_bytes = realness
            .and_then(|r| r.get("min_bytes"))
            .and_then(|v| v.as_u64())
            .unwrap_or(100_000);
        if outputs.is_empty() {
            return false;
        }
        const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        return outputs.iter().all(|o| {
            std::fs::read(o)
                .map(|b| b.len() as u64 >= min_bytes && b.get(..8) == Some(&PNG_MAGIC))
                .unwrap_or(false)
        });
    }

    // html_page（缺省）
    let min_bytes = realness
        .and_then(|r| r.get("min_bytes"))
        .and_then(|v| v.as_u64())
        .unwrap_or(900) as usize;
    let markers: Vec<String> = realness
        .and_then(|r| r.get("contains_any"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_lowercase()))
                .collect()
        })
        .unwrap_or_else(|| vec!["<h1".into(), "data-page-no".into()]);
    let path = project.join(page_output_rel(base_role, page));
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return false,
    };
    if content.len() <= min_bytes {
        return false;
    }
    let lc = content.to_ascii_lowercase();
    markers.iter().any(|m| lc.contains(m))
}

/// 重派单页：从 scheduler --batch-tasks 取该页任务，拼好 SubAgentTask(带"上次未写成、
/// 请尽快直接产出"提示)。找不到该页 / 拼 prompt 失败 → None。
pub fn respawn_page(workspace: &Path, base_role: &str, page: u32) -> Option<SubAgentTask> {
    let project = find_project_dir(workspace)?;
    let scenario = read_scenario(&project).unwrap_or_else(|| "solution_deck".to_string());
    let sched_tasks = fetch_batch_tasks(&project, &scenario, base_role).ok()?;
    let t = sched_tasks.into_iter().find(|t| t.page == page)?;
    let (allowed_tools, max_steps, output_schema, expects_file_output) =
        read_role_registry_tools(base_role);
    if allowed_tools.is_empty() {
        return None;
    }
    let layout_opt = if t.layout.trim().is_empty() {
        None
    } else {
        Some(t.layout.as_str())
    };
    // 重试提示(生图角色的专型提示随 legacy-ppt-workflow 工具 2026-06-11 存档下线,只剩通用版)
    let retry_note =
        "\n\n## ⚠️重试提醒\n上一次本页未写成(很可能读文件太多或超时)。请【直接】把产物写出来,尽快调 write_file,**不要反复 read_file**。";
    let addendum = format!("{}{}", t.addendum, retry_note);
    let prompt = build_spawn_prompt(&project, &scenario, base_role, &addendum, layout_opt).ok()?;
    Some(SubAgentTask {
        agent_role: t.task_id, // "slide_writer#p07"
        page: t.page,
        outputs: t.outputs,
        prompt,
        allowed_tools,
        max_steps,
        output_schema,
        expects_file_output,
    })
}

/// 从失败节点续跑:把 `role_id` 重置为 pending(清重试计数)后,重新走 step_fresh 调度。
/// SDAN 能力暴露——State 把节点标回 pending,next_actionable 自动重选它;上游已 completed
/// 的节点不动(天然不重跑)。返回 step_fresh 的决策(通常是 SpawnAgent 重新派该角色)。
pub fn retry_role(workspace: &Path, role_id: &str) -> HarnessAction {
    let project = match find_project_dir(workspace) {
        Some(p) => p,
        None => return HarnessAction::NotApplicable,
    };
    if stop_marker_path(&project).is_file() {
        return HarnessAction::Blocked {
            message: "工作流已停止，请修改需求后新建任务".to_string(),
        };
    }
    let scenario = read_scenario(&project).unwrap_or_else(|| "solution_deck".to_string());
    // 重置该角色为 pending + 清 retries(scheduler --reset)。失败不阻断,继续 step_fresh
    // (即便 reset 没生效,step_fresh 会照常按当前 State 决策,最坏是原样返回 blocked)。
    if let Err(e) = run_scheduler(&project, &["--scenario", &scenario, "--reset", role_id]) {
        return HarnessAction::Error(format!("reset {role_id} 失败: {e}"));
    }
    step_fresh(workspace)
}

/// 构造 [`HarnessAction::SpawnAgent`]（Step C）：读 registry 工具白名单 + max_steps，
/// 拼 prompt（registry role prompt + addendum）。registry 缺 tools → Error。
/// dispatch 首轮 / gate-L1 / review-L2 / 用户拒绝重做都走这里，DRY。
fn spawn_agent_or_error(
    project: &Path,
    scenario: &str,
    role_id: &str,
    addendum: &str,
) -> HarnessAction {
    let (allowed_tools, max_steps, output_schema, expects_file_output) =
        read_role_registry_tools(role_id);
    if allowed_tools.is_empty() {
        return HarnessAction::Error(format!(
            "agent_registry.json 缺角色 {role_id} 的 tools 白名单，无法 spawn"
        ));
    }
    match build_spawn_prompt(project, scenario, role_id, addendum, None) {
        Ok(prompt) => HarnessAction::SpawnAgent {
            role_id: role_id.to_string(),
            role_name: role_id.to_string(),
            prompt,
            allowed_tools,
            max_steps,
            output_schema,
            expects_file_output,
        },
        Err(e) => HarnessAction::Error(e),
    }
}

/// [per_page] registry.agents.<role>.dispatch.mode == "per_page"。
fn role_is_per_page(role_id: &str) -> bool {
    read_registry_for(&workflow_of_role(role_id))
        .get("agents")
        .and_then(|a| a.get(role_id))
        .and_then(|r| r.get("dispatch"))
        .and_then(|d| d.get("mode"))
        .and_then(|m| m.as_str())
        == Some("per_page")
}

/// [per_page] 取该角色的 per-page 子任务（scheduler --batch-tasks，不改状态）。
fn fetch_batch_tasks(
    project: &Path,
    scenario: &str,
    role_id: &str,
) -> Result<Vec<SchedulerTask>, String> {
    let json = run_scheduler(project, &["--scenario", scenario, "--batch-tasks", role_id])?;
    let v: serde_json::Value = serde_json::from_str(&json)
        .map_err(|e| format!("--batch-tasks {role_id} 解析失败: {e}"))?;
    let tasks = v.get("tasks").cloned().unwrap_or(serde_json::Value::Null);
    serde_json::from_value(tasks)
        .map_err(|e| format!("--batch-tasks {role_id} tasks 解析失败: {e}"))
}

/// [per_page] 把 N 个 scheduler 子任务拼成 `SpawnAgentBatch`（含【一次】`--start` 设 batch.total）。
/// `extra_addendum`：本轮额外交代（如 L1 修复指令），拼在每页 addendum 之后。
fn build_batch_action(
    project: &Path,
    scenario: &str,
    base_role: &str,
    role_name: &str,
    sched_tasks: &[SchedulerTask],
    extra_addendum: &str,
) -> HarnessAction {
    if sched_tasks.is_empty() {
        return HarnessAction::Error(format!(
            "per_page {base_role} 无子任务（outline/page_layout 未就绪？）"
        ));
    }
    let (allowed_tools, max_steps, output_schema, expects_file_output) =
        read_role_registry_tools(base_role);
    if allowed_tools.is_empty() {
        return HarnessAction::Error(format!(
            "agent_registry.json 缺角色 {base_role} 的 tools 白名单"
        ));
    }
    log_flow(
        project,
        "dispatch_batch",
        &[
            ("role_id", base_role),
            ("pages", &sched_tasks.len().to_string()),
        ],
    );
    // 逻辑节点【一次】标记开始（DAG 里仍是单节点）。--batch-total 显式传本批任务数
    // （幂等跳过后的真实 N，≠ outline 页数）供 join 计数 [Codex 评审修复 2026-06-04]。
    let n = sched_tasks.len().to_string();
    let _ = run_scheduler(
        project,
        &[
            "--scenario",
            scenario,
            "--start",
            base_role,
            "--batch-total",
            &n,
        ],
    );

    let mut tasks = Vec::with_capacity(sched_tasks.len());
    for t in sched_tasks {
        let addendum = if extra_addendum.trim().is_empty() {
            t.addendum.clone()
        } else {
            format!("{}\n\n{}", t.addendum, extra_addendum)
        };
        let layout_opt = if t.layout.trim().is_empty() {
            None
        } else {
            Some(t.layout.as_str())
        };
        let prompt = match build_spawn_prompt(project, scenario, base_role, &addendum, layout_opt) {
            Ok(p) => p,
            Err(e) => return HarnessAction::Error(e),
        };
        tasks.push(SubAgentTask {
            agent_role: t.task_id.clone(), // "slide_writer#p01"
            page: t.page,
            outputs: t.outputs.clone(),
            prompt,
            allowed_tools: allowed_tools.clone(),
            max_steps,
            output_schema: output_schema.clone(),
            expects_file_output,
        });
    }
    HarnessAction::SpawnAgentBatch {
        base_role: base_role.to_string(),
        role_name: role_name.to_string(),
        tasks,
    }
}

/// 读 brief.json 的 `user_request_raw`（init 时从 brief_init 合并写入），
/// 供首个角色（无上游、跳过品悟交代）直接喂给 SubAgent。缺则返回空串。
fn read_user_request(project: &Path) -> String {
    std::fs::read_to_string(project.join("_state").join("brief.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v.get("user_request_raw")
                .and_then(|u| u.as_str())
                .map(String::from)
        })
        .unwrap_or_default()
}

fn dispatch_or_wait(decision: SchedulerDecision, project: &Path, scenario: &str) -> HarnessAction {
    match decision.action.as_str() {
        "all_done" => HarnessAction::AllDone,

        "dispatch" => {
            let role_id = match &decision.role_id {
                Some(r) => r.clone(),
                None => return HarnessAction::Error("dispatch without role_id".into()),
            };
            let role_name = decision.role_name.unwrap_or_else(|| role_id.clone());

            log_flow(
                project,
                "dispatch",
                &[("role_id", &role_id), ("role_name", &role_name)],
            );
            let _ = run_scheduler(project, &["--scenario", scenario, "--start", &role_id]);

            // [拆对话线 C] 不再走 Step B 品悟口头交代（SDAN/09 取消对话型品悟）。
            // 所有角色直接派发真 SubAgent；上游交付物地址由 scheduler --prompt 的
            // 「你的输入文件」段注入（SDAN §0 寻址），SubAgent 凭地址 read_file，
            // 不依赖品悟口头转述。首个角色（无上游）附「用户原始请求」做上下文。
            let no_upstream = read_role_def(&role_id)
                .get("depends_on")
                .and_then(|v| v.as_array())
                .is_some_and(|a| a.is_empty());
            let addendum = if no_upstream {
                let user_req = read_user_request(project);
                if user_req.trim().is_empty() {
                    String::new()
                } else {
                    format!("用户原始请求：\n{user_req}")
                }
            } else {
                String::new()
            };
            spawn_agent_or_error(project, scenario, &role_id, &addendum)
        }

        // [per_page] 纵向 fan-out：把单一逻辑节点拆成 N 个 per-page SubAgent 并发派。
        "dispatch_batch" => {
            let base_role = match &decision.role_id {
                Some(r) => r.clone(),
                None => return HarnessAction::Error("dispatch_batch without role_id".into()),
            };
            let role_name = decision
                .role_name
                .clone()
                .unwrap_or_else(|| base_role.clone());
            let sched_tasks = decision.tasks.unwrap_or_default();
            build_batch_action(project, scenario, &base_role, &role_name, &sched_tasks, "")
        }

        "waiting_for_human" => {
            let roles = decision.waiting_roles.unwrap_or_default();
            let role_id = roles.first().cloned().unwrap_or_default();
            let msg = decision.message.unwrap_or_default();
            HarnessAction::WaitForHuman {
                role_id: role_id.clone(),
                role_name: role_id,
                description: msg,
            }
        }

        "role_running" => {
            // 角色还在跑——不做任何事，等下一个 TurnComplete
            HarnessAction::NotApplicable
        }

        "blocked_by_failure" | "blocked" => HarnessAction::Blocked {
            message: decision.message.unwrap_or_else(|| "blocked".into()),
        },

        other => HarnessAction::Error(format!("unknown action: {other}")),
    }
}

/// 从 gate findings 解出回滚用的 violation_type。
///
/// SDAN/02·06：这是**读结构化信号**，不是 substring 猜。gate_runner.py 已对
/// 能确信分类的 structural finding 显式产出 `violation_type`（∈
/// route_table.rollback_dispatch 键）。此处优先取已分类的；遇到未分类的
/// structural（如 PAGENUM/CH MISMATCH、INDEX INIT、LEGACY 等 slide 级结构
/// 问题）显式 `eprintln!` 告警 + 文档化兜底，**绝不静默猜**。
fn find_rollback_rule(findings: &[GateFinding]) -> String {
    // 优先：已分类的 structural finding 的结构化 violation_type。
    for f in findings {
        if f.rollback_scope == "structural" && !f.violation_type.is_empty() {
            return f.violation_type.clone();
        }
    }
    // 有 structural 但 gate_runner 未分类：告警 + 文档化兜底（不静默）。
    if let Some(f) = findings.iter().find(|f| f.rollback_scope == "structural") {
        eprintln!(
            "[harness] structural finding 无 violation_type（rule={}），暂兜底 \
             density_violation —— slide 级结构问题超出现有 3 类 taxonomy，待后续阶段补",
            f.rule
        );
        return "density_violation".to_string();
    }
    // 完全没有 structural finding 却来求回滚：异常路径，告警兜底。
    eprintln!("[harness] find_rollback_rule: 无 structural finding，兜底 density_violation");
    "density_violation".to_string()
}

/// 合成角色定义视图供 prompt 构建：能力字段(name/outputs) ← registry，
/// 裁决/拓扑字段(gate_description ← nodes.soft.criteria、depends_on ← edges 反查) ← route_table。
/// 键名保持 name/outputs/gate_description/depends_on，供 dispatch_or_wait 的
/// no_upstream 判断、resolve_gate_type 等消费（拆对话线后 brief/review prompt 已删）。
fn read_role_def(role_id: &str) -> serde_json::Value {
    // [B2] 差事节点(<bu>#<seq>)按所属部取定义(depends_on 走部的静态 edges:shangshu→部)。
    let bu = bu_of(role_id);
    let role_id = bu.as_str();
    let wf = workflow_of_role(role_id);
    let reg = read_registry_for(&wf);
    let rt = read_route_table_for(&wf);
    let agent = reg.get("agents").and_then(|a| a.get(role_id));
    let node = rt.get("nodes").and_then(|n| n.get(role_id));

    let name = agent
        .and_then(|a| a.get("name"))
        .cloned()
        .unwrap_or_else(|| serde_json::Value::String(role_id.to_string()));
    let outputs = agent
        .and_then(|a| a.get("outputs"))
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
    let gate_description = node
        .and_then(|n| n.get("soft"))
        .filter(|s| s.is_object())
        .and_then(|s| s.get("criteria"))
        .cloned()
        .unwrap_or_else(|| serde_json::Value::String(String::new()));
    // depends_on ← route_table.edges 反查直接上游（全图，prompt 预览够用；
    // 场景感知的有效依赖在 scheduler 侧 build_context_summary 处理）。
    let depends_on: Vec<serde_json::Value> = rt
        .get("edges")
        .and_then(|e| e.as_array())
        .map(|edges| {
            edges
                .iter()
                .filter(|e| e.get("to").and_then(|t| t.as_str()) == Some(role_id))
                .filter_map(|e| e.get("from").cloned())
                .collect()
        })
        .unwrap_or_default();

    serde_json::json!({
        "name": name,
        "outputs": outputs,
        "gate_description": gate_description,
        "depends_on": depends_on,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_python_command_uses_shared_runtime_resolver() {
        let command = crate::platform::process::python_command();
        assert_eq!(
            command.get_program().to_string_lossy(),
            crate::platform::paths::python_command()
        );
    }

    #[test]
    fn warmup_block_reason_returns_concrete_failed_check() {
        let report = serde_json::json!({
            "status": "blocked",
            "checks": {
                "dependencies": { "status": "pass", "details": "ok" },
                "script_presence": {
                    "status": "blocked",
                    "details": "missing scripts: scheduler.py"
                }
            }
        });

        assert_eq!(
            warmup_block_reason(&report).as_deref(),
            Some("missing scripts: scheduler.py")
        );
    }

    #[test]
    fn warmup_block_reason_returns_python_launch_error_without_check_report() {
        let expected = "工作流 Python 启动失败 (interpreter=C:\\Program Files\\pinvou3\\runtime\\python\\pythonw.exe): 系统找不到指定的文件";
        let report = serde_json::json!({
            "status": "blocked",
            "error": expected,
            "report_error": "warmup_report.json missing"
        });

        assert_eq!(warmup_block_reason(&report).as_deref(), Some(expected));
    }

    #[test]
    fn model_auth_failure_is_non_retryable_but_other_http_errors_are_not() {
        for error in [
            "Authentication failed: invalid API key",
            "Authorization failed: access denied",
            "SSE stream request failed: HTTP 401 Unauthorized",
            "Failed to call API: HTTP 403 Forbidden",
        ] {
            assert!(model_auth_failure_reason(error).is_some(), "{error}");
        }
        for error in [
            "SSE stream request failed: HTTP 429 Too Many Requests",
            "Server error (503): unavailable",
            "rendered 403 pages successfully",
        ] {
            assert!(model_auth_failure_reason(error).is_none(), "{error}");
        }
    }

    #[test]
    fn failure_diagnostic_preserves_multiline_reason_and_drops_protocol_sentinel() {
        let raw =
            "SubAgent execution failed\nMCP tool filesystem/write_file failed: access denied\n\
                   <codewhale:subagent.done status=\"failed\">ignored</codewhale:subagent.done>";
        let detail = failure_detail(raw);
        assert!(detail.contains("MCP tool filesystem/write_file failed: access denied"));
        assert!(!detail.contains("codewhale"));
        assert_eq!(failure_category(&detail), "permission");
        assert!(failure_summary(&detail).contains("access denied"));
    }

    #[test]
    fn subprocess_failure_detail_prefers_stderr_and_keeps_stdout_context() {
        let detail = subprocess_failure_detail(
            "partial scheduler output",
            "Traceback (most recent call last):\nModuleNotFoundError: workflow_state",
        );
        assert!(detail.contains("stderr:"));
        assert!(detail.contains("ModuleNotFoundError: workflow_state"));
        assert!(detail.contains("stdout:"));
        assert!(detail.contains("partial scheduler output"));
    }

    #[test]
    fn scheduler_runtime_failure_persists_stderr_in_flow_log() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-scheduler-failure-log-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(root.join("_state")).unwrap();
        let error = "工作流 Python 执行失败: Traceback\nModuleNotFoundError: No module named 'workflow_state'";

        record_runtime_failure_for_project(&root, "", "scheduler_kick", error);

        let content = std::fs::read_to_string(root.join("_state").join("flow_log.jsonl")).unwrap();
        let record: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(record["event"], "runtime_failure");
        assert_eq!(record["stage"], "scheduler_kick");
        assert!(record["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("ModuleNotFoundError")));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn flow_log_is_written_natively_and_updates_last_run_timestamp() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-workflow-log-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&root).unwrap();
        log_flow(
            &root,
            "agent_failed",
            &[
                ("role_id", "taizi"),
                ("reason", "HTTP 503 upstream unavailable"),
            ],
        );

        let path = root.join("_state").join("flow_log.jsonl");
        let content = std::fs::read_to_string(&path).unwrap();
        let record: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(record["event"], "agent_failed");
        assert_eq!(record["reason"], "HTTP 503 upstream unavailable");
        assert!(record["timestamp"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
        assert_eq!(
            read_last_run_ts(&root, "taizi").as_deref(),
            record["timestamp"].as_str()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn workflow_failure_reason_restores_persisted_blocked_state() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pinvou3-workflow-failure-{}-{nonce}",
            std::process::id()
        ));
        let state_dir = root.join("_state");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join("workflow_progress.json"),
            serde_json::json!({
                "roles": {
                    "taizi": {
                        "status": "failed",
                        "error": "模型服务鉴权失败: HTTP 403 Forbidden"
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(
            workflow_failure_reason_for_project(&root).as_deref(),
            Some("taizi: 模型服务鉴权失败: HTTP 403 Forbidden")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn workflow_stop_marker_blocks_resume_and_preserves_brief() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-workflow-stop-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let project = root.join("wf-test");
        let state = project.join("_state");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(
            state.join("workflow_progress.json"),
            r#"{"scenario":"solution_deck","roles":{"zhongshu":{"status":"running"}}}"#,
        )
        .unwrap();
        std::fs::write(
            state.join("brief.json"),
            r#"{"scenario":"solution_deck","user_request_raw":"原始需求"}"#,
        )
        .unwrap();

        let result = stop_workflow(&root, "session-stop-test", "user_stopped").unwrap();
        assert!(state.join(WORKFLOW_STOP_MARKER).is_file());
        assert!(workflow_is_stopped(&root));
        assert_eq!(result["brief"]["user_request_raw"], "原始需求");
        let repeated = stop_workflow(&root, "session-stop-test", "repeat").unwrap();
        assert_eq!(repeated["stopped_at"], result["stopped_at"]);
        assert!(matches!(
            step_fresh(&root),
            HarnessAction::Blocked { message } if message.contains("用户停止")
        ));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn truncate_never_panics_on_chinese_char_boundary() {
        // 复现 P0 根因:大纲是中文,直接 &s[..2000] 会切在多字节字符中间 panic。
        // 构造一个 byte 2000 恰好落在 '压'(3 字节)中间的串。
        let mut s = String::from("# 中国人口发展趋势分析 — 大纲\n");
        while s.len() < 2100 {
            s.push_str("总人口降至14.09亿，老龄化压力持续加大；");
        }
        assert!(s.len() > 2000);
        // 直接字节切片会 panic;helper 必须安全返回 ≤2000 的合法 UTF-8 前缀。
        let out = truncate_on_char_boundary(&s, 2000);
        assert!(out.len() <= 2000);
        assert!(s.starts_with(out));
        // 返回值本身必须是合法 UTF-8(能再被切片/打印而不 panic)。
        let _ = format!("{out}...");
    }

    #[test]
    fn truncate_returns_whole_string_when_under_limit() {
        let s = "短中文串";
        assert_eq!(truncate_on_char_boundary(s, 2000), s);
    }

    #[test]
    fn truncate_handles_boundary_exactly_at_limit() {
        // max_bytes 恰为 char 边界时不应回退。
        let s = "abcd压"; // "abcd"=4 字节, '压'=3 字节
        assert_eq!(truncate_on_char_boundary(s, 4), "abcd");
        // max_bytes 落在 '压' 中间(5,6)→ 回退到 4。
        assert_eq!(truncate_on_char_boundary(s, 5), "abcd");
        assert_eq!(truncate_on_char_boundary(s, 6), "abcd");
    }

    #[test]
    fn output_section_skips_submit_type_agent() {
        let project = std::env::temp_dir().join("test_proj");
        let agent = serde_json::json!({
            "output_schema": { "x-output-file": "some/path.json" },
            "outputs": ["_state/product_brief.md"]
        });

        assert_eq!(
            build_output_section_from_agent(&agent, &project),
            String::new()
        );
    }

    #[test]
    fn output_section_renders_concrete_path() {
        let project = std::env::temp_dir().join("test_proj");
        let section = render_output_section(vec!["_state/product_brief.md".to_string()], &project);

        assert!(section.contains("产物地址"));
        assert!(section.contains(
            &project
                .join("_state/product_brief.md")
                .display()
                .to_string()
        ));
        assert!(section.contains("product_brief.md"));
    }

    #[test]
    fn output_section_renders_glob_path() {
        let project = std::env::temp_dir().join("test_proj");
        let section = render_output_section(vec!["HTML_Deck/slides/*.html".to_string()], &project);

        assert!(section.contains(&project.join("HTML_Deck/slides").display().to_string()));
        assert!(section.contains("*.html"));
    }

    fn gf(rollback_scope: &str, rule: &str, violation_type: &str) -> GateFinding {
        GateFinding {
            severity: "CRITICAL".into(),
            rule: rule.into(),
            rollback_scope: rollback_scope.into(),
            fix_hint: String::new(),
            message: String::new(),
            violation_type: violation_type.into(),
        }
    }

    #[test]
    fn rollback_rule_reads_structured_violation_type() {
        // ① structural finding 带 violation_type → 直接返回该结构化值(不再 substring 猜)。
        let findings = vec![gf(
            "structural",
            "slide_file_missing",
            "narrative_flow_broken",
        )];
        assert_eq!(find_rollback_rule(&findings), "narrative_flow_broken");
    }

    #[test]
    fn rollback_rule_falls_back_when_unclassified() {
        // ② structural 但 gate_runner 未分类(violation_type 空)→ 兜底 density_violation,
        //    实现里会 eprintln 告警(不静默猜)。
        let findings = vec![gf("structural", "pagenum_mismatch", "")];
        assert_eq!(find_rollback_rule(&findings), "density_violation");
    }

    #[test]
    fn rollback_rule_prefers_classified_over_unclassified() {
        // ③ 混合:local 忽略;未分类 structural 在前、已分类 structural 在后
        //    → 优先取已分类的(image_quality_failure),而非按顺序撞上未分类的兜底 density。
        let findings = vec![
            gf("local", "possibly_empty_page", ""),
            gf("structural", "pagenum_mismatch", ""),
            gf("structural", "vlm_score_low", "image_quality_failure"),
        ];
        assert_eq!(find_rollback_rule(&findings), "image_quality_failure");
    }
}
