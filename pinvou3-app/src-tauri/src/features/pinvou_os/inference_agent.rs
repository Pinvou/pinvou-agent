use serde_json::json;

use super::{CapabilityContract, Interruptibility, ResourceClass};

pub const INFERENCE_AGENT_ID: &str = "agent:inference";

pub fn inference_observe_contract() -> CapabilityContract {
    CapabilityContract {
        capability_id: "inference.observe".to_string(),
        version: 1,
        summary: "验证当前大模型路由、凭据与推理可用性".to_string(),
        input_schema: json!({ "type": "object", "additionalProperties": false }),
        output_schema: json!({
            "type": "object",
            "required": ["status"],
            "properties": {
                "status": { "enum": ["unknown", "ready", "degraded", "unavailable"] },
                "model": { "type": ["string", "null"] },
                "lastSuccessAtMs": { "type": ["integer", "null"] },
                "lastSuccessLatencyMs": { "type": ["integer", "null"] },
                "reasonCode": { "type": ["string", "null"] }
            }
        }),
        // Observing an unavailable route is still a successful observation;
        // readiness preconditions belong to inference execution, not health.
        preconditions: Vec::new(),
        permissions: Vec::new(),
        side_effects: Vec::new(),
        resource_class: ResourceClass::Light,
        interruptibility: Interruptibility::Immediate,
        idempotent: true,
    }
}
