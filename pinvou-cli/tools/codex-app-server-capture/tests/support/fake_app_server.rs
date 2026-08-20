use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

fn send(value: Value) {
    println!("{value}");
    io::stdout().flush().unwrap();
}

fn main() {
    let mode = std::env::var("S2_FAKE_MODE").unwrap_or_else(|_| "success".to_owned());
    let stdin = io::stdin();
    let mut turn = 0_u64;
    for line in stdin.lock().lines() {
        let frame: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let method = frame.get("method").and_then(Value::as_str);
        let id = frame.get("id").cloned();
        match method {
            Some("initialize") => {
                send(json!({"method":"fixture/noise","params":{}}));
                send(json!({"id":id,"result":{"serverInfo":{"name":"fake","version":"1"}}}));
            }
            Some("initialized") => {}
            Some("account/read") => {
                if mode == "malformed" {
                    println!("{{not-json");
                    io::stdout().flush().unwrap();
                } else if mode == "quota" {
                    send(json!({"id":id,"error":{"code":-32000,"message":"usage limit exceeded"}}));
                } else if mode == "auth" {
                    send(json!({"id":id,"result":{"requiresOpenaiAuth":true,"account":null}}));
                } else if mode == "timeout" {
                    continue;
                } else {
                    send(
                        json!({"id":id,"result":{"requiresOpenaiAuth":true,"account":{"type":"chatgpt","email":"private@example.invalid","planType":"pro"}}}),
                    );
                }
            }
            Some("account/rateLimits/read") => {
                send(
                    json!({"id":id,"result":{"rateLimits":{"primary":{"usedPercent":10},"rateLimitReachedType":null}}}),
                );
            }
            Some("thread/start") => {
                send(json!({"id":id,"result":{"thread":{"id":format!("thread-{turn}")}}}));
            }
            Some("thread/resume") => {
                send(json!({"id":id,"result":{"thread":{"id":"thread-0"}}}));
            }
            Some("turn/start") => {
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
                    send(
                        json!({"method":"item/agentMessage/delta","params":{"threadId":format!("thread-{}", turn - 1),"turnId":turn_id,"itemId":format!("item-{turn}"),"delta":delta}}),
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
                        let mut cwd = frame.pointer("/params/cwd").cloned().unwrap_or(Value::Null);
                        if mode == "outside-approval" {
                            cwd = cwd
                                .as_str()
                                .and_then(|path| std::path::Path::new(path).parent())
                                .map(|path| json!(path))
                                .unwrap_or(Value::Null);
                        }
                        send(
                            json!({"id":900,"method":if mode == "unexpected-approval" {"item/fileChange/requestApproval"} else {"item/commandExecution/requestApproval"},"params":{"threadId":format!("thread-{}", turn - 1),"turnId":turn_id,"itemId":"approval-item","startedAtMs":1,"command":command,"cwd":cwd}}),
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
                    send(
                        json!({"method":"turn/completed","params":{"threadId":"thread-2","turn":{"id":"turn-3","status":"completed"}}}),
                    );
                }
            }
            _ => {}
        }
    }
}
