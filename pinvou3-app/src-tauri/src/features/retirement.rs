//! 退役功能的一次性兼容清理。
//!
//! 这里只处理应用拥有的不可变 bundle 与旧宿主绑定。用户生成的历史项目、产物、
//! 会话内容和 `~/.pinvou3/web-template` 均不删除。

use crate::platform::paths;
use std::path::Path;

const RETIRED_SKILL_NAMES: &[&str] = &["sansheng-liubu", "legacy-ppt-workflow"];

fn retired_binding(binding: &serde_json::Value) -> bool {
    binding
        .get("name")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|name| RETIRED_SKILL_NAMES.contains(&name))
        || binding
            .get("project_dir")
            .is_some_and(|project| !project.is_null())
}

fn safe_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && !session_id.contains('/')
        && !session_id.contains('\\')
        && !session_id.contains("..")
}

fn move_if_present(source: &Path, target: &Path) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }
    if target.exists() {
        return Err(format!(
            "retirement archive target already exists: {}",
            target.display()
        ));
    }
    std::fs::rename(source, target).map_err(|error| {
        format!(
            "archive retired workflow host {} -> {}: {error}",
            source.display(),
            target.display()
        )
    })
}

fn archive_host(session_id: &str, sessions: &Path) -> Result<(), String> {
    if !safe_session_id(session_id) {
        return Err(format!("refuse unsafe retired host id: {session_id}"));
    }
    let archive = sessions.join("_archived_retired_workflow_hosts");
    std::fs::create_dir_all(&archive)
        .map_err(|error| format!("create retirement archive {}: {error}", archive.display()))?;
    move_if_present(
        &sessions.join(format!("{session_id}.json")),
        &archive.join(format!("{session_id}.json")),
    )?;
    move_if_present(&sessions.join(session_id), &archive.join(session_id))
}

fn write_json_atomically(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("binding file has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let temporary = path.with_extension(format!("json.retiring-{nonce}"));
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("serialize retired bindings: {error}"))?;
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    let backup = path.with_extension(format!("json.retiring-backup-{nonce}"));
    let result = crate::platform::filesystem::replace_file_atomically(&temporary, path, &backup)
        .map_err(|error| format!("replace {}: {error}", path.display()));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
        let _ = std::fs::remove_file(&backup);
    }
    result
}

fn retire_bindings(sessions: &Path) -> Result<usize, String> {
    let bindings_path = sessions.join("_skill_bindings.json");
    let Ok(raw) = std::fs::read_to_string(&bindings_path) else {
        return Ok(0);
    };
    let mut document: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| format!("parse {}: {error}", bindings_path.display()))?;
    let Some(bindings) = document.as_object_mut() else {
        return Err(format!("{} is not a JSON object", bindings_path.display()));
    };
    let candidates = bindings
        .iter()
        .filter(|(_, binding)| retired_binding(binding))
        .map(|(session_id, _)| session_id.clone())
        .collect::<Vec<_>>();
    let mut retired = Vec::new();
    for session_id in candidates {
        match archive_host(&session_id, sessions) {
            Ok(()) => retired.push(session_id),
            Err(error) => eprintln!("[pinvou3-app] {error}; binding retained for retry"),
        }
    }
    for session_id in &retired {
        bindings.remove(session_id);
    }
    if !retired.is_empty() {
        write_json_atomically(&bindings_path, &document)?;
    }
    Ok(retired.len())
}

/// 清除已退役功能的运行时入口，同时保留全部用户历史数据。
pub fn retire_removed_features() -> Result<(), String> {
    let managed_bundle = paths::bundle_root().join("workflow").join("sansheng-liubu");
    if managed_bundle.exists() {
        std::fs::remove_dir_all(&managed_bundle).map_err(|error| {
            format!(
                "remove retired managed bundle {}: {error}",
                managed_bundle.display()
            )
        })?;
    }
    retire_bindings(&paths::sessions_root())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_custom_workflow_bindings_are_retired() {
        assert!(retired_binding(&serde_json::json!({
            "name": "sansheng-liubu",
            "project_dir": null
        })));
        assert!(retired_binding(&serde_json::json!({
            "name": "anything",
            "project_dir": "/legacy/project"
        })));
        assert!(!retired_binding(&serde_json::json!({
            "name": "deep-research",
            "project_dir": null
        })));
    }

    #[test]
    fn session_archive_rejects_path_traversal() {
        assert!(safe_session_id("session-123"));
        assert!(!safe_session_id("../session-123"));
        assert!(!safe_session_id("nested/session"));
    }

    #[test]
    fn retired_hosts_are_archived_while_ordinary_sessions_and_projects_remain() {
        let temp = std::env::temp_dir().join(format!(
            "pinvou3-retirement-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        let sessions = temp.join("sessions");
        let project = temp.join("historical-project");
        std::fs::create_dir_all(sessions.join("retired-host")).expect("host dir");
        std::fs::create_dir_all(&project).expect("project dir");
        std::fs::write(sessions.join("retired-host.json"), b"history").expect("host session");
        std::fs::write(sessions.join("ordinary.json"), b"ordinary").expect("ordinary session");
        std::fs::write(
            sessions.join("_skill_bindings.json"),
            serde_json::to_vec(&serde_json::json!({
                "retired-host": {"name": "legacy-ppt-workflow", "project_dir": project.to_string_lossy()},
                "ordinary": {"name": "deep-research", "project_dir": null}
            }))
            .expect("serialize bindings"),
        )
        .expect("bindings");

        assert_eq!(retire_bindings(&sessions).expect("retire bindings"), 1);
        let archive = sessions.join("_archived_retired_workflow_hosts");
        assert!(archive.join("retired-host.json").is_file());
        assert!(archive.join("retired-host").is_dir());
        assert!(sessions.join("ordinary.json").is_file());
        assert!(
            project.is_dir(),
            "historical user project must not be deleted"
        );

        let bindings: serde_json::Value = serde_json::from_slice(
            &std::fs::read(sessions.join("_skill_bindings.json")).expect("read bindings"),
        )
        .expect("parse bindings");
        assert!(bindings.get("retired-host").is_none());
        assert!(bindings.get("ordinary").is_some());
        std::fs::remove_dir_all(temp).expect("cleanup retirement fixture");
    }
}
