use std::{
    collections::VecDeque,
    time::{SystemTime, UNIX_EPOCH},
};

use pinvou_protocol::{RateClass, RuntimeEventEnvelope, RuntimeEventKind, StreamId};
use pinvou_runtime_api::AdapterError;
use serde_json::{Value, json};

use crate::redact_diagnostic;

const PROTOCOL_VERSION: u16 = 1;
const SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PendingControl {
    Approval {
        request_id: Value,
        approval_id: String,
        thread_id: String,
        response: ApprovalResponse,
    },
    Input {
        request_id: Value,
        input_id: String,
        thread_id: String,
        questions: Value,
    },
    AuthRefresh {
        request_id: Value,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApprovalResponse {
    Decision,
    Permissions { requested: Value },
}

#[derive(Debug)]
pub enum ProjectedFrame {
    Event(RuntimeEventEnvelope),
    Control(PendingControl),
    Ignored,
}

pub struct CodexEventProjector {
    node_id: String,
    attachment_id: String,
    logical_session_id: String,
    next_control_seq: u64,
    next_main_seq: u64,
    pending_controls: VecDeque<PendingControl>,
}

impl CodexEventProjector {
    pub fn new(node_id: impl Into<String>, attachment_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            attachment_id: attachment_id.into(),
            logical_session_id: "codex-pending".into(),
            next_control_seq: 1,
            next_main_seq: 1,
            pending_controls: VecDeque::new(),
        }
    }

    pub fn take_pending_control(&mut self) -> Option<PendingControl> {
        self.pending_controls.pop_front()
    }

    pub(crate) fn approval_resolved(
        &mut self,
        approval_id: &str,
        outcome: &str,
    ) -> Result<RuntimeEventEnvelope, AdapterError> {
        self.event(
            RuntimeEventKind::ApprovalResolved,
            RateClass::R0,
            None,
            json!({"approval_id":approval_id,"outcome":outcome}),
            None,
        )
    }

    pub(crate) fn input_resolved(
        &mut self,
        input_id: &str,
        value: &Value,
    ) -> Result<RuntimeEventEnvelope, AdapterError> {
        self.event(
            RuntimeEventKind::InputResolved,
            RateClass::R0,
            None,
            json!({"input_id":input_id,"value":sanitize_json(value)}),
            None,
        )
    }

    pub(crate) fn attachment_failed(
        &mut self,
        detail: &str,
    ) -> Result<RuntimeEventEnvelope, AdapterError> {
        self.event(
            RuntimeEventKind::AttachmentEnded,
            RateClass::R0,
            None,
            json!({"end_reason":"failed","detail":redact_diagnostic(detail)}),
            None,
        )
    }

    pub fn project(&mut self, frame: &Value) -> Result<ProjectedFrame, AdapterError> {
        let Some(method) = frame.get("method").and_then(Value::as_str) else {
            return Ok(ProjectedFrame::Ignored);
        };
        let params = frame.get("params").cloned().unwrap_or_else(|| json!({}));
        if let Some(thread_id) = thread_id(method, &params) {
            self.logical_session_id = thread_id.to_owned();
        }
        if let Some(request_id) = frame.get("id").cloned() {
            self.project_request(method, request_id, &params)
        } else {
            self.project_notification(method, &params)
        }
    }

    fn project_request(
        &mut self,
        method: &str,
        request_id: Value,
        params: &Value,
    ) -> Result<ProjectedFrame, AdapterError> {
        if !matches!(
            method,
            "item/commandExecution/requestApproval"
                | "execCommandApproval"
                | "item/fileChange/requestApproval"
                | "applyPatchApproval"
                | "item/permissions/requestApproval"
                | "item/tool/requestUserInput"
                | "account/chatgptAuthTokens/refresh"
        ) {
            return Err(AdapterError::Protocol {
                code: None,
                method: Some(method.into()),
                details: format!("unsupported_control_event: {method}"),
            });
        }
        if method == "account/chatgptAuthTokens/refresh" {
            return Ok(ProjectedFrame::Control(PendingControl::AuthRefresh {
                request_id,
            }));
        }
        let item_id = string_at(params, &["/approvalId", "/itemId", "/callId", "/item/id"])
            .ok_or_else(|| AdapterError::Protocol {
                code: None,
                method: Some(method.into()),
                details: "control request has no stable approval/item/call identity".into(),
            })?
            .to_owned();
        let control_thread = string_at(params, &["/threadId", "/conversationId"])
            .ok_or_else(|| AdapterError::Protocol {
                code: None,
                method: Some(method.into()),
                details: "control request has no thread identity".into(),
            })?
            .to_owned();
        let (event, control) = match method {
            "item/commandExecution/requestApproval" | "execCommandApproval" => {
                let summary = string_at(params, &["/command", "/reason"]).map(redact_diagnostic).unwrap_or_else(|| "Codex requests command approval".into());
                (self.event(RuntimeEventKind::ApprovalRequested, RateClass::R0, turn_id(params), json!({"approval_id":item_id,"tool":"command","summary":summary,"options":["allow","deny"],"timeout_ms":number_optional_at(params,&["/timeoutMs","/timeout_ms"])}), None)?, PendingControl::Approval { request_id, approval_id: item_id, thread_id:control_thread, response:ApprovalResponse::Decision })
            }
            "item/fileChange/requestApproval" | "applyPatchApproval" => (self.event(RuntimeEventKind::ApprovalRequested, RateClass::R0, turn_id(params), json!({"approval_id":item_id,"tool":"file_change","summary":"Codex requests a file change","options":["allow","deny"]}), None)?, PendingControl::Approval { request_id, approval_id: item_id, thread_id:control_thread, response:ApprovalResponse::Decision }),
            "item/permissions/requestApproval" => {
                let requested=params.get("permissions").cloned().ok_or_else(||AdapterError::Protocol{code:None,method:Some(method.into()),details:"permissions approval has no permission profile".into()})?;
                (self.event(RuntimeEventKind::ApprovalRequested,RateClass::R0,turn_id(params),json!({"approval_id":item_id,"tool":"permissions","summary":"Codex requests additional permission","options":["allow","deny"],"requested_permissions":sanitize_json(&requested)}),None)?,PendingControl::Approval{request_id,approval_id:item_id,thread_id:control_thread,response:ApprovalResponse::Permissions{requested}})
            }
            "item/tool/requestUserInput" => {
                let questions=params.get("questions").cloned().ok_or_else(|| AdapterError::Protocol { code:None,method:Some(method.into()),details:"requestUserInput has no questions".into() })?;
                let prompt=questions.as_array().and_then(|items|items.first()).and_then(|item|item.get("question")).and_then(Value::as_str).map(redact_diagnostic).unwrap_or_else(||"Codex requests input".into());
                (self.event(RuntimeEventKind::InputRequested, RateClass::R0, turn_id(params), json!({"input_id":item_id,"prompt":prompt,"schema":{"questions":sanitize_json(&questions)}}), None)?, PendingControl::Input { request_id, input_id: item_id,thread_id:control_thread,questions })
            }
            "account/chatgptAuthTokens/refresh" => return Ok(ProjectedFrame::Control(PendingControl::AuthRefresh { request_id })),
            _ => return Err(AdapterError::Protocol { code: None, method: Some(method.into()), details: format!("unsupported_control_event: {method}") }),
        };
        self.pending_controls.push_back(control);
        Ok(ProjectedFrame::Event(event))
    }

    fn project_notification(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<ProjectedFrame, AdapterError> {
        let turn = turn_id(params);
        let event = match method {
            "thread/started" => self.event(RuntimeEventKind::AttachmentStarted, RateClass::R0, None, json!({"runtime_id":"codex","agent_kind":"codex","capabilities_snapshot":{}}), None)?,
            "turn/started" => self.event(RuntimeEventKind::TurnStarted, RateClass::R0, turn, json!({"user_input_ref":"codex:turn/start"}), None)?,
            "turn/completed" => {
                let status = string_at(params, &["/turn/status", "/status"]).unwrap_or("failed");
                if params.pointer("/turn/error/codexErrorInfo").and_then(Value::as_str) == Some("usageLimitExceeded") { return Err(AdapterError::QuotaExceeded); }
                let end_reason = match status { "completed" => "completed", "interrupted" => "interrupted", "cancelled" => "cancelled", _ => "error" };
                self.event(RuntimeEventKind::TurnEnded, RateClass::R0, turn, json!({"end_reason":end_reason}), None)?
            }
            "item/agentMessage/delta" => self.event(RuntimeEventKind::TextDelta, RateClass::R1, turn, json!({"role":"assistant","content":string_at(params, &["/delta"]).unwrap_or("")}), None)?,
            "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => self.event(RuntimeEventKind::ThinkingDelta, RateClass::R1, turn, json!({"content":string_at(params, &["/delta"]).unwrap_or("")}), None)?,
            "item/plan/delta" => self.event(RuntimeEventKind::PlanDelta, RateClass::R1, turn, json!({"content":string_at(params, &["/delta"]).unwrap_or("")}), None)?,
            "item/commandExecution/outputDelta" => self.event(RuntimeEventKind::ToolCallOutputDelta, RateClass::R1, turn, json!({"tool_id":string_at(params, &["/itemId"]).unwrap_or("codex-tool"),"chunk":string_at(params, &["/delta"]).unwrap_or("")}), None)?,
            "item/started" => return self.project_item_started(params),
            "item/completed" => return self.project_item_completed(params),
            "item/fileChange/patchUpdated" => {
                let changes=params.get("changes").and_then(Value::as_array).ok_or_else(||AdapterError::Protocol{code:None,method:Some(method.into()),details:"patchUpdated has no changes array".into()})?;
                let patch=changes.iter().filter_map(|change|change.get("diff").and_then(Value::as_str)).collect::<Vec<_>>().join("\n");
                let paths=changes.iter().filter_map(|change|change.get("path").cloned()).collect::<Vec<_>>();
                self.event(RuntimeEventKind::FileChangeCompleted,RateClass::R1,turn,json!({"tool_id":string_at(params,&["/itemId"]).unwrap_or("codex-file-change"),"patch":patch,"paths":paths,"changes":sanitize_json(&Value::Array(changes.clone()))}),None)?
            }
            "thread/tokenUsage/updated" => {
                let usage = params.get("tokenUsage").unwrap_or(params);
                self.event(RuntimeEventKind::UsageReported, RateClass::R1, turn, json!({"input_tokens":number_at(usage, &["/total/inputTokens","/inputTokens"]),"output_tokens":number_at(usage, &["/total/outputTokens","/outputTokens"]),"cached_tokens":number_at(usage, &["/total/cachedInputTokens","/cachedInputTokens"])}), None)?
            }
            "error" => {
                let info = string_at(params, &["/error/codexErrorInfo"]);
                let message = string_at(params, &["/error/message", "/message"]).unwrap_or("Codex runtime error");
                if info == Some("usageLimitExceeded") || message.to_ascii_lowercase().contains("usage limit") { return Err(AdapterError::QuotaExceeded); }
                self.event(RuntimeEventKind::ErrorRaised, RateClass::R0, turn, json!({"code":info.unwrap_or("codex_error"),"message":redact_diagnostic(message),"fatal":!params.get("willRetry").and_then(Value::as_bool).unwrap_or(false),"source":"runtime"}), None)?
            }
            "warning" | "configWarning" | "deprecationNotice" | "thread/compacted" => self.event(RuntimeEventKind::LogRecord, RateClass::R2, turn, json!({"source":"codex","level":"warning","message":redact_diagnostic(string_at(params, &["/message"]).unwrap_or(method))}), None)?,
            "item/reasoning/summaryPartAdded" | "account/updated" => return Ok(ProjectedFrame::Ignored),
            "mcpServer/startupStatus/updated"
            | "thread/status/changed"
            | "account/rateLimits/updated"
            | "rawResponseItem/completed"
            | "remoteControl/status/changed" => self.event(RuntimeEventKind::Vendor, RateClass::R1, turn, json!({}), Some(json!({"method":method,"params":sanitize_json(params)})))?,
            _ => return Err(AdapterError::Protocol{code:None,method:Some(method.into()),details:format!("unsupported_notification: {method}")}),
        };
        Ok(ProjectedFrame::Event(event))
    }

    fn project_item_started(&mut self, params: &Value) -> Result<ProjectedFrame, AdapterError> {
        let item = params.get("item").unwrap_or(params);
        let item_type = item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if matches!(
            item_type,
            "userMessage" | "agentMessage" | "reasoning" | "plan" | "fileChange"
        ) {
            return Ok(ProjectedFrame::Ignored);
        }
        if !matches!(
            item_type,
            "commandExecution" | "mcpToolCall" | "dynamicToolCall"
        ) {
            return Err(AdapterError::Protocol {
                code: None,
                method: Some("item/started".into()),
                details: format!("unsupported_item: {item_type}"),
            });
        }
        Ok(ProjectedFrame::Event(self.event(RuntimeEventKind::ToolCallStarted, RateClass::R1, turn_id(params), json!({"tool_id":string_at(item, &["/id","/callId"]).unwrap_or("codex-tool"),"name":string_at(item, &["/name","/command"]).unwrap_or(item_type),"args_json":sanitize_json(item.get("arguments").unwrap_or(&Value::Null))}), None)?))
    }

    fn project_item_completed(&mut self, params: &Value) -> Result<ProjectedFrame, AdapterError> {
        let item = params.get("item").unwrap_or(params);
        let item_type = item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let (kind, payload) = if item_type == "agentMessage" {
            (
                RuntimeEventKind::MessageCompleted,
                json!({"role":"assistant","content":string_at(item, &["/text"]).unwrap_or(""),"item_id":string_at(item, &["/id"]).unwrap_or("codex-message")}),
            )
        } else if matches!(
            item_type,
            "commandExecution" | "mcpToolCall" | "dynamicToolCall"
        ) {
            (
                RuntimeEventKind::ToolCallCompleted,
                json!({"tool_id":string_at(item, &["/id","/callId"]).unwrap_or("codex-tool"),"result":sanitize_json(item),"is_error":item.get("status").and_then(Value::as_str).is_some_and(|v| matches!(v,"failed"|"error")),"exit_code":item.get("exitCode").cloned()}),
            )
        } else if matches!(
            item_type,
            "userMessage" | "reasoning" | "plan" | "fileChange"
        ) {
            return Ok(ProjectedFrame::Ignored);
        } else {
            return Err(AdapterError::Protocol {
                code: None,
                method: Some("item/completed".into()),
                details: format!("unsupported_item: {item_type}"),
            });
        };
        Ok(ProjectedFrame::Event(self.event(
            kind,
            RateClass::R1,
            turn_id(params),
            payload,
            None,
        )?))
    }

    fn event(
        &mut self,
        kind: RuntimeEventKind,
        rate: RateClass,
        turn_id: Option<&str>,
        payload: Value,
        vendor_extension: Option<Value>,
    ) -> Result<RuntimeEventEnvelope, AdapterError> {
        let seq = if rate == RateClass::R0 {
            let seq = self.next_control_seq;
            self.next_control_seq = self.next_control_seq.saturating_add(1);
            seq
        } else {
            let seq = self.next_main_seq;
            self.next_main_seq = self.next_main_seq.saturating_add(1);
            seq
        };
        RuntimeEventEnvelope::from_value(json!({"protocol_version":PROTOCOL_VERSION,"schema_version":SCHEMA_VERSION,"node_id":self.node_id,"logical_session_id":self.logical_session_id,"attachment_id":self.attachment_id,"work_id":null,"collaborative_run_id":null,"stream_id":if rate == RateClass::R0 { StreamId::Control } else { StreamId::Main },"turn_id":turn_id,"seq":seq,"source_span":null,"timestamp":rfc3339_now(),"rate_class":rate,"kind":kind,"payload":payload,"vendor_extension":vendor_extension})).map_err(|error| AdapterError::Protocol { code: None, method: Some("event/project".into()), details: error.to_string() })
    }
}

fn thread_id<'a>(method: &str, params: &'a Value) -> Option<&'a str> {
    if method == "thread/started" {
        string_at(params, &["/thread/id"])
    } else {
        string_at(params, &["/threadId"])
    }
}
fn turn_id(params: &Value) -> Option<&str> {
    string_at(params, &["/turnId", "/turn/id"])
}
fn string_at<'a>(value: &'a Value, paths: &[&str]) -> Option<&'a str> {
    paths
        .iter()
        .find_map(|path| value.pointer(path).and_then(Value::as_str))
}
fn number_at(value: &Value, paths: &[&str]) -> u64 {
    paths
        .iter()
        .find_map(|path| value.pointer(path).and_then(Value::as_u64))
        .unwrap_or(0)
}
fn number_optional_at(value: &Value, paths: &[&str]) -> Option<u64> {
    paths
        .iter()
        .find_map(|path| value.pointer(path).and_then(Value::as_u64))
}

fn sanitize_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    let value = if [
                        "token",
                        "key",
                        "secret",
                        "credential",
                        "authorization",
                        "cookie",
                        "password",
                    ]
                    .iter()
                    .any(|needle| lower.contains(needle))
                    {
                        Value::String("[REDACTED]".into())
                    } else {
                        sanitize_json(value)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(sanitize_json).collect()),
        Value::String(value) => Value::String(redact_diagnostic(value)),
        other => other.clone(),
    }
}

fn rfc3339_now() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = duration.as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let sod = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        sod / 3600,
        sod / 60 % 60,
        sod % 60,
        duration.subsec_millis()
    )
}
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    y += i64::from(m <= 2);
    (y, m, d)
}
