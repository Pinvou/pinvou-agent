use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureChannel {
    ClientToServer,
    ServerToClient,
    Stderr,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CaptureRecord {
    pub monotonic_ns: u64,
    pub channel: CaptureChannel,
    pub line: String,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct FixtureFacts {
    pub timestamps_are_monotonic: bool,
    pub has_initialize_request: bool,
    pub has_initialize_response_without_jsonrpc: bool,
    pub has_notification_interleaving: bool,
    pub has_unknown_notification_noise: bool,
    pub has_separate_stderr: bool,
}

pub fn inspect_fixture(jsonl: &str) -> Result<FixtureFacts> {
    let mut records = Vec::new();
    for (index, line) in jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: CaptureRecord = serde_json::from_str(line)
            .with_context(|| format!("invalid capture record on line {}", index + 1))?;
        if record.line.contains(['\r', '\n']) {
            bail!("capture record {} contains more than one line", index + 1);
        }
        records.push(record);
    }

    let timestamps_are_monotonic = records
        .windows(2)
        .all(|pair| pair[0].monotonic_ns <= pair[1].monotonic_ns);
    let mut initialize_id = None;
    let mut initialize_response_index = None;
    let mut notification_before_response = false;
    let mut has_unknown_notification_noise = false;
    let mut has_separate_stderr = false;

    for (index, record) in records.iter().enumerate() {
        match record.channel {
            CaptureChannel::Stderr => has_separate_stderr = true,
            CaptureChannel::ClientToServer | CaptureChannel::ServerToClient => {
                let Ok(frame) = serde_json::from_str::<Value>(&record.line) else {
                    continue;
                };
                if record.channel == CaptureChannel::ClientToServer
                    && frame.get("method").and_then(Value::as_str) == Some("initialize")
                {
                    initialize_id = frame.get("id").cloned();
                }
                if record.channel == CaptureChannel::ServerToClient {
                    if frame.get("method").is_some() && initialize_response_index.is_none() {
                        notification_before_response = true;
                    }
                    if frame.get("method").and_then(Value::as_str)
                        == Some("fixture/unknown-notification")
                    {
                        has_unknown_notification_noise = true;
                    }
                    if initialize_id
                        .as_ref()
                        .is_some_and(|id| frame.get("id") == Some(id))
                        && frame.get("result").is_some()
                        && frame.get("jsonrpc").is_none()
                    {
                        initialize_response_index = Some(index);
                    }
                }
            }
        }
    }

    Ok(FixtureFacts {
        timestamps_are_monotonic,
        has_initialize_request: initialize_id.is_some(),
        has_initialize_response_without_jsonrpc: initialize_response_index.is_some(),
        has_notification_interleaving: notification_before_response
            && initialize_response_index.is_some(),
        has_unknown_notification_noise,
        has_separate_stderr,
    })
}
