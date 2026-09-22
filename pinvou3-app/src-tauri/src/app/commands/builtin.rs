// ---------------------------------------------------------------------------
// Builtin plugin feature switches (docs/builtin-toolset-contract.md §3.3)
// ---------------------------------------------------------------------------

use super::prelude::*;

/// Feature registry: the switchable features declared by builtin plugins
/// (feature id, owning plugins, associated tools, enabled state). This cycle
/// only builds the mechanism: the switch UI is wired into each feature's own
/// settings page as the feature lands (docs/builtin-toolset-contract.md §3.3:
/// switches appear per-feature in that feature's settings, not in a plugin
/// center section).
#[tauri::command]
pub fn list_builtin_features()
-> Result<Vec<crate::features::marketplace::builtin::BuiltinFeature>, String> {
    Ok(crate::features::marketplace::builtin::feature_registry())
}

/// Toggles one builtin feature: writes UserPrefs.disabled_builtin_features +
/// the state file `~/.pinvou3/marketplace/builtin_features.json` (the
/// MCP-server-facing read side), then the same wrap-up as connector
/// switches — hot-refresh the disallowed-tools list (tools removed by union
/// semantics become invisible to the model immediately) and broadcast —
/// returning the fresh registry after persistence.
#[tauri::command]
pub async fn set_builtin_feature_enabled(
    feature_id: String,
    enabled: bool,
    app: AppHandle,
    pool: State<'_, EnginePool>,
) -> Result<Vec<crate::features::marketplace::builtin::BuiltinFeature>, String> {
    let registry =
        crate::features::marketplace::builtin::set_feature_enabled(&feature_id, enabled)?;
    super::connectors::refresh_tools_and_broadcast(&app, pool.inner()).await;
    Ok(registry)
}
