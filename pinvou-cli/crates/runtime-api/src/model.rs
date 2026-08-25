use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AdapterError;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct RuntimeCapabilities {
    pub interactive_chat: bool,
    pub native_resume: bool,
    pub history_import: bool,
    pub tool_approval: bool,
    pub elicitation: bool,
    pub steering: bool,
    pub image_input: bool,
    pub file_reference: bool,
    pub session_listing: bool,
    pub model_catalog: bool,
    pub model_switching: bool,
    pub permission_profiles: bool,
    pub session_modes: Vec<String>,
    pub config_options: Vec<String>,
    pub auth_flows: Vec<String>,
}

macro_rules! validated_identifier {
    ($name:ident, $label:literal) => {
        #[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, AdapterError> {
                let value = value.into();
                if value.trim().is_empty() {
                    Err(AdapterError::InvalidRequest {
                        details: concat!($label, " is empty").into(),
                    })
                } else {
                    Ok(Self(value))
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

validated_identifier!(LogicalSessionId, "logical session id");
validated_identifier!(ModelId, "model id");

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
    Completed,
    Failed,
    Interrupted,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionDescriptor {
    pub id: LogicalSessionId,
    pub title: String,
    pub last_active_at: String,
    pub runtime_id: String,
    pub model_id: Option<ModelId>,
    pub status: SessionStatus,
    pub native_session_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionSnapshot {
    pub descriptor: SessionDescriptor,
    pub cursor: u64,
    pub normalized_events: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelDescriptor {
    pub id: ModelId,
    pub display_name: String,
    pub available: bool,
    pub is_default: bool,
}

impl ModelDescriptor {
    pub fn new(
        id: impl Into<String>,
        display_name: impl Into<String>,
        available: bool,
        is_default: bool,
    ) -> Result<Self, AdapterError> {
        let display_name = display_name.into();
        if display_name.trim().is_empty() {
            return Err(AdapterError::InvalidRequest {
                details: "model display name is empty".into(),
            });
        }
        Ok(Self {
            id: ModelId::new(id)?,
            display_name,
            available,
            is_default,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelCatalog {
    pub runtime_id: String,
    pub current_model: Option<ModelId>,
    pub models: Vec<ModelDescriptor>,
}

impl ModelCatalog {
    pub fn new(
        runtime_id: impl Into<String>,
        current_model: Option<ModelId>,
        models: Vec<ModelDescriptor>,
    ) -> Result<Self, AdapterError> {
        let runtime_id = runtime_id.into();
        if runtime_id.trim().is_empty() {
            return Err(AdapterError::InvalidRequest {
                details: "runtime id is empty".into(),
            });
        }
        if models.iter().filter(|model| model.is_default).count() > 1 {
            return Err(AdapterError::InvalidRequest {
                details: "model catalog has multiple defaults".into(),
            });
        }
        if let Some(current) = current_model.as_ref()
            && !models
                .iter()
                .any(|model| model.available && model.id == *current)
        {
            return Err(AdapterError::InvalidRequest {
                details: "current model is absent or unavailable".into(),
            });
        }
        Ok(Self {
            runtime_id,
            current_model,
            models,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalProfile {
    Request,
    Assisted,
    FullAccess,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlStrength {
    Enforced,
    Partial,
    Unsupported,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PermissionCapability {
    pub supported_profiles: Vec<ApprovalProfile>,
    pub control_strength: ControlStrength,
    pub native_mode: Option<String>,
    pub sandbox: Option<String>,
    pub residual_guards: Vec<String>,
    pub evidence_version: String,
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
            "list_sessions" | "read_session" => value.session_listing = false,
            "list_models" => value.model_catalog = false,
            "inspect_permissions" => value.permission_profiles = false,
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
