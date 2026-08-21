use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AdapterError;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeCapabilities {
    pub interactive_chat: bool,
    pub native_resume: bool,
    pub history_import: bool,
    pub tool_approval: bool,
    pub elicitation: bool,
    pub steering: bool,
    pub image_input: bool,
    pub file_reference: bool,
    pub session_modes: Vec<String>,
    pub config_options: Vec<String>,
    pub auth_flows: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct NegotiatedCapabilities(Option<RuntimeCapabilities>);

impl NegotiatedCapabilities {
    pub fn complete(&mut self, capabilities: RuntimeCapabilities) {
        self.0 = Some(capabilities);
    }
    pub fn snapshot(&self) -> Result<RuntimeCapabilities, AdapterError> {
        self.0.clone().ok_or(AdapterError::NotProbed)
    }
    pub fn method_not_found(&mut self, method: &str) -> Result<(), AdapterError> {
        let value = self.0.as_mut().ok_or(AdapterError::NotProbed)?;
        match method {
            "resume" => value.native_resume = false,
            "import_context" => value.history_import = false,
            "approve" => value.tool_approval = false,
            "respond_input" => value.elicitation = false,
            "steer" => value.steering = false,
            _ => {
                return Err(AdapterError::Protocol {
                    code: Some(-32601),
                    method: Some(method.into()),
                    details: "unknown negotiated method".into(),
                });
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStatus {
    Authenticated,
    Blocked,
    NotRequired,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeSession(String);

impl RuntimeSession {
    pub fn new(value: impl Into<String>) -> Result<Self, AdapterError> {
        let value = value.into();
        if value.is_empty() {
            Err(AdapterError::InvalidRequest {
                details: "runtime session id is empty".into(),
            })
        } else {
            Ok(Self(value))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RuntimeCommand {
    pub kind: String,
    pub payload: Value,
}

impl RuntimeCommand {
    pub fn text(text: impl Into<String>) -> Result<Self, AdapterError> {
        let text = text.into();
        if text.is_empty() {
            return Err(AdapterError::InvalidRequest {
                details: "runtime text is empty".into(),
            });
        }
        Ok(Self {
            kind: "text".into(),
            payload: Value::String(text),
        })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RuntimeOperation {
    pub operation_id: String,
    #[serde(default)]
    pub options: Value,
}

impl RuntimeOperation {
    pub fn new(operation_id: impl Into<String>, options: Value) -> Result<Self, AdapterError> {
        let operation_id = operation_id.into();
        if operation_id.is_empty() {
            Err(AdapterError::InvalidRequest {
                details: "operation id is empty".into(),
            })
        } else {
            Ok(Self {
                operation_id,
                options,
            })
        }
    }
}
