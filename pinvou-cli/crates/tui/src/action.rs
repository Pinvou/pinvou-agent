use pinvou_protocol::RuntimeEventEnvelope;

use crate::backend::{
    BackendError, ModelList, PermissionMode, PermissionStatus, ResumeResult, RuntimeList,
    RuntimeStatus, SessionList,
};

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
    LoadSessionList {
        query: String,
    },
    SessionListLoaded {
        operation_token: crate::model::OperationToken,
        result: Result<SessionList, BackendError>,
    },
    ResumeSession(String),
    SessionResumed {
        operation_token: crate::model::OperationToken,
        result: Result<ResumeResult, BackendError>,
    },
    LoadModelList,
    ModelListLoaded {
        operation_token: crate::model::OperationToken,
        result: Result<ModelList, BackendError>,
    },
    ModelSwitch(String),
    ModelSwitched {
        operation_token: crate::model::OperationToken,
        result: Result<(), BackendError>,
    },
    LoadPermissions,
    PermissionsLoaded {
        operation_token: crate::model::OperationToken,
        result: Result<PermissionStatus, BackendError>,
    },
    PermissionSwitch {
        profile: PermissionMode,
        full_access_confirmed: bool,
    },
    PermissionSwitched {
        operation_token: crate::model::OperationToken,
        result: Result<(), BackendError>,
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
    LoadSessionList {
        operation_token: crate::model::OperationToken,
        query: String,
    },
    ResumeSession {
        operation_token: crate::model::OperationToken,
        session_id: String,
    },
    LoadModelList {
        operation_token: crate::model::OperationToken,
    },
    SwitchModel {
        operation_token: crate::model::OperationToken,
        model_id: String,
    },
    LoadPermissions {
        operation_token: crate::model::OperationToken,
    },
    SwitchPermissions {
        operation_token: crate::model::OperationToken,
        profile: PermissionMode,
        full_access_confirmed: bool,
    },
}
