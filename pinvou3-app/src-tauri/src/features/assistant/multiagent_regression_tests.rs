use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::expert_roster::ExpertRosterSnapshot;
use crate::features::assistant::platform::bridge::Pinvou3Bridge;
use crate::features::personas::PersonaCard;
use deepseek_tui::core::engine::Engine;
use deepseek_tui::core::events::{Event, TurnOutcomeStatus};
use deepseek_tui::core::ops::Op;
use deepseek_tui::tui::app::AppMode;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const BUILTIN_EXPERT_ID: &str = "exp-engineering-frontend-developer";

struct EnvRestore {
    values: Vec<(&'static str, Option<std::ffi::OsString>)>,
    reload_personas: bool,
}

impl EnvRestore {
    fn capture(names: &[&'static str]) -> Self {
        Self {
            values: names
                .iter()
                .map(|name| (*name, std::env::var_os(name)))
                .collect(),
            reload_personas: names.contains(&"PINVOU3_HOME"),
        }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (name, value) in &self.values {
            match value {
                // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
                Some(value) => unsafe { std::env::set_var(name, value) },
                // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
                None => unsafe { std::env::remove_var(name) },
            }
        }
        if self.reload_personas {
            crate::features::personas::reload_user();
        }
    }
}

fn unique_temp_root(label: &str) -> PathBuf {
    let suffix = crate::platform::paths::tests::unique_suffix();
    std::env::temp_dir().join(format!("pinvou3-{label}-{}-{suffix}", std::process::id()))
}

fn write_profile(dir: &Path, id: &str, description: &str) {
    std::fs::create_dir_all(dir).expect("create test profile directory");
    let body =
        format!("id = {id:?}\ndisplay_name = {description:?}\ndescription = {description:?}\n");
    std::fs::write(dir.join(format!("{id}.toml")), body).expect("write test profile");
}

const EXPERT_PROMPT_SENTINEL: &str = "PINVOU_EXPERT_SPAWN_REFRESH_SENTINEL";
const CHILD_RESULT_SENTINEL: &str = "PINVOU_CHILD_PROFILE_OK";

#[derive(Default)]
struct SpawnProbe {
    parent_started: AtomicBool,
    child_requests: AtomicUsize,
    request_bodies: Mutex<Vec<String>>,
}

async fn read_http_request(stream: &mut TcpStream) -> std::io::Result<String> {
    const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
    let mut bytes = Vec::new();
    let mut header_end = None;
    let mut content_length = 0usize;
    loop {
        let mut chunk = [0_u8; 8192];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "mock request exceeded 4 MiB",
            ));
        }
        if header_end.is_none() {
            if let Some(offset) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let end = offset + 4;
                let headers = String::from_utf8_lossy(&bytes[..end]);
                content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                header_end = Some(end);
            }
        }
        if header_end.is_some_and(|end| bytes.len() >= end + content_length) {
            break;
        }
    }
    let body_start = header_end.unwrap_or(bytes.len());
    let body_end = (body_start + content_length).min(bytes.len());
    Ok(String::from_utf8_lossy(&bytes[body_start..body_end]).into_owned())
}

fn text_turn_sse(content: &str) -> String {
    let delta = serde_json::json!({
        "id": "chatcmpl-roster-probe",
        "object": "chat.completion.chunk",
        "model": "qwen36_35b_256k",
        "choices": [{
            "index": 0,
            "delta": {"content": content},
            "finish_reason": serde_json::Value::Null,
        }],
    });
    let finish = serde_json::json!({
        "id": "chatcmpl-roster-probe",
        "object": "chat.completion.chunk",
        "model": "qwen36_35b_256k",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 8, "completion_tokens": 3, "total_tokens": 11},
    });
    format!("data: {delta}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

fn child_chat_response(content: &str) -> String {
    serde_json::json!({
        "id": "chatcmpl-roster-child",
        "object": "chat.completion",
        "model": "qwen36_35b_256k",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": content},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 8, "completion_tokens": 3, "total_tokens": 11},
    })
    .to_string()
}

fn agent_start_sse(profile_id: &str) -> String {
    let arguments = serde_json::json!({
        "action": "start",
        "name": "roster-refresh-probe",
        "profile": profile_id,
        "prompt": "Return one short confirmation and stop.",
        "write_authority": "read_only",
        "max_steps": 1,
        "thinking": "off",
    })
    .to_string();
    let delta = serde_json::json!({
        "id": "chatcmpl-roster-parent",
        "object": "chat.completion.chunk",
        "model": "qwen36_35b_256k",
        "choices": [{
            "index": 0,
            "delta": {"tool_calls": [{
                "index": 0,
                "id": "call_roster_refresh_probe",
                "type": "function",
                "function": {"name": "agent", "arguments": arguments},
            }]},
            "finish_reason": serde_json::Value::Null,
        }],
    });
    let finish = serde_json::json!({
        "id": "chatcmpl-roster-parent",
        "object": "chat.completion.chunk",
        "model": "qwen36_35b_256k",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
    });
    format!("data: {delta}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

async fn serve_probe_connection(
    mut stream: TcpStream,
    profile_id: Arc<String>,
    probe: Arc<SpawnProbe>,
) -> std::io::Result<()> {
    let body = read_http_request(&mut stream).await?;
    probe
        .request_bodies
        .lock()
        .expect("probe body lock")
        .push(body.clone());
    let (content_type, response_body) = if body.contains(EXPERT_PROMPT_SENTINEL) {
        probe.child_requests.fetch_add(1, Ordering::SeqCst);
        (
            "application/json",
            child_chat_response(CHILD_RESULT_SENTINEL),
        )
    } else if !probe.parent_started.swap(true, Ordering::SeqCst) {
        ("text/event-stream", agent_start_sse(&profile_id))
    } else {
        ("text/event-stream", text_turn_sse("PINVOU_PARENT_DONE"))
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response_body}",
        response_body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

async fn start_spawn_probe(
    profile_id: String,
) -> (String, Arc<SpawnProbe>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind spawn probe");
    let address = listener.local_addr().expect("spawn probe address");
    let profile_id = Arc::new(profile_id);
    let probe = Arc::new(SpawnProbe::default());
    let task_probe = Arc::clone(&probe);
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve_probe_connection(
                stream,
                Arc::clone(&profile_id),
                Arc::clone(&task_probe),
            ));
        }
    });
    (format!("http://{address}/v1"), probe, task)
}

/// CodeWhale 的 spawn-time refresh 使用的就是 `FleetRoster::load`：配置层专家
/// 必须独立于 execution/ledger 的位置存在，读取不存在的项目目录也不得反向创建
/// `.codewhale`。同名 Personal / Workspace 覆盖是底座公开语义，应继续允许。
#[test]
fn fleet_config_survives_execution_ledger_split_and_keeps_native_precedence() {
    let _env_lock = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _env_restore = EnvRestore::capture(&["CODEWHALE_HOME"]);
    let root = unique_temp_root("fleet-config-regression");
    let execution = root.join("project");
    let ledger = root.join("ledger");
    let codewhale_home = root.join("codewhale-home");
    std::fs::create_dir_all(&execution).expect("create execution root");
    std::fs::create_dir_all(&ledger).expect("create ledger root");
    // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
    unsafe { std::env::set_var("CODEWHALE_HOME", &codewhale_home) };

    let snapshot = ExpertRosterSnapshot::capture();
    let roster = deepseek_tui::FleetRoster::load(snapshot.fleet_config(), &execution);
    let expert = roster
        .get(BUILTIN_EXPERT_ID)
        .expect("config expert must survive a spawn-time roster reload");
    assert_eq!(expert.profile.role.name, BUILTIN_EXPERT_ID);
    assert_ne!(execution, ledger, "regression requires split roots");
    assert!(
        !execution.join(".codewhale").exists(),
        "loading the in-memory roster must not write the user project"
    );
    assert!(
        !ledger.join(".codewhale").exists(),
        "loading the in-memory roster must not materialize session TOML files"
    );

    write_profile(
        &codewhale_home.join("agents"),
        BUILTIN_EXPERT_ID,
        "personal override",
    );
    let personal = deepseek_tui::FleetRoster::load(snapshot.fleet_config(), &execution);
    assert_eq!(
        personal
            .get(BUILTIN_EXPERT_ID)
            .and_then(|member| member.description.as_deref()),
        Some("personal override"),
        "Personal profiles intentionally override [fleet.profiles]"
    );

    let workspace_profile_dir = execution.join(deepseek_tui::WORKSPACE_AGENT_PROFILE_DIR);
    write_profile(
        &workspace_profile_dir,
        BUILTIN_EXPERT_ID,
        "project override",
    );
    let workspace_profile_path = workspace_profile_dir.join(format!("{BUILTIN_EXPERT_ID}.toml"));
    let workspace_profile_before =
        std::fs::read(&workspace_profile_path).expect("read project override before roster load");
    let project = deepseek_tui::FleetRoster::load(snapshot.fleet_config(), &execution);
    assert_eq!(
        project
            .get(BUILTIN_EXPERT_ID)
            .and_then(|member| member.description.as_deref()),
        Some("project override"),
        "Workspace profiles intentionally override Personal and config profiles"
    );
    assert_eq!(
        std::fs::read(&workspace_profile_path).expect("read project override after roster load"),
        workspace_profile_before,
        "loading the roster must not rewrite a user-managed project profile"
    );
    assert!(
        project.get("general").is_some(),
        "built-in roles must remain available"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// 真实穿过 Engine 工具循环：父模型调用 `agent(profile=exp-*)` 后，CodeWhale 会在
/// `spawn_subagent_from_input` 内从当轮 route.config 重新加载 roster，再执行
/// `apply_spawn_profile`。子请求能携带专家正文 sentinel，证明不是仅初始 roster 假绿。
#[tokio::test(flavor = "current_thread")]
#[allow(clippy::await_holding_lock)]
async fn code_session_real_spawn_refresh_resolves_config_expert_without_project_writes() {
    let _env_lock = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let env_names = [
        "PINVOU3_HOME",
        "PINVOU3_SESSION_ARTIFACTS",
        "CODEWHALE_HOME",
        "DEEPSEEK_PROVIDER",
        "DEEPSEEK_API_KEY",
        "DEEPSEEK_BASE_URL",
        "DEEPSEEK_MODEL",
        "DEEPSEEK_REASONING_EFFORT",
        "DEEPSEEK_ALLOW_INSECURE_HTTP",
        "DEEPSEEK_FORCE_HTTP1",
        "DEEPSEEK_MAX_OUTPUT_TOKENS",
    ];
    let _env_restore = EnvRestore::capture(&env_names);
    let root = unique_temp_root("real-spawn-refresh");
    let project = root.join("user-project");
    let pinvou_home = root.join("pinvou-home");
    let codewhale_home = root.join("codewhale-home");
    std::fs::create_dir_all(&project).expect("create project");
    // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
    unsafe { std::env::set_var("PINVOU3_HOME", &pinvou_home) };
    // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
    unsafe { std::env::set_var("CODEWHALE_HOME", &codewhale_home) };
    // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
    unsafe { std::env::set_var("DEEPSEEK_PROVIDER", "vllm") };
    // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
    unsafe { std::env::set_var("DEEPSEEK_API_KEY", "local-test-key") };
    // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
    unsafe { std::env::set_var("DEEPSEEK_MODEL", "qwen36_35b_256k") };
    // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
    unsafe { std::env::set_var("DEEPSEEK_REASONING_EFFORT", "off") };
    // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
    unsafe { std::env::set_var("DEEPSEEK_ALLOW_INSECURE_HTTP", "1") };
    // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
    unsafe { std::env::set_var("DEEPSEEK_FORCE_HTTP1", "1") };
    // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
    unsafe { std::env::set_var("DEEPSEEK_MAX_OUTPUT_TOKENS", "4096") };

    let created = crate::features::personas::create_user_persona(PersonaCard {
        id: String::new(),
        dept: "testing".into(),
        name: "Spawn Profile Probe".into(),
        description: "spawn-time fleet refresh regression".into(),
        emoji: "🧪".into(),
        color: "#123456".into(),
        body: format!("You are the probe expert. {EXPERT_PROMPT_SENTINEL}"),
        source: "user".into(),
        conversational_only: false,
    })
    .expect("create probe persona");
    let profile_id = format!("exp-{}", created.id);
    let (base_url, probe, server_task) = start_spawn_probe(profile_id.clone()).await;
    // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
    unsafe { std::env::set_var("DEEPSEEK_BASE_URL", base_url) };

    let mut bridge = Pinvou3Bridge::boot_with_workspace(project.clone()).expect("boot bridge");
    bridge.set_code_session_predicate(Arc::new(|session_id| session_id.starts_with("code-")));
    let execution = project.clone();
    bridge.set_execution_root_resolver(Arc::new(move |session_id| {
        session_id.starts_with("code-").then(|| execution.clone())
    }));

    let roots_a = bridge.session_roots("code-a");
    let roots_b = bridge.session_roots("code-b");
    assert_eq!(roots_a.execution, project);
    assert_eq!(roots_b.execution, project);
    assert_ne!(roots_a.ledger, roots_b.ledger);
    assert_ne!(roots_a.execution, roots_a.ledger);

    let snapshot = ExpertRosterSnapshot::capture();
    assert!(
        snapshot.fleet_config().profiles.contains_key(&profile_id),
        "probe persona must be projected into [fleet.profiles]"
    );
    let mut engine_config =
        bridge.build_engine_config_for_multi_agent("code-a", roots_a.clone(), &snapshot, true);
    let second_config =
        bridge.build_engine_config_for_multi_agent("code-b", roots_b.clone(), &snapshot, true);
    assert_eq!(
        engine_config.subagent_state_root.as_ref(),
        Some(&roots_a.ledger)
    );
    assert_eq!(
        second_config.subagent_state_root.as_ref(),
        Some(&roots_b.ledger)
    );
    assert!(engine_config.fleet_roster.get(&profile_id).is_some());
    assert!(second_config.fleet_roster.get(&profile_id).is_some());

    let plain_roots = bridge.session_roots("plain-a");
    let plain_config = bridge.build_engine_config_for_session_roots("plain-a", plain_roots);
    assert!(
        plain_config
            .fleet_roster
            .members()
            .iter()
            .all(|member| !member.id.starts_with("exp-")),
        "ordinary sessions must not gain Pinvou experts"
    );
    assert!(
        bridge.build_dt_config().fleet_config().profiles.is_empty(),
        "ordinary turn routes must keep the expert layer disabled"
    );

    assert!(
        !project.join(".codewhale").exists(),
        "config construction must not materialize profiles in the user project"
    );
    assert!(
        !roots_a
            .ledger
            .join(deepseek_tui::WORKSPACE_AGENT_PROFILE_DIR)
            .exists(),
        "session ledger must not receive expert TOML copies"
    );
    assert!(
        !roots_b
            .ledger
            .join(deepseek_tui::WORKSPACE_AGENT_PROFILE_DIR)
            .exists(),
        "second session ledger must not receive expert TOML copies"
    );

    engine_config.snapshots_enabled = false;
    engine_config.terminal_chrome_enabled = false;
    let dt_config = bridge.build_multi_agent_dt_config(&snapshot);
    let (engine, handle) = Engine::new(engine_config, &dt_config);
    let run_task = tokio::spawn(engine.run());
    let op = bridge
        .build_multi_agent_send_message_op(
            "code-a",
            "Dispatch the probe expert now.".to_string(),
            AppMode::Yolo,
            None,
            false,
            &project,
            &snapshot,
        )
        .expect("build multi-agent turn");
    handle.send(op).await.expect("send multi-agent turn");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut saw_agent_tool_success = false;
    let mut saw_agent_spawned = false;
    let mut saw_agent_complete = false;
    let mut saw_parent_complete = false;
    let mut errors = Vec::new();
    let mut events = handle.rx_event.write().await;
    while !(saw_agent_complete && saw_parent_complete) {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("timed out waiting for real expert spawn")
            .expect("engine event channel closed");
        match event {
            Event::ApprovalRequired { id, .. } => {
                let approval_handle = handle.clone();
                tokio::spawn(async move {
                    approval_handle
                        .approve_tool_call(id)
                        .await
                        .expect("approve probe spawn");
                });
            }
            Event::ToolCallComplete { name, result, .. } if name == "agent" => {
                result.expect("agent(profile=exp-*) must resolve successfully");
                saw_agent_tool_success = true;
            }
            Event::AgentSpawned { .. } => saw_agent_spawned = true,
            Event::AgentComplete { failed, result, .. } => {
                assert!(!failed, "probe child failed: {result}");
                assert!(result.contains(CHILD_RESULT_SENTINEL), "{result}");
                saw_agent_complete = true;
            }
            Event::TurnComplete { status, error, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed, "{error:?}");
                saw_parent_complete = true;
            }
            Event::Error { envelope, .. } => errors.push(envelope.message),
            _ => {}
        }
    }
    drop(events);

    assert!(
        saw_agent_tool_success,
        "agent tool did not complete successfully"
    );
    assert!(saw_agent_spawned, "no AgentSpawned event was emitted");
    assert!(
        errors
            .iter()
            .all(|message| !message.contains("Unknown fleet role/profile")),
        "spawn-time refresh lost the expert profile: {errors:?}"
    );
    assert_eq!(probe.child_requests.load(Ordering::SeqCst), 1);
    assert!(
        probe
            .request_bodies
            .lock()
            .expect("probe request lock")
            .iter()
            .any(|body| body.contains(EXPERT_PROMPT_SENTINEL)),
        "child request did not receive the selected expert instructions"
    );
    assert!(
        !project.join(".codewhale").exists(),
        "real spawn must keep all control-plane state out of the user project"
    );
    assert!(
        roots_a.ledger.join(".codewhale").join("state").is_dir(),
        "the first session must persist delegated state under its ledger"
    );
    assert!(
        !roots_b.ledger.join(".codewhale").join("state").exists(),
        "an idle sibling session must not share the first session's ledger"
    );

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    tokio::time::timeout(Duration::from_secs(5), run_task)
        .await
        .expect("engine shutdown timeout")
        .expect("engine task failed");
    server_task.abort();
    crate::features::personas::delete_user_persona(&created.id).expect("delete probe persona");
    let _ = std::fs::remove_dir_all(root);
}
