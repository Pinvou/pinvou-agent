# Milestone Contract Runtime Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 pinvou3 的里程碑编排升级为“静态契约 + 动态拆解 + 运行时验证”的完整流程系统。

**Architecture:** 新增 contract / runtime / validator / dynamic planner 四组模块。app.toml 和动态拆解都输出同一种 MilestoneContract；ContractRuntime 决定本轮动作；StepBuilder 只渲染契约 prompt；Validator 对工具调用和响应做硬边界检查；Web 主路径优先动态计划，失败回退静态契约。

**Tech Stack:** Rust 2024, serde/toml/serde_json, regex, tokio, axum SSE, existing `AgentHarness` / `PlatformEngine` / `ConversationState`

**Spec:** `docs/superpowers/specs/2026-05-09-milestone-contract-runtime-design.md`

---

## File Structure

```
pinvou-platform/src/
├── app.rs                  # Modify: PlanningConfig, MilestoneContract, TOML parsing defaults
├── workflow.rs             # Modify: runtime bookkeeping fields
├── contract.rs             # Create: contract data types and defaults
├── contract_runtime.rs     # Create: TurnDirective decision engine
├── contract_validator.rs   # Create: tool/response/plan validation
├── dynamic_planner.rs      # Create: dynamic plan prompt, parsing, fallback
├── step_builder.rs         # Modify: render ContractPrompt instead of phase guessing
├── engine.rs               # Modify: ensure_plan_initialized + contract-runtime integration
├── web/mod.rs              # Modify: stream_chat uses ContractRuntime and Validator
└── lib.rs                  # Modify: expose new modules

apps/
├── 计划敲定/app.toml       # Modify: add explicit contracts
├── 文档生成/app.toml       # Modify: add explicit contracts
└── 数据分析/app.toml       # Modify: add explicit contracts
```

根目录当前不是有效 git 仓库，因此本计划不包含 git commit 步骤；每个任务以 `cargo test --manifest-path pinvou-platform/Cargo.toml` 作为完成门禁。

---

### Task 1: Add Contract Data Model

**Files:**
- Create: `pinvou-platform/src/contract.rs`
- Modify: `pinvou-platform/src/lib.rs`
- Modify: `pinvou-platform/src/app.rs`

- [ ] **Step 1: Write failing tests for legacy defaults and explicit contract parsing**

Add tests in `pinvou-platform/src/app.rs`:

```rust
#[test]
fn test_legacy_milestone_gets_default_contract() {
    let root = std::env::temp_dir().join(format!("pinvou-contract-legacy-{}", std::process::id()));
    let app_dir = root.join("计划敲定");
    std::fs::create_dir_all(&app_dir).unwrap();
    std::fs::write(app_dir.join("prompt.md"), "prompt").unwrap();
    std::fs::write(app_dir.join("app.toml"), r#"
[app]
name = "计划敲定"
description = "测试"
prompt_file = "prompt.md"

[[milestones]]
id = "goal"
label = "明确目标"
"#).unwrap();

    let registry = AppRegistry::load(&root).unwrap();
    let app = registry.find("计划敲定").unwrap();
    assert_eq!(app.planning.mode, PlanningMode::DynamicWithStaticFallback);
    assert_eq!(app.milestones[0].contract.mode, MilestoneMode::Collect);
    assert_eq!(app.milestones[0].contract.question_budget, 1);
}

#[test]
fn test_parse_explicit_milestone_contract() {
    let root = std::env::temp_dir().join(format!("pinvou-contract-explicit-{}", std::process::id()));
    let app_dir = root.join("计划敲定");
    std::fs::create_dir_all(&app_dir).unwrap();
    std::fs::write(app_dir.join("prompt.md"), "prompt").unwrap();
    std::fs::write(app_dir.join("app.toml"), r#"
[app]
name = "计划敲定"
description = "测试"
prompt_file = "prompt.md"

[app.planning]
mode = "static_only"
confirm_dynamic_plan = true
max_plan_retries = 1

[[milestones]]
id = "options"
label = "方案对比"
contract_mode = "produce_options"
question_budget = 1
required_context = ["goal", "budget"]
produced_context = ["selected_option"]
allowed_tools = ["request_user_input"]
output_requirements = ["min_options:2", "max_options:3", "must_contain_table", "no_open_question"]
advance_policy = "on_choice"
"#).unwrap();

    let registry = AppRegistry::load(&root).unwrap();
    let app = registry.find("计划敲定").unwrap();
    assert_eq!(app.planning.mode, PlanningMode::StaticOnly);
    let contract = &app.milestones[0].contract;
    assert_eq!(contract.mode, MilestoneMode::ProduceOptions);
    assert_eq!(contract.required_context, vec!["goal", "budget"]);
    assert!(contract.output_requirements.contains(&OutputRequirement::MustContainTable));
    assert!(contract.output_requirements.contains(&OutputRequirement::MinOptions(2)));
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test --manifest-path pinvou-platform/Cargo.toml app::tests::test_
```

Expected: compile fails because `PlanningMode`, `MilestoneMode`, `OutputRequirement`, `planning`, and `contract` do not exist.

- [ ] **Step 3: Implement `contract.rs`**

Create `pinvou-platform/src/contract.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanningMode {
    StaticOnly,
    DynamicWithStaticFallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanningConfig {
    #[serde(default = "default_planning_mode")]
    pub mode: PlanningMode,
    #[serde(default)]
    pub confirm_dynamic_plan: bool,
    #[serde(default = "default_max_plan_retries")]
    pub max_plan_retries: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MilestoneMode {
    Collect,
    ProduceOptions,
    RefineSelectedOption,
    FinalOutput,
    Freeform,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdvancePolicy {
    OnChoice,
    OnValidOutput,
    ManualContinue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputRequirement {
    MinOptions(u8),
    MaxOptions(u8),
    MustContainTable,
    MustContainSchedule,
    MustContainRiskSection,
    NoOpenQuestion,
    NoToolCall,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MilestoneContract {
    #[serde(default = "default_milestone_mode")]
    pub mode: MilestoneMode,
    #[serde(default = "default_question_budget")]
    pub question_budget: u8,
    #[serde(default)]
    pub required_context: Vec<String>,
    #[serde(default)]
    pub produced_context: Vec<String>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub forbidden_tools: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_output_requirements")]
    pub output_requirements: Vec<OutputRequirement>,
    #[serde(default = "default_advance_policy")]
    pub advance_policy: AdvancePolicy,
}

impl Default for PlanningConfig {
    fn default() -> Self {
        Self {
            mode: PlanningMode::DynamicWithStaticFallback,
            confirm_dynamic_plan: false,
            max_plan_retries: 2,
        }
    }
}

impl Default for MilestoneContract {
    fn default() -> Self {
        Self {
            mode: MilestoneMode::Collect,
            question_budget: 1,
            required_context: vec![],
            produced_context: vec![],
            allowed_tools: vec![],
            forbidden_tools: vec![],
            output_requirements: vec![],
            advance_policy: AdvancePolicy::ManualContinue,
        }
    }
}

pub fn default_contract_for_label(label: &str) -> MilestoneContract {
    let mut contract = MilestoneContract::default();
    if label.contains("方案") || label.contains("对比") {
        contract.mode = MilestoneMode::ProduceOptions;
        contract.question_budget = 1;
        contract.allowed_tools = vec!["request_user_input".into()];
        contract.output_requirements = vec![
            OutputRequirement::MinOptions(2),
            OutputRequirement::MaxOptions(3),
            OutputRequirement::MustContainTable,
            OutputRequirement::NoOpenQuestion,
        ];
        contract.advance_policy = AdvancePolicy::OnChoice;
    } else if label.contains("细化") {
        contract.mode = MilestoneMode::RefineSelectedOption;
        contract.question_budget = 0;
        contract.output_requirements = vec![
            OutputRequirement::MustContainSchedule,
            OutputRequirement::MustContainRiskSection,
            OutputRequirement::NoOpenQuestion,
        ];
        contract.advance_policy = AdvancePolicy::OnValidOutput;
    } else if label.contains("输出") || label.contains("定稿") {
        contract.mode = MilestoneMode::FinalOutput;
        contract.question_budget = 0;
        contract.output_requirements = vec![OutputRequirement::NoToolCall];
        contract.advance_policy = AdvancePolicy::OnValidOutput;
    } else {
        contract.mode = MilestoneMode::Collect;
        contract.question_budget = 1;
        contract.allowed_tools = vec!["request_user_input".into()];
        contract.advance_policy = AdvancePolicy::OnChoice;
    }
    contract
}

fn default_planning_mode() -> PlanningMode { PlanningMode::DynamicWithStaticFallback }
fn default_max_plan_retries() -> u8 { 2 }
fn default_milestone_mode() -> MilestoneMode { MilestoneMode::Collect }
fn default_question_budget() -> u8 { 1 }
fn default_advance_policy() -> AdvancePolicy { AdvancePolicy::ManualContinue }

pub fn deserialize_output_requirements<'de, D>(deserializer: D) -> Result<Vec<OutputRequirement>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Vec::<String>::deserialize(deserializer)?;
    raw.into_iter()
        .map(|item| parse_output_requirement(&item).map_err(serde::de::Error::custom))
        .collect()
}

pub fn parse_output_requirement(raw: &str) -> Result<OutputRequirement, String> {
    match raw {
        "must_contain_table" => Ok(OutputRequirement::MustContainTable),
        "must_contain_schedule" => Ok(OutputRequirement::MustContainSchedule),
        "must_contain_risk_section" => Ok(OutputRequirement::MustContainRiskSection),
        "no_open_question" => Ok(OutputRequirement::NoOpenQuestion),
        "no_tool_call" => Ok(OutputRequirement::NoToolCall),
        _ if raw.starts_with("min_options:") => raw["min_options:".len()..]
            .parse::<u8>()
            .map(OutputRequirement::MinOptions)
            .map_err(|e| format!("invalid min_options requirement {raw}: {e}")),
        _ if raw.starts_with("max_options:") => raw["max_options:".len()..]
            .parse::<u8>()
            .map(OutputRequirement::MaxOptions)
            .map_err(|e| format!("invalid max_options requirement {raw}: {e}")),
        _ => Err(format!("unknown output requirement: {raw}")),
    }
}
```

- [ ] **Step 4: Wire model into `lib.rs` and `app.rs`**

In `pinvou-platform/src/lib.rs`, add:

```rust
pub mod contract;
```

In `pinvou-platform/src/app.rs`, import and extend structs:

```rust
use crate::contract::{default_contract_for_label, MilestoneContract, PlanningConfig};

#[serde(default)]
pub planning: PlanningConfig,

#[serde(default)]
pub contract: MilestoneContract,
```

Derive `Default` for `Milestone`, or implement it manually with empty `id` / `label`, `None` optional fields, and `MilestoneContract::default()`. The tests in later tasks rely on `..Default::default()` for milestone fixtures.

Add compatibility fields to `Milestone` for flat TOML:

```rust
#[serde(default)]
pub contract_mode: Option<crate::contract::MilestoneMode>,
#[serde(default)]
pub question_budget: Option<u8>,
#[serde(default)]
pub required_context: Vec<String>,
#[serde(default)]
pub produced_context: Vec<String>,
#[serde(default)]
pub allowed_tools: Vec<String>,
#[serde(default)]
pub forbidden_tools: Vec<String>,
#[serde(default, deserialize_with = "crate::contract::deserialize_output_requirements")]
pub output_requirements: Vec<crate::contract::OutputRequirement>,
#[serde(default)]
pub advance_policy: Option<crate::contract::AdvancePolicy>,
```

After TOML load, normalize each milestone:

```rust
for milestone in &mut config.milestones {
    let mut contract = if milestone.contract == MilestoneContract::default() {
        default_contract_for_label(&milestone.label)
    } else {
        milestone.contract.clone()
    };
    if let Some(mode) = milestone.contract_mode.take() {
        contract.mode = mode;
    }
    if let Some(budget) = milestone.question_budget {
        contract.question_budget = budget;
    }
    if !milestone.required_context.is_empty() {
        contract.required_context = milestone.required_context.clone();
    }
    if !milestone.produced_context.is_empty() {
        contract.produced_context = milestone.produced_context.clone();
    }
    if !milestone.allowed_tools.is_empty() {
        contract.allowed_tools = milestone.allowed_tools.clone();
    }
    if !milestone.forbidden_tools.is_empty() {
        contract.forbidden_tools = milestone.forbidden_tools.clone();
    }
    if !milestone.output_requirements.is_empty() {
        contract.output_requirements = milestone.output_requirements.clone();
    }
    if let Some(policy) = milestone.advance_policy.take() {
        contract.advance_policy = policy;
    }
    milestone.contract = contract;
}
```

- [ ] **Step 5: Verify**

Run:

```bash
cargo test --manifest-path pinvou-platform/Cargo.toml app::tests::test_
```

Expected: both tests pass.

---

### Task 2: Add ContractRuntime Directives

**Files:**
- Create: `pinvou-platform/src/contract_runtime.rs`
- Modify: `pinvou-platform/src/lib.rs`
- Modify: `pinvou-platform/src/workflow.rs`

- [ ] **Step 1: Write failing runtime tests**

Create `pinvou-platform/src/contract_runtime.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Milestone;
    use crate::contract::{AdvancePolicy, MilestoneContract, MilestoneMode, OutputRequirement};
    use crate::workflow::ConversationState;

    fn milestone(id: &str, mode: MilestoneMode) -> Milestone {
        Milestone {
            id: id.into(),
            label: id.into(),
            prompt_hint: Some(format!("hint {id}")),
            icon: None,
            contract: MilestoneContract {
                mode,
                question_budget: 1,
                allowed_tools: vec!["request_user_input".into()],
                advance_policy: AdvancePolicy::OnChoice,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn collect_with_budget_returns_llm_call_allowing_user_input() {
        let ms = milestone("goal", MilestoneMode::Collect);
        let cs = ConversationState::new("计划敲定".into(), vec![ms.clone()]);
        let directive = ContractRuntime::next_directive(&ms, &cs, "用户需求").unwrap();
        assert!(matches!(directive, TurnDirective::CallLlm(_)));
        let TurnDirective::CallLlm(prompt) = directive else { unreachable!() };
        assert!(prompt.allowed_tools.contains(&"request_user_input".into()));
        assert!(prompt.system_requirements.iter().any(|r| r.contains("最多调用")));
    }

    #[test]
    fn final_output_disallows_tools_and_requires_final_document() {
        let mut ms = milestone("output", MilestoneMode::FinalOutput);
        ms.contract.question_budget = 0;
        ms.contract.allowed_tools = vec![];
        ms.contract.output_requirements = vec![OutputRequirement::NoToolCall];
        let cs = ConversationState::new("计划敲定".into(), vec![ms.clone()]);
        let directive = ContractRuntime::next_directive(&ms, &cs, "继续").unwrap();
        let TurnDirective::CallLlm(prompt) = directive else { unreachable!() };
        assert!(prompt.allowed_tools.is_empty());
        assert!(prompt.system_requirements.iter().any(|r| r.contains("最终")));
    }
}
```

Run:

```bash
cargo test --manifest-path pinvou-platform/Cargo.toml contract_runtime::tests::
```

Expected: compile fails because runtime types do not exist.

- [ ] **Step 2: Implement runtime types**

Replace top of `contract_runtime.rs` with:

```rust
use anyhow::Result;

use crate::app::Milestone;
use crate::contract::MilestoneMode;
use crate::workflow::ConversationState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceRequest {
    pub call_id: String,
    pub questions: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractPrompt {
    pub milestone_id: String,
    pub user_message: String,
    pub allowed_tools: Vec<String>,
    pub system_requirements: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnDirective {
    AskUser(ChoiceRequest),
    CallLlm(ContractPrompt),
    CompleteStep(String),
    Blocked(String),
}

pub struct ContractRuntime;

impl ContractRuntime {
    pub fn next_directive(
        milestone: &Milestone,
        _state: &ConversationState,
        user_message: &str,
    ) -> Result<TurnDirective> {
        let mut requirements = vec![format!("当前阶段：{}", milestone.label)];
        let contract = &milestone.contract;

        match contract.mode {
            MilestoneMode::Collect => {
                requirements.push(format!(
                    "最多调用 request_user_input {} 次；收到选择后立即总结并完成阶段。",
                    contract.question_budget
                ));
            }
            MilestoneMode::ProduceOptions => {
                requirements.push("必须先给出 2-3 个可选方案，并包含成本、时间、风险、收益对比。".into());
                requirements.push("可以调用 request_user_input 让用户选择一个方案，但选项数量必须是 2-3 个。".into());
            }
            MilestoneMode::RefineSelectedOption => {
                requirements.push("必须基于已选方案细化，不能继续收集目标类信息。".into());
                requirements.push("必须包含时间表、资源分配和风险预案。".into());
            }
            MilestoneMode::FinalOutput => {
                requirements.push("输出最终 markdown 文档，不要再提问，不要调用工具。".into());
            }
            MilestoneMode::Freeform => {
                requirements.push("按当前阶段目标完成任务。".into());
            }
        }

        Ok(TurnDirective::CallLlm(ContractPrompt {
            milestone_id: milestone.id.clone(),
            user_message: user_message.to_string(),
            allowed_tools: contract.allowed_tools.clone(),
            system_requirements: requirements,
        }))
    }
}
```

In `lib.rs`:

```rust
pub mod contract_runtime;
```

- [ ] **Step 3: Add runtime bookkeeping to ConversationState**

In `pinvou-platform/src/workflow.rs`, add fields:

```rust
#[serde(default)]
pub plan_initialized: bool,
#[serde(default)]
pub question_counts: HashMap<String, u8>,
```

Initialize them in `ConversationState::new`.

Add helpers:

```rust
pub fn question_count(&self, milestone_id: &str) -> u8 {
    self.question_counts.get(milestone_id).copied().unwrap_or(0)
}

pub fn increment_question_count(&mut self, milestone_id: &str) {
    let entry = self.question_counts.entry(milestone_id.to_string()).or_insert(0);
    *entry += 1;
}
```

- [ ] **Step 4: Verify**

Run:

```bash
cargo test --manifest-path pinvou-platform/Cargo.toml contract_runtime::tests:: workflow::tests::
```

Expected: runtime and workflow tests pass.

---

### Task 3: Add ContractValidator

**Files:**
- Create: `pinvou-platform/src/contract_validator.rs`
- Modify: `pinvou-platform/src/lib.rs`

- [ ] **Step 1: Write failing validator tests**

Create `pinvou-platform/src/contract_validator.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{MilestoneContract, MilestoneMode, OutputRequirement};

    #[test]
    fn produce_options_rejects_single_choice_option() {
        let contract = MilestoneContract {
            mode: MilestoneMode::ProduceOptions,
            output_requirements: vec![OutputRequirement::MinOptions(2)],
            allowed_tools: vec!["request_user_input".into()],
            ..Default::default()
        };
        let args = serde_json::json!({
            "questions": [{
                "header": "方案",
                "id": "selected_option",
                "question": "请选择方案",
                "options": [{"label": "湖畔水库", "description": "只看水库"}]
            }]
        });
        let result = ContractValidator::validate_tool_call(&contract, "request_user_input", &args);
        assert!(!result.ok);
        assert!(result.issues.iter().any(|i| i.contains("至少 2 个选项")));
    }

    #[test]
    fn final_output_rejects_tool_call() {
        let contract = MilestoneContract {
            mode: MilestoneMode::FinalOutput,
            output_requirements: vec![OutputRequirement::NoToolCall],
            ..Default::default()
        };
        let result = ContractValidator::validate_tool_call(&contract, "request_user_input", &serde_json::json!({}));
        assert!(!result.ok);
        assert!(result.issues.iter().any(|i| i.contains("不允许调用工具")));
    }

    #[test]
    fn refine_rejects_open_question_response() {
        let contract = MilestoneContract {
            mode: MilestoneMode::RefineSelectedOption,
            output_requirements: vec![OutputRequirement::NoOpenQuestion],
            ..Default::default()
        };
        let result = ContractValidator::validate_response(
            &contract,
            "这是细化方案。您还想补充什么需求？",
        );
        assert!(!result.ok);
        assert!(result.issues.iter().any(|i| i.contains("开放式问题")));
    }
}
```

Run:

```bash
cargo test --manifest-path pinvou-platform/Cargo.toml contract_validator::tests::
```

Expected: compile fails because `ContractValidator` does not exist.

- [ ] **Step 2: Implement validator**

Add implementation:

```rust
use crate::contract::{MilestoneContract, OutputRequirement};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationResult {
    pub ok: bool,
    pub issues: Vec<String>,
}

impl ValidationResult {
    pub fn ok() -> Self { Self { ok: true, issues: vec![] } }
    pub fn fail(issue: impl Into<String>) -> Self { Self { ok: false, issues: vec![issue.into()] } }
    pub fn push_issue(&mut self, issue: impl Into<String>) {
        self.ok = false;
        self.issues.push(issue.into());
    }
}

pub struct ContractValidator;

impl ContractValidator {
    pub fn validate_tool_call(
        contract: &MilestoneContract,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> ValidationResult {
        let mut result = ValidationResult::ok();

        if contract.output_requirements.contains(&OutputRequirement::NoToolCall)
            || contract.forbidden_tools.iter().any(|t| t == tool_name)
            || (!contract.allowed_tools.is_empty() && !contract.allowed_tools.iter().any(|t| t == tool_name))
        {
            result.push_issue(format!("当前阶段不允许调用工具 {tool_name}"));
        }

        if tool_name == "request_user_input" {
            let option_counts: Vec<usize> = arguments
                .get("questions")
                .and_then(|v| v.as_array())
                .map(|questions| {
                    questions
                        .iter()
                        .map(|q| q.get("options").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0))
                        .collect()
                })
                .unwrap_or_default();

            for req in &contract.output_requirements {
                match req {
                    OutputRequirement::MinOptions(min) => {
                        if option_counts.iter().any(|count| *count < *min as usize) {
                            result.push_issue(format!("request_user_input 至少 {} 个选项", min));
                        }
                    }
                    OutputRequirement::MaxOptions(max) => {
                        if option_counts.iter().any(|count| *count > *max as usize) {
                            result.push_issue(format!("request_user_input 最多 {} 个选项", max));
                        }
                    }
                    _ => {}
                }
            }
        }

        result
    }

    pub fn validate_response(contract: &MilestoneContract, text: &str) -> ValidationResult {
        let mut result = ValidationResult::ok();
        for req in &contract.output_requirements {
            match req {
                OutputRequirement::MustContainTable => {
                    if !text.lines().any(|line| line.contains('|')) {
                        result.push_issue("响应必须包含对比表格");
                    }
                }
                OutputRequirement::MustContainSchedule => {
                    if !text.contains("时间") && !text.contains("上午") && !text.contains("下午") && !text.contains(':') {
                        result.push_issue("响应必须包含时间表");
                    }
                }
                OutputRequirement::MustContainRiskSection => {
                    if !text.contains("风险") && !text.contains("预案") {
                        result.push_issue("响应必须包含风险或预案");
                    }
                }
                OutputRequirement::NoOpenQuestion => {
                    let tail: String = text.chars().rev().take(120).collect::<Vec<_>>().into_iter().rev().collect();
                    if tail.contains("还想") || tail.contains("需要补充") || tail.contains("你想") || tail.contains("您想") {
                        result.push_issue("响应不能以开放式问题继续收集需求");
                    }
                }
                _ => {}
            }
        }
        result
    }
}
```

In `lib.rs`:

```rust
pub mod contract_validator;
```

- [ ] **Step 3: Verify**

Run:

```bash
cargo test --manifest-path pinvou-platform/Cargo.toml contract_validator::tests::
```

Expected: validator tests pass.

---

### Task 4: Add DynamicPlanner With Static Fallback

**Files:**
- Create: `pinvou-platform/src/dynamic_planner.rs`
- Modify: `pinvou-platform/src/lib.rs`
- Modify: `pinvou-platform/src/engine.rs`

- [ ] **Step 1: Write failing planner tests**

Create tests in `pinvou-platform/src/dynamic_planner.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AppConfig, Milestone};
    use crate::contract::{MilestoneContract, MilestoneMode};

    fn app_template() -> AppConfig {
        AppConfig {
            id: "计划敲定".into(),
            name: "计划敲定".into(),
            description: "测试".into(),
            icon: "*".into(),
            prompt_file: None,
            prompt: None,
            model_preference: "large".into(),
            tools: vec!["request_user_input".into()],
            milestones: vec![Milestone {
                id: "goal".into(),
                label: "明确目标".into(),
                prompt_hint: Some("确认目标".into()),
                icon: None,
                contract: MilestoneContract { mode: MilestoneMode::Collect, ..Default::default() },
                ..Default::default()
            }],
            planning: Default::default(),
            granularity: Some("fine".into()),
            confirm_at: None,
            ban_list: vec![],
            meta: Default::default(),
        }
    }

    #[test]
    fn parse_valid_dynamic_plan_specializes_static_template() {
        let json = r#"{
          "milestones": [
            {
              "id": "goal",
              "label": "明确水生水库徒步目标",
              "prompt_hint": "确认徒步强度、兴趣和时长",
              "contract_mode": "collect",
              "produced_context": ["interest", "duration", "intensity"]
            }
          ]
        }"#;
        let plan = DynamicPlanner::parse_plan(json, &app_template()).unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].id, "goal");
        assert_eq!(plan[0].label, "明确水生水库徒步目标");
        assert_eq!(plan[0].contract.mode, MilestoneMode::Collect);
        assert_eq!(plan[0].contract.produced_context, vec!["interest", "duration", "intensity"]);
    }

    #[test]
    fn invalid_dynamic_plan_falls_back_to_static_template() {
        let plan = DynamicPlanner::parse_plan("not json", &app_template()).unwrap_or_else(|_| app_template().milestones);
        assert_eq!(plan[0].label, "明确目标");
    }
}
```

Run:

```bash
cargo test --manifest-path pinvou-platform/Cargo.toml dynamic_planner::tests::
```

Expected: compile fails because DynamicPlanner does not exist.

- [ ] **Step 2: Implement parser and prompt builder**

Implement:

```rust
use anyhow::{Context, Result};
use serde::Deserialize;

use crate::app::{AppConfig, Milestone};
use crate::contract::MilestoneMode;

pub struct DynamicPlanner;

#[derive(Debug, Deserialize)]
struct DynamicPlanDto {
    milestones: Vec<DynamicMilestoneDto>,
}

#[derive(Debug, Deserialize)]
struct DynamicMilestoneDto {
    id: String,
    label: String,
    #[serde(default)]
    prompt_hint: Option<String>,
    #[serde(default)]
    contract_mode: Option<MilestoneMode>,
    #[serde(default)]
    required_context: Vec<String>,
    #[serde(default)]
    produced_context: Vec<String>,
}

impl DynamicPlanner {
    pub fn build_prompt(user_message: &str, app: &AppConfig) -> String {
        let templates = app.milestones.iter().map(|m| {
            format!("- id={} label={} mode={:?}", m.id, m.label, m.contract.mode)
        }).collect::<Vec<_>>().join("\n");
        format!(
            "用户需求: {user_message}\n当前应用: {} -- {}\n\n基于以下静态模板生成动态里程碑，只能复用已有 id，不能新增未知 id。\n{templates}\n\n输出 JSON: {{\"milestones\":[{{\"id\":\"...\",\"label\":\"...\",\"prompt_hint\":\"...\",\"contract_mode\":\"collect|produce_options|refine_selected_option|final_output\",\"required_context\":[],\"produced_context\":[]}}]}}",
            app.name, app.description
        )
    }

    pub fn parse_plan(text: &str, app: &AppConfig) -> Result<Vec<Milestone>> {
        let json = extract_json_object(text).context("dynamic plan response has no JSON object")?;
        let dto: DynamicPlanDto = serde_json::from_str(json).context("failed to parse dynamic plan JSON")?;
        let mut output = Vec::new();
        for item in dto.milestones {
            let template = app.milestones.iter().find(|m| m.id == item.id)
                .with_context(|| format!("dynamic milestone references unknown id {}", item.id))?;
            let mut milestone = template.clone();
            milestone.label = item.label;
            milestone.prompt_hint = item.prompt_hint.or(milestone.prompt_hint);
            if let Some(mode) = item.contract_mode {
                milestone.contract.mode = mode;
            }
            if !item.required_context.is_empty() {
                milestone.contract.required_context = item.required_context;
            }
            if !item.produced_context.is_empty() {
                milestone.contract.produced_context = item.produced_context;
            }
            output.push(milestone);
        }
        if output.is_empty() {
            anyhow::bail!("dynamic plan has no milestones");
        }
        Ok(output)
    }
}

fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    Some(&text[start..=end])
}
```

In `lib.rs`:

```rust
pub mod dynamic_planner;
```

- [ ] **Step 3: Verify**

Run:

```bash
cargo test --manifest-path pinvou-platform/Cargo.toml dynamic_planner::tests::
```

Expected: planner parser tests pass.

---

### Task 5: Refactor StepBuilder To Render ContractPrompt

**Files:**
- Modify: `pinvou-platform/src/step_builder.rs`

- [ ] **Step 1: Write failing StepBuilder test**

Add test:

```rust
#[test]
fn test_build_from_contract_prompt_includes_contract_rules() {
    let prompt = crate::contract_runtime::ContractPrompt {
        milestone_id: "options".into(),
        user_message: "继续".into(),
        allowed_tools: vec!["request_user_input".into()],
        system_requirements: vec![
            "必须给出 2-3 个可选方案".into(),
            "不能只问偏好".into(),
        ],
    };

    let rendered = StepBuilder::build_contract_prompt(
        &prompt,
        &Default::default(),
        Some("计划敲定 prompt"),
    );

    assert!(rendered.system.contains("必须给出 2-3 个可选方案"));
    assert!(rendered.system.contains("request_user_input"));
    assert!(rendered.system.contains("计划敲定 prompt"));
}
```

Run:

```bash
cargo test --manifest-path pinvou-platform/Cargo.toml step_builder::tests::test_build_from_contract_prompt_includes_contract_rules
```

Expected: compile fails because `build_contract_prompt` does not exist.

- [ ] **Step 2: Implement build_contract_prompt**

Add:

```rust
pub fn build_contract_prompt(
    prompt: &crate::contract_runtime::ContractPrompt,
    context: &std::collections::HashMap<String, String>,
    app_prompt: Option<&str>,
) -> StepPrompt {
    let mut parts = Vec::new();
    if let Some(app_prompt) = app_prompt {
        parts.push("## 应用角色".to_string());
        parts.push(app_prompt.to_string());
    }
    parts.push("\n## 当前契约要求".to_string());
    for req in &prompt.system_requirements {
        parts.push(format!("- {req}"));
    }
    if !context.is_empty() {
        parts.push("\n## 已知信息".to_string());
        for (k, v) in context {
            parts.push(format!("- **{k}**: {v}"));
        }
    }
    if prompt.allowed_tools.is_empty() {
        parts.push("\n## 工具限制\n- 当前阶段不要调用工具。".to_string());
    } else {
        parts.push(format!("\n## 可用工具\n- {}", prompt.allowed_tools.join("\n- ")));
    }
    parts.push("\n## 用户消息".to_string());
    parts.push(prompt.user_message.clone());
    parts.push("\n## 状态信号\n回复末尾附加 [OK] / [MORE] 还需要:{具体内容} / [BLOCKED] 原因:{具体原因}".to_string());

    StepPrompt {
        system: parts.join("\n"),
        append_user_message: false,
    }
}
```

- [ ] **Step 3: Verify**

Run:

```bash
cargo test --manifest-path pinvou-platform/Cargo.toml step_builder::tests::test_build_from_contract_prompt_includes_contract_rules
```

Expected: test passes.

---

### Task 6: Integrate ContractRuntime Into Engine

**Files:**
- Modify: `pinvou-platform/src/engine.rs`

- [ ] **Step 1: Write failing engine tests**

Add tests in `engine::mock`:

```rust
#[tokio::test]
async fn test_ensure_plan_initialized_uses_static_fallback_when_dynamic_parse_fails() {
    let mut mock = MockHarness::new();
    mock.responses = vec!["not json".into()];
    let registry = registry_with_plan_app("fine");
    let mut engine = PlatformEngine::new(mock, registry, PathBuf::from("."));
    engine.load_app("计划敲定").unwrap();

    engine.ensure_plan_initialized("我周末去水库徒步").await.unwrap();

    let cs = engine.conv_state.as_ref().unwrap();
    assert!(cs.plan_initialized);
    assert_eq!(cs.milestones[0].0.id, "goal");
}

#[tokio::test]
async fn test_next_contract_prompt_uses_runtime_directive() {
    let mock = MockHarness::new();
    let registry = registry_with_plan_app("fine");
    let mut engine = PlatformEngine::new(mock, registry, PathBuf::from("."));
    engine.load_app("计划敲定").unwrap();

    let prompt = engine.build_next_contract_prompt("继续").unwrap();
    assert!(prompt.system.contains("当前契约要求"));
}
```

Run:

```bash
cargo test --manifest-path pinvou-platform/Cargo.toml engine::mock::test_
```

Expected: compile fails because engine APIs do not exist.

- [ ] **Step 2: Implement ensure_plan_initialized**

Add to `PlatformEngine`:

```rust
pub async fn ensure_plan_initialized(&mut self, user_message: &str) -> Result<()> {
    let Some(app) = self.current_app.clone() else {
        return Ok(());
    };
    if self.conv_state.as_ref().map(|cs| cs.plan_initialized).unwrap_or(false) {
        return Ok(());
    }

    let milestones = match app.planning.mode {
        crate::contract::PlanningMode::StaticOnly => app.milestones.clone(),
        crate::contract::PlanningMode::DynamicWithStaticFallback => {
            let prompt = crate::dynamic_planner::DynamicPlanner::build_prompt(user_message, &app);
            match self.harness.chat(ChatRequest {
                user_message: prompt,
                platform_system_prompt: Some("你是流程拆解器。只输出 JSON。".into()),
                context: Default::default(),
                tools: vec![],
                model: None,
                session_id: None,
                previous_messages: vec![],
            }).await.and_then(|text| crate::dynamic_planner::DynamicPlanner::parse_plan(&text, &app)) {
                Ok(plan) => plan,
                Err(_) => app.milestones.clone(),
            }
        }
    };

    if let Some(ref mut cs) = self.conv_state {
        *cs = ConversationState::new(app.id.clone(), milestones);
        cs.plan_initialized = true;
    }
    Ok(())
}
```

- [ ] **Step 3: Implement build_next_contract_prompt**

Add:

```rust
pub fn build_next_contract_prompt(&self, user_message: &str) -> Result<super::step_builder::StepPrompt> {
    let cs = self.conv_state.as_ref().ok_or_else(|| anyhow::anyhow!("没有对话状态"))?;
    let milestone = cs.active_milestone().ok_or_else(|| anyhow::anyhow!("没有活跃里程碑"))?;
    let directive = crate::contract_runtime::ContractRuntime::next_directive(milestone, cs, user_message)?;
    match directive {
        crate::contract_runtime::TurnDirective::CallLlm(prompt) => Ok(super::step_builder::StepBuilder::build_contract_prompt(
            &prompt,
            &cs.context,
            self.app_system_prompt().as_deref(),
        )),
        _ => anyhow::bail!("当前 directive 不是 CallLlm"),
    }
}
```

- [ ] **Step 4: Verify**

Run:

```bash
cargo test --manifest-path pinvou-platform/Cargo.toml engine::mock::test_
```

Expected: tests pass.

---

### Task 7: Integrate ContractRuntime Into Web Stream

**Files:**
- Modify: `pinvou-platform/src/web/mod.rs`

- [ ] **Step 1: Add validator tests for Web-adjacent behavior in engine/runtime**

Do not test Axum SSE directly yet. Add unit tests around the pure functions created in Tasks 2-6:

```rust
#[test]
fn produce_options_contract_rejects_one_option_before_frontend_receives_it() {
    use crate::contract::{MilestoneContract, MilestoneMode, OutputRequirement};
    use crate::contract_validator::ContractValidator;

    let contract = MilestoneContract {
        mode: MilestoneMode::ProduceOptions,
        allowed_tools: vec!["request_user_input".into()],
        output_requirements: vec![OutputRequirement::MinOptions(2)],
        ..Default::default()
    };
    let args = serde_json::json!({
        "questions": [{
            "id": "selected_option",
            "header": "方案",
            "question": "请选择",
            "options": [{"label": "A", "description": "only one"}]
        }]
    });
    let result = ContractValidator::validate_tool_call(&contract, "request_user_input", &args);
    assert!(!result.ok);
}
```

- [ ] **Step 2: Modify `stream_chat()` initialization**

After `engine.load_app(&req.app_id)` and before tool resolution:

```rust
if req.tool_result.is_none() {
    if let Err(e) = engine.ensure_plan_initialized(&req.message).await {
        let ev = sse_err(format!("初始化计划失败: {e}"));
        return Box::new(stream::iter(vec![Ok(ev)]));
    }
}
```

- [ ] **Step 3: Use contract prompt instead of old StepBuilder::build path**

Replace old scoped prompt construction with:

```rust
let mut chat_req = {
    match engine.build_next_contract_prompt(effective_message) {
        Ok(step_prompt) => {
            let mut req = engine.build_request(effective_message, tools);
            req.platform_system_prompt = Some(step_prompt.system);
            req
        }
        Err(_) => {
            let mut req = engine.build_request(effective_message, tools);
            if let (Some(ms), Some(app)) = (&active_milestone, &app_config) {
                let sp = StepBuilder::build(ms, &context, effective_message, app);
                req.platform_system_prompt = Some(sp.system);
            }
            req
        }
    }
};
```

- [ ] **Step 4: Validate request_user_input ToolCallStart**

Inside `ToolCallStart` branch:

```rust
let validation = {
    let contract = milestone_for_check.as_ref().map(|m| m.contract.clone());
    contract.map(|c| ContractValidator::validate_tool_call(&c, &tool_name, &arguments))
};

if let Some(validation) = validation {
    if !validation.ok {
        let msg = format!("当前阶段输出不符合契约：{}", validation.issues.join("；"));
        return Ok(sse_err(msg));
    }
}
```

This prevents the frontend from showing invalid one-option choice cards.

- [ ] **Step 5: Validate final response before auto-advance**

Before milestone `should_advance`, call:

```rust
let contract_ok = ContractValidator::validate_response(&ms.contract, &text);
let should_advance = contract_ok.ok && match app.granularity.as_deref() { ... };
```

If invalid, return milestone event with `next_action = "Continue"` and include issues in SSE delta.

- [ ] **Step 6: Verify**

Run:

```bash
cargo test --manifest-path pinvou-platform/Cargo.toml
```

Expected: all tests pass.

---

### Task 8: Update App TOML Contracts

**Files:**
- Modify: `apps/计划敲定/app.toml`
- Modify: `apps/文档生成/app.toml`
- Modify: `apps/数据分析/app.toml`

- [ ] **Step 1: Add explicit planning config to each app**

For all three apps, add:

```toml
[app.planning]
mode = "dynamic_with_static_fallback"
confirm_dynamic_plan = false
max_plan_retries = 2
```

- [ ] **Step 2: Add 计划敲定 contracts**

For each milestone in `apps/计划敲定/app.toml`, add the contract fields from the design spec section 7.1.

- [ ] **Step 3: Add 文档生成 contracts**

Use:

```toml
contract_mode = "collect"          # requirement/material
contract_mode = "refine_selected_option" # draft/review
contract_mode = "final_output"     # finalize
```

Draft stage requirements:

```toml
output_requirements = ["no_open_question"]
advance_policy = "on_valid_output"
```

Finalize stage:

```toml
allowed_tools = ["file_write"]
output_requirements = ["no_open_question"]
advance_policy = "on_valid_output"
```

- [ ] **Step 4: Add 数据分析 contracts**

Use:

```toml
upload/explore: collect, question_budget = 1
analyze: refine_selected_option, question_budget = 0
visualize: refine_selected_option, question_budget = 0
export: final_output, question_budget = 0
```

Visualization may allow shell/python if these tools are actually implemented later; for current tool list, keep contract aligned with available tool names.

- [ ] **Step 5: Verify config loading**

Run:

```bash
cargo test --manifest-path pinvou-platform/Cargo.toml app::tests::test_load_milestones_from_toml
```

Expected: existing app load test still passes.

---

### Task 9: Update Documentation and Process State

**Files:**
- Modify: `process.md`
- Modify: `设计架构文档-pinvou3.md`

- [ ] **Step 1: Update process.md**

Move `decompose_and_execute 接入 web 流` out of “待接入” when implemented. Add:

```markdown
| Milestone Contract Runtime | app.toml 和动态拆解统一输出 contract，运行时按 contract 决策 |
| DynamicPlanner Web 接入 | Web 第一条用户消息优先生成动态计划，失败回退静态 contracts |
| ContractValidator | 工具调用和输出按阶段契约做硬边界检查 |
```

- [ ] **Step 2: Update architecture doc**

Add a new section after current 3.7:

```markdown
### 3.8 Milestone Contract Runtime

静态 app.toml 与动态拆解都输出 MilestoneContract。ContractRuntime 决定本轮动作，StepBuilder 只渲染 prompt，ContractValidator 负责硬边界。
```

- [ ] **Step 3: Verify docs mention no `/api/choice`**

Run:

```bash
rg -n "/api/choice" process.md 设计架构文档-pinvou3.md docs
```

Expected: no stale `/api/choice` references unless explicitly described as removed/stale.

---

### Task 10: Full Verification

**Files:**
- No source changes unless previous tasks fail.

- [ ] **Step 1: Format**

Run:

```bash
cargo fmt --manifest-path pinvou-platform/Cargo.toml
```

Expected: no output.

- [ ] **Step 2: Run all platform tests**

Run:

```bash
cargo test --manifest-path pinvou-platform/Cargo.toml
```

Expected: all tests pass.

- [ ] **Step 3: Manual acceptance scenario**

Run the app:

```bash
./run.sh
```

Use the test prompt:

```text
我周末去广州市黄埔区水生水库游玩徒步，你给我计划
```

Expected behavior:

1. 明确目标：最多一轮选择题。
2. 梳理约束：最多一轮选择题。
3. 方案对比：输出 2-3 个方案，并用 2-3 个选项让用户选择；不能只给“湖畔水库”一个选项。
4. 细化方案：直接细化已选方案，不能再问“你希望什么时候去”这类目标/约束问题。
5. 输出计划书：只输出最终 markdown 计划，不再发选择题。

---

## Self-Review

- Spec coverage: 本计划覆盖 contract model、runtime、validator、dynamic planner、StepBuilder、Engine、Web、app.toml、docs、验证。
- Placeholder scan: 无 TBD/TODO/占位任务；每个任务有明确文件、测试和期望结果。
- Type consistency: `PlanningConfig`、`MilestoneContract`、`TurnDirective`、`ContractPrompt`、`ValidationResult` 在任务间命名一致。
