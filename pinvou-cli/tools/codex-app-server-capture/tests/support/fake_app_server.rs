use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

const APPROVAL_MARKER_NAME: &str = ".codex-s2-approval-marker";
const APPROVAL_MARKER_BYTES: &[u8] = b"S2_APPROVED";

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
    let escaped = expected.replace('\\', r"\\").replace('"', r#"\""#);
    format!(r#""{}" -Command "{}""#, resolved_pwsh().display(), escaped)
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
            "version-descendant" => {
                spawn_pipe_descendant();
                println!("codex-cli 0.139.0");
            }
            _ => println!("codex-cli 0.139.0"),
        }
        return;
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
            Some("thread/start") => {
                if let Some(cwd) = frame.pointer("/params/cwd").and_then(Value::as_str) {
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
                send(json!({"id":id,"result":{"thread":{"id":format!("thread-{turn}")}}}));
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
                let count = if prompt.contains("S2-B") { 40 } else { 12 };
                for index in 0..count {
                    let delta = "x".repeat(8 + (index % 8) * 7);
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
                    send(
                        json!({"method":delta_method,"params":{"threadId":format!("thread-{}", turn - 1),"turnId":turn_id,"itemId":format!("item-{turn}"),"delta":delta}}),
                    );
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
