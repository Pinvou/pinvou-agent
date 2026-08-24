use pinvou_protocol::RuntimeEventEnvelope;

use crate::backend::RuntimeStatus;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalDecision {
    AllowOnce,
    Deny,
}

#[derive(Debug)]
pub enum Action {
    Submit(String),
    Runtime(RuntimeEventEnvelope),
    ApprovalChosen(ApprovalDecision),
    InputSubmitted(String),
    Interrupt,
    RuntimeSwitch(String),
    RuntimeSwitched(Result<RuntimeStatus, String>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Effect {
    StartTurn { prompt: String },
    ResolveApproval { approval_id: String, accepted: bool },
    ResolveInput { input_id: String, value: String },
    Interrupt { turn_id: String },
    SwitchRuntime { runtime: String },
}
