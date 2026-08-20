use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, value::RawValue};
use thiserror::Error;

pub const PROTOCOL_VERSION: u16 = 1;
pub const SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RateClass {
    R0,
    R1,
    R2,
    R3,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamId {
    Control,
    Main,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceSpan {
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceKind {
    File,
    Directory,
    Image,
    Audio,
    Video,
    Document,
    Archive,
    Patch,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceAccess {
    Preview,
    Stream,
    Download,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceLifecycle {
    Workspace,
    Session,
    Temporary,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ResourceRef {
    resource_id: String,
    node_id: String,
    kind: ResourceKind,
    display_name: String,
    size: u64,
    mime_type: String,
    checksum: String,
    access: ResourceAccess,
    lifecycle: ResourceLifecycle,
    version: u64,
    #[serde(flatten)]
    extensions: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
struct WireResourceRef {
    resource_id: String,
    node_id: String,
    kind: ResourceKind,
    display_name: String,
    size: u64,
    mime_type: String,
    checksum: String,
    access: ResourceAccess,
    lifecycle: ResourceLifecycle,
    version: u64,
    #[serde(flatten)]
    extensions: BTreeMap<String, Value>,
}

impl<'de> Deserialize<'de> for ResourceRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WireResourceRef::deserialize(deserializer)?;
        if wire.extensions.contains_key("remote_path")
            || wire
                .extensions
                .values()
                .any(|value| contains_forbidden_key(value, "remote_path"))
        {
            return Err(serde::de::Error::custom(
                "remote_path is forbidden in ResourceRef",
            ));
        }
        if [
            wire.resource_id.as_str(),
            wire.node_id.as_str(),
            wire.display_name.as_str(),
            wire.mime_type.as_str(),
            wire.checksum.as_str(),
        ]
        .into_iter()
        .any(str::is_empty)
        {
            return Err(serde::de::Error::custom(
                "ResourceRef string fields must not be empty",
            ));
        }
        Ok(Self {
            resource_id: wire.resource_id,
            node_id: wire.node_id,
            kind: wire.kind,
            display_name: wire.display_name,
            size: wire.size,
            mime_type: wire.mime_type,
            checksum: wire.checksum,
            access: wire.access,
            lifecycle: wire.lifecycle,
            version: wire.version,
            extensions: wire.extensions,
        })
    }
}

impl ResourceRef {
    pub fn resource_id(&self) -> &str {
        &self.resource_id
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    pub const fn kind(&self) -> ResourceKind {
        self.kind
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub const fn size(&self) -> u64 {
        self.size
    }

    pub fn mime_type(&self) -> &str {
        &self.mime_type
    }

    pub fn checksum(&self) -> &str {
        &self.checksum
    }

    pub const fn access(&self) -> ResourceAccess {
        self.access
    }

    pub const fn lifecycle(&self) -> ResourceLifecycle {
        self.lifecycle
    }

    pub const fn version(&self) -> u64 {
        self.version
    }

    pub fn extensions(&self) -> &BTreeMap<String, Value> {
        &self.extensions
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RuntimeEventKind {
    #[serde(rename = "attachment.started")]
    AttachmentStarted,
    #[serde(rename = "attachment.ended")]
    AttachmentEnded,
    #[serde(rename = "turn.started")]
    TurnStarted,
    #[serde(rename = "turn.ended")]
    TurnEnded,
    #[serde(rename = "approval.requested")]
    ApprovalRequested,
    #[serde(rename = "approval.resolved")]
    ApprovalResolved,
    #[serde(rename = "input.requested")]
    InputRequested,
    #[serde(rename = "input.resolved")]
    InputResolved,
    #[serde(rename = "error.raised")]
    ErrorRaised,
    #[serde(rename = "resource.ref_created")]
    ResourceRefCreated,
    #[serde(rename = "stream.aborted")]
    StreamAborted,
    #[serde(rename = "stream.gap")]
    StreamGap,
    #[serde(rename = "text.delta")]
    TextDelta,
    #[serde(rename = "thinking.delta")]
    ThinkingDelta,
    #[serde(rename = "plan.delta")]
    PlanDelta,
    #[serde(rename = "message.completed")]
    MessageCompleted,
    #[serde(rename = "tool.call.started")]
    ToolCallStarted,
    #[serde(rename = "tool.call.args_delta")]
    ToolCallArgsDelta,
    #[serde(rename = "tool.call.output_delta")]
    ToolCallOutputDelta,
    #[serde(rename = "tool.call.completed")]
    ToolCallCompleted,
    #[serde(rename = "file.change.completed")]
    FileChangeCompleted,
    #[serde(rename = "usage.reported")]
    UsageReported,
    #[serde(rename = "log.record")]
    LogRecord,
    #[serde(rename = "diagnostic.gap")]
    DiagnosticGap,
    #[serde(rename = "progress.tick")]
    ProgressTick,
    #[serde(rename = "resource.sample")]
    ResourceSample,
    #[serde(rename = "vendor")]
    Vendor,
}

impl RuntimeEventKind {
    pub const ALL: [Self; 27] = [
        Self::AttachmentStarted,
        Self::AttachmentEnded,
        Self::TurnStarted,
        Self::TurnEnded,
        Self::ApprovalRequested,
        Self::ApprovalResolved,
        Self::InputRequested,
        Self::InputResolved,
        Self::ErrorRaised,
        Self::ResourceRefCreated,
        Self::StreamAborted,
        Self::StreamGap,
        Self::TextDelta,
        Self::ThinkingDelta,
        Self::PlanDelta,
        Self::MessageCompleted,
        Self::ToolCallStarted,
        Self::ToolCallArgsDelta,
        Self::ToolCallOutputDelta,
        Self::ToolCallCompleted,
        Self::FileChangeCompleted,
        Self::UsageReported,
        Self::LogRecord,
        Self::DiagnosticGap,
        Self::ProgressTick,
        Self::ResourceSample,
        Self::Vendor,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AttachmentStarted => "attachment.started",
            Self::AttachmentEnded => "attachment.ended",
            Self::TurnStarted => "turn.started",
            Self::TurnEnded => "turn.ended",
            Self::ApprovalRequested => "approval.requested",
            Self::ApprovalResolved => "approval.resolved",
            Self::InputRequested => "input.requested",
            Self::InputResolved => "input.resolved",
            Self::ErrorRaised => "error.raised",
            Self::ResourceRefCreated => "resource.ref_created",
            Self::StreamAborted => "stream.aborted",
            Self::StreamGap => "stream.gap",
            Self::TextDelta => "text.delta",
            Self::ThinkingDelta => "thinking.delta",
            Self::PlanDelta => "plan.delta",
            Self::MessageCompleted => "message.completed",
            Self::ToolCallStarted => "tool.call.started",
            Self::ToolCallArgsDelta => "tool.call.args_delta",
            Self::ToolCallOutputDelta => "tool.call.output_delta",
            Self::ToolCallCompleted => "tool.call.completed",
            Self::FileChangeCompleted => "file.change.completed",
            Self::UsageReported => "usage.reported",
            Self::LogRecord => "log.record",
            Self::DiagnosticGap => "diagnostic.gap",
            Self::ProgressTick => "progress.tick",
            Self::ResourceSample => "resource.sample",
            Self::Vendor => "vendor",
        }
    }

    const fn required_rate(self) -> Option<RateClass> {
        match self {
            Self::AttachmentStarted
            | Self::AttachmentEnded
            | Self::TurnStarted
            | Self::TurnEnded
            | Self::ApprovalRequested
            | Self::ApprovalResolved
            | Self::InputRequested
            | Self::InputResolved
            | Self::ErrorRaised
            | Self::ResourceRefCreated
            | Self::StreamAborted
            | Self::StreamGap => Some(RateClass::R0),
            Self::TextDelta
            | Self::ThinkingDelta
            | Self::PlanDelta
            | Self::MessageCompleted
            | Self::ToolCallStarted
            | Self::ToolCallArgsDelta
            | Self::ToolCallOutputDelta
            | Self::ToolCallCompleted
            | Self::FileChangeCompleted
            | Self::UsageReported => Some(RateClass::R1),
            Self::LogRecord | Self::DiagnosticGap => Some(RateClass::R2),
            Self::ProgressTick | Self::ResourceSample => Some(RateClass::R3),
            Self::Vendor => None,
        }
    }

    const fn required_fields(self) -> &'static [&'static str] {
        match self {
            Self::AttachmentStarted => &["runtime_id", "agent_kind", "capabilities_snapshot"],
            Self::AttachmentEnded => &["end_reason"],
            Self::TurnStarted => &["user_input_ref"],
            Self::TurnEnded => &["end_reason"],
            Self::ApprovalRequested => &["approval_id", "tool", "summary", "options"],
            Self::ApprovalResolved => &["approval_id", "outcome"],
            Self::InputRequested => &["input_id", "prompt"],
            Self::InputResolved => &["input_id", "value"],
            Self::ErrorRaised => &["code", "message", "fatal", "source"],
            Self::ResourceRefCreated => &["ref"],
            Self::StreamAborted => &["reason"],
            Self::StreamGap => &["reason", "affected_rate_classes"],
            Self::TextDelta => &["role", "content"],
            Self::ThinkingDelta | Self::PlanDelta => &["content"],
            Self::MessageCompleted => &["role", "content", "item_id"],
            Self::ToolCallStarted => &["tool_id", "name"],
            Self::ToolCallArgsDelta => &["tool_id", "args_delta"],
            Self::ToolCallOutputDelta => &["tool_id", "chunk"],
            Self::ToolCallCompleted => &["tool_id", "result", "is_error"],
            Self::FileChangeCompleted => &["tool_id", "patch", "paths"],
            Self::UsageReported => &["input_tokens", "output_tokens"],
            Self::LogRecord => &["source", "level", "message"],
            Self::DiagnosticGap => &["reason"],
            Self::ProgressTick => &["turn_id", "phase"],
            Self::ResourceSample => &["cpu", "memory", "sampled_at"],
            Self::Vendor => &[],
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RuntimeEventEnvelope {
    protocol_version: u16,
    schema_version: u16,
    node_id: String,
    logical_session_id: String,
    attachment_id: String,
    work_id: Option<String>,
    collaborative_run_id: Option<String>,
    stream_id: StreamId,
    turn_id: Option<String>,
    seq: u64,
    source_span: Option<SourceSpan>,
    timestamp: String,
    rate_class: RateClass,
    kind: RuntimeEventKind,
    payload: Box<RawValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vendor_extension: Option<Value>,
    #[serde(flatten)]
    extensions: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
struct WireEnvelope {
    protocol_version: u16,
    schema_version: u16,
    node_id: String,
    logical_session_id: String,
    attachment_id: String,
    work_id: Value,
    collaborative_run_id: Value,
    stream_id: StreamId,
    turn_id: Option<String>,
    seq: u64,
    source_span: Option<SourceSpan>,
    timestamp: String,
    rate_class: RateClass,
    kind: RuntimeEventKind,
    payload: Box<RawValue>,
    #[serde(default)]
    vendor_extension: Option<Value>,
    #[serde(flatten)]
    extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Error)]
pub enum EventSchemaError {
    #[error("event JSON is invalid")]
    InvalidJson(#[from] serde_json::Error),
    #[error("unsupported protocol version {0}")]
    UnsupportedProtocolVersion(u16),
    #[error("unsupported schema version {0}")]
    UnsupportedSchemaVersion(u16),
    #[error("stage-one event has a delegation identifier")]
    UnsupportedDelegationId,
    #[error("event identifier field is empty")]
    EmptyIdentifier,
    #[error("event timestamp is empty")]
    EmptyTimestamp,
    #[error("source span start exceeds end")]
    InvalidSourceSpan,
    #[error("event kind has the wrong rate class")]
    InvalidRateClass,
    #[error("event rate class is routed to the wrong stream")]
    InvalidStream,
    #[error("event payload must be an object")]
    InvalidPayload,
    #[error("event payload is missing required field {0}")]
    MissingPayloadField(&'static str),
    #[error("event payload enum field is invalid")]
    InvalidPayloadEnum,
    #[error("resource reference is invalid")]
    InvalidResourceRef,
    #[error("vendor event requires vendor_extension.method")]
    MissingVendorMethod,
}

impl TryFrom<WireEnvelope> for RuntimeEventEnvelope {
    type Error = EventSchemaError;

    fn try_from(wire: WireEnvelope) -> Result<Self, Self::Error> {
        let envelope = Self {
            protocol_version: wire.protocol_version,
            schema_version: wire.schema_version,
            node_id: wire.node_id,
            logical_session_id: wire.logical_session_id,
            attachment_id: wire.attachment_id,
            work_id: (!wire.work_id.is_null())
                .then(|| wire.work_id.as_str().unwrap_or_default().to_owned()),
            collaborative_run_id: (!wire.collaborative_run_id.is_null()).then(|| {
                wire.collaborative_run_id
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            }),
            stream_id: wire.stream_id,
            turn_id: wire.turn_id,
            seq: wire.seq,
            source_span: wire.source_span,
            timestamp: wire.timestamp,
            rate_class: wire.rate_class,
            kind: wire.kind,
            payload: wire.payload,
            vendor_extension: wire.vendor_extension,
            extensions: wire.extensions,
        };
        envelope.validate()?;
        Ok(envelope)
    }
}

impl<'de> Deserialize<'de> for RuntimeEventEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WireEnvelope::deserialize(deserializer)?;
        Self::try_from(wire).map_err(serde::de::Error::custom)
    }
}

impl RuntimeEventEnvelope {
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, EventSchemaError> {
        Ok(serde_json::from_slice(bytes)?)
    }

    pub fn from_value(value: Value) -> Result<Self, EventSchemaError> {
        Ok(serde_json::from_value(value)?)
    }

    pub fn to_json_vec(&self) -> Result<Vec<u8>, EventSchemaError> {
        Ok(serde_json::to_vec(self)?)
    }

    pub const fn kind(&self) -> &'static str {
        self.kind.as_str()
    }

    pub const fn protocol_version(&self) -> u16 {
        self.protocol_version
    }

    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    pub fn logical_session_id(&self) -> &str {
        &self.logical_session_id
    }

    pub fn attachment_id(&self) -> &str {
        &self.attachment_id
    }

    pub const fn stream_id(&self) -> StreamId {
        self.stream_id
    }

    pub fn turn_id(&self) -> Option<&str> {
        self.turn_id.as_deref()
    }

    pub const fn seq(&self) -> u64 {
        self.seq
    }

    pub const fn source_span(&self) -> Option<SourceSpan> {
        self.source_span
    }

    pub fn timestamp(&self) -> &str {
        &self.timestamp
    }

    pub const fn rate_class(&self) -> RateClass {
        self.rate_class
    }

    pub const fn event_kind(&self) -> RuntimeEventKind {
        self.kind
    }

    pub fn payload(&self) -> &RawValue {
        &self.payload
    }

    pub fn vendor_extension(&self) -> Option<&Value> {
        self.vendor_extension.as_ref()
    }

    pub fn extensions(&self) -> &BTreeMap<String, Value> {
        &self.extensions
    }

    fn validate(&self) -> Result<(), EventSchemaError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(EventSchemaError::UnsupportedProtocolVersion(
                self.protocol_version,
            ));
        }
        if self.schema_version != SCHEMA_VERSION {
            return Err(EventSchemaError::UnsupportedSchemaVersion(
                self.schema_version,
            ));
        }
        if self.work_id.is_some() || self.collaborative_run_id.is_some() {
            return Err(EventSchemaError::UnsupportedDelegationId);
        }
        if self.node_id.is_empty()
            || self.logical_session_id.is_empty()
            || self.attachment_id.is_empty()
            || self.turn_id.as_deref().is_some_and(str::is_empty)
        {
            return Err(EventSchemaError::EmptyIdentifier);
        }
        if self.timestamp.is_empty() {
            return Err(EventSchemaError::EmptyTimestamp);
        }
        validate_span(self.source_span)?;
        if self
            .kind
            .required_rate()
            .is_some_and(|required| required != self.rate_class)
        {
            return Err(EventSchemaError::InvalidRateClass);
        }
        if self.kind == RuntimeEventKind::Vendor && self.rate_class == RateClass::R0 {
            return Err(EventSchemaError::InvalidRateClass);
        }
        let expected_stream = if self.rate_class == RateClass::R0 {
            StreamId::Control
        } else {
            StreamId::Main
        };
        if self.stream_id != expected_stream {
            return Err(EventSchemaError::InvalidStream);
        }
        let payload: Value = serde_json::from_str(self.payload.get())?;
        if self.extensions.contains_key("remote_path")
            || self
                .extensions
                .values()
                .any(|value| contains_forbidden_key(value, "remote_path"))
            || contains_forbidden_key(&payload, "remote_path")
            || self
                .vendor_extension
                .as_ref()
                .is_some_and(|value| contains_forbidden_key(value, "remote_path"))
        {
            return Err(EventSchemaError::InvalidResourceRef);
        }
        let object = payload
            .as_object()
            .ok_or(EventSchemaError::InvalidPayload)?;
        for field in self.kind.required_fields() {
            if !object.contains_key(*field) {
                return Err(EventSchemaError::MissingPayloadField(field));
            }
        }
        self.validate_payload(object)?;
        if self.kind == RuntimeEventKind::Vendor
            && self
                .vendor_extension
                .as_ref()
                .and_then(Value::as_object)
                .and_then(|value| value.get("method"))
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        {
            return Err(EventSchemaError::MissingVendorMethod);
        }
        Ok(())
    }

    fn validate_payload(
        &self,
        object: &serde_json::Map<String, Value>,
    ) -> Result<(), EventSchemaError> {
        match self.kind {
            RuntimeEventKind::AttachmentStarted => {
                string_field(object, "runtime_id")?;
                string_field(object, "agent_kind")?;
                object_field(object, "capabilities_snapshot")?;
            }
            RuntimeEventKind::AttachmentEnded => {
                enum_field(
                    object,
                    "end_reason",
                    &["completed", "failed", "fenced", "interrupted"],
                )?;
                optional_string_field(object, "detail")?;
            }
            RuntimeEventKind::TurnStarted => non_null_field(object, "user_input_ref")?,
            RuntimeEventKind::TurnEnded => {
                enum_field(
                    object,
                    "end_reason",
                    &["completed", "interrupted", "error", "cancelled"],
                )?;
                optional_object_or_string_field(object, "error")?;
            }
            RuntimeEventKind::ApprovalRequested => {
                string_field(object, "approval_id")?;
                string_field(object, "tool")?;
                string_value(object, "summary")?;
                string_array_field(object, "options", true)?;
                optional_u64_field(object, "timeout_ms")?;
            }
            RuntimeEventKind::ApprovalResolved => {
                string_field(object, "approval_id")?;
                enum_field(object, "outcome", &["approved", "denied", "cancelled"])?;
            }
            RuntimeEventKind::InputRequested => {
                string_field(object, "input_id")?;
                string_value(object, "prompt")?;
                optional_schema_field(object, "schema")?;
            }
            RuntimeEventKind::InputResolved => {
                string_field(object, "input_id")?;
            }
            RuntimeEventKind::ErrorRaised => {
                string_field(object, "code")?;
                string_value(object, "message")?;
                bool_field(object, "fatal")?;
                enum_field(object, "source", &["adapter", "runtime", "node"])?;
            }
            RuntimeEventKind::TextDelta => {
                enum_field(object, "role", &["assistant"])?;
                string_value(object, "content")?;
                optional_positive_u64_field(object, "merged_count")?;
            }
            RuntimeEventKind::ThinkingDelta | RuntimeEventKind::PlanDelta => {
                string_value(object, "content")?;
                optional_positive_u64_field(object, "merged_count")?;
            }
            RuntimeEventKind::MessageCompleted => {
                string_field(object, "role")?;
                string_value(object, "content")?;
                string_field(object, "item_id")?;
            }
            RuntimeEventKind::ToolCallStarted => {
                string_field(object, "tool_id")?;
                string_field(object, "name")?;
            }
            RuntimeEventKind::ToolCallArgsDelta => {
                string_field(object, "tool_id")?;
                string_value(object, "args_delta")?;
            }
            RuntimeEventKind::ToolCallOutputDelta => {
                string_field(object, "tool_id")?;
                string_value(object, "chunk")?;
            }
            RuntimeEventKind::ToolCallCompleted => {
                string_field(object, "tool_id")?;
                bool_field(object, "is_error")?;
                optional_i64_field(object, "exit_code")?;
            }
            RuntimeEventKind::FileChangeCompleted => {
                string_field(object, "tool_id")?;
                string_value(object, "patch")?;
                string_array_field(object, "paths", true)?;
            }
            RuntimeEventKind::UsageReported => {
                u64_field(object, "input_tokens")?;
                u64_field(object, "output_tokens")?;
                optional_u64_field(object, "cached_tokens")?;
                optional_string_field(object, "model")?;
            }
            RuntimeEventKind::LogRecord => {
                string_field(object, "source")?;
                string_field(object, "level")?;
                string_value(object, "message")?;
                optional_bool_field(object, "truncated")?;
                optional_u64_field(object, "original_len")?;
                if object.get("truncated").and_then(Value::as_bool) == Some(true)
                    && object.get("original_len").and_then(Value::as_u64).is_none()
                {
                    return Err(EventSchemaError::InvalidPayload);
                }
            }
            RuntimeEventKind::ProgressTick => {
                string_field(object, "turn_id")?;
                string_field(object, "phase")?;
                optional_percent_field(object, "percent")?;
            }
            RuntimeEventKind::ResourceSample => {
                number_field(object, "cpu")?;
                u64_field(object, "memory")?;
                string_field(object, "sampled_at")?;
            }
            RuntimeEventKind::ResourceRefCreated => {
                serde_json::from_value::<ResourceRef>(object["ref"].clone())
                    .map_err(|_| EventSchemaError::InvalidResourceRef)?;
            }
            RuntimeEventKind::StreamGap => {
                let classes = object["affected_rate_classes"]
                    .as_array()
                    .ok_or(EventSchemaError::InvalidPayloadEnum)?;
                for class in classes {
                    serde_json::from_value::<RateClass>(class.clone())
                        .map_err(|_| EventSchemaError::InvalidPayloadEnum)?;
                }
                validate_optional_span(object.get("known_source_span"))?;
            }
            RuntimeEventKind::DiagnosticGap => {
                string_field(object, "reason")?;
                validate_optional_span(object.get("source_span"))?;
                if object.get("source_span").is_none_or(Value::is_null)
                    && object.get("time_window").is_none_or(Value::is_null)
                {
                    return Err(EventSchemaError::InvalidPayload);
                }
            }
            RuntimeEventKind::StreamAborted => string_field(object, "reason")?,
            RuntimeEventKind::Vendor => {}
        }
        Ok(())
    }
}

fn enum_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
    allowed: &[&str],
) -> Result<(), EventSchemaError> {
    let value = object
        .get(field)
        .and_then(Value::as_str)
        .ok_or(EventSchemaError::InvalidPayloadEnum)?;
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(EventSchemaError::InvalidPayloadEnum)
    }
}

fn validate_optional_span(value: Option<&Value>) -> Result<(), EventSchemaError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let span: SourceSpan =
        serde_json::from_value(value.clone()).map_err(|_| EventSchemaError::InvalidSourceSpan)?;
    validate_span(Some(span))
}

fn validate_span(span: Option<SourceSpan>) -> Result<(), EventSchemaError> {
    if span.is_some_and(|span| span.start > span.end) {
        Err(EventSchemaError::InvalidSourceSpan)
    } else {
        Ok(())
    }
}

fn non_null_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<(), EventSchemaError> {
    if object.get(field).is_some_and(|value| !value.is_null()) {
        Ok(())
    } else {
        Err(EventSchemaError::InvalidPayload)
    }
}

fn string_value(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<(), EventSchemaError> {
    if object.get(field).and_then(Value::as_str).is_some() {
        Ok(())
    } else {
        Err(EventSchemaError::InvalidPayload)
    }
}

fn string_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<(), EventSchemaError> {
    if object
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
    {
        Ok(())
    } else {
        Err(EventSchemaError::InvalidPayload)
    }
}

fn object_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<(), EventSchemaError> {
    if object.get(field).and_then(Value::as_object).is_some() {
        Ok(())
    } else {
        Err(EventSchemaError::InvalidPayload)
    }
}

fn bool_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<(), EventSchemaError> {
    if object.get(field).and_then(Value::as_bool).is_some() {
        Ok(())
    } else {
        Err(EventSchemaError::InvalidPayload)
    }
}

fn u64_field(object: &serde_json::Map<String, Value>, field: &str) -> Result<(), EventSchemaError> {
    if object.get(field).and_then(Value::as_u64).is_some() {
        Ok(())
    } else {
        Err(EventSchemaError::InvalidPayload)
    }
}

fn number_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<(), EventSchemaError> {
    if object
        .get(field)
        .and_then(Value::as_f64)
        .is_some_and(f64::is_finite)
    {
        Ok(())
    } else {
        Err(EventSchemaError::InvalidPayload)
    }
}

fn string_array_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
    require_nonempty: bool,
) -> Result<(), EventSchemaError> {
    let values = object
        .get(field)
        .and_then(Value::as_array)
        .ok_or(EventSchemaError::InvalidPayload)?;
    if (require_nonempty && values.is_empty())
        || values
            .iter()
            .any(|value| value.as_str().is_none_or(str::is_empty))
    {
        Err(EventSchemaError::InvalidPayload)
    } else {
        Ok(())
    }
}

fn optional_string_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<(), EventSchemaError> {
    optional_field(object, field, |value| value.as_str().is_some())
}

fn optional_schema_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<(), EventSchemaError> {
    optional_field(object, field, |value| {
        value.as_object().is_some() || value.as_bool().is_some()
    })
}

fn optional_object_or_string_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<(), EventSchemaError> {
    optional_field(object, field, |value| {
        value.as_object().is_some() || value.as_str().is_some()
    })
}

fn optional_bool_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<(), EventSchemaError> {
    optional_field(object, field, |value| value.as_bool().is_some())
}

fn optional_u64_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<(), EventSchemaError> {
    optional_field(object, field, |value| value.as_u64().is_some())
}

fn optional_positive_u64_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<(), EventSchemaError> {
    optional_field(object, field, |value| {
        value.as_u64().is_some_and(|value| value > 0)
    })
}

fn optional_i64_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<(), EventSchemaError> {
    optional_field(object, field, |value| value.as_i64().is_some())
}

fn optional_percent_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<(), EventSchemaError> {
    optional_field(object, field, |value| {
        value
            .as_f64()
            .is_some_and(|value| value.is_finite() && (0.0..=100.0).contains(&value))
    })
}

fn optional_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
    validate: impl FnOnce(&Value) -> bool,
) -> Result<(), EventSchemaError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(()),
        Some(value) if validate(value) => Ok(()),
        Some(_) => Err(EventSchemaError::InvalidPayload),
    }
}

fn contains_forbidden_key(value: &Value, forbidden: &str) -> bool {
    match value {
        Value::Object(object) => {
            object.contains_key(forbidden)
                || object
                    .values()
                    .any(|value| contains_forbidden_key(value, forbidden))
        }
        Value::Array(values) => values
            .iter()
            .any(|value| contains_forbidden_key(value, forbidden)),
        _ => false,
    }
}
