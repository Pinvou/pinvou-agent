// 本文件为命令层测试,借 platform::paths::tests::ENV_LOCK(std Mutex)串行化全局 env;跨 await 持有无竞争者,不会死锁。
#![allow(clippy::await_holding_lock)]

use super::prelude::*;
use super::{
    artifacts::*, attachments::*, files::*, interaction::*, knowledge::*, marketplace::*,
    personas::*, sessions::*, voice::*, workflows::*,
};
use crate::platform::filesystem::tests::{remove_dir_link, try_link_dir, try_link_file};
use crate::platform::path_policy::validate_user_path;
use std::path::{Path, PathBuf};

#[test]
fn marketplace_auth_status_only_oauth_is_connected_for_oauth_tools() {
    use deepseek_tui::mcp::oauth::McpAuthStatus;

    let (status, _, token_present) =
        marketplace_auth_status_fields(true, true, true, Some(McpAuthStatus::OAuth));
    assert_eq!(status, "connected");
    assert!(token_present);

    for auth_status in [
        McpAuthStatus::NotLoggedIn,
        McpAuthStatus::Unsupported,
        McpAuthStatus::BearerToken,
    ] {
        let (status, _, token_present) =
            marketplace_auth_status_fields(true, true, true, Some(auth_status));
        assert_eq!(status, "config_installed_auth_pending");
        assert!(!token_present);
    }
}

#[test]
fn marketplace_auth_status_preserves_non_oauth_installed_semantics() {
    let (status, _, token_present) = marketplace_auth_status_fields(true, false, false, None);
    assert_eq!(status, "connected");
    assert!(!token_present);

    let (status, _, token_present) = marketplace_auth_status_fields(false, false, false, None);
    assert_eq!(status, "not_installed");
    assert!(!token_present);
}

#[test]
fn marketplace_auth_status_requires_mcp_config_for_oauth_connected() {
    use deepseek_tui::mcp::oauth::McpAuthStatus;

    let (status, _, token_present) =
        marketplace_auth_status_fields(true, true, false, Some(McpAuthStatus::OAuth));
    assert_eq!(status, "auth_pending");
    assert!(!token_present);
}

#[test]
fn marketplace_oauth_messages_are_connector_neutral() {
    let (_, connected_message, connected_token) = marketplace_auth_status_fields(
        true,
        true,
        true,
        Some(deepseek_tui::mcp::oauth::McpAuthStatus::OAuth),
    );
    assert!(connected_token);
    assert!(!connected_message.contains("元典"));
    assert!(!connected_message.contains("华宇元典"));

    let (_, pending_message, pending_token) =
        marketplace_auth_status_fields(true, true, true, None);
    assert!(!pending_token);
    assert!(!pending_message.contains("元典"));
    assert!(!pending_message.contains("华宇元典"));

    let error_result = marketplace_oauth_error_result(
        "canva_mcp".to_string(),
        anyhow::anyhow!("oauth provider rejected authorization"),
    );
    assert_eq!(error_result.status, "provider_error");
    assert!(!error_result.message.contains("元典"));
    assert!(!error_result.message.contains("华宇元典"));
}

struct TempPinvou3Home {
    root: PathBuf,
    previous: Option<String>,
}

impl TempPinvou3Home {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let previous = std::env::var("PINVOU3_HOME").ok();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("PINVOU3_HOME", &root);
        Self { root, previous }
    }
}

impl Drop for TempPinvou3Home {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn write_json(path: &Path, value: serde_json::Value) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

#[test]
fn role_logs_merge_real_jsonl_files_and_append_authoritative_failure() {
    let root = std::env::temp_dir().join(format!(
        "pinvou3-role-logs-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let state = root.join("_state");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(
            state.join("flow_log.jsonl"),
            concat!(
                "{\"timestamp\":\"2026-07-21T10:00:00+08:00\",\"event\":\"dispatch\",\"role_id\":\"taizi\"}\n",
                "{\"timestamp\":\"2026-07-21T10:01:00+08:00\",\"event\":\"agent_failed\",\"role_id\":\"taizi\",\"detail\":\"HTTP 503 upstream unavailable\"}\n",
                "{\"timestamp\":\"2026-07-21T10:02:00+08:00\",\"event\":\"agent_failed\",\"role_id\":\"zhongshu\"}\n"
            ),
        )
        .unwrap();
    std::fs::write(
            state.join("agent_log.jsonl"),
            "{\"timestamp\":\"2026-07-21T10:00:30+08:00\",\"event\":\"tool_call\",\"role_id\":\"taizi\",\"tool\":\"web_search\"}\n",
        )
        .unwrap();
    write_json(
        &state.join("workflow_progress.json"),
        serde_json::json!({
            "updated_at": "2026-07-21T10:03:00+08:00",
            "roles": {
                "taizi": {
                    "status": "failed",
                    "error": "subagent 执行失败: HTTP 503 upstream unavailable"
                }
            }
        }),
    );

    let logs = read_role_logs_from_project(&root, "taizi", 60).unwrap();
    let events = logs
        .iter()
        .filter_map(|record| record.get("event").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();
    assert_eq!(
        events,
        vec!["dispatch", "tool_call", "agent_failed", "failure_state"]
    );
    assert_eq!(
        logs.last()
            .and_then(|record| record.get("reason"))
            .and_then(serde_json::Value::as_str),
        Some("subagent 执行失败: HTTP 503 upstream unavailable")
    );
    assert!(!logs
        .iter()
        .any(|record| record.get("role_id") == Some(&serde_json::json!("zhongshu"))));
    let _ = std::fs::remove_dir_all(root);
}

fn write_test_oauth_marketplace_files(server_name: &str, mcp_server: serde_json::Value) {
    let manifest = serde_json::json!({
        "id": "yuandian-mcp",
        "name": "华宇元典法律数据",
        "description": "test",
        "version": "1.0.0",
        "icon": "BookOpen",
        "category": "kb",
        "mcp_tools": [],
        "command": "",
        "args": [],
        "servers": [{
            "name": server_name,
            "url": mcp_server.get("url").and_then(|v| v.as_str()).unwrap_or("not-a-url"),
            "scopes": ["legal"],
            "oauth": { "client_id": "test-client" }
        }]
    });
    write_json(
        &crate::platform::paths::bundle_mcp_servers_dir()
            .join("yuandian-mcp")
            .join("manifest.json"),
        manifest,
    );
    write_json(
        &crate::platform::paths::pinvou3_home()
            .join("marketplace")
            .join("installed.json"),
        serde_json::json!(["yuandian-mcp"]),
    );
    write_json(
        &crate::platform::paths::mcp_config_path(),
        serde_json::json!({ "servers": { server_name: mcp_server } }),
    );
}

fn test_oauth_store_key(server_name: &str, url: &str) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use sha2::{Digest, Sha256};

    let mut payload = Vec::with_capacity(server_name.len() + url.len() + 1);
    payload.extend_from_slice(server_name.as_bytes());
    payload.push(0);
    payload.extend_from_slice(url.as_bytes());
    let digest = Sha256::digest(&payload);
    format!("mcp_oauth_{}", URL_SAFE_NO_PAD.encode(digest))
}

fn set_test_oauth_token(server_name: &str, url: &str, value: &str) -> String {
    let key = test_oauth_store_key(server_name, url);
    codewhale_secrets::Secrets::auto_detect()
        .set(&key, value)
        .unwrap();
    key
}

fn delete_test_oauth_token_key(key: &str) {
    let _ = codewhale_secrets::Secrets::auto_detect().delete(key);
}

fn test_oauth_token_exists(key: &str) -> bool {
    codewhale_secrets::Secrets::auto_detect()
        .get(key)
        .unwrap()
        .is_some()
}

#[tokio::test(flavor = "current_thread")]
async fn marketplace_auth_status_does_not_treat_missing_or_corrupt_token_as_connected() {
    let _g = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _home = TempPinvou3Home::new("oauth-status");
    let server_name = "yuandian-mcp-status-test";
    let url = "not-a-url";
    write_test_oauth_marketplace_files(
        server_name,
        serde_json::json!({
            "url": url,
            "scopes": ["legal"],
            "oauth": { "client_id": "test-client" }
        }),
    );

    let missing = get_marketplace_tool_auth_status("yuandian-mcp".to_string())
        .await
        .unwrap();
    assert!(missing.installed);
    assert!(missing.oauth_required);
    assert!(missing.mcp_configured);
    assert!(!missing.oauth_token_present);
    assert_eq!(missing.status, "config_installed_auth_pending");

    let key = set_test_oauth_token(server_name, url, "not-json");
    let corrupt = get_marketplace_tool_auth_status("yuandian-mcp".to_string())
        .await
        .unwrap();
    assert!(corrupt.installed);
    assert!(corrupt.oauth_required);
    assert!(corrupt.mcp_configured);
    assert!(!corrupt.oauth_token_present);
    assert_eq!(corrupt.status, "config_installed_auth_pending");
    delete_test_oauth_token_key(&key);
}

#[tokio::test]
async fn forkguard_marketplace_oauth_replacement_waits_for_previous_flow() {
    let coordinator = MarketplaceOAuthLoginCoordinator::default();
    let first = coordinator.register("yuandian-mcp", "request-1").await;
    let second = coordinator.register("yuandian-mcp", "request-2").await;

    assert!(
        first.cancellation_token.is_cancelled(),
        "registering a replacement must cancel the older OAuth flow"
    );
    assert!(
        coordinator.is_current("yuandian-mcp", "request-2").await,
        "the replacement must own the tool slot"
    );

    coordinator
        .finish("yuandian-mcp", "request-1", first.completion_sender)
        .await;
    wait_for_oauth_completion(
        second
            .previous_completion
            .expect("replacement should wait for the previous flow"),
    )
    .await;

    assert!(!second.cancellation_token.is_cancelled());
    coordinator
        .finish("yuandian-mcp", "request-2", second.completion_sender)
        .await;
    assert!(!coordinator.is_current("yuandian-mcp", "request-2").await);
}

#[tokio::test]
async fn forkguard_marketplace_oauth_cancel_waits_until_flow_finishes() {
    let coordinator = std::sync::Arc::new(MarketplaceOAuthLoginCoordinator::default());
    let registration = coordinator.register("yuandian-mcp", "request-1").await;
    let cancellation_token = registration.cancellation_token.clone();
    let cancel_coordinator = std::sync::Arc::clone(&coordinator);
    let cancel_task =
        tokio::spawn(async move { cancel_coordinator.cancel("yuandian-mcp", "request-1").await });
    tokio::task::yield_now().await;

    assert!(cancellation_token.is_cancelled());
    assert!(
        !cancel_task.is_finished(),
        "cancel must not return before the OAuth future has stopped"
    );

    coordinator
        .finish("yuandian-mcp", "request-1", registration.completion_sender)
        .await;
    assert!(cancel_task.await.unwrap());
    let newer = coordinator.register("yuandian-mcp", "request-2").await;
    assert!(
        !coordinator.cancel("yuandian-mcp", "stale-request").await,
        "a stale request id must not cancel a newer flow"
    );
    assert!(!newer.cancellation_token.is_cancelled());
    coordinator
        .finish("yuandian-mcp", "request-2", newer.completion_sender)
        .await;
}

#[tokio::test]
async fn forkguard_marketplace_oauth_remembers_cancel_before_register() {
    let coordinator = MarketplaceOAuthLoginCoordinator::default();
    assert!(
        coordinator
            .cancel("yuandian-mcp", "request-before-register")
            .await
    );

    let registration = coordinator
        .register("yuandian-mcp", "request-before-register")
        .await;
    assert!(
        registration.cancellation_token.is_cancelled(),
        "a fast UI cancel must stop an OAuth command even if it registers later"
    );
    coordinator
        .finish(
            "yuandian-mcp",
            "request-before-register",
            registration.completion_sender,
        )
        .await;
}

#[test]
fn uninstall_marketplace_tool_deletes_oauth_token_before_mcp_config() {
    let _g = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _home = TempPinvou3Home::new("oauth-uninstall");
    let server_name = "yuandian-mcp-uninstall-test";
    let url = "not-a-url";
    write_test_oauth_marketplace_files(
        server_name,
        serde_json::json!({
            "url": url,
            "scopes": ["legal"],
            "oauth": { "client_id": "test-client" }
        }),
    );
    let key = set_test_oauth_token(server_name, url, "not-json");
    assert!(test_oauth_token_exists(&key));

    uninstall_marketplace_tool_sync("yuandian-mcp").unwrap();

    assert!(!test_oauth_token_exists(&key));
    assert!(!crate::features::marketplace::MarketplaceManager::new()
        .installed_ids()
        .contains(&"yuandian-mcp".to_string()));
    let mcp_content = std::fs::read_to_string(crate::platform::paths::mcp_config_path()).unwrap();
    let mcp: serde_json::Value = serde_json::from_str(&mcp_content).unwrap();
    assert!(mcp["servers"].get(server_name).is_none());
}

#[test]
fn uninstall_marketplace_tool_aborts_if_oauth_token_delete_fails() {
    let _g = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _home = TempPinvou3Home::new("oauth-uninstall-error");
    let server_name = "yuandian-mcp-uninstall-error-test";
    write_test_oauth_marketplace_files(
        server_name,
        serde_json::json!({
            "scopes": ["legal"],
            "oauth": { "client_id": "test-client" }
        }),
    );

    let err = uninstall_marketplace_tool_sync("yuandian-mcp").unwrap_err();
    assert!(err.contains("删除 MCP OAuth token 失败"));
    assert!(crate::features::marketplace::MarketplaceManager::new()
        .installed_ids()
        .contains(&"yuandian-mcp".to_string()));
    let mcp_content = std::fs::read_to_string(crate::platform::paths::mcp_config_path()).unwrap();
    let mcp: serde_json::Value = serde_json::from_str(&mcp_content).unwrap();
    assert!(mcp["servers"].get(server_name).is_some());
}

struct TestPinvouHome {
    root: std::path::PathBuf,
    previous: Option<String>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Drop for TestPinvouHome {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn test_pinvou_home(tag: &str) -> TestPinvouHome {
    let guard = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let root = std::env::temp_dir().join(format!("{tag}-{}", std::process::id()));
    let previous = std::env::var("PINVOU3_HOME").ok();
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::env::set_var("PINVOU3_HOME", &root);
    TestPinvouHome {
        root,
        previous,
        _guard: guard,
    }
}

struct TestE2EFlag {
    previous: Option<String>,
}

impl TestE2EFlag {
    fn enable() -> Self {
        let previous = std::env::var("PINVOU3_E2E").ok();
        std::env::set_var("PINVOU3_E2E", "1");
        Self { previous }
    }
}

impl Drop for TestE2EFlag {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var("PINVOU3_E2E", value),
            None => std::env::remove_var("PINVOU3_E2E"),
        }
    }
}

struct RemoveE2EOnDrop {
    previous: Option<String>,
}

impl RemoveE2EOnDrop {
    fn clear() -> Self {
        let previous = std::env::var("PINVOU3_E2E").ok();
        std::env::remove_var("PINVOU3_E2E");
        Self { previous }
    }
}

impl Drop for RemoveE2EOnDrop {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var("PINVOU3_E2E", value),
            None => std::env::remove_var("PINVOU3_E2E"),
        }
    }
}

#[tokio::test]
async fn verify_upload_refuses_in_production_env() {
    let _guard = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _restore = RemoveE2EOnDrop::clear();
    let error = verify_upload("up_testprod".to_string()).await.unwrap_err();
    assert!(!error.contains(".pinvou3") && !error.contains('/') && !error.contains('\\'));
    assert!(error.contains("e2e") || error.contains("E2E") || error.contains("disabled"));
}

#[tokio::test]
async fn verify_upload_returns_sha256_when_e2e_enabled_and_not_leak_path() {
    let _home = test_pinvou_home("verify-upload-e2e");
    let _e2e = TestE2EFlag::enable();
    let upload_dir = crate::platform::paths::pinvou3_home()
        .join("uploads")
        .join("up_ok1");
    std::fs::create_dir_all(&upload_dir).unwrap();
    let data = b"hello verify_upload";
    std::fs::write(upload_dir.join("data.bin"), data).unwrap();
    let output = verify_upload("up_ok1".to_string()).await.unwrap();
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    let expected = crate::platform::encoding::hex_lower(&hasher.finalize());
    assert_eq!(output.sha256, expected);
    assert_eq!(output.byte_size, data.len() as u64);
}

#[tokio::test]
async fn verify_upload_missing_file_does_not_leak_path_or_distinguish_errors() {
    let _home = test_pinvou_home("verify-upload-missing");
    let _e2e = TestE2EFlag::enable();
    let error = verify_upload("up_missing1".to_string()).await.unwrap_err();
    assert!(!error.contains(".pinvou3") && !error.contains('/') && !error.contains('\\'));
    assert!(!error.contains("No such file") && !error.to_lowercase().contains("not found"));
}

fn session_artifact_path(session_id: &str, name: &str) -> std::path::PathBuf {
    let dir = crate::platform::paths::session_artifacts_dir(session_id);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn direct_skill_install_uninstall_scope_state_roundtrip() {
    use crate::features::assistant::skill_materialization as sm;
    use crate::features::marketplace::ConnectorScope;

    let _g = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let root = std::env::temp_dir().join(format!(
        "pinvou3-skill-reinstall-test-{}",
        std::process::id()
    ));
    let previous = std::env::var("PINVOU3_HOME").ok();
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::env::set_var("PINVOU3_HOME", &root);

    // 用户关闭 visualizer（独立 disabled_skills.json，不再借道连接器文件）→
    // 组合目录计算排除该技能。
    sm::save_disabled_skills_for(ConnectorScope::Plain, &["visualizer".to_string()]);
    install_marketplace_skill_sync("visualizer").unwrap();
    assert!(
        !sm::enabled_skills_for(ConnectorScope::Plain, None)
            .iter()
            .any(|(n, _)| n == "visualizer"),
        "安装后仍受用户关闭状态约束（plain scope 禁用集）"
    );
    // code scope 未初始化时新装技能默认全禁，初始化后自动加入 code 禁用集。
    assert!(sm::load_disabled_skills_for(ConnectorScope::Code)
        .iter()
        .any(|id| id == "visualizer"));

    uninstall_marketplace_skill_sync("visualizer").unwrap();
    // 卸载清除两个 scope 禁用集残留（与连接器同语义）→ 重装后默认启用。
    assert!(
        !sm::load_disabled_skills_for(ConnectorScope::Plain)
            .iter()
            .any(|id| id == "visualizer"),
        "卸载应从禁用集清除残留 id"
    );
    install_marketplace_skill_sync("visualizer").unwrap();
    assert!(
        sm::enabled_skills_for(ConnectorScope::Plain, None)
            .iter()
            .any(|(n, _)| n == "visualizer"),
        "卸载清除残留后重装默认启用（与连接器卸载语义一致）"
    );

    match previous {
        Some(value) => std::env::set_var("PINVOU3_HOME", value),
        None => std::env::remove_var("PINVOU3_HOME"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn file_url_from_path_encodes_local_artifact_paths() {
    let tmp = std::env::temp_dir().join(format!("pinvou3 file-url test {}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let path = tmp.join("中文 page.html");
    std::fs::write(&path, "<!doctype html>").unwrap();

    let canon = std::fs::canonicalize(&path).unwrap();
    let url = crate::platform::os::file_url_from_path(&canon).unwrap();
    let text = url.as_str();

    assert_eq!(url.scheme(), "file");
    assert!(text.starts_with("file://"), "unexpected file URL: {text}");
    assert!(
        !text.contains('\\'),
        "file URL must not contain backslashes: {text}"
    );
    assert!(
        !text.contains(r"\\?\"),
        "file URL must not contain verbatim prefix: {text}"
    );
    assert!(
        text.contains("%20"),
        "spaces should be percent-encoded: {text}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn write_artifact_text_allows_markdown() {
    let _home = test_pinvou_home("pinvou3-md-write-test");
    let md = session_artifact_path("s1", "note.md");
    std::fs::write(&md, "# Old\n").unwrap();

    write_artifact_text_impl(md.to_str().unwrap(), "# New\n\nBody").unwrap();

    assert_eq!(std::fs::read_to_string(&md).unwrap(), "# New\n\nBody");
}

#[test]
fn write_artifact_text_allows_markdown_extension() {
    let _home = test_pinvou_home("pinvou3-markdown-write-test");
    let md = session_artifact_path("s1", "note.markdown");
    std::fs::write(&md, "old").unwrap();

    write_artifact_text_impl(md.to_str().unwrap(), "new").unwrap();

    assert_eq!(std::fs::read_to_string(&md).unwrap(), "new");
}

#[test]
fn write_artifact_text_blocks_non_markdown() {
    let _home = test_pinvou_home("pinvou3-non-md-write-test");
    let txt = session_artifact_path("s1", "note.txt");
    std::fs::write(&txt, "old").unwrap();

    let err = write_artifact_text_impl(txt.to_str().unwrap(), "new").unwrap_err();

    assert!(err.contains("only markdown artifacts"));
    assert_eq!(std::fs::read_to_string(&txt).unwrap(), "old");
}

#[test]
fn write_artifact_text_blocks_markdown_outside_session_storage() {
    let _home = test_pinvou_home("pinvou3-md-outside-session-test");
    let outside =
        std::env::temp_dir().join(format!("pinvou3-md-outside-session-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(&outside).unwrap();
    let md = outside.join("note.md");
    std::fs::write(&md, "old").unwrap();

    let err = write_artifact_text_impl(md.to_str().unwrap(), "new").unwrap_err();

    assert!(err.contains("outside session storage"));
    assert_eq!(std::fs::read_to_string(&md).unwrap(), "old");
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn write_artifact_text_blocks_sensitive_path() {
    let tmp = std::env::temp_dir().join(format!(
        "pinvou3-sensitive-md-write-test-{}",
        std::process::id()
    ));
    let sensitive_dir = tmp.join(".ssh");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&sensitive_dir).unwrap();
    let md = sensitive_dir.join("note.md");
    std::fs::write(&md, "old").unwrap();

    let err = write_artifact_text_impl(md.to_str().unwrap(), "new").unwrap_err();

    assert!(err.contains("sensitive component"));
    assert_eq!(std::fs::read_to_string(&md).unwrap(), "old");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn write_artifact_text_requires_existing_file() {
    let tmp = std::env::temp_dir().join(format!(
        "pinvou3-missing-md-write-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let md = tmp.join("missing.md");

    let err = write_artifact_text_impl(md.to_str().unwrap(), "new").unwrap_err();

    assert!(err.contains("not a file"));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn write_artifact_text_blocks_directory_target() {
    let _home = test_pinvou_home("pinvou3-md-dir-write-test");
    let md_dir = session_artifact_path("s1", "note.md");
    std::fs::create_dir_all(&md_dir).unwrap();

    let err = write_artifact_text_impl(md_dir.to_str().unwrap(), "new").unwrap_err();

    assert!(err.contains("not a file"));
    assert!(md_dir.is_dir());
}

#[test]
fn write_artifact_text_blocks_oversized_content() {
    let _home = test_pinvou_home("pinvou3-md-large-write-test");
    let md = session_artifact_path("s1", "note.md");
    std::fs::write(&md, "old").unwrap();
    let content = "x".repeat(MAX_EDITABLE_MARKDOWN_BYTES + 1);

    let err = write_artifact_text_impl(md.to_str().unwrap(), &content).unwrap_err();

    assert!(err.contains("too large"));
    assert_eq!(std::fs::read_to_string(&md).unwrap(), "old");
}

#[test]
fn write_artifact_text_preserves_utf8_content() {
    let _home = test_pinvou_home("pinvou3-md-utf8-write-test");
    let md = session_artifact_path("s1", "note.md");
    std::fs::write(&md, "old").unwrap();
    let content =
        "# 标题\n\n| 名称 | 状态 |\n| --- | --- |\n| Alpha | 进行中 |\n\n```text\n保留中文\n```";

    write_artifact_text_impl(md.to_str().unwrap(), content).unwrap();

    assert_eq!(std::fs::read_to_string(&md).unwrap(), content);
}

#[test]
fn write_artifact_text_cleans_backup_file_after_success() {
    let _home = test_pinvou_home("pinvou3-md-backup-clean-test");
    let md = session_artifact_path("s1", "note.md");
    std::fs::write(&md, "old").unwrap();

    write_artifact_text_impl(md.to_str().unwrap(), "new").unwrap();

    let leftovers: Vec<_> = std::fs::read_dir(md.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".bak-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "backup files left behind: {leftovers:?}"
    );
    assert_eq!(std::fs::read_to_string(&md).unwrap(), "new");
}

#[test]
fn write_artifact_text_cleans_temp_file_on_error() {
    let tmp = std::env::temp_dir().join(format!(
        "pinvou3-temp-clean-write-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let md = tmp.join("note.md");
    std::fs::create_dir_all(&md).unwrap();

    let err = atomic_write_utf8(&md, "new").unwrap_err();

    assert!(!err.to_string().is_empty());
    let leftovers: Vec<_> = std::fs::read_dir(&tmp)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left behind: {leftovers:?}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// accept_plan 切 Yolo 后注入的执行指令必须裹住方案全文 + 带"立即执行"信号——
/// 否则切了模式但 AI 收到一句空指令,不知道要执行什么(切了白切)。
#[test]
fn accept_plan_instruction_embeds_full_plan() {
    let plan = "1. 建目录\n2. 写 index.html\n3. 起本地服务器";
    let msg = accept_plan_instruction(plan);
    assert!(msg.contains(plan), "注入指令丢了方案全文");
    assert!(msg.contains("立即开始执行"), "缺少明确执行信号");
}

/// 挂集时 Self-RAG 引导:含知识集名 + 必调 kb_search + 按需 kb_open_source + 禁止二进制
/// read_file + 无依据说不知道;空名兜底。
#[test]
fn agentic_guide_mentions_collection_and_kb_search() {
    let g = build_kb_agentic_guide(&["硬件资料".to_string(), "团队规范".to_string()]);
    assert!(g.contains("《硬件资料》"));
    assert!(g.contains("kb_search"));
    assert!(g.contains("kb_open_source"));
    assert!(g.contains("不要对 XLSX/"));
    assert!(g.contains("绝不凭记忆编造"));
    assert!(g.contains("《团队规范》"));
    assert!(build_kb_agentic_guide(&[]).contains("《本地知识集》"));
}

#[test]
fn parses_local_asr_plain_text_output() {
    let text = parse_local_asr_text("hello from voice\n", "").expect("plain text");
    assert_eq!(text, "hello from voice");
}

#[test]
fn explicit_asr_cli_fallback_accepts_all_supported_env_names() {
    assert!(!has_nonempty_asr_cli_config(&[None, None, None]));
    assert!(!has_nonempty_asr_cli_config(&[Some(""), Some(" \t"), None]));
    assert!(has_nonempty_asr_cli_config(&[
        Some("/tmp/pinvou-asr"),
        None,
        None
    ]));
    assert!(has_nonempty_asr_cli_config(&[
        None,
        Some("/tmp/deepspeech2"),
        None
    ]));
    assert!(has_nonempty_asr_cli_config(&[
        None,
        None,
        Some("/tmp/paddlespeech")
    ]));
}

#[test]
fn parses_local_asr_list_output() {
    let text =
        parse_local_asr_text("[INFO] loading\n['hello from voice']\n", "").expect("list output");
    assert_eq!(text, "hello from voice");
}

#[test]
fn local_asr_env_points_wrapper_to_resolved_model_path() {
    let root = std::env::temp_dir().join(format!("pinvou3 local asr env {}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let model = root.join("sensevoice-small-q8.gguf");
    std::fs::write(&model, b"model").unwrap();

    let mut command = std::process::Command::new("pinvou-asr");
    apply_local_asr_model_env(&mut command, Some(model.clone()));
    let configured = command
        .get_envs()
        .find(|(key, _)| *key == std::ffi::OsStr::new("PINVOU3_SENSEVOICE_MODEL"))
        .and_then(|(_, value)| value);

    assert_eq!(configured, Some(model.as_os_str()));
    let _ = std::fs::remove_dir_all(&root);
}

/// present_artifact 漂工具名失败时,成品卡兜底用 write_file 的相对 path
/// (如 `snake-game.html`),点 Open 必须先按 session workspace 解析成绝对路径,
/// 否则 `validate_user_path` 直接拒「path must be absolute」。
#[test]
fn resolve_artifact_path_relative_joins_active_workspace() {
    let _g = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-resolve-test");
    let store = SessionStore::boot().expect("boot");

    // 无 active session 且无显式 session → 相对路径原样返回(行为同旧版)
    assert_eq!(
        resolve_artifact_path("snake-game.html", None, &store).expect("no active workspace"),
        "snake-game.html"
    );

    // 有 active session、无显式 session → 回退 active 的 workspace
    store.set_active(Some("sess-1".into()));
    let want = crate::platform::paths::session_workspace_dir("sess-1")
        .join("snake-game.html")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        resolve_artifact_path("snake-game.html", None, &store).expect("active workspace"),
        want
    );

    // 显式 session **优先**于 active:卡片自带 session 才能跨会话切换稳定解析
    // (active 停在切走时去的会话也不影响)。这是本次跨 session「打不开」的修复点。
    let want_explicit = crate::platform::paths::session_workspace_dir("sess-owner")
        .join("snake-game.html")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        resolve_artifact_path("snake-game.html", Some("sess-owner"), &store)
            .expect("explicit workspace"),
        want_explicit
    );

    // 绝对路径原样返回,无视 session(present_artifact 成功 / 产物面板给的已是绝对)
    let absolute = std::env::current_dir().unwrap().join("artifact-test.html");
    let absolute = absolute.to_string_lossy().to_string();
    assert_eq!(
        resolve_artifact_path(&absolute, Some("sess-owner"), &store).expect("absolute artifact"),
        absolute
    );
}

#[test]
fn scheduled_attachment_staging_and_artifact_resolution_use_task_workspace() {
    use crate::features::sessions::{ScheduledRunMode, ScheduledRunProfile};

    let _g = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = std::env::temp_dir().join(format!(
        "pinvou3-scheduled-workspace-test-{}",
        std::process::id()
    ));
    let previous = std::env::var("PINVOU3_HOME").ok();
    let _ = std::fs::remove_dir_all(&root);
    std::env::set_var("PINVOU3_HOME", &root);
    let scheduled_root = root.join("scheduled");
    std::fs::create_dir_all(&root).expect("test root");
    let source = root.join("source.png");
    std::fs::write(&source, b"\x89PNG\r\n\x1a\nfake-bytes").expect("image source");
    let store =
        SessionStore::boot_with_scheduled_root(scheduled_root.clone()).expect("session store");
    let scheduled = store
        .create_scheduled_run(ScheduledRunProfile {
            task_id: "workspace-task".to_string(),
            model: "scheduled-model".to_string(),
            model_id: None,
            workspace: root.join("ignored-task-workspace"),
            mode: ScheduledRunMode::Agent,
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
        })
        .expect("scheduled session");
    // transcript 覆盖类命令仍拒绝 scheduled 会话(引擎独占持久化)。
    let manage_error = ensure_chat_session(&store, &scheduled.metadata.id, "save_session_messages")
        .expect_err("scheduled runs must reject UI transcript overwrites");
    assert!(manage_error.contains("scheduled-run sessions are managed from Scheduled"));
    let locked = store
        .ledger_root(&scheduled.metadata.id)
        .expect("locked scheduled workspace");
    // 每次运行对话独立，同一 automation 共享自己的工作间；输入路径不会生效。
    assert_eq!(
        locked,
        scheduled_root.join("workspace-task").join("workspace")
    );
    let workspace = locked.clone();
    std::fs::create_dir_all(&workspace).expect("session workspace");
    let staged_dir = format!(
        ".pinvou3/scheduled-attachments/{}/attachments",
        scheduled.metadata.id
    );
    let prompt = build_message_with_attachments_in_dir(
        "inspect".to_string(),
        vec![crate::features::files::file_ingest::ingest(&source)],
        &locked,
        &staged_dir,
        false,
    );

    let staged_relative = format!("{staged_dir}/source.png");
    let large_text_relative = format!("{staged_dir}/big.txt");
    let large_text_prompt = build_message_with_attachments_in_dir(
        "inspect all".to_string(),
        vec![mk_attachment("text", "big.log", 9_000, 50_000)],
        &locked,
        &staged_dir,
        false,
    );
    let report = workspace.join("report.md");
    std::fs::write(&report, "scheduled result").expect("scheduled workspace artifact");

    assert_eq!(locked, workspace);
    assert!(workspace.join(&staged_relative).exists());
    assert!(prompt.contains(&staged_relative));
    assert!(workspace.join(&large_text_relative).exists());
    assert!(large_text_prompt.contains(&large_text_relative));
    assert!(
        list_workspace_files_for_session(&scheduled.metadata.id, &store)
            .expect("scan scheduled workspace")
            .contains(&report.to_string_lossy().into_owned()),
        "workspace reconciliation must scan the configured execution workspace"
    );
    assert_eq!(
        resolve_artifact_path("report.md", Some(&scheduled.metadata.id), &store)
            .expect("scheduled artifact workspace"),
        workspace.join("report.md").to_string_lossy().into_owned()
    );

    drop(store);
    match previous {
        Some(value) => std::env::set_var("PINVOU3_HOME", value),
        None => std::env::remove_var("PINVOU3_HOME"),
    }
    let _ = std::fs::remove_dir_all(root);
}

/// 定时运行会话按 SessionKind 分发后，重命名/置顶/归档复用 Session 元数据
/// (rename_session / set_session_pinned / set_session_archived 的真实后端路径)，
/// 而删除仍然只能走 automation 联动，不允许把 sched-* 当普通会话直删。
#[test]
fn scheduled_session_metadata_dispatch_supports_rename_pin_archive() {
    use crate::features::sessions::{ScheduledRunMode, ScheduledRunProfile};

    let _g = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = std::env::temp_dir().join(format!(
        "pinvou3-scheduled-metadata-test-{}",
        std::process::id()
    ));
    let previous = std::env::var("PINVOU3_HOME").ok();
    let _ = std::fs::remove_dir_all(&root);
    std::env::set_var("PINVOU3_HOME", &root);
    let store =
        SessionStore::boot_with_scheduled_root(root.join("scheduled")).expect("session store");
    let scheduled = store
        .create_scheduled_run(ScheduledRunProfile {
            task_id: "metadata-task".to_string(),
            model: "scheduled-model".to_string(),
            model_id: None,
            workspace: root.join("ignored"),
            mode: ScheduledRunMode::Yolo,
            allow_shell: false,
            trust_mode: true,
            auto_approve: true,
        })
        .expect("scheduled session");
    let id = scheduled.metadata.id.clone();
    assert!(matches!(
        store.session_kind(&id),
        Ok(SessionKind::ScheduledRun)
    ));

    // rename_session 路径：set_title 对 scheduled 会话生效并落盘。
    store
        .set_title(&id, "重命名后的定时运行".to_string())
        .expect("rename");
    assert_eq!(
        store.load(&id).expect("reload").metadata.title,
        "重命名后的定时运行"
    );

    // set_session_pinned 路径：共用置顶表。
    store.set_pinned(&id, true);
    assert!(store.is_pinned(&id));
    assert!(store.pinned_at(&id).is_some());

    // set_session_archived 路径：共用收起表,且归档列表能列出 sched-* 会话。
    store.set_hidden(&id, true);
    assert!(store.is_hidden(&id));
    assert!(store
        .list_scheduled()
        .expect("list scheduled")
        .iter()
        .any(|metadata| metadata.id == id));
    // 收起会强制取消置顶(与普通会话一致)。
    assert!(!store.is_pinned(&id));
    store.set_hidden(&id, false);
    assert!(!store.is_hidden(&id));

    // 删除不允许绕过 automation 联动直删。
    let delete_error = store
        .delete(&id)
        .expect_err("scheduled sessions must not be deleted as ordinary chats");
    assert!(delete_error
        .to_string()
        .contains("deleted through their automation"));

    drop(store);
    match previous {
        Some(value) => std::env::set_var("PINVOU3_HOME", value),
        None => std::env::remove_var("PINVOU3_HOME"),
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ordinary_ledger_root_behavior_is_unchanged() {
    let _g = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = std::env::temp_dir().join(format!(
        "pinvou3-chat-workspace-test-{}",
        std::process::id()
    ));
    let previous = std::env::var("PINVOU3_HOME").ok();
    let _ = std::fs::remove_dir_all(&root);
    std::env::set_var("PINVOU3_HOME", &root);
    let store = SessionStore::boot().expect("session store");
    let chat = store
        .create_new(
            "chat-model".to_string(),
            None,
            root.join("legacy-metadata-workspace"),
        )
        .expect("ordinary chat");
    let expected = crate::platform::paths::session_workspace_dir(&chat.metadata.id);

    assert_eq!(
        store
            .ledger_root(&chat.metadata.id)
            .expect("ordinary ledger root"),
        expected
    );
    assert_eq!(
        resolve_artifact_path("report.md", Some(&chat.metadata.id), &store)
            .expect("ordinary artifact workspace"),
        expected.join("report.md").to_string_lossy().into_owned()
    );

    drop(store);
    match previous {
        Some(value) => std::env::set_var("PINVOU3_HOME", value),
        None => std::env::remove_var("PINVOU3_HOME"),
    }
    let _ = std::fs::remove_dir_all(root);
}

/// merge_resolutions 锁住实测 bug:勾选写进 sidecar 的 resolution 不能被后续不含
/// resolution 的全量 save(典型=核账 record 的原始快照)覆盖,否则核账无法跳过「接受现状」。
#[test]
fn merge_preserves_old_resolution_when_new_missing() {
    let old = serde_json::json!([
        {"pos":10,"review":{"issues":[{"text":"a","resolution":"modify"},{"text":"b","resolution":"accept"}],"recommendations":[]}}
    ]);
    // 核账 record 的 new:同一条 review 的 issues 不带 resolution(原始快照态)+ 追加 pos=44
    let new = serde_json::json!([
        {"pos":10,"review":{"issues":[{"text":"a"},{"text":"b"}],"recommendations":[]}},
        {"pos":44,"review":{"issues":[],"recommendations":[]}}
    ]);
    let merged = merge_resolutions(old, new);
    assert_eq!(merged[0]["review"]["issues"][0]["resolution"], "modify");
    assert_eq!(merged[0]["review"]["issues"][1]["resolution"], "accept");
}

#[test]
fn merge_respects_new_resolution_for_changed_verdict() {
    // Boss 改裁决:new 自带新值,不被 old 覆盖
    let old = serde_json::json!([{"pos":10,"review":{"issues":[{"text":"a","resolution":"modify"}],"recommendations":[]}}]);
    let new = serde_json::json!([{"pos":10,"review":{"issues":[{"text":"a","resolution":"accept"}],"recommendations":[]}}]);
    let merged = merge_resolutions(old, new);
    assert_eq!(merged[0]["review"]["issues"][0]["resolution"], "accept");
}

#[test]
fn merge_covers_recommendations_field() {
    let old = serde_json::json!([{"pos":1,"review":{"issues":[],"recommendations":[{"topic":"t","resolution":"modify"}]}}]);
    let new =
        serde_json::json!([{"pos":1,"review":{"issues":[],"recommendations":[{"topic":"t"}]}}]);
    let merged = merge_resolutions(old, new);
    assert_eq!(
        merged[0]["review"]["recommendations"][0]["resolution"],
        "modify"
    );
}

/// `open_external_url` 只允许已审计外部入口和严格本机 loopback HTTP(S)。
/// 任意其他 host / 危险 scheme 都立即 reject，这是前端 webview 的最后一道防线。
#[tokio::test]
async fn open_external_url_rejects_off_allowlist_targets() {
    let rejected = [
        "http://metaso.cn/",                                      // 非 https
        "https://evil.example.com/",                              // host 不在白名单
        "https://metaso.cn.evil.com/",                            // 子域钓鱼
        "https://console.bce.baidu.com.evil.com/",                // 百度子域钓鱼
        "https://app.tavily.com.evil.com/",                       // tavily 子域钓鱼
        "https://www.canva.cn.evil.com/api/action",               // Canva 子域钓鱼
        "https://export-download.canva.cn.evil.com/x.png",        // Canva 资源域钓鱼
        "https://meeting.tencent.com.evil.com/qrcode-login.html", // 腾讯会议授权域钓鱼
        "https://bce.baidu.com/",                                 // 非 console 子域,不放行
        "javascript:alert(1)",                                    // js scheme
        "file:///etc/passwd",                                     // file scheme
        "https://google.com/",                                    // 任何第三方域
        "http://localhost.evil.com:8080/",                        // localhost 前缀钓鱼
        "http://user@localhost:8080/",                            // 不允许 URL 凭据
        "http://192.168.1.10:8080/",                              // 局域网地址不是 loopback
        "",                                                       // 空串
        "metaso.cn/",                                             // 缺 scheme
    ];
    for url in rejected {
        let err = open_external_url(url.to_string()).await.err();
        assert!(err.is_some(), "must reject URL: {url:?}");
        assert!(
            err.as_deref().unwrap().contains("allowlist"),
            "reject reason should name allowlist for {url:?}, got {err:?}"
        );
    }
}

#[test]
fn user_clicked_external_urls_allow_http_https_only() {
    for allowed in [
        "https://example.com/path?q=1#section",
        "http://127.0.0.1:8080/preview",
        "https://例子.测试/",
    ] {
        assert!(
            user_external_url(allowed).is_some(),
            "user-clicked HTTP(S) URL should be accepted: {allowed}"
        );
    }
    for rejected in [
        "javascript:alert(1)",
        "file:///etc/passwd",
        "data:text/html,hello",
        "https://user@example.com/",
        "https://user:secret@example.com/",
        "https:///missing-host",
        "https:\\\\example.com",
        "https://\\example.com",
        "",
    ] {
        assert!(
            user_external_url(rejected).is_none(),
            "unsafe user-clicked URL must be rejected: {rejected}"
        );
    }
}

/// 扩 EXTERNAL_URL_ALLOWLIST 必须加测试:目标域名放行,仿冒/非 https 拒绝。
#[test]
fn external_allowlist_allows_known_targets_rejects_lookalikes() {
    assert!(url_in_external_allowlist("https://obsidian.md/download"));
    assert!(url_in_external_allowlist("https://metaso.cn/"));
    assert!(url_in_external_allowlist(
        "https://console.amap.com/dev/key/app"
    ));
    assert!(url_in_external_allowlist(
        "https://ima.qq.com/agent-interface"
    ));
    assert!(!url_in_external_allowlist(
        "https://open.chineselaw.com/oauth/authorize"
    ));
    assert!(!url_in_external_allowlist(
        "https://passport.legalmind.cn/ssologin?appId=apiplatform"
    ));
    assert!(url_in_external_allowlist(
        "https://open.zhihuiya.com/dashboard/api-keys"
    ));
    assert!(url_in_external_allowlist(
        "https://www.canva.cn/api/action?token=abc"
    ));
    assert!(url_in_external_allowlist(
        "https://export-download.canva.cn/example/preview.png"
    ));
    assert!(url_in_external_allowlist(
        "https://meeting.tencent.com/qrcode-login.html?code=abc"
    ));
    assert!(url_in_external_allowlist("http://localhost:8080/"));
    assert!(url_in_external_allowlist("https://127.0.0.1:8443/preview"));
    assert!(url_in_external_allowlist("http://[::1]:3000/"));
    assert!(!url_in_external_allowlist("https://obsidian.md.evil.com/"));
    assert!(!url_in_external_allowlist(
        "https://console.amap.com.evil.com/dev/key/app"
    ));
    assert!(!url_in_external_allowlist("https://ima.qq.com.evil.com/"));
    assert!(!url_in_external_allowlist(
        "https://open.zhihuiya.com.evil.com/dashboard/api-keys"
    ));
    assert!(!url_in_external_allowlist(
        "https://www.canva.cn.evil.com/api/action?token=abc"
    ));
    assert!(!url_in_external_allowlist(
        "https://export-download.canva.cn.evil.com/example/preview.png"
    ));
    assert!(!url_in_external_allowlist(
        "https://meeting.tencent.com.evil.com/qrcode-login.html"
    ));
    assert!(!url_in_external_allowlist("http://obsidian.md/"));
    assert!(!url_in_external_allowlist("http://meeting.tencent.com/"));
    assert!(!url_in_external_allowlist("http://open.zhihuiya.com/"));
    assert!(!url_in_external_allowlist(
        "http://www.canva.cn/api/action?token=abc"
    ));
    assert!(!url_in_external_allowlist("https://evil.example.com/"));
    assert!(!url_in_external_allowlist(
        "http://localhost.evil.com:8080/"
    ));
    assert!(!url_in_external_allowlist("http://192.168.1.10:8080/"));
    assert!(!url_in_external_allowlist("file://localhost/tmp/demo.html"));
}

/// detect_obsidian 选库规则:open:true 优先,否则 ts 最大;空/无 vaults → None;容忍 BOM。
#[test]
fn pick_vault_path_prefers_open_then_latest() {
    let j = r#"{"vaults":{"a":{"path":"/A","ts":100},"b":{"path":"/B","ts":1,"open":true}}}"#;
    assert_eq!(pick_vault_path(j).as_deref(), Some("/B"));
    let j = r#"{"vaults":{"a":{"path":"/A","ts":100},"b":{"path":"/B","ts":9}}}"#;
    assert_eq!(pick_vault_path(j).as_deref(), Some("/A"));
    assert_eq!(pick_vault_path(r#"{"vaults":{}}"#), None);
    let j = "\u{feff}{\"vaults\":{\"a\":{\"path\":\"/A\",\"ts\":1,\"open\":true}}}";
    assert_eq!(pick_vault_path(j).as_deref(), Some("/A"));
}

/// 成品归属推断:成品词出现在质检节时必须归被审对象,不归审核者
/// (天真就近归因实测把成品判给 xingbu——刑部节里"对礼部整合的最终报告
/// 进行审核"的关键词全在审核者章节内)。
#[test]
fn infer_product_bu_attributes_to_audited_not_auditor() {
    let report = "\
## 各部成果\n\n\
### 4. 礼部（libu_1.md）—— 报告撰写与方案整合\n\n\
核心产出：将各部数据整合为完整的研究报告\n\n\
### 5. 刑部（xingbu_1.md）—— 方案质量审核验收\n\n\
核心产出：对礼部整合的最终报告进行全面质量审核\n\n\
## 结果对账\n\n\
| 具体方案交付 | ✅ 达成 | 礼部报告提供三套完整方案 |\n";
    let (bu, score) = infer_product_bu(report).expect("应有推断结果");
    assert_eq!(bu, "libu", "成品应归被审的礼部,不归审核者刑部");
    assert!(score >= 8, "多路信号应汇聚到可信阈值,实得 {score}");
}

/// libu_ 前缀不得误吃 libu_renshi_N.md(后随数字才算点名)。
#[test]
fn infer_product_bu_no_prefix_confusion() {
    let report = "### 吏部（libu_renshi_1.md）—— 流程规范\n\n整合完整的最终成品汇总\n";
    let (bu, _) = infer_product_bu(report).expect("应有推断结果");
    assert_eq!(bu, "libu_renshi");
}

/// L2-1: A 方案放宽 — /tmp 下任意文件可校验通过（不强 $HOME 限制）。
#[test]
fn validate_user_path_allows_tmp() {
    let tmp = std::env::temp_dir().join("pinvou3-validate-test");
    std::fs::create_dir_all(&tmp).unwrap();
    let p = tmp.join("foo.txt");
    std::fs::write(&p, "").unwrap();
    let result = validate_user_path(p.to_str().unwrap());
    std::fs::remove_dir_all(&tmp).ok();
    assert!(result.is_ok(), "/tmp 下普通文件应该通过, got {result:?}");
}

/// L2-2: 凭据组件名拦截 — 任何路径深度命中 .ssh/id_rsa 即拒绝。
#[test]
fn validate_user_path_blocks_ssh() {
    // 不需要文件真实存在，canonicalize 失败时退回 raw path 继续校验
    let p = "/home/anyuser/.ssh/id_rsa";
    let result = validate_user_path(p);
    assert!(result.is_err(), "凭据路径必须拒绝, got {result:?}");
    let err = result.unwrap_err();
    assert!(
        err.contains(".ssh") || err.contains("id_rsa"),
        "错误信息应指明命中的组件, got {err}"
    );
}

/// L2-3: 系统级敏感前缀拦截 — /etc/shadow 等被列在 BLOCKED_PREFIXES。
#[test]
fn validate_user_path_blocks_etc_shadow() {
    if !crate::platform::capabilities::is_unix() {
        return;
    }
    let result = validate_user_path("/etc/shadow");
    assert!(result.is_err(), "/etc/shadow 必须拒绝, got {result:?}");
    assert!(result.unwrap_err().contains("system-sensitive"));
}

/// 图片附件 → build_message_with_attachments 的视觉通路:验证图被拷进 workspace
/// 的 attachments/ 且 prompt 引导 LLM 调 image_analyze(不再走 OCR)。
/// 用临时 png + 临时 workspace,不依赖外部文件,正常跑(非 ignore)。
#[test]
fn image_attachment_stages_and_guides_image_analyze() {
    let tmp = std::env::temp_dir().join(format!("pinvou3-vis-test-{}", std::process::id()));
    let ws = tmp.join("workspace");
    std::fs::create_dir_all(&ws).expect("建 workspace");
    let src = tmp.join("shot.png");
    std::fs::write(&src, b"\x89PNG\r\n\x1a\nfake-bytes").expect("写假 png");

    let r = crate::features::files::file_ingest::ingest(&src);
    assert_eq!(r.kind, "image", "应识别为 image");
    assert!(
        r.markdown.is_none(),
        "图片不再预解析出 markdown(OCR 已移除)"
    );

    let prompt = build_message_with_attachments("这张图里画了什么？".to_string(), vec![r], &ws);
    assert!(
        prompt.contains("image_analyze"),
        "prompt 应引导调 image_analyze"
    );
    assert!(
        prompt.contains("attachments/shot.png"),
        "prompt 应给出 workspace 相对路径"
    );
    assert!(
        ws.join("attachments/shot.png").exists(),
        "图片应被拷进 workspace 的 attachments/"
    );
    assert!(!prompt.contains("没有视觉能力"), "不应再出现无视觉提示");

    let _ = std::fs::remove_dir_all(&tmp);
}

// ── Native 图片输入(设计 §9.2,阶段 D)─────────────────────────────────

fn mk_image_attachment(
    basename: &str,
    path: &Path,
    byte_size: u64,
) -> crate::features::files::file_ingest::IngestResult {
    crate::features::files::file_ingest::IngestResult {
        kind: "image".into(),
        basename: basename.into(),
        path: path.to_string_lossy().to_string(),
        markdown: None,
        token_estimate: 0,
        byte_size,
        warning: None,
    }
}

/// Native(v0.9.5 官方标记方案):文字 + 多图 + 文本附件 → 消息文本含图片
/// `[Attached image: <path>]` 标记行(按用户选择顺序);不注入"看不到图"
/// 硬规则、不引导 image_analyze;文本附件 markdown 内联行为与 Fallback
/// 路径一致;标记路径只来自暂存结果(不接受前端直给路径)。
#[test]
fn native_image_message_builds_marker_lines_in_order() {
    let tmp = std::env::temp_dir().join(format!("pinvou3-native-test-{}", std::process::id()));
    let ws = tmp.join("workspace");
    std::fs::create_dir_all(&ws).expect("建 workspace");
    let src_a = tmp.join("a.png");
    let src_b = tmp.join("b.jpg");
    std::fs::write(&src_a, b"\x89PNG\r\n\x1a\nfake-a").expect("写假 png");
    std::fs::write(&src_b, b"\xff\xd8\xfffake-b").expect("写假 jpg");
    let text_attachment = mk_attachment("text", "notes.txt", 2, 10);

    let segment = prepare_native_user_message_in_dir(
        "这几张图和笔记说明了什么？".to_string(),
        vec![
            mk_image_attachment("a.png", &src_a, 15),
            text_attachment,
            mk_image_attachment("b.jpg", &src_b, 11),
        ],
        &ws,
        "attachments",
    )
    .expect("Native 构造应成功");

    assert!(segment.contains("这几张图和笔记说明了什么？"));
    assert!(
        !segment.contains("看不到这张图") && !segment.contains("image_analyze"),
        "Native 不得注入 image_analyze 硬规则提示,得到:\n{segment}"
    );
    assert!(
        !segment.contains(src_a.to_string_lossy().as_ref()),
        "图片的 workspace 外绝对路径不得进模型消息(设计 §10.1)"
    );
    assert!(
        segment.contains("### a.png (image, 15 bytes)")
            && segment.contains("### b.jpg (image, 11 bytes)"),
        "附件清单应保留图片条目"
    );
    assert!(
        segment.contains("row-1,value-1"),
        "文本附件 markdown 应照旧内联"
    );
    // 官方标记行按用户选择顺序出现,路径为暂存后的 workspace 路径。
    let marker_a = format!(
        "[Attached image: {}]",
        ws.join("attachments/a.png").display()
    );
    let marker_b = format!(
        "[Attached image: {}]",
        ws.join("attachments/b.jpg").display()
    );
    let marker_a_pos = segment.find(&marker_a).expect("a.png 标记行必须存在");
    let marker_b_pos = segment.find(&marker_b).expect("b.jpg 标记行必须存在");
    assert!(
        marker_a_pos < marker_b_pos,
        "标记行按用户选择顺序(先 a 后 b):\n{segment}"
    );
    // 暂存产物真实落盘,路径来自暂存结果。
    assert!(ws.join("attachments/a.png").exists());
    assert!(ws.join("attachments/b.jpg").exists());

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Native: 纯图片消息(空文字 + 图片)允许发送,不伪造用户文字(设计 §9.4)。
#[test]
fn native_image_message_allows_pure_image() {
    let tmp = std::env::temp_dir().join(format!("pinvou3-native-pure-{}", std::process::id()));
    let ws = tmp.join("workspace");
    std::fs::create_dir_all(&ws).expect("建 workspace");
    let src = tmp.join("only.png");
    std::fs::write(&src, b"\x89PNG\r\n\x1a\nfake").expect("写假 png");

    let segment = prepare_native_user_message_in_dir(
        String::new(),
        vec![mk_image_attachment("only.png", &src, 13)],
        &ws,
        "attachments",
    )
    .expect("纯图片消息应允许");

    // 只有附件清单与标记行,没有伪造的用户文字。
    assert!(segment.contains("用户附上了以下文件"));
    assert!(
        segment.contains(&format!(
            "[Attached image: {}]",
            ws.join("attachments/only.png").display()
        )),
        "图片标记行必须存在:\n{segment}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Native: 任一图片暂存失败 → 整条消息 Err,不静默降级为纯文本(设计 §10.2)。
#[test]
fn native_image_message_stage_failure_is_error_not_silent_text() {
    let tmp = std::env::temp_dir().join(format!("pinvou3-native-fail-{}", std::process::id()));
    let ws = tmp.join("workspace");
    std::fs::create_dir_all(&ws).expect("建 workspace");
    let missing = tmp.join("missing.png"); // 故意不创建

    let result = prepare_native_user_message_in_dir(
        "看图".to_string(),
        vec![mk_image_attachment("missing.png", &missing, 0)],
        &ws,
        "attachments",
    );
    let error = result.expect_err("暂存失败必须报错");
    assert!(
        error.contains("missing.png") && error.contains("暂存"),
        "错误应指明哪张图暂存失败: {error}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Native 构造函数对纯文本/非图片附件的回归:与文本拼接路径结果一致。
#[test]
fn native_prepare_text_only_and_non_image_regression() {
    let ws = mk_test_ws("native-regression");

    let text_only =
        prepare_native_user_message_in_dir("你好".to_string(), Vec::new(), &ws, "attachments")
            .expect("纯文本");
    assert_eq!(text_only, "你好");

    let with_text = prepare_native_user_message_in_dir(
        "查全文".to_string(),
        vec![mk_attachment("text", "a.txt", 3, 10)],
        &ws,
        "attachments",
    )
    .expect("文本附件");
    let legacy = build_message_with_attachments(
        "查全文".to_string(),
        vec![mk_attachment("text", "a.txt", 3, 10)],
        &ws,
    );
    assert_eq!(
        with_text, legacy,
        "非图片附件路径必须与现有文本拼接结果一致"
    );

    let _ = std::fs::remove_dir_all(&ws);
}

/// Unsupported 路径的稳定错误码(设计 §9.2):前端按码匹配,码后必须带
/// 用户可操作指引。钉住格式防手滑改码。
#[test]
fn image_input_unsupported_error_keeps_stable_code() {
    assert!(super::chat::IMAGE_INPUT_UNSUPPORTED_ERROR.starts_with("image_input_unsupported"));
    assert!(super::chat::IMAGE_INPUT_UNSUPPORTED_ERROR.contains("不支持图片"));
    assert!(super::chat::IMAGE_INPUT_UNSUPPORTED_ERROR.contains("视觉模型"));
}

/// Unknown 与确认不支持共用稳定错误码前缀,但 Unknown 文案必须给出显式确认出口
/// (设计 §6.3:Unknown 不冒充支持,允许用户手工开启)。
#[test]
fn image_input_unknown_error_offers_explicit_confirm_path() {
    assert!(super::chat::IMAGE_INPUT_UNKNOWN_ERROR.starts_with("image_input_unsupported"));
    assert!(super::chat::IMAGE_INPUT_UNKNOWN_ERROR.contains("能力未知"));
    assert!(super::chat::IMAGE_INPUT_UNKNOWN_ERROR.contains("支持图片"));
}

/// `get_image_input_capability` 返回体的 wire 形状:前端按字段名与字符串值匹配
/// (选图即时警告),钉住防手滑改字段。
#[test]
fn image_input_capability_info_serializes_stable_fields() {
    let info = super::settings::ImageInputCapabilityInfo {
        capability: "unknown".to_string(),
        image_mode: "vision_tool_fallback".to_string(),
        has_vision_model: true,
        is_local_endpoint: false,
        vision_is_local_endpoint: Some(false),
    };
    let value = serde_json::to_value(&info).expect("serialize ImageInputCapabilityInfo");
    assert_eq!(
        value,
        serde_json::json!({
            "capability": "unknown",
            "image_mode": "vision_tool_fallback",
            "has_vision_model": true,
            "is_local_endpoint": false,
            "vision_is_local_endpoint": false,
        })
    );
    // 未配置视觉模型时字段仍稳定出现(为 null),前端按 fail-open 处理。
    let no_vision = super::settings::ImageInputCapabilityInfo {
        capability: "unsupported".to_string(),
        image_mode: "unsupported".to_string(),
        has_vision_model: false,
        is_local_endpoint: true,
        vision_is_local_endpoint: None,
    };
    let value = serde_json::to_value(&no_vision).expect("serialize without vision model");
    assert_eq!(value["vision_is_local_endpoint"], serde_json::Value::Null);
}

/// 造一个指定 kind / token 估算的 IngestResult,markdown 是 `rows` 行可定位文本。
fn mk_attachment(
    kind: &str,
    basename: &str,
    rows: usize,
    tokens: u32,
) -> crate::features::files::file_ingest::IngestResult {
    let md: String = (1..=rows).map(|i| format!("row-{i},value-{i}\n")).collect();
    crate::features::files::file_ingest::IngestResult {
        kind: kind.into(),
        basename: basename.into(),
        path: format!("/tmp/fake/{basename}"),
        markdown: Some(md),
        token_estimate: tokens,
        byte_size: 1,
        warning: None,
    }
}

fn mk_test_ws(tag: &str) -> std::path::PathBuf {
    let ws = std::env::temp_dir().join(format!("pinvou3-attach-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(&ws).expect("建 workspace");
    ws
}

#[test]
fn project_bound_code_session_references_staged_attachments_by_absolute_path() {
    // 原生代码会话绑项目目录后：附件仍落盘到账本根（会话私有目录），但消息里的
    // 引用必须是绝对路径——引擎 cwd 是项目目录，相对路径会解析落空。
    let ledger = mk_test_ws("absolute-ledger");
    let prompt = build_message_with_attachments_in_dir(
        "inspect all".to_string(),
        vec![mk_attachment("xlsx", "big.xlsx", 9_000, 50_000)],
        &ledger,
        "attachments",
        true,
    );
    let staged = ledger.join("attachments/big.csv");
    assert!(
        staged.exists(),
        "附件必须落盘到账本根: {}",
        staged.display()
    );
    let absolute = staged.to_string_lossy().into_owned();
    assert!(prompt.contains(&absolute), "消息必须引用绝对路径");
    assert!(!prompt.contains("路径: `attachments/big.csv`"));

    // 普通会话（两根一致）维持相对路径引用，行为不变。
    let relative_ledger = mk_test_ws("relative-ledger");
    let relative_prompt = build_message_with_attachments_in_dir(
        "inspect all".to_string(),
        vec![mk_attachment("xlsx", "big.xlsx", 9_000, 50_000)],
        &relative_ledger,
        "attachments",
        false,
    );
    assert!(relative_prompt.contains("`attachments/big.csv`"));
    let _ = std::fs::remove_dir_all(&ledger);
    let _ = std::fs::remove_dir_all(&relative_ledger);
}

#[test]
fn remote_artifact_authorization_stays_on_private_ledger_root() {
    // 红线回归：即使原生代码会话把执行根绑到用户项目目录，远程下载授权根也必须
    // 是会话私有目录——resolve_session_artifact_path 的 workspace_root 来自
    // SessionStore::ledger_root（账本语义），项目内文件不可下载。
    let _home = test_pinvou_home("pinvou3-remote-authz-test");
    let store = SessionStore::boot().expect("session store");
    let chat = store
        .create_new("remote-authz-test".to_string(), None, std::env::temp_dir())
        .expect("chat session");
    let sid = chat.metadata.id.clone();
    let ledger = store.ledger_root(&sid).expect("ledger workspace");
    std::fs::create_dir_all(&ledger).expect("ledger dir");
    std::fs::write(ledger.join("ok.txt"), "ok").expect("ledger file");

    let project = mk_test_ws("remote-authz-project");
    let outside = project.join("secret.txt");
    std::fs::write(&outside, "secret").expect("project file");

    crate::features::remote_control::file_access::resolve_session_artifact_path(
        &store, &sid, "ok.txt",
    )
    .expect("账本根内的文件必须可下载");
    assert!(
        crate::features::remote_control::file_access::resolve_session_artifact_path(
            &store,
            &sid,
            outside.to_str().expect("utf8 path"),
        )
        .is_err(),
        "项目目录（执行根）内的文件不得经远程通道下载"
    );

    let _ = store.delete(&sid);
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn staged_attachment_basename_rejects_path_traversal_and_drive_prefixes() {
    for invalid in ["", ".", "..", "../x", "..\\x", "/x", "C:\\x"] {
        assert!(
            validate_staged_attachment_basename(invalid).is_err(),
            "staged basename must reject {invalid:?}"
        );
    }

    for valid in ["x", "report.txt", "报告 2026.xlsx"] {
        assert!(
            validate_staged_attachment_basename(valid).is_ok(),
            "ordinary basename must remain valid: {valid:?}"
        );
    }
}

#[test]
fn staged_targets_never_follow_preexisting_dangling_symlinks() {
    let root = mk_test_ws("target-symlink");
    let workspace = root.join("workspace");
    let outside = root.join("outside");
    let attachments = workspace.join("attachments");
    std::fs::create_dir_all(&attachments).expect("attachments");
    std::fs::create_dir_all(&outside).expect("outside");
    let source = root.join("source.png");
    std::fs::write(&source, b"safe image bytes").expect("source image");
    let escaped_image = outside.join("escaped.png");
    let image_link = attachments.join("source.png");
    if !try_link_file(&escaped_image, &image_link) {
        eprintln!("symlink creation unavailable; skipping platform-specific assertion");
        let _ = std::fs::remove_dir_all(root);
        return;
    }

    let escaped_text = outside.join("escaped.txt");
    let text_link = attachments.join("report.txt");
    if !try_link_file(&escaped_text, &text_link) {
        let _ = std::fs::remove_file(&image_link);
        let _ = std::fs::remove_dir_all(root);
        return;
    }

    let staged_image = stage_image_in_workspace(
        source.to_string_lossy().as_ref(),
        "source.png",
        &workspace,
        "attachments",
    )
    .expect("safe image fallback name");
    let staged_text =
        stage_text_in_workspace("safe text", "report.md", "txt", &workspace, "attachments")
            .expect("safe text fallback name");

    assert_eq!(staged_image, "attachments/source-1.png");
    assert_eq!(staged_text, "attachments/report-1.txt");
    assert!(!escaped_image.exists(), "image staging escaped workspace");
    assert!(!escaped_text.exists(), "text staging escaped workspace");
    assert_eq!(
        std::fs::read(workspace.join(&staged_image)).expect("staged image"),
        b"safe image bytes"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join(&staged_text)).expect("staged text"),
        "safe text"
    );

    let _ = std::fs::remove_file(image_link);
    let _ = std::fs::remove_file(text_link);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn staged_parent_chain_rejects_symlink_escape_without_touching_external_tree() {
    let root = mk_test_ws("parent-symlink");
    let workspace = root.join("workspace");
    let outside = root.join("outside");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&outside).expect("outside");
    let sentinel = outside.join("sentinel.txt");
    std::fs::write(&sentinel, "unchanged").expect("sentinel");
    let linked_parent = workspace.join("linked");
    if !try_link_dir(&outside, &linked_parent) {
        eprintln!("directory symlink creation unavailable; skipping platform-specific assertion");
        let _ = std::fs::remove_dir_all(root);
        return;
    }

    assert!(
        stage_text_in_workspace(
            "must stay inside",
            "report.md",
            "txt",
            &workspace,
            "linked/attachments",
        )
        .is_none(),
        "an escaping parent link must reject the stage"
    );
    assert_eq!(
        std::fs::read_to_string(&sentinel).expect("sentinel remains"),
        "unchanged"
    );
    assert!(
        !outside.join("attachments").exists(),
        "validation must happen before creating children through the link"
    );

    remove_dir_link(&linked_parent);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn concurrent_staging_reserves_distinct_targets_atomically() {
    let workspace = mk_test_ws("concurrent-create-new");
    let workers = 32;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(workers));
    let mut joins = Vec::new();
    for index in 0..workers {
        let barrier = barrier.clone();
        let workspace = workspace.clone();
        joins.push(std::thread::spawn(move || {
            let content = format!("worker-{index}-{}", "x".repeat(256 * 1024));
            barrier.wait();
            stage_text_in_workspace(&content, "report.md", "txt", &workspace, "attachments")
                .expect("concurrent stage")
        }));
    }
    let paths = joins
        .into_iter()
        .map(|join| join.join().expect("worker joins"))
        .collect::<Vec<_>>();
    let unique = paths.iter().collect::<std::collections::HashSet<_>>();
    assert_eq!(
        unique.len(),
        workers,
        "every writer needs an exclusive path"
    );
    assert!(
        paths.iter().all(|path| workspace.join(path).exists()),
        "every reserved target must contain its writer's output"
    );
    let _ = std::fs::remove_dir_all(workspace);
}

#[test]
fn session_owned_attachment_reuses_workspace_relative_path() {
    let workspace = mk_test_ws("reuse-session-attachment");
    let attachment = workspace
        .join("attachments")
        .join("desktop_attach_test")
        .join("image.png");
    std::fs::create_dir_all(attachment.parent().unwrap()).expect("attachment directory");
    std::fs::write(&attachment, b"image bytes").expect("attachment");

    assert_eq!(
        existing_workspace_relative_file(attachment.to_string_lossy().as_ref(), &workspace),
        Some("attachments/desktop_attach_test/image.png".to_string())
    );
    assert_eq!(
        std::fs::read_dir(attachment.parent().unwrap())
            .expect("attachment entries")
            .count(),
        1,
        "reusing a session-owned image must not create a second copy"
    );
    let _ = std::fs::remove_dir_all(workspace);
}

#[test]
fn atomic_staging_keeps_legal_image_and_text_behavior() {
    let workspace = mk_test_ws("legal-atomic-staging");
    let source = workspace.join("source.png");
    std::fs::write(&source, b"image bytes").expect("source");

    let image = stage_image_in_workspace(
        source.to_string_lossy().as_ref(),
        "safe.png",
        &workspace,
        "attachments",
    )
    .expect("image stage");
    let text = stage_text_in_workspace("text bytes", "safe.md", "txt", &workspace, "attachments")
        .expect("text stage");

    assert_eq!(
        std::fs::read(workspace.join(image)).unwrap(),
        b"image bytes"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join(text)).unwrap(),
        "text bytes"
    );
    let _ = std::fs::remove_dir_all(workspace);
}

#[test]
fn remote_attachment_source_survives_upload_temp_cleanup() {
    let root =
        std::env::temp_dir().join(format!("pinvou3-remote-source-test-{}", std::process::id()));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("create workspace");

    let image_source = root.join("remote.png");
    std::fs::write(&image_source, b"\x89PNG\r\n\x1a\nremote-image").unwrap();
    let mut image = crate::features::files::file_ingest::ingest(&image_source);
    let staged_image =
        stage_remote_attachment_source(image_source.to_str().unwrap(), &image.basename, &workspace)
            .expect("stage remote image");
    image.path = staged_image.to_string_lossy().to_string();
    std::fs::remove_file(&image_source).unwrap();
    let image_prompt = build_message_with_attachments("看图".into(), vec![image], &workspace);
    assert!(image_prompt.contains("image_analyze"));
    assert!(staged_image.exists());
    assert!(image_prompt.contains(".pinvou3/remote-attachments/remote.png"));

    let text_source = root.join("remote.txt");
    std::fs::write(&text_source, "远控大文本\n".repeat(20_000)).unwrap();
    let mut text = crate::features::files::file_ingest::ingest(&text_source);
    assert!(text.token_estimate > ATTACH_INLINE_MAX_TOKENS);
    let staged_text =
        stage_remote_attachment_source(text_source.to_str().unwrap(), &text.basename, &workspace)
            .expect("stage remote text");
    text.path = staged_text.to_string_lossy().to_string();
    std::fs::remove_file(&text_source).unwrap();
    let text_prompt = build_message_with_attachments("查全文".into(), vec![text], &workspace);
    assert!(staged_text.exists());
    assert!(text_prompt.contains(&staged_text.to_string_lossy().to_string()));
    assert!(!text_prompt.contains("此文件过大无法内嵌,且转换产物落盘失败"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn post_reservation_failure_never_unlinks_the_current_path() {
    let workspace = mk_test_ws("post-reserve-failure");
    let reserved = std::sync::Arc::new(std::sync::Mutex::new(None));
    let observed = reserved.clone();
    let result = stage_text_in_workspace_with_writer(
        "report.md",
        "txt",
        &workspace,
        "attachments",
        move |_destination, path| {
            *observed.lock().expect("capture path") = Some(path.to_path_buf());
            Err(std::io::Error::other("injected post-reservation failure"))
        },
    );
    assert!(result.is_none());
    let reserved_path = reserved
        .lock()
        .expect("reserved path")
        .clone()
        .expect("writer observed reserved path");
    assert!(
        reserved_path.exists(),
        "failure must leave the exclusively-created orphan instead of unlinking by path"
    );

    let replacement_workspace = mk_test_ws("post-reserve-replacement");
    let replacement_survived = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let replacement_flag = replacement_survived.clone();
    let result = stage_text_in_workspace_with_writer(
        "report.md",
        "txt",
        &replacement_workspace,
        "attachments",
        move |_destination, path| {
            let orphan = path.with_extension("reserved-orphan");
            if std::fs::rename(path, &orphan).is_ok() {
                std::fs::write(path, "replacement").expect("install replacement");
                replacement_flag.store(true, std::sync::atomic::Ordering::Release);
            }
            Err(std::io::Error::other("injected after path replacement"))
        },
    );
    assert!(result.is_none());
    if replacement_survived.load(std::sync::atomic::Ordering::Acquire) {
        assert_eq!(
            std::fs::read_to_string(replacement_workspace.join("attachments/report.txt"))
                .expect("replacement remains"),
            "replacement"
        );
    }

    let _ = std::fs::remove_dir_all(workspace);
    let _ = std::fs::remove_dir_all(replacement_workspace);
}

/// 小附件维持全量内联:内容在代码块里,且明确告知无需 read_file。
#[test]
fn small_attachment_stays_inline() {
    let ws = mk_test_ws("inline");
    let prompt = build_message_with_attachments(
        "看下这个".into(),
        vec![mk_attachment("xlsx", "small.xlsx", 10, 100)],
        &ws,
    );
    assert!(prompt.contains("row-10,value-10"), "小附件应全量内联");
    assert!(
        prompt.contains("不需要再调 read_file"),
        "内联段应声明无需 read_file"
    );
    assert!(!ws.join("attachments").exists(), "小附件不应落盘");
    let _ = std::fs::remove_dir_all(&ws);
}

/// 大表格 → 落盘 CSV + 预览 + 工具引导,完整内容不进 prompt。
#[test]
fn large_spreadsheet_goes_path_mode() {
    let ws = mk_test_ws("xlsx");
    let a = mk_attachment("xlsx", "data.xlsx", 5000, 100_000);
    let full_md = a.markdown.clone().unwrap();
    let prompt = build_message_with_attachments("分析一下".into(), vec![a], &ws);

    assert!(
        !prompt.contains("row-5000,value-5000"),
        "完整内容不应进 prompt"
    );
    assert!(prompt.contains("row-1,value-1"), "应有开头预览");
    assert!(
        prompt.contains("attachments/data.csv"),
        "应给出落盘 CSV 相对路径"
    );
    assert!(
        prompt.contains("read_file") && prompt.contains("exec_shell"),
        "应引导工具消化"
    );
    assert!(prompt.contains("没有**嵌入"), "应声明未嵌入完整内容");
    let staged = std::fs::read_to_string(ws.join("attachments/data.csv")).expect("CSV 应落盘");
    assert_eq!(staged, full_md, "落盘内容应与转换产物一致");
    // 体量验证:prompt 远小于全量(预览+引导 vs 5000 行)
    assert!(prompt.len() < full_md.len() / 10, "prompt 应远小于全量内容");
    let _ = std::fs::remove_dir_all(&ws);
}

/// 大纯文本(txt/csv 原文件):直接引用原始路径,不落盘副本。
#[test]
fn large_text_uses_original_path() {
    let ws = mk_test_ws("text");
    let a = mk_attachment("text", "big.log", 9000, 50_000);
    let prompt = build_message_with_attachments("查错误".into(), vec![a], &ws);

    assert!(prompt.contains("/tmp/fake/big.log"), "应引用原始路径");
    assert!(!ws.join("attachments").exists(), "text 类不应落盘副本");
    assert!(
        !prompt.contains("row-9000,value-9000"),
        "完整内容不应进 prompt"
    );
    let _ = std::fs::remove_dir_all(&ws);
}

/// 多附件累计超预算:单个都不超 INLINE_MAX,但第三个把累计顶过 TOTAL_BUDGET → 转路径模式。
#[test]
fn cumulative_budget_overflows_to_path_mode() {
    let ws = mk_test_ws("budget");
    let prompt = build_message_with_attachments(
        "汇总".into(),
        vec![
            mk_attachment("docx", "a.docx", 50, 7_000),
            mk_attachment("docx", "b.docx", 60, 7_000),
            mk_attachment("docx", "c.docx", 70, 7_000),
        ],
        &ws,
    );
    assert!(prompt.contains("row-50,value-50"), "a 应内联");
    assert!(
        prompt.contains("row-60,value-60"),
        "b 应内联(累计 14K ≤ 16K)"
    );
    assert!(
        !prompt.contains("row-70,value-70"),
        "c 应转路径模式(累计 21K > 16K)"
    );
    assert!(ws.join("attachments/c.md").exists(), "c 的产物应落盘为 md");
    let _ = std::fs::remove_dir_all(&ws);
}

/// 为多种附件类型生成「问题 + 附件内容」的最终 prompt，写到 /tmp 供外部脚本
/// 发真实 vLLM 验证模型能否据附件作答。依赖 /tmp/e2e_files，故 `#[ignore]`。
#[test]
#[ignore = "生成多类型 prompt 供 vLLM 验证"]
fn e2e_build_llm_cases() {
    let dir = std::path::Path::new("/tmp/e2e_files");
    if !dir.exists() {
        eprintln!("跳过: 无 /tmp/e2e_files");
        return;
    }
    let cases = [
        (
            "sample.png",
            "这张图里的项目编号是多少？只回答编号。",
            "/tmp/llm_png.txt",
        ),
        (
            "scan.pdf",
            "这份文件的文号是多少？只回答文号。",
            "/tmp/llm_scan.txt",
        ),
        (
            "sample.xlsx",
            "表格里李四的金额是多少？只回答数字。",
            "/tmp/llm_xlsx.txt",
        ),
        (
            "sample.pptx",
            "演示文稿第一章讲什么？提到的编号是？",
            "/tmp/llm_pptx.txt",
        ),
        (
            "mail.eml",
            "这封邮件的主题是什么？正文里的编号是多少？",
            "/tmp/llm_eml.txt",
        ),
        (
            "bundle.zip",
            "这个压缩包里图片上的项目编号是多少？",
            "/tmp/llm_zip.txt",
        ),
    ];
    for (f, q, out) in cases {
        let p = dir.join(f);
        if !p.exists() {
            continue;
        }
        let r = crate::features::files::file_ingest::ingest(&p);
        let ws = std::env::temp_dir().join("pinvou3-e2e-ws");
        let _ = std::fs::create_dir_all(&ws);
        let prompt = build_message_with_attachments(q.to_string(), vec![r], &ws);
        std::fs::write(out, prompt).expect("写 prompt");
    }
}
