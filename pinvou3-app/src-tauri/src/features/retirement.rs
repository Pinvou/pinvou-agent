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

fn preflight_move(source: &Path, target: &Path) -> Result<bool, String> {
    if !source.exists() {
        return Ok(false);
    }
    if target.exists() {
        return Err(format!(
            "retirement archive target already exists: {}",
            target.display()
        ));
    }
    Ok(true)
}

fn rename_archived_entry<F>(source: &Path, target: &Path, rename: &mut F) -> Result<(), String>
where
    F: FnMut(&Path, &Path) -> std::io::Result<()>,
{
    rename(source, target).map_err(|error| {
        format!(
            "archive retired workflow host {} -> {}: {error}",
            source.display(),
            target.display()
        )
    })
}

fn archive_host_with_rename<F>(
    session_id: &str,
    sessions: &Path,
    mut rename: F,
) -> Result<(), String>
where
    F: FnMut(&Path, &Path) -> std::io::Result<()>,
{
    if !safe_session_id(session_id) {
        return Err(format!("refuse unsafe retired host id: {session_id}"));
    }
    let archive = sessions.join("_archived_retired_workflow_hosts");
    std::fs::create_dir_all(&archive)
        .map_err(|error| format!("create retirement archive {}: {error}", archive.display()))?;
    let session_source = sessions.join(format!("{session_id}.json"));
    let session_target = archive.join(format!("{session_id}.json"));
    let directory_source = sessions.join(session_id);
    let directory_target = archive.join(session_id);

    // 改动任一源路径前先检查两个目标，避免目录冲突时只把会话 JSON 留在归档区。
    let move_session = preflight_move(&session_source, &session_target)?;
    let move_directory = preflight_move(&directory_source, &directory_target)?;

    if move_session {
        rename_archived_entry(&session_source, &session_target, &mut rename)?;
    }
    if move_directory {
        if let Err(directory_error) =
            rename_archived_entry(&directory_source, &directory_target, &mut rename)
        {
            if move_session {
                match rename(&session_target, &session_source) {
                    Ok(()) => {
                        return Err(format!(
                            "{directory_error}; rolled back archived session JSON"
                        ));
                    }
                    Err(rollback_error) => {
                        return Err(format!(
                            "{directory_error}; rollback {} -> {} failed: {rollback_error}",
                            session_target.display(),
                            session_source.display()
                        ));
                    }
                }
            }
            return Err(directory_error);
        }
    }
    Ok(())
}

fn archive_host(session_id: &str, sessions: &Path) -> Result<(), String> {
    archive_host_with_rename(session_id, sessions, |source, target| {
        std::fs::rename(source, target)
    })
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
        .map(|_| ())
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

    #[test]
    fn archive_preflight_keeps_session_together_when_directory_target_exists() {
        let temp = std::env::temp_dir().join(format!(
            "pinvou3-retirement-preflight-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        let sessions = temp.join("sessions");
        let archive = sessions.join("_archived_retired_workflow_hosts");
        std::fs::create_dir_all(sessions.join("retired-host")).expect("host dir");
        std::fs::create_dir_all(archive.join("retired-host")).expect("conflicting archive dir");
        std::fs::write(sessions.join("retired-host.json"), b"history").expect("host session");

        let error = archive_host("retired-host", &sessions).expect_err("target collision");
        assert!(error.contains("archive target already exists"));
        assert!(sessions.join("retired-host.json").is_file());
        assert!(sessions.join("retired-host").is_dir());
        assert!(!archive.join("retired-host.json").exists());
        std::fs::remove_dir_all(temp).expect("cleanup preflight fixture");
    }

    #[test]
    fn retire_bindings_keeps_binding_and_live_session_on_archive_conflict() {
        let temp = std::env::temp_dir().join(format!(
            "pinvou3-retirement-binding-retry-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        let sessions = temp.join("sessions");
        let archive = sessions.join("_archived_retired_workflow_hosts");
        std::fs::create_dir_all(sessions.join("retired-host")).expect("host dir");
        std::fs::create_dir_all(archive.join("retired-host")).expect("conflicting archive dir");
        std::fs::write(sessions.join("retired-host.json"), b"history").expect("host session");
        std::fs::write(
            sessions.join("_skill_bindings.json"),
            serde_json::to_vec(&serde_json::json!({
                "retired-host": {"name": "sansheng-liubu", "project_dir": null}
            }))
            .expect("serialize bindings"),
        )
        .expect("bindings");

        assert_eq!(retire_bindings(&sessions).expect("retryable conflict"), 0);
        let bindings: serde_json::Value = serde_json::from_slice(
            &std::fs::read(sessions.join("_skill_bindings.json")).expect("read bindings"),
        )
        .expect("parse bindings");
        assert!(bindings.get("retired-host").is_some());
        assert!(sessions.join("retired-host.json").is_file());
        assert!(sessions.join("retired-host").is_dir());
        assert!(!archive.join("retired-host.json").exists());
        std::fs::remove_dir_all(temp).expect("cleanup binding retry fixture");
    }

    #[test]
    fn archive_rolls_back_session_json_when_directory_move_fails() {
        let temp = std::env::temp_dir().join(format!(
            "pinvou3-retirement-rollback-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        let sessions = temp.join("sessions");
        std::fs::create_dir_all(sessions.join("retired-host")).expect("host dir");
        std::fs::write(sessions.join("retired-host.json"), b"history").expect("host session");
        let mut rename_attempt = 0;

        let error = archive_host_with_rename("retired-host", &sessions, |source, target| {
            rename_attempt += 1;
            if rename_attempt == 2 {
                return Err(std::io::Error::other("injected directory move failure"));
            }
            std::fs::rename(source, target)
        })
        .expect_err("directory move must fail");

        assert!(error.contains("rolled back archived session JSON"));
        assert_eq!(
            rename_attempt, 3,
            "session move, directory failure, rollback"
        );
        assert!(sessions.join("retired-host.json").is_file());
        assert!(sessions.join("retired-host").is_dir());
        let archive = sessions.join("_archived_retired_workflow_hosts");
        assert!(!archive.join("retired-host.json").exists());
        assert!(!archive.join("retired-host").exists());

        archive_host("retired-host", &sessions).expect("retry after rollback");
        assert!(archive.join("retired-host.json").is_file());
        assert!(archive.join("retired-host").is_dir());
        std::fs::remove_dir_all(temp).expect("cleanup rollback fixture");
    }

    #[test]
    fn archive_resumes_when_session_json_was_already_moved() {
        let temp = std::env::temp_dir().join(format!(
            "pinvou3-retirement-resume-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        let sessions = temp.join("sessions");
        let archive = sessions.join("_archived_retired_workflow_hosts");
        std::fs::create_dir_all(sessions.join("retired-host")).expect("host dir");
        std::fs::create_dir_all(&archive).expect("archive dir");
        std::fs::write(archive.join("retired-host.json"), b"history")
            .expect("already archived session");

        archive_host("retired-host", &sessions).expect("resume partial archive");
        assert!(archive.join("retired-host.json").is_file());
        assert!(archive.join("retired-host").is_dir());
        assert!(!sessions.join("retired-host").exists());
        std::fs::remove_dir_all(temp).expect("cleanup resume fixture");
    }
}
