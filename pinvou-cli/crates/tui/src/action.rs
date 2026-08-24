use pinvou_protocol::RuntimeEventEnvelope;

use crate::backend::{BackendError, RuntimeList, RuntimeStatus};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalDecision {
    AllowOnce,
    Deny,
}

#[derive(Debug)]
pub enum Action {
    Submit(String),
    Runtime {
        operation_token: crate::model::OperationToken,
        event: RuntimeEventEnvelope,
    },
    ApprovalChosen(ApprovalDecision),
    ApprovalResolutionCompleted {
        turn_id: String,
        approval_id: String,
        operation_token: crate::model::OperationToken,
        result: Result<(), BackendError>,
    },
    InputSubmitted(String),
    InputResolutionCompleted {
        turn_id: String,
        input_id: String,
        operation_token: crate::model::OperationToken,
        result: Result<(), BackendError>,
    },
    TurnStreamCompleted {
        operation_token: crate::model::OperationToken,
        result: Result<(), BackendError>,
    },
    Interrupt,
    InterruptCompleted {
        turn_id: String,
        operation_token: crate::model::OperationToken,
        result: Result<(), BackendError>,
    },
    LoadRuntimeList,
    RuntimeListLoaded {
        operation_token: crate::model::OperationToken,
        result: Result<RuntimeList, BackendError>,
    },
    RuntimeSwitch(String),
    RuntimeSwitched {
        operation_token: crate::model::OperationToken,
        result: Result<RuntimeStatus, BackendError>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Effect {
    StartTurn {
        prompt: String,
        operation_token: crate::model::OperationToken,
    },
    ResolveApproval {
        turn_id: String,
        approval_id: String,
        operation_token: crate::model::OperationToken,
        accepted: bool,
    },
    ResolveInput {
        turn_id: String,
        input_id: String,
        operation_token: crate::model::OperationToken,
        value: String,
    },
    Interrupt {
        turn_id: String,
        operation_token: crate::model::OperationToken,
    },
    LoadRuntimeList {
        operation_token: crate::model::OperationToken,
    },
    SwitchRuntime {
        runtime: String,
        operation_token: crate::model::OperationToken,
    },
}
