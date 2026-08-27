//! 连接器编排注册表（阶段 3a）：8 个通用命令按 id 分派——内置 4 连接器路由到
//! `features/connectors/<id>.rs` 的既有实现（编排逻辑零改动，仅出口事件已换
//! 统一契约），声明式 Upload 包路由到契约驱动通用编排器（[`super::declared`]）。
//! 未知 id → Err（与旧硬编码命令的「不存在该命令」同语义，显式报错）。

use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::features::connectors::{declared, dingtalk, feishu, tmeet, wecom};
use crate::features::marketplace::bundle::upload_cli_connector_decl;
use crate::features::marketplace::plugin_import::CliConnectorDecl;

/// 内置连接器 id（硬编码路由白名单，行为锁定）。
pub(crate) fn is_builtin_connector(id: &str) -> bool {
    matches!(id, "feishu" | "wecom" | "dingtalk" | "tmeet")
}

/// 取声明式包声明；非声明式 id 报错（内置/未知在此分流前的兜底）。
fn declared_decl(id: &str) -> Result<CliConnectorDecl, String> {
    upload_cli_connector_decl(id)
        .map(|(_, decl)| decl)
        .ok_or_else(|| format!("未知连接器 '{id}'"))
}

/// 展示名（ensure_cli 安装遥测用）：内置取中文名，声明式包取 plugin.json name。
pub(crate) fn display_name(id: &str) -> String {
    match id {
        "feishu" => "飞书".to_string(),
        "wecom" => "企业微信".to_string(),
        "dingtalk" => "钉钉".to_string(),
        "tmeet" => "腾讯会议".to_string(),
        _ => upload_cli_connector_decl(id)
            .map(|(plugin, _)| plugin.name)
            .unwrap_or_else(|| id.to_string()),
    }
}

pub async fn ensure_cli(id: &str) -> Result<Value, String> {
    match id {
        "feishu" => feishu::feishu_ensure_cli().await,
        "wecom" => wecom::wecom_ensure_cli().await,
        "dingtalk" => dingtalk::dingtalk_ensure_cli().await,
        "tmeet" => tmeet::tmeet_ensure_cli().await,
        _ => declared::declared_ensure_cli(id, &declared_decl(id)?).await,
    }
}

pub async fn status(id: &str) -> Result<Value, String> {
    match id {
        "feishu" => feishu::feishu_status().await,
        "wecom" => wecom::wecom_status().await,
        "dingtalk" => dingtalk::dingtalk_status().await,
        "tmeet" => tmeet::tmeet_status().await,
        _ => declared::declared_status(id, &declared_decl(id)?).await,
    }
}

pub async fn connect_begin(app: &AppHandle, id: &str) -> Result<Value, String> {
    match id {
        "feishu" => feishu::feishu_connect_begin(app.clone()).await,
        "wecom" => wecom::wecom_connect_begin(app.clone()).await,
        "dingtalk" => dingtalk::dingtalk_connect_begin(app.clone()).await,
        "tmeet" => tmeet::tmeet_connect_begin(app.clone()).await,
        _ => declared::declared_connect_begin(app, id, &declared_decl(id)?).await,
    }
}

pub async fn cancel(app: &AppHandle, id: &str) -> Result<Value, String> {
    match id {
        "feishu" => feishu::feishu_cancel(app.clone()).await,
        "wecom" => wecom::wecom_cancel(app.clone()).await,
        "dingtalk" => dingtalk::dingtalk_cancel(app.clone()).await,
        "tmeet" => tmeet::tmeet_cancel(app.clone()).await,
        // 声明式包与内置共用 ConnectorConn 槽位（通用编排器同样登记长驻 pid）
        _ => {
            let pid = app
                .state::<crate::features::connectors::connector_cli::ConnectorConn>()
                .cancel(id);
            if let Some(pid) = pid {
                let _ = tokio::task::spawn_blocking(move || {
                    crate::features::connectors::connector_cli::kill_pid_tree(pid);
                })
                .await;
            }
            Ok(serde_json::json!({ "ok": true }))
        }
    }
}

pub async fn logout(id: &str) -> Result<Value, String> {
    match id {
        "feishu" => feishu::feishu_logout().await,
        "wecom" => wecom::wecom_logout().await,
        "dingtalk" => dingtalk::dingtalk_logout().await,
        "tmeet" => tmeet::tmeet_logout().await,
        _ => declared::declared_logout(id, &declared_decl(id)?).await,
    }
}

pub async fn apply_skills(id: &str) -> Result<Value, String> {
    match id {
        "feishu" => feishu::feishu_apply_skills().await,
        "wecom" => wecom::wecom_apply_skills().await,
        "dingtalk" => dingtalk::dingtalk_apply_skills().await,
        "tmeet" => tmeet::tmeet_apply_skills().await,
        _ => declared::declared_apply_skills(id).await,
    }
}

pub async fn set_enabled(id: &str, enabled: bool) -> Result<Value, String> {
    match id {
        "feishu" => feishu::set_feishu_enabled(enabled).await,
        "wecom" => wecom::set_wecom_enabled(enabled).await,
        "dingtalk" => dingtalk::set_dingtalk_enabled(enabled).await,
        "tmeet" => tmeet::set_tmeet_enabled(enabled).await,
        _ => declared::declared_set_enabled(id, enabled).await,
    }
}

pub async fn skills_state(id: &str) -> Result<Value, String> {
    match id {
        "feishu" => feishu::feishu_skills_state().await,
        "wecom" => wecom::wecom_skills_state().await,
        "dingtalk" => dingtalk::dingtalk_skills_state().await,
        "tmeet" => tmeet::tmeet_skills_state().await,
        _ => declared::declared_skills_state(id, &declared_decl(id)?).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 分派白名单：内置 4 id 命中，声明式/未知不命中（未知 id 走 declared_decl
    /// 报「未知连接器」）。
    #[test]
    fn builtin_connector_whitelist() {
        for id in ["feishu", "wecom", "dingtalk", "tmeet"] {
            assert!(is_builtin_connector(id), "{id} 应命中内置白名单");
            assert!(!display_name(id).is_empty());
        }
        for id in ["up-cli", "feishu2", "", "FEISHU"] {
            assert!(!is_builtin_connector(id), "{id} 不应命中");
        }
    }

    /// 声明式包名展示：盘上 plugin.json 的 name 优先，未知 id 回退 id 本身。
    #[test]
    fn display_name_prefers_declared_plugin_name() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-registry-test-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let pkg = dir.join("bundles/up-cli");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("plugin.json"),
            r#"{"manifest_version":1,"id":"up-cli","name":"上行 CLI","components":{"cli_connectors":[{"id":"up-cli","bin":"up-cli-bin"}]}}"#,
        )
        .unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);

        assert_eq!(display_name("up-cli"), "上行 CLI");
        assert_eq!(display_name("no-such"), "no-such");
        assert!(declared_decl("no-such").is_err());
        // 内置 id 不走盘上反查（即便盘上恰有同名声明）
        assert!(declared_decl("feishu").is_err());

        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
