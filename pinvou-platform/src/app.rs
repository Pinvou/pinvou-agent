//! 应用配置系统 — "应用即配置"，加新场景不需要写代码。
//!
//! 每个应用 = 一个目录 + 一个 app.toml + 一个 prompt.md。

#![allow(dead_code)] // Phase 1 定义，Phase 2 使用

use crate::contract::{
    AdvancePolicy, MilestoneContract, MilestoneMode, PlanningConfig, default_contract_for_label,
};
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// 应用配置（从 app.toml 加载）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// 应用唯一标识（留空则自动使用目录名）
    #[serde(default)]
    pub id: String,
    /// 显示名称
    pub name: String,
    /// 简短描述（显示在启动器列表中）
    pub description: String,
    /// 图标字符（终端兼容）
    #[serde(default = "default_icon")]
    pub icon: String,
    /// system prompt 文件路径（相对于 app 目录）
    pub prompt_file: Option<String>,
    /// 内联 system prompt（与 prompt_file 二选一）
    pub prompt: Option<String>,
    /// 推荐模型偏好: "small" / "medium" / "large"
    #[serde(default = "default_model_preference")]
    pub model_preference: String,
    /// 工具白名单
    #[serde(default)]
    pub tools: Vec<String>,
    /// 里程碑列表（侧边栏导航步骤）
    #[serde(default)]
    pub milestones: Vec<Milestone>,
    /// planning contract runtime 配置
    #[serde(default)]
    pub planning: PlanningConfig,
    /// 颗粒度: "fine"（每步停）/ "medium"（仅 confirm_at 停）/ "coarse"（全自动）
    #[serde(default)]
    pub granularity: Option<String>,
    /// 需要用户确认的里程碑 id 列表
    #[serde(default)]
    pub confirm_at: Option<Vec<String>>,
    /// 自定义禁止规则（追加到 StepBuilder 默认 ban_list 之后）
    #[serde(default)]
    pub ban_list: Vec<String>,
    /// 元数据
    #[serde(default)]
    pub meta: HashMap<String, String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            icon: default_icon(),
            prompt_file: None,
            prompt: None,
            model_preference: default_model_preference(),
            tools: Vec::new(),
            milestones: Vec::new(),
            planning: PlanningConfig::default(),
            granularity: None,
            confirm_at: None,
            ban_list: Vec::new(),
            meta: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Milestone {
    /// 唯一标识
    pub id: String,
    /// 显示标签
    pub label: String,
    /// 提示文本（AI 行为引导，注入到 prompt 中）
    #[serde(default)]
    pub prompt_hint: Option<String>,
    /// 图标
    #[serde(default)]
    pub icon: Option<String>,
    /// milestone contract runtime 配置
    #[serde(default)]
    pub contract: MilestoneContract,
    /// 兼容 app.toml 中历史/扁平 contract 字段
    #[serde(default, skip_serializing)]
    pub contract_mode: Option<MilestoneMode>,
    #[serde(default, skip_serializing)]
    pub question_budget: Option<u8>,
    #[serde(default, skip_serializing)]
    pub required_context: Vec<String>,
    #[serde(default, skip_serializing)]
    pub produced_context: Vec<String>,
    #[serde(default, skip_serializing)]
    pub allowed_tools: Vec<String>,
    #[serde(default, skip_serializing)]
    pub forbidden_tools: Vec<String>,
    #[serde(
        default,
        deserialize_with = "crate::contract::deserialize_output_requirements",
        skip_serializing
    )]
    pub output_requirements: Vec<crate::contract::OutputRequirement>,
    #[serde(default, skip_serializing)]
    pub advance_policy: Option<AdvancePolicy>,
}

impl Default for Milestone {
    fn default() -> Self {
        Self {
            id: String::new(),
            label: String::new(),
            prompt_hint: None,
            icon: None,
            contract: MilestoneContract::default(),
            contract_mode: None,
            question_budget: None,
            required_context: Vec::new(),
            produced_context: Vec::new(),
            allowed_tools: Vec::new(),
            forbidden_tools: Vec::new(),
            output_requirements: Vec::new(),
            advance_policy: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawMilestone {
    id: String,
    label: String,
    #[serde(default)]
    prompt_hint: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    contract: Option<MilestoneContract>,
    #[serde(default)]
    contract_mode: Option<MilestoneMode>,
    #[serde(default)]
    question_budget: Option<u8>,
    required_context: Option<Vec<String>>,
    produced_context: Option<Vec<String>>,
    allowed_tools: Option<Vec<String>>,
    forbidden_tools: Option<Vec<String>>,
    output_requirements: Option<Vec<crate::contract::OutputRequirement>>,
    #[serde(default)]
    advance_policy: Option<AdvancePolicy>,
}

#[derive(Debug, Deserialize)]
struct RawAppConfig {
    #[serde(default)]
    id: String,
    name: String,
    description: String,
    #[serde(default = "default_icon")]
    icon: String,
    prompt_file: Option<String>,
    prompt: Option<String>,
    #[serde(default = "default_model_preference")]
    model_preference: String,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    milestones: Vec<RawMilestone>,
    #[serde(default)]
    planning: PlanningConfig,
    #[serde(default)]
    granularity: Option<String>,
    #[serde(default)]
    confirm_at: Option<Vec<String>>,
    #[serde(default)]
    ban_list: Vec<String>,
    #[serde(default)]
    meta: HashMap<String, String>,
}

impl From<RawAppConfig> for AppConfig {
    fn from(raw: RawAppConfig) -> Self {
        Self {
            id: raw.id,
            name: raw.name,
            description: raw.description,
            icon: raw.icon,
            prompt_file: raw.prompt_file,
            prompt: raw.prompt,
            model_preference: raw.model_preference,
            tools: raw.tools,
            milestones: raw
                .milestones
                .into_iter()
                .map(normalize_raw_milestone)
                .collect(),
            planning: raw.planning,
            granularity: raw.granularity,
            confirm_at: raw.confirm_at,
            ban_list: raw.ban_list,
            meta: raw.meta,
        }
    }
}

fn default_icon() -> String {
    "📋".to_string()
}

fn default_model_preference() -> String {
    "medium".to_string()
}

/// 应用注册表 — 从 apps/ 目录加载所有应用
#[derive(Debug, Default)]
pub struct AppRegistry {
    apps: Vec<AppConfig>,
    /// 应用内 prompt 缓存
    prompts: HashMap<String, String>,
}

impl AppRegistry {
    /// 从应用目录加载所有应用
    pub fn load(apps_dir: &Path) -> Result<Self> {
        let mut registry = Self::default();

        if !apps_dir.is_dir() {
            return Ok(registry); // 目录不存在则空
        }

        for entry in std::fs::read_dir(apps_dir)? {
            let entry = entry?;
            let app_dir = entry.path();
            if !app_dir.is_dir() {
                continue;
            }

            let config_path = app_dir.join("app.toml");
            if !config_path.exists() {
                continue;
            }

            let config_str = std::fs::read_to_string(&config_path)
                .with_context(|| format!("无法读取 {:?}", config_path))?;

            // toml 格式为 [app] 段 + 根级 [[milestones]] 数组
            #[derive(Deserialize)]
            struct AppToml {
                app: RawAppConfig,
                #[serde(default)]
                milestones: Vec<RawMilestone>,
            }
            let app_toml: AppToml = toml::from_str(&config_str)
                .map_err(|err| anyhow!("解析失败: {:?}: {err}", config_path))?;
            let mut config = AppConfig::from(app_toml.app);
            // 合并根级别的 [[milestones]] 到 app 配置
            if !app_toml.milestones.is_empty() {
                config.milestones = app_toml
                    .milestones
                    .into_iter()
                    .map(normalize_raw_milestone)
                    .collect();
            }

            // 用目录名作为 id
            if config.id.is_empty() {
                config.id = app_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
            }

            // 加载 prompt 文件
            if let Some(ref prompt_file) = config.prompt_file {
                let prompt_path = app_dir.join(prompt_file);
                if prompt_path.exists() {
                    let prompt = std::fs::read_to_string(&prompt_path)?;
                    registry.prompts.insert(config.id.clone(), prompt);
                }
            }

            registry.apps.push(config);
        }

        // 默认排序
        registry.apps.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(registry)
    }

    /// 列出所有应用
    pub fn list(&self) -> &[AppConfig] {
        &self.apps
    }

    /// 按 ID 查找应用
    pub fn find(&self, id: &str) -> Option<&AppConfig> {
        self.apps.iter().find(|a| a.id == id)
    }

    /// 获取应用的完整 system prompt
    pub fn get_prompt(&self, app_id: &str) -> Option<&str> {
        self.prompts.get(app_id).map(|s| s.as_str())
    }

    /// 获取内联 prompt
    pub fn get_inline_prompt(&self, app_id: &str) -> Option<&str> {
        self.find(app_id).and_then(|a| a.prompt.as_deref())
    }

    /// 获取应用的完整 prompt（优先 prompt_file，fallback 到 inline）
    pub fn resolve_prompt(&self, app_id: &str) -> Option<String> {
        if let Some(p) = self.get_prompt(app_id) {
            return Some(p.to_string());
        }
        self.get_inline_prompt(app_id).map(|s| s.to_string())
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.apps.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.apps.is_empty()
    }
}

fn normalize_raw_milestone(raw: RawMilestone) -> Milestone {
    let mut contract = raw
        .contract
        .clone()
        .unwrap_or_else(|| default_contract_for_label(&raw.label));

    if let Some(mode) = raw.contract_mode.clone() {
        contract.mode = mode;
    }
    if let Some(question_budget) = raw.question_budget {
        contract.question_budget = question_budget;
    }
    if let Some(required_context) = raw.required_context.clone() {
        contract.required_context = required_context;
    }
    if let Some(produced_context) = raw.produced_context.clone() {
        contract.produced_context = produced_context;
    }
    if let Some(allowed_tools) = raw.allowed_tools.clone() {
        contract.allowed_tools = allowed_tools;
    }
    if let Some(forbidden_tools) = raw.forbidden_tools.clone() {
        contract.forbidden_tools = forbidden_tools;
    }
    if let Some(output_requirements) = raw.output_requirements.clone() {
        contract.output_requirements = output_requirements;
    }
    if let Some(advance_policy) = raw.advance_policy.clone() {
        contract.advance_policy = advance_policy;
    }

    Milestone {
        id: raw.id,
        label: raw.label,
        prompt_hint: raw.prompt_hint,
        icon: raw.icon,
        contract,
        contract_mode: raw.contract_mode,
        question_budget: raw.question_budget,
        required_context: raw.required_context.unwrap_or_default(),
        produced_context: raw.produced_context.unwrap_or_default(),
        allowed_tools: raw.allowed_tools.unwrap_or_default(),
        forbidden_tools: raw.forbidden_tools.unwrap_or_default(),
        output_requirements: raw.output_requirements.unwrap_or_default(),
        advance_policy: raw.advance_policy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{AdvancePolicy, MilestoneMode, OutputRequirement, PlanningMode};
    use crate::workflow::{ConversationState, MilestoneStatus};

    #[test]
    fn test_load_milestones_from_toml() {
        let registry = AppRegistry::load(Path::new("../apps")).unwrap();
        // 计划敲定 应该有 5 个里程碑
        let app = registry.find("计划敲定").expect("计划敲定 app not found");
        assert_eq!(
            app.milestones.len(),
            5,
            "计划敲定 should have 5 milestones, got {:?}",
            app.milestones
        );
        assert_eq!(app.milestones[0].id, "goal");
        assert_eq!(app.milestones[0].label, "明确目标");
        assert_eq!(app.milestones[4].id, "output");

        // 文档生成 应该有 4 个里程碑
        let doc = registry.find("文档生成").expect("文档生成 app not found");
        assert_eq!(
            doc.milestones.len(),
            5,
            "文档生成 should have 5 milestones, got {:?}",
            doc.milestones
        );
    }

    #[test]
    fn test_legacy_milestone_gets_default_contract() {
        let root =
            std::env::temp_dir().join(format!("pinvou-contract-legacy-{}", std::process::id()));
        let app_dir = root.join("计划敲定");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(app_dir.join("prompt.md"), "prompt").unwrap();
        std::fs::write(
            app_dir.join("app.toml"),
            r#"
[app]
name = "计划敲定"
description = "测试"
prompt_file = "prompt.md"

[[milestones]]
id = "goal"
label = "明确目标"
"#,
        )
        .unwrap();

        let registry = AppRegistry::load(&root).unwrap();
        let app = registry.find("计划敲定").unwrap();
        assert_eq!(app.planning.mode, PlanningMode::DynamicWithStaticFallback);
        assert_eq!(app.milestones[0].contract.mode, MilestoneMode::Collect);
        assert_eq!(app.milestones[0].contract.question_budget, 1);
    }

    #[test]
    fn test_parse_explicit_milestone_contract() {
        let root =
            std::env::temp_dir().join(format!("pinvou-contract-explicit-{}", std::process::id()));
        let app_dir = root.join("计划敲定");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(app_dir.join("prompt.md"), "prompt").unwrap();
        std::fs::write(
            app_dir.join("app.toml"),
            r#"
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
"#,
        )
        .unwrap();

        let registry = AppRegistry::load(&root).unwrap();
        let app = registry.find("计划敲定").unwrap();
        assert_eq!(app.planning.mode, PlanningMode::StaticOnly);
        let contract = &app.milestones[0].contract;
        assert_eq!(contract.mode, MilestoneMode::ProduceOptions);
        assert_eq!(contract.required_context, vec!["goal", "budget"]);
        assert!(
            contract
                .output_requirements
                .contains(&OutputRequirement::MustContainTable)
        );
        assert!(
            contract
                .output_requirements
                .contains(&OutputRequirement::MinOptions(2))
        );
    }

    #[test]
    fn test_output_requirement_json_roundtrip_in_conversation_state() {
        let milestone = Milestone {
            id: "options".into(),
            label: "方案对比".into(),
            contract: MilestoneContract {
                output_requirements: vec![
                    OutputRequirement::MinOptions(2),
                    OutputRequirement::MustContainTable,
                ],
                ..MilestoneContract::default()
            },
            ..Default::default()
        };
        let state = ConversationState {
            app_id: "计划敲定".into(),
            milestones: vec![(milestone, MilestoneStatus::Active)],
            context: HashMap::new(),
            plan_initialized: false,
            question_counts: HashMap::new(),
            turn_count: 0,
            current_phase: None,
        };

        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"min_options:2\""));
        assert!(json.contains("\"must_contain_table\""));
        let restored: ConversationState = serde_json::from_str(&json).unwrap();
        let requirements = &restored.milestones[0].0.contract.output_requirements;
        assert_eq!(
            requirements,
            &vec![
                OutputRequirement::MinOptions(2),
                OutputRequirement::MustContainTable,
            ]
        );
    }

    #[test]
    fn test_explicit_default_contract_is_not_label_defaulted() {
        let root = std::env::temp_dir().join(format!(
            "pinvou-contract-explicit-default-{}",
            std::process::id()
        ));
        let app_dir = root.join("计划敲定");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(app_dir.join("prompt.md"), "prompt").unwrap();
        std::fs::write(
            app_dir.join("app.toml"),
            r#"
[app]
name = "计划敲定"
description = "测试"
prompt_file = "prompt.md"

[[milestones]]
id = "options"
label = "方案对比"

[milestones.contract]
mode = "collect"
advance_policy = "manual_continue"
"#,
        )
        .unwrap();

        let registry = AppRegistry::load(&root).unwrap();
        let app = registry.find("计划敲定").unwrap();
        let contract = &app.milestones[0].contract;
        assert_eq!(contract.mode, MilestoneMode::Collect);
        assert_eq!(contract.advance_policy, AdvancePolicy::ManualContinue);
    }

    #[test]
    fn test_explicit_empty_flat_arrays_clear_label_defaults() {
        let root =
            std::env::temp_dir().join(format!("pinvou-contract-empty-flat-{}", std::process::id()));
        let app_dir = root.join("计划敲定");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(app_dir.join("prompt.md"), "prompt").unwrap();
        std::fs::write(
            app_dir.join("app.toml"),
            r#"
[app]
name = "计划敲定"
description = "测试"
prompt_file = "prompt.md"

[[milestones]]
id = "options"
label = "方案对比"
allowed_tools = []
output_requirements = []
"#,
        )
        .unwrap();

        let registry = AppRegistry::load(&root).unwrap();
        let app = registry.find("计划敲定").unwrap();
        let contract = &app.milestones[0].contract;
        assert!(contract.allowed_tools.is_empty());
        assert!(contract.output_requirements.is_empty());
    }

    #[test]
    fn test_app_level_explicit_default_contract_is_not_label_defaulted() {
        let root = std::env::temp_dir().join(format!(
            "pinvou-contract-app-level-explicit-default-{}",
            std::process::id()
        ));
        let app_dir = root.join("计划敲定");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(app_dir.join("prompt.md"), "prompt").unwrap();
        std::fs::write(
            app_dir.join("app.toml"),
            r#"
[app]
name = "计划敲定"
description = "测试"
prompt_file = "prompt.md"

[[app.milestones]]
id = "options"
label = "方案对比"

[app.milestones.contract]
mode = "collect"
advance_policy = "manual_continue"
"#,
        )
        .unwrap();

        let registry = AppRegistry::load(&root).unwrap();
        let app = registry.find("计划敲定").unwrap();
        let contract = &app.milestones[0].contract;
        assert_eq!(contract.mode, MilestoneMode::Collect);
        assert_eq!(contract.advance_policy, AdvancePolicy::ManualContinue);
    }

    #[test]
    fn test_app_level_empty_flat_arrays_clear_label_defaults() {
        let root = std::env::temp_dir().join(format!(
            "pinvou-contract-app-level-empty-flat-{}",
            std::process::id()
        ));
        let app_dir = root.join("计划敲定");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(app_dir.join("prompt.md"), "prompt").unwrap();
        std::fs::write(
            app_dir.join("app.toml"),
            r#"
[app]
name = "计划敲定"
description = "测试"
prompt_file = "prompt.md"

[[app.milestones]]
id = "options"
label = "方案对比"
allowed_tools = []
output_requirements = []
"#,
        )
        .unwrap();

        let registry = AppRegistry::load(&root).unwrap();
        let app = registry.find("计划敲定").unwrap();
        let contract = &app.milestones[0].contract;
        assert!(contract.allowed_tools.is_empty());
        assert!(contract.output_requirements.is_empty());
    }

    #[test]
    fn test_unknown_output_requirement_returns_load_error() {
        let root = std::env::temp_dir().join(format!("pinvou-contract-bad-{}", std::process::id()));
        let app_dir = root.join("计划敲定");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(app_dir.join("prompt.md"), "prompt").unwrap();
        std::fs::write(
            app_dir.join("app.toml"),
            r#"
[app]
name = "计划敲定"
description = "测试"
prompt_file = "prompt.md"

[[milestones]]
id = "options"
label = "方案对比"
output_requirements = ["bad_requirement"]
"#,
        )
        .unwrap();

        let err = AppRegistry::load(&root).unwrap_err();
        assert!(err.to_string().contains("bad_requirement"));
    }

    #[test]
    fn test_runtime_milestone_serialization_omits_flat_compat_fields() {
        let milestone = Milestone {
            id: "options".into(),
            label: "方案对比".into(),
            contract_mode: Some(MilestoneMode::Collect),
            allowed_tools: vec!["request_user_input".into()],
            output_requirements: vec![OutputRequirement::MustContainTable],
            ..Default::default()
        };

        let json = serde_json::to_string(&milestone).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let object = value.as_object().unwrap();
        assert!(json.contains("\"contract\""));
        assert!(!object.contains_key("contract_mode"));
        assert!(!object.contains_key("allowed_tools"));
        assert!(!object.contains_key("output_requirements"));
    }
}
