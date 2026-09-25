//! Builtin plugin framework (docs/builtin-toolset-contract.md §3.1/§3.3).
//!
//! Builtin plugins (manifest `builtin: true`) ship with the application:
//! **no uninstall, no disable** (server-side defense in depth — the frontend
//! simply not offering the action is only a UX layer; direct command/IPC
//! calls must be rejected in the backend). The data security level and the
//! data-access scopes pass through `MarketplaceToolInfo` for the frontend to
//! localize.
//!
//! Feature-level switches (§3.3): a builtin plugin's tools belong to
//! independently switchable features via `tool_features` (full tool name ->
//! feature id array) — e.g. session-reader's read_session/list_sessions serve
//! both "session mention" (session-mention) and "long memory" (long-memory).
//! Switch state persists in `settings.json` as
//! `UserPrefs::disabled_builtin_features` and is mirrored to
//! `~/.pinvou3/marketplace/builtin_features.json` (`{"schema_version":1,
//! "disabled_features":[...]}`, atomic write; a missing file means all
//! enabled) for MCP server processes to read.
//! Tool removal follows **union semantics**: a tool is removed from the
//! registry only when **all** features listed in its `tool_features` entry
//! are switched off (see [`feature_disabled_tool_names`], merged into
//! `super::unavailable_tool_names_for`); tools not listed in `tool_features`
//! are unaffected by feature switches.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use serde::Serialize;

use super::mcp_catalog;
use crate::platform::paths;
use crate::platform::prefs::UserPrefs;

/// Feature-switch state file (read by MCP server processes; missing = all
/// enabled).
const BUILTIN_FEATURES_STATE_FILE: &str = "builtin_features.json";

/// Process-wide serialization for feature toggles: the prefs commit and the
/// state-file write are two stores without a shared transaction, so
/// concurrent toggles could interleave and persist a state file that
/// disagrees with prefs. The mutex spans validation → prefs commit → state
/// write (poisoning is recovered — a panicked toggle must not deadlock later
/// ones).
static FEATURE_TOGGLE_LOCK: Mutex<()> = Mutex::new(());

/// Builtin determination trusts only the compile-time embedded catalog
/// (`mcp_catalog::embedded_manifest`, a read-only snapshot shipped by the
/// publisher). The released `bundles/<id>/mcp/manifest.json` is user-writable
/// and must never confer builtin status (trust boundary, see
/// docs/builtin-toolset-contract.md §3.1); ids missing from the catalog or
/// failing to parse are treated as non-builtin — better to allow uninstalling
/// a normal plugin than to lock one by mistake.
pub fn is_builtin_tool(id: &str) -> bool {
    mcp_catalog::embedded_manifest(id)
        .ok()
        .flatten()
        .map(|manifest| manifest.builtin)
        .unwrap_or(false)
}

/// Guard before writing disable/hide lists (docs/builtin-toolset-contract.md
/// §3.3: builtin plugins can be neither disabled nor hidden). Any builtin id
/// fails the whole write — no silent filtering, which would make the frontend
/// believe a toggle took effect. Ids are normalized with the same
/// `to_package_id` rule the persistence layer applies (strip the `skill:`
/// prefix, map companion skills to their owner package) so that e.g.
/// `skill:session-reader` cannot slip past the check.
pub fn reject_builtin_ids(ids: &[String]) -> Result<(), String> {
    let builtin: Vec<String> = ids
        .iter()
        .map(|id| super::scope::to_package_id(id))
        .filter(|id| is_builtin_tool(id))
        .collect();
    if builtin.is_empty() {
        return Ok(());
    }
    Err(format!(
        "builtin plugins cannot be disabled or hidden: {}",
        builtin.join(", ")
    ))
}

/// Feature registry entry: one switchable builtin feature and its origins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuiltinFeature {
    /// Feature id (e.g. "session-mention").
    pub id: String,
    /// Ids of the builtin plugins declaring this feature (deduped, sorted).
    pub plugins: Vec<String>,
    /// Full names of the tools belonging to this feature (deduped, sorted).
    pub tools: Vec<String>,
    /// Current switch state (absent from `disabled_builtin_features` =
    /// enabled).
    pub enabled: bool,
}

/// Scans `tool_features` of every builtin manifest in the embedded catalog
/// (feature id -> tools/plugins) and aggregates the feature registry (sorted
/// by feature id); enabled is decided by `UserPrefs::disabled_builtin_features`.
/// A catalog manifest that fails to parse is skipped (same policy as
/// `available_tools` — one bad package must not break the whole registry).
pub fn feature_registry() -> Vec<BuiltinFeature> {
    let disabled = disabled_feature_ids();
    feature_registry_with_disabled(&disabled)
}

/// The pure part of registry aggregation (the disabled set is supplied by the
/// caller), shared by `feature_registry` and the lock-holding writer (which
/// recomputes it after the state lands) so both use one policy.
fn feature_registry_with_disabled(disabled: &BTreeSet<String>) -> Vec<BuiltinFeature> {
    // feature id -> (plugin id set, full tool name set); BTree* keeps the
    // output deterministic.
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

/// All `builtin: true` manifests in the embedded catalog (parse failures are
/// skipped).
fn embedded_builtin_manifests() -> Vec<super::types::ToolManifest> {
    mcp_catalog::MCP_PACKAGES
        .iter()
        .filter_map(|spec| {
            serde_json::from_str::<super::types::ToolManifest>(spec.manifest_json)
                .map_err(|e| {
                    eprintln!(
                        "[builtin] embedded manifest parse failed ({}): {e}",
                        spec.id
                    );
                    e
                })
                .ok()
        })
        .filter(|manifest| manifest.builtin)
        .collect()
}

/// Currently disabled feature ids (settings.json; unreadable = all enabled).
fn disabled_feature_ids() -> BTreeSet<String> {
    UserPrefs::load()
        .disabled_builtin_features
        .into_iter()
        .collect()
}

/// Feature switch (docs/builtin-toolset-contract.md §3.3): writes
/// `UserPrefs::disabled_builtin_features` (field-level transaction), then
/// atomically writes the state file for MCP servers, and returns the fresh
/// registry after persistence. Unknown feature ids are rejected (a frontend
/// typo fails loudly instead of silently landing).
/// The whole operation is serialized under `FEATURE_TOGGLE_LOCK` (see its
/// comment).
pub fn set_feature_enabled(id: &str, enabled: bool) -> Result<Vec<BuiltinFeature>, String> {
    let _guard = FEATURE_TOGGLE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
    // The state file is a read-side copy for MCP server processes; the prefs
    // (which the engine path reads) are authoritative. A failed write must
    // neither fail the toggle nor skip the caller's hot refresh, so it
    // degrades to a warning; the next boot replay or toggle rewrites it.
    if let Err(error) = write_feature_state_file(&disabled) {
        eprintln!(
            "[builtin] write builtin feature state failed (prefs remain authoritative): {error}"
        );
    }
    Ok(feature_registry_with_disabled(&disabled))
}

/// Boot-time replay: rewrite the MCP-server-facing state file from
/// `UserPrefs::disabled_builtin_features` (the authoritative store) so a
/// crash between the prefs commit and the state write cannot strand a stale
/// list. If prefs themselves are unreadable they load as defaults, which the
/// engine path also sees — replaying simply aligns the file with that same
/// reality. Best-effort: failures only log (a missing state file already
/// means "all enabled" for readers, and the next toggle rewrites it).
pub fn replay_feature_state_from_prefs() {
    if let Err(error) = write_feature_state_file(&disabled_feature_ids()) {
        eprintln!("[builtin] replay builtin feature state failed: {error}");
    }
}

/// Atomically writes `~/.pinvou3/marketplace/builtin_features.json` (tmp +
/// rename, `platform::filesystem::atomic_write`). An empty array is written
/// when everything is enabled too (the state file always reflects the latest
/// switch state, so MCP servers never read a stale list).
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

/// Full model-visible tool names that feature switches should remove (fed to
/// the engine's disallowed_tools).
///
/// Union semantics: a tool is removed only when **all** features listed in
/// its `tool_features` entry are off (while one feature is still on, the tool
/// stays); tools with an empty/absent `tool_features` entry are unaffected.
/// The output is lowercased (the engine's `command_denies_tool` matches
/// lowercase exactly, same as `model_tool_names`).
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

    /// Runs the closure with PINVOU3_HOME pointing at a clean temp directory,
    /// then restores and cleans up. Shares `platform::paths::tests::ENV_LOCK`
    /// with prefs/store tests that mutate PINVOU3_HOME (same environment
    /// variable, so the same lock must serialize them).
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
        // A normal preset package in the embedded catalog is not builtin.
        assert!(!is_builtin_tool("weather"));
        // An unknown id (absent from disk too) counts as non-builtin.
        assert!(!is_builtin_tool("no-such-tool"));
    }

    /// Trust boundary: a released on-disk manifest claiming `builtin: true` is
    /// NOT honored — builtin status comes only from the embedded catalog, and
    /// the upload pipeline rejects packages claiming it (see plugin_import).
    #[test]
    fn released_manifest_builtin_claim_is_not_trusted() {
        with_temp_home(|| {
            let mcp_dir = mcp_catalog::package_mcp_dir("custom-builtin");
            std::fs::create_dir_all(&mcp_dir).unwrap();
            std::fs::write(
                mcp_dir.join("manifest.json"),
                r#"{"id":"custom-builtin","name":"c","description":"d","version":"1","icon":"x","category":"c","mcp_tools":[],"command":"python","args":[],"builtin":true}"#,
            )
            .unwrap();
            assert!(
                !is_builtin_tool("custom-builtin"),
                "an on-disk manifest must never confer builtin status"
            );
        });
    }

    #[test]
    fn reject_builtin_ids_lists_offenders() {
        assert!(reject_builtin_ids(&["weather".to_string()]).is_ok());
        let err =
            reject_builtin_ids(&["weather".to_string(), "session-reader".to_string()]).unwrap_err();
        assert!(
            err.contains("session-reader"),
            "the builtin id should be named: {err}"
        );
        assert!(
            !err.contains("weather"),
            "a normal plugin must not be named: {err}"
        );
    }

    /// Ids are judged after the same `to_package_id` normalization the
    /// persistence layer applies, so a `skill:`-prefixed alias of a builtin
    /// package cannot slip past the guard.
    #[test]
    fn reject_builtin_ids_normalizes_before_judging() {
        let err = reject_builtin_ids(&["skill:session-reader".to_string()]).unwrap_err();
        assert!(
            err.contains("session-reader"),
            "the normalized builtin id should be named: {err}"
        );
        assert!(reject_builtin_ids(&["skill:weather".to_string()]).is_ok());
    }

    /// Feature registry: session-reader's tool_features aggregate into
    /// session-mention / long-memory with correct plugin and tool ownership;
    /// everything is enabled by default.
    #[test]
    fn registry_aggregates_session_reader_features() {
        with_temp_home(|| {
            let registry = feature_registry();
            let ids: Vec<&str> = registry.iter().map(|f| f.id.as_str()).collect();
            assert_eq!(ids, ["long-memory", "session-mention"], "sorted output");
            for feature in &registry {
                assert_eq!(feature.plugins, ["session-reader".to_string()]);
                assert_eq!(
                    feature.tools,
                    [
                        "mcp_session-reader_list_sessions".to_string(),
                        "mcp_session-reader_read_session".to_string()
                    ]
                );
                assert!(feature.enabled, "default (no state) is all enabled");
            }
        });
    }

    /// Union semantics: disabling only one of two features removes nothing;
    /// disabling both removes both tools.
    #[test]
    fn union_semantics_gate_tool_removal() {
        with_temp_home(|| {
            // All enabled: nothing removed.
            assert!(feature_disabled_tool_names().is_empty());

            // Only session-mention off (long-memory still on): read_session
            // is not removed.
            set_feature_enabled("session-mention", false).unwrap();
            assert!(
                feature_disabled_tool_names().is_empty(),
                "long-memory is still enabled, so neither tool may be removed"
            );

            // Both off: read_session / list_sessions are removed.
            set_feature_enabled("long-memory", false).unwrap();
            assert_eq!(
                feature_disabled_tool_names(),
                [
                    "mcp_session-reader_list_sessions".to_string(),
                    "mcp_session-reader_read_session".to_string()
                ]
            );

            // Re-enable one: the removal lifts.
            set_feature_enabled("long-memory", true).unwrap();
            assert!(feature_disabled_tool_names().is_empty());
        });
    }

    /// Tools not listed in any tool_features entry are unaffected by feature
    /// switches (weather stays with everything switched off).
    #[test]
    fn tools_outside_tool_features_are_unaffected() {
        with_temp_home(|| {
            set_feature_enabled("session-mention", false).unwrap();
            set_feature_enabled("long-memory", false).unwrap();
            let removed = feature_disabled_tool_names();
            assert!(
                !removed.iter().any(|n| n.contains("weather")),
                "tools outside tool_features must not be removed by feature switches: {removed:?}"
            );
        });
    }

    /// Switch persistence: settings.json's disabled_builtin_features and the
    /// state file builtin_features.json (schema_version + disabled_features)
    /// carry the correct content.
    #[test]
    fn set_feature_enabled_persists_prefs_and_state_file() {
        with_temp_home(|| {
            let registry = set_feature_enabled("session-mention", false).unwrap();
            let mention = registry.iter().find(|f| f.id == "session-mention").unwrap();
            assert!(!mention.enabled);
            let long_memory = registry.iter().find(|f| f.id == "long-memory").unwrap();
            assert!(long_memory.enabled);

            // settings.json persistence.
            let prefs = UserPrefs::load();
            assert_eq!(
                prefs.disabled_builtin_features,
                ["session-mention".to_string()]
            );

            // The state file (the MCP-server-facing read side).
            let state_path = paths::pinvou3_home()
                .join("marketplace")
                .join(BUILTIN_FEATURES_STATE_FILE);
            let state: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(&state_path)
                    .expect("the state file should have been written atomically"),
            )
            .unwrap();
            assert_eq!(state["schema_version"], 1);
            assert_eq!(
                state["disabled_features"],
                serde_json::json!(["session-mention"])
            );

            // Re-enable: prefs cleared, the state file holds an empty array
            // (no stale list left behind).
            set_feature_enabled("session-mention", true).unwrap();
            assert!(UserPrefs::load().disabled_builtin_features.is_empty());
            let state: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
            assert_eq!(state["disabled_features"], serde_json::json!([]));
        });
    }

    /// A failing state-file write degrades to a warning: the toggle still
    /// succeeds and prefs (the authoritative store) reflect it.
    #[test]
    fn state_file_write_failure_does_not_fail_toggle() {
        with_temp_home(|| {
            // Block state-file creation: `marketplace` exists as a file.
            std::fs::write(paths::pinvou3_home().join("marketplace"), b"x").unwrap();
            let registry = set_feature_enabled("session-mention", false).unwrap();
            assert!(
                !registry
                    .iter()
                    .find(|f| f.id == "session-mention")
                    .unwrap()
                    .enabled
            );
            assert_eq!(
                UserPrefs::load().disabled_builtin_features,
                ["session-mention".to_string()]
            );
        });
    }

    /// Boot replay rewrites the state file from prefs: a state file stranded
    /// by a crash window (prefs committed, state write lost) is healed.
    #[test]
    fn replay_feature_state_heals_stale_state_file() {
        with_temp_home(|| {
            set_feature_enabled("session-mention", false).unwrap();
            let state_path = paths::pinvou3_home()
                .join("marketplace")
                .join(BUILTIN_FEATURES_STATE_FILE);
            std::fs::remove_file(&state_path).unwrap();
            replay_feature_state_from_prefs();
            let state: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
            assert_eq!(
                state["disabled_features"],
                serde_json::json!(["session-mention"])
            );
        });
    }

    /// Concurrency: FEATURE_TOGGLE_LOCK spans validation → prefs commit →
    /// state-file write, so racing toggles must leave prefs (the authoritative
    /// store) and the MCP-facing state file in agreement — without the mutex
    /// the two stores could interleave and persist disagreeing states.
    #[test]
    fn concurrent_feature_toggles_leave_consistent_state() {
        with_temp_home(|| {
            let mut handles = Vec::new();
            for thread in 0..8 {
                handles.push(std::thread::spawn(move || {
                    for round in 0..25 {
                        let feature = if (thread + round) % 2 == 0 {
                            "session-mention"
                        } else {
                            "long-memory"
                        };
                        set_feature_enabled(feature, (thread + round) % 3 == 0).unwrap();
                    }
                }));
            }
            for handle in handles {
                handle.join().unwrap();
            }
            let prefs_disabled: BTreeSet<String> = UserPrefs::load()
                .disabled_builtin_features
                .into_iter()
                .collect();
            let state_path = paths::pinvou3_home()
                .join("marketplace")
                .join(BUILTIN_FEATURES_STATE_FILE);
            let state: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(&state_path)
                    .expect("the state file should exist after toggles"),
            )
            .unwrap();
            let file_disabled: BTreeSet<String> = state["disabled_features"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            assert_eq!(
                prefs_disabled, file_disabled,
                "racing toggles must leave prefs and the state file in agreement"
            );
        });
    }

    #[test]
    fn unknown_feature_id_is_rejected() {
        with_temp_home(|| {
            let err = set_feature_enabled("no-such-feature", false).unwrap_err();
            assert!(
                err.contains("no-such-feature") && err.contains("session-mention"),
                "the error should echo the unknown id and list known features: {err}"
            );
            // Unknown ids must not be persisted.
            assert!(UserPrefs::load().disabled_builtin_features.is_empty());
        });
    }

    /// Server-side defense in depth (docs/builtin-toolset-contract.md §3.1):
    /// the manager layer rejects uninstalling a builtin plugin (the guard
    /// runs before any state is touched, so no temp HOME is needed). That a
    /// normal plugin is not caught by the builtin guard is covered by the
    /// negative cases of `session_reader_is_builtin_via_embedded_manifest`.
    #[test]
    fn manager_uninstall_rejects_builtin() {
        let err = crate::features::marketplace::MarketplaceManager::new()
            .uninstall("session-reader")
            .unwrap_err();
        assert!(
            err.contains("cannot be uninstalled"),
            "the error should carry the not-uninstallable semantics: {err}"
        );
        // The guard normalizes ids like the disable/hide guards: a
        // `skill:`-prefixed alias of a builtin package must be rejected with
        // the same semantics, not fall through to a generic not-installed
        // error.
        let err = crate::features::marketplace::MarketplaceManager::new()
            .uninstall("skill:session-reader")
            .unwrap_err();
        assert!(
            err.contains("cannot be uninstalled"),
            "the skill:-prefixed alias must hit the same guard: {err}"
        );
    }

    /// The connector-disable write path rejects builtin ids (loud error, not
    /// silent filtering).
    #[tokio::test]
    async fn apply_disabled_connectors_rejects_builtin() {
        let err = crate::features::marketplace::apply_disabled_connectors_for(
            crate::features::marketplace::ConnectorScope::Plain,
            vec!["session-reader".to_string()],
        )
        .await
        .unwrap_err();
        assert!(
            err.contains("session-reader"),
            "the builtin id should be named: {err}"
        );
    }

    /// The feature-removal list merges into the engine-gate aggregation
    /// (unavailable_tool_names_for).
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
