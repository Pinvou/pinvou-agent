use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

const APPROVAL_MARKER_NAME: &str = ".codex-s2-approval-marker";
const APPROVAL_MARKER_BYTES: &[u8] = b"S2_APPROVED";

#[cfg(windows)]
fn notification_cwd(cwd: &str) -> String {
    if let Some(path) = cwd.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{path}");
    }
    cwd.strip_prefix(r"\\?\").unwrap_or(cwd).to_owned()
}

#[cfg(not(windows))]
fn notification_cwd(cwd: &str) -> String {
    cwd.to_owned()
}

#[cfg(windows)]
fn resolved_pwsh() -> std::path::PathBuf {
    let canonical = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|directory| directory.join("pwsh.exe"))
        .find(|candidate| candidate.is_file())
        .unwrap()
        .canonicalize()
        .unwrap();
    let rendered = canonical.to_string_lossy();
    if let Some(path) = rendered.strip_prefix(r"\\?\UNC\") {
        return std::path::PathBuf::from(format!(r"\\{path}"));
    }
    if let Some(path) = rendered.strip_prefix(r"\\?\") {
        return std::path::PathBuf::from(path);
    }
    canonical
}

#[cfg(windows)]
fn approval_wrapper(expected: &str) -> String {
    let escaped_pwsh = resolved_pwsh().to_string_lossy().replace('\\', r"\\");
    let escaped = expected.replace('\\', r"\\").replace('"', r#"\""#);
    format!(r#""{escaped_pwsh}" -Command "{escaped}""#)
}

#[cfg(windows)]
fn add_backslash_before_wrapper_quotes(mut command: String, only_ordinal: Option<usize>) -> String {
    let second = command.find("\" -Command ").unwrap();
    let third = second + "\" -Command ".len();
    let fourth = command.len() - 1;
    let positions = [0, second, third, fourth];
    let mut positions = only_ordinal
        .map(|ordinal| vec![positions[ordinal]])
        .unwrap_or_else(|| positions.to_vec());
    positions.sort_unstable_by(|left, right| right.cmp(left));
    for position in positions {
        command.insert(position, '\\');
    }
    command
}

fn send(value: Value) {
    println!("{value}");
    io::stdout().flush().unwrap();
}

fn thread_value(thread_id: &str, cwd: &str) -> Value {
    json!({
        "id":thread_id,
        "sessionId":thread_id,
        "forkedFromId":null,
        "parentThreadId":null,
        "preview":"",
        "ephemeral":true,
        "modelProvider":"openai",
        "createdAt":1,
        "updatedAt":1,
        "status":{"type":"idle"},
        "path":null,
        "cwd":cwd,
        "cliVersion":"0.139.0",
        "source":"vscode",
        "threadSource":null,
        "agentNickname":null,
        "agentRole":null,
        "gitInfo":null,
        "name":null,
        "turns":[]
    })
}

fn user_message_item(item_id: &str, prompt: &str) -> Value {
    json!({
        "type":"userMessage",
        "id":item_id,
        "clientId":null,
        "content":[{"type":"text","text":prompt,"text_elements":[]}]
    })
}

fn spawn_pipe_descendant() {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--hold-pipes-child")
        .spawn()
        .unwrap();
    if let Ok(marker) = std::env::var("S2_FAKE_MARKER") {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !std::path::Path::new(&marker).exists() && std::time::Instant::now() < deadline {
            assert!(
                child.try_wait().unwrap().is_none(),
                "pipe-holding child exited before signalling readiness"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            std::path::Path::new(&marker).exists(),
            "pipe-holding child did not signal readiness before deadline"
        );
    }
}

fn main() {
    let mode = std::env::var("S2_FAKE_MODE").unwrap_or_else(|_| {
        let executable = std::env::current_exe()
            .ok()
            .and_then(|path| {
                path.file_stem()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        if executable.contains("version-budget") {
            "version-budget".to_owned()
        } else {
            "success".to_owned()
        }
    });
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if let Ok(path) = std::env::var("S2_FAKE_ARGV_LOG") {
        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(log, "{}", serde_json::to_string(&args).unwrap()).unwrap();
    }
    if args
        .first()
        .is_some_and(|arg| arg == "--marker-helper-child")
    {
        let mut pids = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(".codex-s2-helper-test-pids")
            .unwrap();
        writeln!(pids, "{}", std::process::id()).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(60));
        return;
    }
    if args
        .first()
        .is_some_and(|arg| arg == "--marker-helper-fixture")
    {
        let fixture = args.get(1).map(String::as_str).unwrap_or("");
        let mut operation = String::new();
        io::stdin().read_line(&mut operation).unwrap();
        match fixture {
            "stall" => {
                let mut pids = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(".codex-s2-helper-test-pids")
                    .unwrap();
                writeln!(pids, "{}", std::process::id()).unwrap();
                pids.flush().unwrap();
                std::process::Command::new(std::env::current_exe().unwrap())
                    .arg("--marker-helper-child")
                    .spawn()
                    .unwrap();
                while std::fs::read_to_string(".codex-s2-helper-test-pids")
                    .unwrap_or_default()
                    .lines()
                    .count()
                    < 2
                {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
            "malformed" => println!("NOT_A_HELPER_STATUS"),
            "oversize" => {
                io::stdout().write_all(&vec![b'X'; 8192]).unwrap();
                io::stdout().flush().unwrap();
            }
            _ => std::process::exit(9),
        }
        return;
    }
    if args.first().is_some_and(|arg| arg == "--hold-pipes-child") {
        if let Ok(delay_ms) = std::env::var("S2_FAKE_CHILD_READY_DELAY_MS") {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms.parse().unwrap()));
        }
        if let Ok(marker) = std::env::var("S2_FAKE_MARKER") {
            std::fs::write(marker, std::process::id().to_string()).unwrap();
        }
        std::thread::sleep(std::time::Duration::from_secs(15));
        return;
    }
    if args.first().is_some_and(|arg| arg == "--version") {
        match mode.as_str() {
            "version-mismatch" => println!("codex-cli 0.138.0"),
            "version-malformed" => println!("not-a-codex-version"),
            "version-nonzero" => std::process::exit(7),
            "version-timeout" => std::thread::sleep(std::time::Duration::from_secs(60)),
            "version-budget" => {
                std::thread::sleep(std::time::Duration::from_secs(1));
                println!("codex-cli 0.139.0");
            }
            "version-slow-pinned" => {
                std::thread::sleep(std::time::Duration::from_millis(6_500));
                println!("codex-cli 0.139.0");
            }
            "version-descendant" => {
                spawn_pipe_descendant();
                println!("codex-cli 0.139.0");
            }
            _ => println!("codex-cli 0.139.0"),
        }
        return;
    }
    if args.first().is_some_and(|arg| arg == "app-server")
        && let Ok(audit_path) = std::env::var("S2_FAKE_HOME_AUDIT")
    {
        let home = std::path::PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        let original = std::path::PathBuf::from(std::env::var_os("S2_FAKE_ORIGINAL_HOME").unwrap());
        let config = std::fs::read_to_string(home.join("config.toml")).unwrap_or_default();
        let auth = std::fs::read(home.join("auth.json")).unwrap_or_default();
        let expected_auth = std::env::var("S2_FAKE_EXPECTED_AUTH").unwrap();
        let auth_refresh_succeeded = match std::env::var("S2_FAKE_REFRESH_AUTH").as_deref() {
            Ok("valid") => std::fs::write(home.join("auth.json"), br#"{"refreshed":true}"#).is_ok(),
            Ok("invalid") => std::fs::write(home.join("auth.json"), b"not-json").is_ok(),
            _ => false,
        };
        let mut auth_write_blocked = Value::Null;
        let mut config_replace_blocked = Value::Null;
        if std::env::var_os("S2_FAKE_TAMPER_HOME").is_some() {
            auth_write_blocked = Value::Bool(
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(home.join("auth.json"))
                    .is_err(),
            );
            let config_path = home.join("config.toml");
            let removed = std::fs::remove_file(&config_path).is_ok();
            config_replace_blocked = Value::Bool(!removed);
            if removed {
                std::fs::write(config_path, b"tampered = true\n").unwrap();
            }
        }
        let risky_env_absent = [
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "CODEX_ACCESS_TOKEN",
            "CODEX_EXEC_SERVER_URL",
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "TRACEPARENT",
            "CODEX_SQLITE_HOME",
            "CODEX_ROLLOUT_TRACE_ROOT",
            "CODEX_APP_SERVER_MANAGED_CONFIG_PATH",
            "CODEX_TUI_SESSION_LOG_PATH",
        ]
        .iter()
        .all(|key| std::env::var_os(key).is_none());
        std::fs::write(
            audit_path,
            serde_json::to_vec(&json!({
                "home": home,
                "isolated": home != original,
                "auth_matches": auth == expected_auth.as_bytes(),
                "config": config,
                "current_dir": std::env::current_dir().unwrap(),
                "neutral_marker": std::env::current_dir().unwrap().join(".codex-s2-root").is_file(),
                "no_project_inputs": !std::env::current_dir().unwrap().join(".codex").exists()
                    && !std::env::current_dir().unwrap().join(".agents").exists()
                    && !std::env::current_dir().unwrap().join("AGENTS.md").exists(),
                "auth_refresh_succeeded": auth_refresh_succeeded,
                "auth_write_blocked": auth_write_blocked,
                "config_replace_blocked": config_replace_blocked,
                "risky_env_absent": risky_env_absent,
                "proxy_preserved": std::env::var("HTTPS_PROXY").ok().as_deref() == Some("http://proxy.invalid:8443"),
                "ca_preserved": std::env::var("CODEX_CA_CERTIFICATE").ok().as_deref() == Some("test-ca-marker"),
                "home_env": std::env::var("HOME").ok(),
                "userprofile_env": std::env::var("USERPROFILE").ok(),
                "homedrive_env": std::env::var("HOMEDRIVE").ok(),
                "homepath_env": std::env::var("HOMEPATH").ok(),
                "real_home_skill_visible": original.join(".agents/skills/private-marker/SKILL.md").exists()
                    && std::env::var_os("HOME").is_some_and(|value| std::path::Path::new(&value).join(".agents/skills/private-marker/SKILL.md").exists()),
            }))
            .unwrap(),
        )
        .unwrap();
    }
    if mode == "immediate-child" {
        spawn_pipe_descendant();
        return;
    }
    let stdin = io::stdin();
    let mut turn = 0_u64;
    let mut approval_cwd = None;
    #[cfg(target_os = "linux")]
    let mut runner_replaced = false;
    for line in stdin.lock().lines() {
        let frame: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let method = frame.get("method").and_then(Value::as_str);
        let id = frame.get("id").cloned();
        match method {
            Some("initialize") => {
                #[cfg(target_os = "linux")]
                if mode == "replace-runner-image" && !runner_replaced {
                    use std::os::unix::fs::PermissionsExt;

                    let runner =
                        std::path::PathBuf::from(std::env::var_os("S2_FAKE_RUNNER_PATH").unwrap());
                    std::fs::rename(&runner, runner.with_extension("original")).unwrap();
                    std::fs::copy(std::env::current_exe().unwrap(), &runner).unwrap();
                    std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o700))
                        .unwrap();
                    runner_replaced = true;
                }
                send(json!({"method":"fixture/noise","params":{}}));
                if mode == "bad-init" {
                    send(json!({"id":id,"result":{}}));
                } else {
                    send(
                        json!({"id":id,"result":{"codexHome":std::env::current_dir().unwrap(),"platformFamily":if cfg!(windows) {"windows"} else {"unix"},"platformOs":std::env::consts::OS,"userAgent":"fake/0.139"}}),
                    );
                }
            }
            Some("initialized") => {}
            Some("account/read") => {
                if mode == "descendant-pipes" {
                    spawn_pipe_descendant();
                    return;
                }
                if mode == "noise-flood" {
                    for index in 0..20_000 {
                        send(json!({"method":"fixture/flood","params":{"index":index}}));
                    }
                }
                if mode == "malformed" {
                    println!("{{not-json");
                    io::stdout().flush().unwrap();
                } else if mode == "quota" {
                    send(json!({"id":id,"error":{"code":-32000,"message":"usage limit exceeded"}}));
                } else if mode == "auth" {
                    send(json!({"id":id,"result":{"requiresOpenaiAuth":true,"account":null}}));
                } else if matches!(
                    mode.as_str(),
                    "timeout" | "version-budget" | "immediate-child"
                ) {
                    continue;
                } else if mode == "bad-account" {
                    send(json!({"id":id,"result":{}}));
                } else {
                    send(
                        json!({"id":id,"result":{"requiresOpenaiAuth":true,"account":{"type":"chatgpt","email":"private@example.invalid","planType":"pro"}}}),
                    );
                }
            }
            Some("account/rateLimits/read") => {
                if mode == "bad-limits" {
                    send(json!({"id":id,"result":{}}));
                } else if mode == "quota-float" {
                    send(
                        json!({"id":id,"result":{"rateLimits":{"primary":{"usedPercent":100.0},"rateLimitReachedType":null}}}),
                    );
                } else {
                    send(
                        json!({"id":id,"result":{"rateLimits":{"primary":{"usedPercent":10},"rateLimitReachedType":null}}}),
                    );
                }
            }
            Some("config/read") => {
                #[cfg(windows)]
                let system_config = std::path::PathBuf::from(
                    std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into()),
                )
                .join("OpenAI")
                .join("Codex")
                .join("config.toml");
                #[cfg(not(windows))]
                let system_config = std::path::PathBuf::from("/etc/codex/config.toml");
                let session_name = json!({"type":"sessionFlags"});
                let user_name = json!({"type":"user","file":std::path::PathBuf::from(std::env::var_os("CODEX_HOME").unwrap()).join("config.toml"),"profile":null});
                let system_name = json!({"type":"system","file":system_config});
                let session_config = json!({
                    "notify":[],"project_root_markers":[".codex-s2-root"],"project_doc_max_bytes":0,
                    "skills":{"include_instructions":false,"bundled":{"enabled":false}},
                    "analytics":{"enabled":false},
                    "otel":{"exporter":"none","trace_exporter":"none","metrics_exporter":"none"},
                    "features":{"hooks":false,"plugins":false,"apps":false,"shell_snapshot":false,"memories":false}
                });
                let user_config = json!({
                    "cli_auth_credentials_store":"file",
                    "analytics":{"enabled":false},
                    "otel":{"exporter":"none","trace_exporter":"none","metrics_exporter":"none"},
                    "skills":{"include_instructions":false,"bundled":{"enabled":false}}
                });
                let mut origins = serde_json::Map::new();
                origins.insert(
                    "cli_auth_credentials_store".into(),
                    json!({"name":user_name.clone(),"version":"user-1"}),
                );
                for key in [
                    "project_root_markers.0",
                    "project_doc_max_bytes",
                    "skills.include_instructions",
                    "skills.bundled.enabled",
                    "analytics.enabled",
                    "otel.exporter",
                    "otel.trace_exporter",
                    "otel.metrics_exporter",
                    "features.hooks",
                    "features.plugins",
                    "features.apps",
                    "features.shell_snapshot",
                    "features.memories",
                ] {
                    origins.insert(
                        key.into(),
                        json!({"name":session_name.clone(),"version":"session-1"}),
                    );
                }
                let mut response = json!({"id":id,"result":{
                    "config":{
                        "cli_auth_credentials_store":"file",
                        "notify":[],
                        "project_doc_max_bytes":0,
                        "project_root_markers":[".codex-s2-root"],
                        "mcp_servers":{},
                        "model_providers":{},
                        "experimental_thread_config_endpoint":null,
                        "analytics":{"enabled":false},
                        "otel":{"exporter":"none","trace_exporter":"none","metrics_exporter":"none"},
                        "skills":{"include_instructions":false,"bundled":{"enabled":false}},
                        "features":{"hooks":false,"plugins":false,"apps":false,"shell_snapshot":false,"memories":false}
                    },
                    "origins":origins,
                    "layers":[
                        {"name":session_name,"version":"session-1","config":session_config},
                        {"name":user_name,"version":"user-1","config":user_config},
                        {"name":system_name,"version":"system-1","config":{}}
                    ]
                }});
                match mode.as_str() {
                    "config-managed-layer" => {
                        response["result"]["layers"] = json!([{"name":{"type":"enterpriseManaged","id":"id","name":"managed"},"version":"1","config":{}}])
                    }
                    "config-side-effect" => {
                        response["result"]["config"]["mcp_servers"] = json!({"unsafe":{}})
                    }
                    "config-malformed" => response["result"] = json!({}),
                    _ => {}
                }
                send(response);
            }
            Some("configRequirements/read") => {
                if mode == "config-requirements" {
                    send(
                        json!({"id":id,"result":{"requirements":{"allowedApprovalPolicies":["never"]}}}),
                    );
                } else {
                    send(json!({"id":id,"result":{"requirements":null}}));
                }
            }
            Some("thread/start") => {
                let cwd = frame
                    .pointer("/params/cwd")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if !cwd.is_empty() {
                    let _ = std::fs::write(std::path::Path::new(cwd).join("cmd.exe"), b"untrusted");
                    if mode == "preexisting-marker" && turn == 2 {
                        std::fs::write(
                            std::path::Path::new(cwd).join(APPROVAL_MARKER_NAME),
                            APPROVAL_MARKER_BYTES,
                        )
                        .unwrap();
                    }
                }
                if mode == "slow-phases" {
                    std::thread::sleep(std::time::Duration::from_millis(120));
                }
                let thread_id = format!("thread-{turn}");
                send(json!({"id":id,"result":{"thread":{"id":thread_id}}}));
                let notification_thread_id = if mode == "agent-thread-wrong-id" {
                    "wrong-thread"
                } else {
                    &thread_id
                };
                let thread = if mode == "agent-thread-malformed" {
                    json!({"id":notification_thread_id})
                } else {
                    thread_value(notification_thread_id, &notification_cwd(cwd))
                };
                let notification = json!({"method":"thread/started","params":{"thread":thread}});
                if mode != "agent-thread-missing" {
                    send(notification.clone());
                    if mode == "agent-thread-duplicate" {
                        send(notification);
                    }
                    let startup = |name: &str, status: &str, error: Value| {
                        json!({"method":"mcpServer/startupStatus/updated","params":{
                            "threadId":thread_id,"name":name,"status":status,"error":error
                        }})
                    };
                    match mode.as_str() {
                        "mcp-startup-extra" => {
                            let mut frame = startup("server", "starting", Value::Null);
                            frame["params"]["extra"] = json!(true);
                            send(frame);
                        }
                        "mcp-startup-null-thread" => {
                            let mut frame = startup("server", "starting", Value::Null);
                            frame["params"]["threadId"] = Value::Null;
                            send(frame);
                        }
                        "mcp-startup-wrong-thread" => {
                            let mut frame = startup("server", "starting", Value::Null);
                            frame["params"]["threadId"] = json!("wrong-thread");
                            send(frame);
                        }
                        "mcp-startup-empty-name" => send(startup("", "starting", Value::Null)),
                        "mcp-startup-status" => send(startup("server", "future", Value::Null)),
                        "mcp-startup-starting-error" => {
                            send(startup("server", "starting", json!("unexpected")))
                        }
                        "mcp-startup-ready-error" => {
                            send(startup("server", "ready", json!("unexpected")))
                        }
                        "mcp-startup-failed-null" => send(startup("server", "failed", Value::Null)),
                        "mcp-startup-terminal-first" => {
                            send(startup("server", "ready", Value::Null))
                        }
                        "mcp-startup-duplicate-start" => {
                            send(startup("server", "starting", Value::Null));
                            send(startup("server", "starting", Value::Null));
                        }
                        "mcp-startup-duplicate-terminal" => {
                            send(startup("server", "starting", Value::Null));
                            send(startup("server", "ready", Value::Null));
                            send(startup("server", "ready", Value::Null));
                        }
                        "mcp-startup-conflicting-terminal" => {
                            send(startup("server", "starting", Value::Null));
                            send(startup("server", "ready", Value::Null));
                            send(startup("server", "failed", json!("conflicting failure")));
                        }
                        _ => {
                            send(startup("fixture-ready", "starting", Value::Null));
                            send(startup("fixture-failed", "starting", Value::Null));
                            send(json!({"method":"warning","params":{
                                "threadId":thread_id,"message":"fixture warning before turn"
                            }}));
                        }
                    }
                }
            }
            Some("thread/resume") => {
                send(json!({"id":id,"result":{"thread":{"id":"thread-0"}}}));
            }
            Some("turn/start") => {
                if mode == "slow-phases" {
                    std::thread::sleep(std::time::Duration::from_millis(120));
                }
                turn += 1;
                let turn_id = format!("turn-{turn}");
                let prompt = frame
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                send(json!({"id":id,"result":{"turn":{"id":turn_id,"status":"inProgress"}}}));
                send(
                    json!({"method":"turn/started","params":{"threadId":format!("thread-{}", turn - 1),"turn":{"id":turn_id,"status":"inProgress"}}}),
                );
                let thread_id = format!("thread-{}", turn - 1);
                if !mode.starts_with("mcp-startup-") && mode != "agent-thread-missing" {
                    send(json!({"method":"mcpServer/startupStatus/updated","params":{
                        "threadId":thread_id,"name":"fixture-ready","status":"ready","error":null
                    }}));
                    send(json!({"method":"mcpServer/startupStatus/updated","params":{
                        "threadId":thread_id,"name":"fixture-failed","status":"failed","error":"fixture failure"
                    }}));
                    send(json!({"method":"warning","params":{
                        "threadId":thread_id,"message":"fixture warning after turn"
                    }}));
                }
                let user_raw_thread = if mode == "agent-user-raw-wrong-thread" {
                    "wrong-thread"
                } else {
                    &thread_id
                };
                let user_raw_turn = if mode == "agent-user-raw-wrong-turn" {
                    "wrong-turn"
                } else {
                    &turn_id
                };
                let user_raw_item = match mode.as_str() {
                    "agent-user-raw-role" => {
                        json!({"type":"message","role":"assistant","content":[{"type":"input_text","text":prompt}]})
                    }
                    "agent-user-raw-text" => {
                        json!({"type":"message","role":"user","content":[{"type":"input_text","text":"different"}]})
                    }
                    "agent-user-raw-multipart" => {
                        json!({"type":"message","role":"user","content":[{"type":"input_text","text":prompt},{"type":"input_text","text":prompt}]})
                    }
                    "agent-user-raw-malformed" => json!({"type":"message","role":"user"}),
                    _ => {
                        json!({"type":"message","role":"user","content":[{"type":"input_text","text":prompt}]})
                    }
                };
                let user_raw = json!({"method":"rawResponseItem/completed","params":{
                    "threadId":user_raw_thread,"turnId":user_raw_turn,"item":user_raw_item
                }});
                if mode.starts_with("agent-user-raw-")
                    && mode != "agent-user-raw-missing"
                    && mode != "agent-user-raw-late"
                {
                    send(user_raw.clone());
                    if mode == "agent-user-raw-duplicate" {
                        send(user_raw.clone());
                    }
                }
                let user_item_id = format!("user-{turn}");
                let user_prompt = if mode == "agent-user-prompt-mismatch" {
                    "different prompt"
                } else {
                    prompt
                };
                let mut user_item = user_message_item(&user_item_id, user_prompt);
                if mode == "agent-user-malformed" {
                    user_item.as_object_mut().unwrap().remove("id");
                }
                let user_thread_id = if mode == "agent-user-wrong-ids" {
                    "wrong-thread"
                } else {
                    &thread_id
                };
                let started_user = json!({"method":"item/started","params":{"threadId":user_thread_id,"turnId":turn_id,"startedAtMs":1,"item":user_item}});
                let completed_item_id = if mode == "agent-user-item-id-mismatch" {
                    "different-user-item"
                } else {
                    &user_item_id
                };
                let completed_user = json!({"method":"item/completed","params":{"threadId":user_thread_id,"turnId":turn_id,"completedAtMs":2,"item":user_message_item(completed_item_id,user_prompt)}});
                if mode == "agent-user-out-of-order" {
                    send(completed_user.clone());
                    send(started_user);
                } else {
                    send(started_user.clone());
                    if mode == "agent-user-raw-late" {
                        send(user_raw);
                    }
                    if mode == "agent-user-duplicate" {
                        send(started_user);
                    }
                    if mode != "agent-user-missing-completed" {
                        send(completed_user.clone());
                        if mode == "agent-user-duplicate-completed" {
                            send(completed_user);
                        }
                    }
                }
                if mode.starts_with("transport-retry-") && turn == 1 {
                    let mut retry = json!({"method":"error","params":{
                        "error":{
                            "message":"fixture transport interruption",
                            "codexErrorInfo":{"responseStreamDisconnected":{"httpStatusCode":null}},
                            "additionalDetails":"fixture detail"
                        },
                        "willRetry":true,
                        "threadId":thread_id,
                        "turnId":turn_id
                    }});
                    match mode.as_str() {
                        "transport-retry-wrong-identity" => {
                            retry["params"]["threadId"] = json!("wrong-thread");
                        }
                        "transport-retry-wrong-status" => {
                            retry["params"]["error"]["codexErrorInfo"] = json!({
                                "responseStreamDisconnected":{"httpStatusCode":503}
                            });
                        }
                        "transport-retry-extra-field" => {
                            retry["params"]["error"]["extra"] = json!(true);
                        }
                        _ => {}
                    }
                    let copies = if mode == "transport-retry-spam" { 6 } else { 1 };
                    for _ in 0..copies {
                        send(retry.clone());
                    }
                    if mode == "transport-retry-fatal" {
                        retry["params"]["willRetry"] = json!(false);
                        send(retry);
                    }
                    if mode != "transport-retry-success" {
                        continue;
                    }
                }
                let agent_only_violation = match mode.as_str() {
                    "agent-tool-command" => Some(
                        json!({"method":"item/started","params":{"threadId":format!("thread-{}",turn-1),"turnId":turn_id,"item":{"type":"commandExecution"}}}),
                    ),
                    "agent-tool-file" => Some(
                        json!({"method":"item/fileChange/outputDelta","params":{"threadId":format!("thread-{}",turn-1),"turnId":turn_id,"delta":"x"}}),
                    ),
                    "agent-tool-mcp" => Some(
                        json!({"method":"item/mcpToolCall/progress","params":{"threadId":format!("thread-{}",turn-1),"turnId":turn_id}}),
                    ),
                    "agent-tool-dynamic" => Some(
                        json!({"method":"item/started","params":{"threadId":format!("thread-{}",turn-1),"turnId":turn_id,"item":{"type":"dynamicToolCall"}}}),
                    ),
                    "agent-tool-web" => Some(
                        json!({"method":"item/started","params":{"threadId":format!("thread-{}",turn-1),"turnId":turn_id,"item":{"type":"webSearch"}}}),
                    ),
                    "agent-tool-collab" => Some(
                        json!({"method":"item/started","params":{"threadId":format!("thread-{}",turn-1),"turnId":turn_id,"item":{"type":"collabAgentToolCall"}}}),
                    ),
                    "agent-hook-started" => Some(
                        json!({"method":"hook/started","params":{"threadId":format!("thread-{}",turn-1),"turnId":turn_id,"run":{"handlerType":"command"}}}),
                    ),
                    "agent-tool-unknown" => Some(
                        json!({"method":"item/unknownTool/progress","params":{"threadId":format!("thread-{}",turn-1),"turnId":turn_id}}),
                    ),
                    _ => None,
                };
                if let Some(violation) = agent_only_violation {
                    send(violation);
                    continue;
                }
                if mode == "agent-server-request" {
                    send(
                        json!({"id":901,"method":"item/dynamicToolCall/requestApproval","params":{"threadId":format!("thread-{}",turn-1),"turnId":turn_id}}),
                    );
                    continue;
                }
                if mode == "agent-error-request" {
                    send(
                        json!({"id":902,"method":"error","params":{"message":"request, not notification"}}),
                    );
                    continue;
                }
                if mode == "agent-raw-out-of-order" {
                    send(
                        json!({"method":"rawResponseItem/completed","params":{"threadId":thread_id,"turnId":turn_id,"item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"early"}]}}}),
                    );
                    continue;
                }
                let reasoning_id = format!("reasoning-{turn}");
                send(
                    json!({"method":"item/started","params":{"threadId":thread_id,"turnId":turn_id,"startedAtMs":3,"item":{"type":"reasoning","id":reasoning_id,"summary":[],"content":[]}}}),
                );
                send(
                    json!({"method":"item/completed","params":{"threadId":thread_id,"turnId":turn_id,"completedAtMs":4,"item":{"type":"reasoning","id":reasoning_id,"summary":[],"content":[]}}}),
                );
                if mode.starts_with("agent-raw-") {
                    send(
                        json!({"method":"rawResponseItem/completed","params":{"threadId":thread_id,"turnId":turn_id,"item":{"type":"reasoning","summary":[],"content":null,"encrypted_content":null}}}),
                    );
                }
                let raw_thread_id = if mode == "agent-raw-wrong-ids" {
                    "wrong-thread"
                } else {
                    &thread_id
                };
                let raw_turn_id = if mode == "agent-raw-wrong-turn" {
                    "wrong-turn"
                } else {
                    &turn_id
                };
                let raw_item = match mode.as_str() {
                    "agent-raw-role-user" => {
                        json!({"type":"message","role":"user","content":[{"type":"output_text","text":"x"}]})
                    }
                    "agent-raw-function-call" => {
                        json!({"type":"function_call","name":"tool","arguments":"{}","call_id":"call-1"})
                    }
                    "agent-raw-local-shell" => {
                        json!({"type":"local_shell_call","status":"completed","action":{"type":"exec","command":["echo"]}})
                    }
                    "agent-raw-web-search" => {
                        json!({"type":"web_search_call","status":"completed"})
                    }
                    "agent-raw-computer" => json!({"type":"computer_call"}),
                    "agent-raw-tool-output" => {
                        json!({"type":"function_call_output","call_id":"call-1","output":"x"})
                    }
                    "agent-raw-custom-tool" => {
                        json!({"type":"custom_tool_call","call_id":"call-1","name":"tool","input":"x"})
                    }
                    "agent-raw-unknown" => json!({"type":"future_item"}),
                    "agent-raw-malformed" => json!({"type":"message","role":"assistant"}),
                    _ => {
                        json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"fixture output"}]})
                    }
                };
                if mode == "agent-aggregated-only" || (mode == "agent-early-complete" && turn == 1)
                {
                    if mode == "agent-aggregated-only" {
                        send(
                            json!({"method":"item/completed","params":{"threadId":format!("thread-{}",turn-1),"turnId":turn_id,"item":{"type":"agentMessage","text":"aggregated only"}}}),
                        );
                    }
                    send(
                        json!({"method":"turn/completed","params":{"threadId":format!("thread-{}",turn-1),"turn":{"id":turn_id,"status":"completed"}}}),
                    );
                    continue;
                }
                let agent_phase = if turn % 2 == 1 {
                    json!("final_answer")
                } else {
                    Value::Null
                };
                send(
                    json!({"method":"item/started","params":{"threadId":thread_id,"turnId":turn_id,"startedAtMs":3,"item":{"type":"agentMessage","id":format!("item-{turn}"),"text":"","phase":agent_phase,"memoryCitation":null}}}),
                );
                let count = if prompt.contains("S2-B") { 40 } else { 12 };
                for index in 0..count {
                    let delta = if mode == "agent-empty-delta" {
                        String::new()
                    } else {
                        "x".repeat(8 + (index % 8) * 7)
                    };
                    let delta_method = if mode == "command-output-success" {
                        "item/commandExecution/outputDelta"
                    } else {
                        "item/agentMessage/delta"
                    };
                    if mode == "command-output-success" {
                        send(
                            json!({"method":delta_method,"params":{"threadId":"wrong-thread","turnId":turn_id,"itemId":format!("noise-{turn}"),"delta":"WRONG_THREAD_NOISE"}}),
                        );
                        send(
                            json!({"method":"fixture/unrelatedDelta","params":{"threadId":format!("thread-{}", turn - 1),"turnId":turn_id,"delta":"UNRELATED_NOISE"}}),
                        );
                    }
                    let delta_thread = if mode == "agent-wrong-ids" {
                        "wrong-thread".to_owned()
                    } else {
                        format!("thread-{}", turn - 1)
                    };
                    send(
                        json!({"method":delta_method,"params":{"threadId":delta_thread,"turnId":turn_id,"itemId":format!("item-{turn}"),"delta":delta}}),
                    );
                    if mode == "agent-d-early-complete" && prompt.contains("S2-D") && index == 0 {
                        send(
                            json!({"method":"turn/completed","params":{"threadId":format!("thread-{}",turn-1),"turn":{"id":turn_id,"status":"completed"}}}),
                        );
                        break;
                    }
                }
                if !prompt.contains("S2-D") {
                    send(
                        json!({"method":"item/completed","params":{"threadId":thread_id,"turnId":turn_id,"completedAtMs":4,"item":{"type":"agentMessage","id":format!("item-{turn}"),"text":"aggregated output","phase":agent_phase,"memoryCitation":null}}}),
                    );
                    let mut raw_item = raw_item;
                    if raw_item.get("type").and_then(Value::as_str) == Some("message")
                        && raw_item.get("role").and_then(Value::as_str) == Some("assistant")
                        && !agent_phase.is_null()
                    {
                        raw_item["phase"] = agent_phase;
                    }
                    let duplicate_raw_item = raw_item.clone();
                    if mode.starts_with("agent-raw-") {
                        send(
                            json!({"method":"rawResponseItem/completed","params":{"threadId":raw_thread_id,"turnId":raw_turn_id,"item":raw_item}}),
                        );
                        if mode == "agent-raw-duplicate" {
                            send(
                                json!({"method":"rawResponseItem/completed","params":{"threadId":thread_id,"turnId":turn_id,"item":duplicate_raw_item}}),
                            );
                        }
                    }
                }
                if mode.starts_with("agent-raw-") {
                    continue;
                }
                if prompt.contains("S2-C") {
                    if mode != "missing-approval" {
                        let mut command = prompt
                            .split("APPROVAL_COMMAND_JSON:")
                            .nth(1)
                            .and_then(|raw| serde_json::from_str::<String>(raw.trim()).ok())
                            .unwrap_or_else(|| "unexpected-command".to_owned());
                        if mode == "unexpected-command" {
                            command = "not-allowlisted".to_owned();
                        }
                        #[cfg(windows)]
                        if matches!(
                            mode.as_str(),
                            "wrapper-approval"
                                | "wrapper-write-attempt"
                                | "wrapper-command-mutated"
                                | "wrapper-extra-action"
                                | "wrapper-wrong-pwsh"
                                | "wrapper-outer-backslashes"
                                | "wrapper-path-separator-single"
                                | "wrapper-path-separator-triple"
                                | "wrapper-extra-argv"
                                | "wrapper-inner-backslash-missing"
                        ) || mode.starts_with("wrapper-outer-backslash-")
                        {
                            if mode == "wrapper-write-attempt" {
                                let target = std::env::var("S2_FAKE_WRAPPER_TARGET").unwrap();
                                let outcome = std::fs::OpenOptions::new()
                                    .write(true)
                                    .open(target)
                                    .map(|_| "opened")
                                    .unwrap_or("blocked");
                                std::fs::write(
                                    std::env::var("S2_FAKE_WRAPPER_RESULT").unwrap(),
                                    outcome,
                                )
                                .unwrap();
                            }
                            command = approval_wrapper(&command);
                            if mode == "wrapper-command-mutated" {
                                command.push(' ');
                            } else if mode == "wrapper-wrong-pwsh" {
                                command = command.replacen("pwsh.exe", "other-pwsh.exe", 1);
                            } else if mode == "wrapper-outer-backslashes" {
                                command = add_backslash_before_wrapper_quotes(command, None);
                            } else if mode == "wrapper-path-separator-single" {
                                command = command.replacen(r"\\", r"\", 1);
                            } else if mode == "wrapper-path-separator-triple" {
                                command = command.replacen(r"\\", r"\\\", 1);
                            } else if mode == "wrapper-extra-argv" {
                                command.push_str(" -NoProfile");
                            } else if let Some(ordinal) = mode
                                .strip_prefix("wrapper-outer-backslash-")
                                .and_then(|value| value.parse().ok())
                            {
                                command =
                                    add_backslash_before_wrapper_quotes(command, Some(ordinal));
                            } else if mode == "wrapper-inner-backslash-missing" {
                                let inner_start =
                                    command.find(" -Command \"").unwrap() + " -Command \"".len();
                                let backslash =
                                    inner_start + command[inner_start..].find('\\').unwrap();
                                command.remove(backslash);
                            }
                        }
                        let mut cwd = frame.pointer("/params/cwd").cloned().unwrap_or(Value::Null);
                        if mode == "outside-approval" {
                            cwd = cwd
                                .as_str()
                                .and_then(|path| std::path::Path::new(path).parent())
                                .map(|path| json!(path))
                                .unwrap_or(Value::Null);
                        } else if mode == "child-approval" {
                            cwd = cwd
                                .as_str()
                                .map(|path| std::path::Path::new(path).join("child"))
                                .map(|path| {
                                    std::fs::create_dir_all(&path).unwrap();
                                    json!(path)
                                })
                                .unwrap_or(Value::Null);
                        }
                        let approval_thread = if mode == "wrong-approval-thread" {
                            "wrong-thread".to_owned()
                        } else {
                            format!("thread-{}", turn - 1)
                        };
                        let approval_turn = if mode == "wrong-approval-turn" {
                            "wrong-turn".to_owned()
                        } else {
                            turn_id.clone()
                        };
                        approval_cwd = cwd.as_str().map(std::path::PathBuf::from);
                        if mode == "approval-preseed-marker" {
                            std::fs::write(
                                approval_cwd.as_ref().unwrap().join(APPROVAL_MARKER_NAME),
                                APPROVAL_MARKER_BYTES,
                            )
                            .unwrap();
                        }
                        let mut params = json!({"threadId":approval_thread,"turnId":approval_turn,"itemId":"approval-item","startedAtMs":1,"command":command,"cwd":cwd});
                        #[cfg(windows)]
                        if mode.starts_with("wrapper-") {
                            let expected = prompt
                                .split("APPROVAL_COMMAND_JSON:")
                                .nth(1)
                                .and_then(|raw| serde_json::from_str::<String>(raw.trim()).ok())
                                .unwrap();
                            params["commandActions"] = if mode == "wrapper-extra-action" {
                                json!([{"command":expected},{"command":"extra"}])
                            } else {
                                json!([{"command":expected}])
                            };
                        }
                        let expected = prompt
                            .split("APPROVAL_COMMAND_JSON:")
                            .nth(1)
                            .and_then(|raw| serde_json::from_str::<String>(raw.trim()).ok())
                            .unwrap();
                        if mode == "direct-extra-action" {
                            params["commandActions"] =
                                json!([{"command":expected},{"command":"extra"}]);
                        } else if mode == "direct-mismatched-action" {
                            params["commandActions"] = json!([{"command":"different"}]);
                        } else if mode == "direct-malformed-action" {
                            params["commandActions"] = json!({"command":expected});
                        }
                        send(
                            json!({"id":900,"method":if mode == "unexpected-approval" {"item/fileChange/requestApproval"} else {"item/commandExecution/requestApproval"},"params":params}),
                        );
                        continue;
                    }
                }
                if prompt.contains("S2-D") {
                    continue;
                }
                send(
                    json!({"method":"turn/completed","params":{"threadId":format!("thread-{}", turn - 1),"turn":{"id":turn_id,"status":"completed"}}}),
                );
            }
            Some("turn/interrupt") => {
                if mode != "missing-interrupt-response" {
                    send(json!({"id":id,"result":{}}));
                }
                if mode != "missing-interrupted-terminal" {
                    send(
                        json!({"method":"turn/completed","params":{"threadId":"thread-3","turn":{"id":"turn-4","status":"interrupted"}}}),
                    );
                }
            }
            None if frame.get("id") == Some(&json!(900)) => {
                if mode != "missing-approval" {
                    if frame.pointer("/result/decision").and_then(Value::as_str) == Some("accept") {
                        if mode == "wrong-marker" {
                            std::fs::write(
                                approval_cwd.as_ref().unwrap().join(APPROVAL_MARKER_NAME),
                                b"WRONG",
                            )
                            .unwrap();
                        } else if mode != "accepted-no-marker" {
                            std::fs::write(
                                approval_cwd.as_ref().unwrap().join(APPROVAL_MARKER_NAME),
                                APPROVAL_MARKER_BYTES,
                            )
                            .unwrap();
                        }
                    }
                    send(
                        json!({"method":"turn/completed","params":{"threadId":"thread-2","turn":{"id":"turn-3","status":"completed"}}}),
                    );
                }
            }
            _ => {}
        }
    }
}
