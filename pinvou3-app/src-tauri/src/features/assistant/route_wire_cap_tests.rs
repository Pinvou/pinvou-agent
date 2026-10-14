//! Wire-cap 回归：宿主申报的路由输出事实必须穿过底座出站缝抵达钢丝。
//!
//! 端到端钉住 PR #698 + CodeWhale#107 的组合行为：app 侧
//! `PINVOU_DOCUMENTED_OUTPUT` 申报目录外云模型的官方输出上限，
//! `Op::SendMessage` 经 `resolve_runtime_route_with_limits` 把事实规划进
//! 路由候选，底座 `bind_request_to_protocol` 对同模型请求返回客户端冻结
//! 的路由限额（model-aware kind 亦然——修复前 Deepseek 走静态重解析，
//! `limit_overrides` 为空，把未收录 wire id 重新钳回 8192，官方
//! deepseek-flash 的思维链在 Responses 线上烧完 8192 即截断报错）。
//!
//! 用本地 mock 端点驱动真实引擎完整一轮（app 路由解析 → Engine::new →
//! Op::SendMessage → 底座出站 → HTTP），断言钢丝请求体的 `max_tokens`：
//! 两个 wire-policy 家族（Deepseek=model-aware、Zai=固定 wire）都必须落在
//! `min(底座自动请求默认 64000, 申报值)`，而不是 8192。

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::platform::bridge::Pinvou3Bridge;
use crate::platform::test_support::EnvRestore;
use deepseek_tui::AppMode;
use deepseek_tui::core::engine::Engine;
use deepseek_tui::core::events::Event;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// 期望的钢丝输出上限：底座对未收录名字的自动请求默认
/// `(128K 窗口 / 2).min(64K)` = 64000（deepseek 家族经 DeepSeek legacy
/// 供应商启发式取 128K 窗口，glm 经通用 `unwrap_or(128_000)` 回退；两处
/// /2 同值），经申报事实 `min(64000, 384000/131072)` 后不变。修复前
/// 两家族的 model-aware 侧被静态重解析钳回 8192。
const EXPECTED_WIRE_MAX_TOKENS: u64 = 64_000;

#[derive(Default)]
struct WireCapProbe {
    bodies: Mutex<Vec<String>>,
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
                "wire-cap mock request exceeded 4 MiB",
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

fn plain_turn_sse() -> String {
    let delta = serde_json::json!({
        "id": "chatcmpl-wire-cap",
        "object": "chat.completion.chunk",
        "model": "wire-cap-probe",
        "choices": [{
            "index": 0,
            "delta": {"content": "ok"},
            "finish_reason": serde_json::Value::Null,
        }],
    });
    let finish = serde_json::json!({
        "id": "chatcmpl-wire-cap",
        "object": "chat.completion.chunk",
        "model": "wire-cap-probe",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 8, "completion_tokens": 3, "total_tokens": 11},
    });
    format!("data: {delta}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

async fn serve_wire_cap_connection(
    mut stream: TcpStream,
    probe: Arc<WireCapProbe>,
) -> std::io::Result<()> {
    let body = read_http_request(&mut stream).await?;
    probe.bodies.lock().expect("wire-cap body lock").push(body);
    let response_body = plain_turn_sse();
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response_body}",
        response_body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

async fn start_wire_cap_probe() -> (String, Arc<WireCapProbe>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind wire-cap probe");
    let address = listener.local_addr().expect("wire-cap probe address");
    let probe = Arc::new(WireCapProbe::default());
    let task_probe = Arc::clone(&probe);
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve_wire_cap_connection(stream, Arc::clone(&task_probe)));
        }
    });
    (format!("http://{address}/v1"), probe, task)
}

/// 跑一轮真实引擎对话。mock 端点对任意请求回一条极短的 SSE 完成流，
/// turn 立即结束；调用方随后读取 probe 捕获的请求体断言钢丝限额。
/// `expected_declared_output` 钉住引擎收到的路由申报（PINVOU_DOCUMENTED_OUTPUT
/// 的表行值），确保后续的钢丝断言确实在验证"申报穿过出站缝"。
async fn run_turn_against_mock(
    label: &str,
    preset: crate::platform::prefs::ModelPreset,
    model: &str,
    base_url: &str,
    project: &Path,
    expected_declared_output: u64,
) {
    let mut bridge =
        Pinvou3Bridge::boot_with_workspace(project.to_path_buf()).expect("boot wire-cap bridge");
    bridge.prefs.advanced.saved_models = vec![crate::platform::prefs::SavedModel {
        id: "wire-cap-model".to_string(),
        name: model.to_string(),
        alias: None,
        preset,
        context_window_tokens: None,
        max_output_tokens: None,
        reasoning_effort: None,
        model: model.to_string(),
        base_url: base_url.to_string(),
        provider_kind: None,
        vendor: None,
        endpoint_mode: None,
        image_capability_override: Default::default(),
        vision_model_id: None,
        api_key: "wire-cap-key".to_string(),
        credential_ref: None,
        credential_state: crate::platform::credential_store::CredentialState::Missing,
        has_secret: false,
        credential_action: None,
    }];
    bridge.prefs.advanced.active_model_id = Some("wire-cap-model".to_string());

    let dt_config = bridge.build_dt_config();
    let provider_kind = dt_config.api_provider();
    let engine_config = bridge.build_engine_config_for_session("wire-cap-session");
    assert_eq!(
        engine_config
            .active_route_limits
            .and_then(|limits| limits.output_tokens),
        Some(expected_declared_output),
        "{label}: the engine config must carry the documented output declaration"
    );
    let mut engine_config = engine_config;
    engine_config.snapshots_enabled = false;
    engine_config.terminal_chrome_enabled = false;
    let (engine, handle) = Engine::new(engine_config, &dt_config);
    let run_task = tokio::spawn(engine.run());
    let op = bridge
        .build_send_message_op(
            "wire-cap-session",
            "Say ok.".to_string(),
            AppMode::Agent,
            None,
            false,
        )
        .expect("build wire-cap turn");
    handle.send(op).await.expect("send wire-cap turn");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut events = handle.rx_event.write().await;
    loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .unwrap_or_else(|_| panic!("{label}: wire-cap turn timed out"))
            .unwrap_or_else(|| panic!("{label}: engine event channel closed"));
        match event {
            Event::ApprovalRequired { id, .. } => {
                let approval_handle = handle.clone();
                tokio::spawn(async move {
                    let _ = approval_handle.approve_tool_call(id).await;
                });
            }
            Event::TurnComplete { .. } => break,
            _ => {}
        }
    }
    drop(events);
    run_task.abort();
    eprintln!(
        "PROBE[{label}] model={model} api_provider={provider_kind:?} declared_output={expected_declared_output}"
    );
}

/// 两个 wire-policy 家族的钢丝输出上限都必须携带宿主申报事实：
/// Deepseek（model-aware，修复对象）与 Zai（固定 wire，回归保护）。
#[tokio::test(flavor = "current_thread")]
#[allow(clippy::await_holding_lock)]
async fn host_declared_output_fact_reaches_the_wire_for_both_wire_policies() {
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
        "PINVOU3_MAX_OUTPUT_TOKENS",
    ];
    let _env_restore =
        EnvRestore::capture_with_post_restore(&env_names, crate::features::personas::reload_user);
    let root = std::env::temp_dir().join(format!(
        "pinvou3-wire-cap-{}-{}",
        std::process::id(),
        crate::platform::paths::tests::unique_suffix()
    ));
    let project = root.join("user-project");
    std::fs::create_dir_all(&project).expect("create wire-cap project");
    // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
    unsafe { std::env::set_var("PINVOU3_HOME", root.join("pinvou-home")) };
    // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
    unsafe { std::env::set_var("CODEWHALE_HOME", root.join("codewhale-home")) };
    // loopback 明文 HTTP + 钉死 HTTP/1.1：与 run-dev.sh 的 Linux 本机端点口径一致。
    // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
    unsafe { std::env::set_var("DEEPSEEK_ALLOW_INSECURE_HTTP", "1") };
    // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
    unsafe { std::env::set_var("DEEPSEEK_FORCE_HTTP1", "1") };
    for name in env_names {
        if name.ends_with("PROVIDER")
            || name.ends_with("API_KEY")
            || name.ends_with("BASE_URL")
            || name.ends_with("MODEL")
            || name.ends_with("REASONING_EFFORT")
            || name.ends_with("MAX_OUTPUT_TOKENS")
        {
            // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
            unsafe { std::env::remove_var(name) };
        }
    }

    for (label, preset, model, declared_output) in [
        (
            "deepseek-model-aware",
            crate::platform::prefs::ModelPreset::Deepseek,
            "deepseek-flash",
            384_000_u64,
        ),
        (
            "glm-fixed-wire",
            crate::platform::prefs::ModelPreset::Glm,
            "glm-5.3-flashx",
            131_072_u64,
        ),
    ] {
        let (base_url, probe, server_task) = start_wire_cap_probe().await;
        run_turn_against_mock(label, preset, model, &base_url, &project, declared_output).await;
        let bodies = probe.bodies.lock().expect("wire-cap bodies").clone();
        server_task.abort();
        assert!(
            !bodies.is_empty(),
            "{label}: the mock endpoint must capture the outbound request"
        );
        for (index, body) in bodies.iter().enumerate() {
            let value: serde_json::Value = serde_json::from_str(body).unwrap_or_else(|error| {
                panic!("{label} request {index} is not JSON: {error}\n{body}")
            });
            assert_eq!(
                value.get("model").and_then(serde_json::Value::as_str),
                Some(model),
                "{label} request {index}: the wire model id must be preserved"
            );
            assert_eq!(
                value.get("max_tokens").and_then(serde_json::Value::as_u64),
                Some(EXPECTED_WIRE_MAX_TOKENS),
                "{label} request {index}: the host-declared output fact must reach the wire"
            );
        }
    }

    let _ = std::fs::remove_dir_all(root);
}
