use std::path::PathBuf;

use serde::Deserialize;

use crate::platform::paths;

#[derive(Debug, Clone)]
pub struct WorkflowInfo {
    pub id: String,
    pub name: Option<String>,
    pub enabled: bool,
    pub scenarios: Vec<String>,
    pub dir: PathBuf,
    pub ui: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct WorkflowJson {
    id: Option<String>,
    name: Option<String>,
    #[serde(default = "default_enabled")]
    enabled: bool,
    #[serde(default)]
    scenarios: Vec<String>,
    #[serde(default)]
    ui: serde_json::Value,
}

fn default_enabled() -> bool {
    true
}

pub fn discover() -> Vec<WorkflowInfo> {
    let workflow_root = paths::bundle_workflow_dir();
    let Ok(entries) = std::fs::read_dir(workflow_root) else {
        return Vec::new();
    };

    let mut workflows = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }

        let workflow_json = dir.join("workflow.json");
        if !workflow_json.is_file() {
            continue;
        }

        let Ok(content) = std::fs::read_to_string(&workflow_json) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<WorkflowJson>(&content) else {
            continue;
        };
        let Some(id) = parsed.id else {
            continue;
        };

        workflows.push(WorkflowInfo {
            id,
            name: parsed.name,
            enabled: parsed.enabled,
            scenarios: parsed.scenarios,
            dir,
            ui: parsed.ui,
        });
    }
    workflows
}

/// scenario → 工作流。**不过滤 enabled**:禁用只挡"新建"入口(由创建方检查
/// `wf.enabled`),已有项目/历史 run 的路径解析、查看不能因禁用而失效。
pub fn by_scenario(scenario: &str) -> Option<WorkflowInfo> {
    discover()
        .into_iter()
        .find(|workflow| workflow.scenarios.iter().any(|s| s == scenario))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_smoke() {
        let _ = discover();
    }
}
