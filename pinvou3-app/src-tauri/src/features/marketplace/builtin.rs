//! 内置插件框架（内置工具集长期契约 §3.1/§3.3）。
//!
//! 内置插件（manifest `builtin: true`）随应用发布：**不可卸载、不可停用**（服务端
//! 纵深防御——前端不下发动作只是体验层，命令/IPC 直达必须在后端被拒），数据安全
//! 等级与数据访问 scope 经 `MarketplaceToolInfo` 透传给前端本地化展示。
//!
//! 功能级开关（§3.3）：内置插件的工具按 `tool_features`（工具全名 → 功能 id 数组）
//! 归属到可独立开关的功能（如 session-reader 的 read_session/list_sessions 同时服务
//! 「引用对话 session-mention」与「超长记忆 long-memory」）。开关状态持久化在
//! `settings.json` 的 `UserPrefs::disabled_builtin_features`，并同步写
//! `~/.pinvou3/marketplace/builtin_features.json`（`{"schema_version":1,
//! "disabled_features":[...]}`，原子写；文件缺失 = 全部启用）供 MCP server 进程读取。
//! 工具摘除取**并集语义**：某工具仅当其 `tool_features` 列出的功能**全部**被关闭时
//! 才从注册表摘除（见 [`feature_disabled_tool_names`]，并入
//! `super::unavailable_tool_names_for`）；`tool_features` 未列出的工具不受功能开关影响。

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::mcp_catalog;
use crate::platform::paths;
use crate::platform::prefs::UserPrefs;

/// 功能开关状态文件（MCP server 进程侧读取；缺失 = 全部启用）。
const BUILTIN_FEATURES_STATE_FILE: &str = "builtin_features.json";

/// 内置判定：优先编译期内嵌 catalog（只读快照，不信任用户目录同名 manifest），
/// 回读已释放的 `bundles/<id>/mcp/manifest.json`（自定义/上传包路径）；解析失败按
/// 非内置处理（宁可放行普通插件的卸载，不误锁）。
pub fn is_builtin_tool(id: &str) -> bool {
    if let Ok(Some(manifest)) = mcp_catalog::embedded_manifest(id) {
        return manifest.builtin;
    }
    let path = mcp_catalog::package_mcp_dir(id).join("manifest.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<super::types::ToolManifest>(&content).ok())
        .map(|manifest| manifest.builtin)
        .unwrap_or(false)
}

/// 写入禁用/隐藏列表前的内置校验（契约 §3.3：内置插件不可停用/不可隐藏）。
/// 含内置 id 即整个写入报错（不是静默过滤——静默过滤会让前端以为开关已生效）。
pub fn reject_builtin_ids(ids: &[String]) -> Result<(), String> {
    let builtin: Vec<&str> = ids
        .iter()
        .filter(|id| is_builtin_tool(id))
        .map(String::as_str)
        .collect();
    if builtin.is_empty() {
        return Ok(());
    }
    Err(format!(
        "builtin plugins cannot be disabled or hidden: {}",
        builtin.join(", ")
    ))
}

/// 功能注册表条目：一个可开关的内置功能及其来源。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuiltinFeature {
    /// 功能 id（如 "session-mention"）。
    pub id: String,
    /// 声明该功能的内置插件 id（去重、字典序）。
    pub plugins: Vec<String>,
    /// 归属该功能的工具全名（去重、字典序）。
    pub tools: Vec<String>,
    /// 当前开关状态（不在 `disabled_builtin_features` 即启用）。
    pub enabled: bool,
}

/// 扫描内嵌 catalog 中所有内置 manifest 的 `tool_features`（功能 id → 工具/插件），
/// 聚合成功能注册表（按功能 id 字典序）；enabled 由
/// `UserPrefs::disabled_builtin_features` 判定。catalog manifest 解析失败跳过该包
/// （与 `available_tools` 同口径，不让一个坏包拖垮整个注册表）。
pub fn feature_registry() -> Vec<BuiltinFeature> {
    let disabled = disabled_feature_ids();
    feature_registry_with_disabled(&disabled)
}

/// 注册表聚合的纯函数部分（禁用集由调用方给），供 `feature_registry` 与持锁写方
/// （状态落盘后现算返回）共用同一口径。
fn feature_registry_with_disabled(disabled: &BTreeSet<String>) -> Vec<BuiltinFeature> {
    // 功能 id → (插件 id 集, 工具全名集)；BTree* 保证输出确定性。
    let mut by_feature: BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)> = BTreeMap::new();
    for manifest in embedded_builtin_manifests() {
        for (tool, features) in &manifest.tool_features {
            for feature in features {
                let (plugins, tools) = by_feature.entry(feature.clone()).or_default();
                plugins.insert(manifest.id.clone());
                tools.insert(tool.clone());
            }
        }
    }
    by_feature
        .into_iter()
        .map(|(id, (plugins, tools))| BuiltinFeature {
            enabled: !disabled.contains(&id),
            id,
            plugins: plugins.into_iter().collect(),
            tools: tools.into_iter().collect(),
        })
        .collect()
}

/// 内嵌 catalog 中所有 `builtin: true` 的 manifest（解析失败跳过）。
fn embedded_builtin_manifests() -> Vec<super::types::ToolManifest> {
    mcp_catalog::MCP_PACKAGES
        .iter()
        .filter_map(|spec| {
            serde_json::from_str::<super::types::ToolManifest>(spec.manifest_json)
                .map_err(|e| {
                    eprintln!("[builtin] 内嵌 manifest 解析失败（{}）: {e}", spec.id);
                    e
                })
                .ok()
        })
        .filter(|manifest| manifest.builtin)
        .collect()
}

/// 当前被关闭的功能 id 集（settings.json；读不到 = 全部启用）。
fn disabled_feature_ids() -> BTreeSet<String> {
    UserPrefs::load()
        .disabled_builtin_features
        .into_iter()
        .collect()
}

/// 功能开关（契约 §3.3）：写 `UserPrefs::disabled_builtin_features`（字段级事务），
/// 随后原子写状态文件供 MCP server 读取，返回落盘后的最新注册表。
/// 未知功能 id 报错（前端笔误直接失败，不静默落盘）。
pub fn set_feature_enabled(id: &str, enabled: bool) -> Result<Vec<BuiltinFeature>, String> {
    let known: BTreeSet<String> = feature_registry_with_disabled(&BTreeSet::new())
        .into_iter()
        .map(|feature| feature.id)
        .collect();
    if !known.contains(id) {
        return Err(format!(
            "unknown builtin feature '{id}'; known features: {}",
            known.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    let prefs = UserPrefs::update_transaction(|prefs| {
        if enabled {
            prefs.disabled_builtin_features.retain(|f| f != id);
        } else if !prefs.disabled_builtin_features.iter().any(|f| f == id) {
            prefs.disabled_builtin_features.push(id.to_string());
        }
        Ok(())
    })?;
    let disabled: BTreeSet<String> = prefs.disabled_builtin_features.into_iter().collect();
    write_feature_state_file(&disabled)?;
    Ok(feature_registry_with_disabled(&disabled))
}

/// 原子写 `~/.pinvou3/marketplace/builtin_features.json`（tmp + rename，
/// `platform::filesystem::atomic_write`）。全启用时也写空数组（状态文件始终反映
/// 最新开关态，避免 MCP server 读到过期名单）。
fn write_feature_state_file(disabled: &BTreeSet<String>) -> Result<(), String> {
    let dir = paths::pinvou3_home().join("marketplace");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create marketplace dir failed: {e}"))?;
    let payload = serde_json::json!({
        "schema_version": 1,
        "disabled_features": disabled.iter().collect::<Vec<_>>(),
    });
    let content = serde_json::to_vec(&payload)
        .map_err(|e| format!("serialize builtin feature state failed: {e}"))?;
    crate::platform::filesystem::atomic_write(&dir.join(BUILTIN_FEATURES_STATE_FILE), &content)
        .map_err(|e| format!("write builtin feature state failed: {e}"))
}

/// 功能开关应摘除的模型可见工具全名（喂给引擎 disallowed_tools）。
///
/// 并集语义：某工具仅当其 `tool_features` 列出的功能**全部**被关闭时才摘除
/// （一个功能还开着，工具就还在）；`tool_features` 为空/未列出的工具不受影响。
/// 输出小写（引擎 `command_denies_tool` 按小写精确匹配，同 `model_tool_names`）。
pub fn feature_disabled_tool_names() -> Vec<String> {
    let disabled = disabled_feature_ids();
    if disabled.is_empty() {
        return Vec::new();
    }
    let mut names: BTreeSet<String> = BTreeSet::new();
    for manifest in embedded_builtin_manifests() {
        for (tool, features) in &manifest.tool_features {
            if !features.is_empty() && features.iter().all(|f| disabled.contains(f)) {
                names.insert(tool.to_ascii_lowercase());
            }
        }
    }
    names.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把 PINVOU3_HOME 指到干净临时目录跑闭包，跑完恢复并清理。
    /// 借 `platform::paths::tests::ENV_LOCK` 与 prefs/store 等 mutate
    /// PINVOU3_HOME 的测试串行（同一环境变量，必须同一把锁）。
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-builtin-test-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        f();
        match prev {
            // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_reader_is_builtin_via_embedded_manifest() {
        assert!(is_builtin_tool("session-reader"));
        // 内嵌目录里的普通预置包不是内置插件。
        assert!(!is_builtin_tool("weather"));
        // 未知 id（盘上也没有）按非内置。
        assert!(!is_builtin_tool("no-such-tool"));
    }

    /// 内嵌目录缺失时回读已释放的 bundles/<id>/mcp/manifest.json。
    #[test]
    fn released_manifest_marks_custom_bundle_builtin() {
        with_temp_home(|| {
            let mcp_dir = mcp_catalog::package_mcp_dir("custom-builtin");
            std::fs::create_dir_all(&mcp_dir).unwrap();
            std::fs::write(
                mcp_dir.join("manifest.json"),
                r#"{"id":"custom-builtin","name":"c","description":"d","version":"1","icon":"x","category":"c","mcp_tools":[],"command":"python","args":[],"builtin":true}"#,
            )
            .unwrap();
            assert!(is_builtin_tool("custom-builtin"));
        });
    }

    #[test]
    fn reject_builtin_ids_lists_offenders() {
        assert!(reject_builtin_ids(&["weather".to_string()]).is_ok());
        let err =
            reject_builtin_ids(&["weather".to_string(), "session-reader".to_string()]).unwrap_err();
        assert!(err.contains("session-reader"), "错误应点名内置 id: {err}");
        assert!(!err.contains("weather"), "普通插件不应被点名: {err}");
    }

    /// 功能注册表：session-reader 的 tool_features 聚合出 session-mention /
    /// long-memory，插件与工具归属正确，缺省全部启用。
    #[test]
    fn registry_aggregates_session_reader_features() {
        with_temp_home(|| {
            let registry = feature_registry();
            let ids: Vec<&str> = registry.iter().map(|f| f.id.as_str()).collect();
            assert_eq!(ids, ["long-memory", "session-mention"], "字典序输出");
            for feature in &registry {
                assert_eq!(feature.plugins, ["session-reader".to_string()]);
                assert_eq!(
                    feature.tools,
                    [
                        "mcp_session-reader_list_sessions".to_string(),
                        "mcp_session-reader_read_session".to_string()
                    ]
                );
                assert!(feature.enabled, "缺省（无状态）全部启用");
            }
        });
    }

    /// 并集语义：两个功能只关一个 → 工具不摘除；全关 → 两个工具都摘除。
    #[test]
    fn union_semantics_gate_tool_removal() {
        with_temp_home(|| {
            // 全部启用：无摘除。
            assert!(feature_disabled_tool_names().is_empty());

            // 只关 session-mention（long-memory 还开着）：read_session 不摘除。
            set_feature_enabled("session-mention", false).unwrap();
            assert!(
                feature_disabled_tool_names().is_empty(),
                "long-memory 仍启用，两个工具都不应摘除"
            );

            // 两个都关：read_session / list_sessions 都摘除。
            set_feature_enabled("long-memory", false).unwrap();
            assert_eq!(
                feature_disabled_tool_names(),
                [
                    "mcp_session-reader_list_sessions".to_string(),
                    "mcp_session-reader_read_session".to_string()
                ]
            );

            // 重新打开一个：摘除解除。
            set_feature_enabled("long-memory", true).unwrap();
            assert!(feature_disabled_tool_names().is_empty());
        });
    }

    /// 不在任何 tool_features 里的工具不受功能开关影响（weather 全关场景下仍在）。
    #[test]
    fn tools_outside_tool_features_are_unaffected() {
        with_temp_home(|| {
            set_feature_enabled("session-mention", false).unwrap();
            set_feature_enabled("long-memory", false).unwrap();
            let removed = feature_disabled_tool_names();
            assert!(
                !removed.iter().any(|n| n.contains("weather")),
                "未列入 tool_features 的工具不得被功能开关摘除: {removed:?}"
            );
        });
    }

    /// 开关持久化：settings.json 的 disabled_builtin_features 与状态文件
    /// builtin_features.json（schema_version + disabled_features）内容正确。
    #[test]
    fn set_feature_enabled_persists_prefs_and_state_file() {
        with_temp_home(|| {
            let registry = set_feature_enabled("session-mention", false).unwrap();
            let mention = registry.iter().find(|f| f.id == "session-mention").unwrap();
            assert!(!mention.enabled);
            let long_memory = registry.iter().find(|f| f.id == "long-memory").unwrap();
            assert!(long_memory.enabled);

            // settings.json 持久化。
            let prefs = UserPrefs::load();
            assert_eq!(
                prefs.disabled_builtin_features,
                ["session-mention".to_string()]
            );

            // 状态文件（MCP server 读取面）。
            let state_path = paths::pinvou3_home()
                .join("marketplace")
                .join(BUILTIN_FEATURES_STATE_FILE);
            let state: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(&state_path).expect("状态文件应已原子写入"),
            )
            .unwrap();
            assert_eq!(state["schema_version"], 1);
            assert_eq!(
                state["disabled_features"],
                serde_json::json!(["session-mention"])
            );

            // 重新打开：prefs 清空、状态文件写空数组（不残留过期名单）。
            set_feature_enabled("session-mention", true).unwrap();
            assert!(UserPrefs::load().disabled_builtin_features.is_empty());
            let state: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
            assert_eq!(state["disabled_features"], serde_json::json!([]));
        });
    }

    #[test]
    fn unknown_feature_id_is_rejected() {
        with_temp_home(|| {
            let err = set_feature_enabled("no-such-feature", false).unwrap_err();
            assert!(
                err.contains("no-such-feature") && err.contains("session-mention"),
                "错误应回显未知 id 并列出已知功能: {err}"
            );
            // 未知 id 不得落盘。
            assert!(UserPrefs::load().disabled_builtin_features.is_empty());
        });
    }

    /// 服务端纵深防御（契约 §3.3）：manager 层卸载内置插件被拒（guard 在拆任何
    /// 状态之前，无需临时 HOME）。普通插件不被内置 guard 误拦已由
    /// `session_reader_is_builtin_via_embedded_manifest` 的负例覆盖。
    #[test]
    fn manager_uninstall_rejects_builtin() {
        let err = crate::features::marketplace::MarketplaceManager::new()
            .uninstall("session-reader")
            .unwrap_err();
        assert!(
            err.contains("cannot be uninstalled"),
            "错误应含不可卸载语义: {err}"
        );
    }

    /// 连接器停用写入路径拒绝内置 id（报错而非静默过滤）。
    #[tokio::test]
    async fn apply_disabled_connectors_rejects_builtin() {
        let err = crate::features::marketplace::apply_disabled_connectors_for(
            crate::features::marketplace::ConnectorScope::Plain,
            vec!["session-reader".to_string()],
        )
        .await
        .unwrap_err();
        assert!(err.contains("session-reader"), "错误应点名内置 id: {err}");
    }

    /// 功能摘除名单并入引擎门控聚合（unavailable_tool_names_for）。
    #[test]
    fn feature_removal_flows_into_unavailable_tool_names() {
        with_temp_home(|| {
            let plain = crate::features::marketplace::unavailable_tool_names_for(
                crate::features::marketplace::ConnectorScope::Plain,
            );
            assert!(!plain.iter().any(|n| n.contains("session-reader")));
            set_feature_enabled("session-mention", false).unwrap();
            set_feature_enabled("long-memory", false).unwrap();
            let plain = crate::features::marketplace::unavailable_tool_names_for(
                crate::features::marketplace::ConnectorScope::Plain,
            );
            assert!(plain.contains(&"mcp_session-reader_read_session".to_string()));
            assert!(plain.contains(&"mcp_session-reader_list_sessions".to_string()));
        });
    }
}
