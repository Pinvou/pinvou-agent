use std::io::{self, Write};

use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;

pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;
const LENGTH_PREFIX_LEN: usize = size_of::<u32>();
const IPC_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IpcMessageKind {
    Req,
    Rsp,
    Evt,
    Ack,
    Err,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct IpcMessage {
    v: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    kind: IpcMessageKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    topic: Option<String>,
    payload: Value,
}

#[derive(Deserialize)]
struct WireMessage {
    v: u16,
    #[serde(default)]
    id: Option<Value>,
    kind: IpcMessageKind,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    payload: Value,
}

impl<'de> Deserialize<'de> for IpcMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WireMessage::deserialize(deserializer)?;
        if wire.v != IPC_VERSION {
            return Err(serde::de::Error::custom("unsupported IPC version"));
        }
        let valid_id = wire.id.as_ref().is_none_or(|id| !id.is_null());
        let valid = valid_id
            && match wire.kind {
                IpcMessageKind::Req => {
                    wire.id.is_some()
                        && wire.method.as_deref().is_some_and(|v| !v.is_empty())
                        && wire.topic.is_none()
                }
                IpcMessageKind::Rsp => {
                    wire.id.is_some() && wire.method.is_none() && wire.topic.is_none()
                }
                IpcMessageKind::Evt | IpcMessageKind::Ack => {
                    wire.id.is_none()
                        && wire.method.is_none()
                        && wire.topic.as_deref().is_some_and(|v| !v.is_empty())
                }
                IpcMessageKind::Err => wire.method.is_none() && wire.topic.is_none(),
            };
        if !valid {
            return Err(serde::de::Error::custom("invalid IPC message fields"));
        }
        Ok(Self {
            v: wire.v,
            id: wire.id,
            kind: wire.kind,
            method: wire.method,
            topic: wire.topic,
            payload: wire.payload,
        })
    }
}

impl IpcMessage {
    pub fn request(
        id: Value,
        method: impl Into<String>,
        payload: Value,
    ) -> Result<Self, FrameError> {
        Self::new(
            Some(id),
            IpcMessageKind::Req,
            Some(method.into()),
            None,
            payload,
        )
    }

    pub fn response(id: Value, payload: Value) -> Result<Self, FrameError> {
        Self::new(Some(id), IpcMessageKind::Rsp, None, None, payload)
    }

    pub fn event(topic: impl Into<String>, payload: Value) -> Result<Self, FrameError> {
        Self::new(None, IpcMessageKind::Evt, None, Some(topic.into()), payload)
    }

    pub fn ack(topic: impl Into<String>, payload: Value) -> Result<Self, FrameError> {
        Self::new(None, IpcMessageKind::Ack, None, Some(topic.into()), payload)
    }

    pub fn error(id: Option<Value>, payload: Value) -> Result<Self, FrameError> {
        Self::new(id, IpcMessageKind::Err, None, None, payload)
    }

    fn new(
        id: Option<Value>,
        kind: IpcMessageKind,
        method: Option<String>,
        topic: Option<String>,
        payload: Value,
    ) -> Result<Self, FrameError> {
        let message = Self {
            v: IPC_VERSION,
            id,
            kind,
            method,
            topic,
            payload,
        };
        let valid_id = message.id.as_ref().is_none_or(|id| !id.is_null());
        let valid = valid_id
            && match message.kind {
                IpcMessageKind::Req => {
                    message.id.is_some()
                        && message.method.as_deref().is_some_and(|v| !v.is_empty())
                        && message.topic.is_none()
                }
                IpcMessageKind::Rsp => {
                    message.id.is_some() && message.method.is_none() && message.topic.is_none()
                }
                IpcMessageKind::Evt | IpcMessageKind::Ack => {
                    message.id.is_none()
                        && message.method.is_none()
                        && message.topic.as_deref().is_some_and(|v| !v.is_empty())
                }
                IpcMessageKind::Err => message.method.is_none() && message.topic.is_none(),
            };
        if valid {
            Ok(message)
        } else {
            Err(FrameError::InvalidMessage)
        }
    }

    pub const fn version(&self) -> u16 {
        self.v
    }

    pub fn id(&self) -> Option<&Value> {
        self.id.as_ref()
    }

    pub const fn kind(&self) -> IpcMessageKind {
        self.kind
    }

    pub fn method(&self) -> Option<&str> {
        self.method.as_deref()
    }

    pub fn topic(&self) -> Option<&str> {
        self.topic.as_deref()
    }

    pub fn payload(&self) -> &Value {
        &self.payload
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum HelloKind {
    Hello,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HelloClient {
    kind: HelloKind,
    protocol_version: u16,
    client_info: Value,
}

impl HelloClient {
    pub fn new(client_info: Value) -> Result<Self, FrameError> {
        if !client_info.is_object() {
            return Err(FrameError::InvalidMessage);
        }
        Ok(Self {
            kind: HelloKind::Hello,
            protocol_version: IPC_VERSION,
            client_info,
        })
    }

    pub const fn protocol_version(&self) -> u16 {
        self.protocol_version
    }

    pub fn client_info(&self) -> &Value {
        &self.client_info
    }
}

#[derive(Deserialize)]
struct WireHelloClient {
    kind: HelloKind,
    protocol_version: u16,
    client_info: Value,
}

impl<'de> Deserialize<'de> for HelloClient {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WireHelloClient::deserialize(deserializer)?;
        if wire.protocol_version != IPC_VERSION
            || wire.kind != HelloKind::Hello
            || !wire.client_info.is_object()
        {
            return Err(serde::de::Error::custom("invalid hello client"));
        }
        Ok(Self {
            kind: wire.kind,
            protocol_version: wire.protocol_version,
            client_info: wire.client_info,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HelloServer {
    instance_id: String,
    protocol_version: u16,
}

impl HelloServer {
    pub fn new(instance_id: impl Into<String>) -> Result<Self, FrameError> {
        let instance_id = instance_id.into();
        if instance_id.is_empty() {
            return Err(FrameError::InvalidMessage);
        }
        Ok(Self {
            instance_id,
            protocol_version: IPC_VERSION,
        })
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub const fn protocol_version(&self) -> u16 {
        self.protocol_version
    }
}

#[derive(Deserialize)]
struct WireHelloServer {
    instance_id: String,
    protocol_version: u16,
}

impl<'de> Deserialize<'de> for HelloServer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WireHelloServer::deserialize(deserializer)?;
        if wire.protocol_version != IPC_VERSION || wire.instance_id.is_empty() {
            return Err(serde::de::Error::custom("invalid hello server"));
        }
        Ok(Self {
            instance_id: wire.instance_id,
            protocol_version: wire.protocol_version,
        })
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("frame is missing its four-byte length prefix")]
    MissingLengthPrefix,
    #[error("frame length {declared} exceeds maximum {maximum}")]
    FrameTooLarge { declared: usize, maximum: usize },
    #[error("frame declared {declared} payload bytes but received {actual}")]
    LengthMismatch { declared: usize, actual: usize },
    #[error("frame payload is not UTF-8")]
    InvalidUtf8,
    #[error("frame payload is not valid JSON or violates the message contract")]
    InvalidJson,
    #[error("IPC message fields violate the envelope contract")]
    InvalidMessage,
}

pub fn decode_length_prefix(prefix: [u8; LENGTH_PREFIX_LEN]) -> Result<usize, FrameError> {
    let declared = u32::from_le_bytes(prefix) as usize;
    if declared > MAX_FRAME_LEN {
        return Err(FrameError::FrameTooLarge {
            declared,
            maximum: MAX_FRAME_LEN,
        });
    }
    Ok(declared)
}

pub fn encode_frame<T: Serialize>(message: &T) -> Result<Vec<u8>, FrameError> {
    let mut writer = LimitedWriter::new(MAX_FRAME_LEN);
    if serde_json::to_writer(&mut writer, message).is_err() {
        return if writer.exceeded {
            Err(FrameError::FrameTooLarge {
                declared: MAX_FRAME_LEN + 1,
                maximum: MAX_FRAME_LEN,
            })
        } else {
            Err(FrameError::InvalidJson)
        };
    }
    let payload = writer.bytes;
    let mut frame = Vec::with_capacity(LENGTH_PREFIX_LEN + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub fn decode_frame<T: DeserializeOwned>(frame: &[u8]) -> Result<T, FrameError> {
    let prefix: [u8; LENGTH_PREFIX_LEN] = frame
        .get(..LENGTH_PREFIX_LEN)
        .ok_or(FrameError::MissingLengthPrefix)?
        .try_into()
        .expect("length already checked");
    let declared = decode_length_prefix(prefix)?;
    let payload = &frame[LENGTH_PREFIX_LEN..];
    if payload.len() != declared {
        return Err(FrameError::LengthMismatch {
            declared,
            actual: payload.len(),
        });
    }
    let json = std::str::from_utf8(payload).map_err(|_| FrameError::InvalidUtf8)?;
    serde_json::from_str(json).map_err(|_| FrameError::InvalidJson)
}

struct LimitedWriter {
    bytes: Vec<u8>,
    maximum: usize,
    exceeded: bool,
}

impl LimitedWriter {
    fn new(maximum: usize) -> Self {
        Self {
            bytes: Vec::new(),
            maximum,
            exceeded: false,
        }
    }
}

impl Write for LimitedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.len() > self.maximum.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(io::Error::other("frame limit exceeded"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
