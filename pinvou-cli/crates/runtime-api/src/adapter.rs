use pinvou_protocol::RuntimeEventEnvelope;

use crate::{
    AdapterError, AuthStatus, ModelCatalog, PermissionCapability, RuntimeCapabilities,
    RuntimeCommand, RuntimeOperation, RuntimeSession, SessionDescriptor, SessionSnapshot,
};

pub const RUNTIME_ADAPTER_INTERFACE_VERSION: u16 = 2;
pub type RuntimeEventSubscription =
    Box<dyn Iterator<Item = Result<RuntimeEventEnvelope, AdapterError>> + Send>;

pub trait AgentRuntimeAdapter: Send {
    fn interface_version(&self) -> u16 {
        RUNTIME_ADAPTER_INTERFACE_VERSION
    }
    fn probe(&mut self) -> Result<(), AdapterError>;
    /// Returns the negotiated capability snapshot after `probe`; pre-probe access must fail.
    fn capabilities(&self) -> Result<RuntimeCapabilities, AdapterError>;
    fn auth_status(&mut self) -> Result<AuthStatus, AdapterError>;
    fn start_auth(&mut self, _: RuntimeOperation) -> Result<(), AdapterError> {
        Err(AdapterError::unsupported("start_auth"))
    }
    fn create(&mut self, _: RuntimeOperation) -> Result<RuntimeSession, AdapterError> {
        Err(AdapterError::unsupported("create"))
    }
    fn resume(&mut self, _: RuntimeOperation) -> Result<RuntimeSession, AdapterError> {
        Err(AdapterError::unsupported("resume"))
    }
    fn list_sessions(
        &mut self,
        _: RuntimeOperation,
    ) -> Result<Vec<SessionDescriptor>, AdapterError> {
        Err(AdapterError::unsupported("list_sessions"))
    }
    fn read_session(&mut self, _: RuntimeOperation) -> Result<SessionSnapshot, AdapterError> {
        Err(AdapterError::unsupported("read_session"))
    }
    fn list_models(&mut self, _: RuntimeOperation) -> Result<ModelCatalog, AdapterError> {
        Err(AdapterError::unsupported("list_models"))
    }
    fn inspect_permissions(
        &mut self,
        _: RuntimeOperation,
    ) -> Result<PermissionCapability, AdapterError> {
        Err(AdapterError::unsupported("inspect_permissions"))
    }
    fn import_context(
        &mut self,
        _: &RuntimeSession,
        _: RuntimeOperation,
    ) -> Result<(), AdapterError> {
        Err(AdapterError::unsupported("import_context"))
    }
    fn send(&mut self, _: &RuntimeSession, _: RuntimeCommand) -> Result<(), AdapterError> {
        Err(AdapterError::unsupported("send"))
    }
    fn approve(&mut self, _: &RuntimeSession, _: RuntimeOperation) -> Result<(), AdapterError> {
        Err(AdapterError::unsupported("approve"))
    }
    fn respond_input(
        &mut self,
        _: &RuntimeSession,
        _: RuntimeOperation,
    ) -> Result<(), AdapterError> {
        Err(AdapterError::unsupported("respond_input"))
    }
    fn steer(&mut self, _: &RuntimeSession, _: RuntimeCommand) -> Result<(), AdapterError> {
        Err(AdapterError::unsupported("steer"))
    }
    fn interrupt(&mut self, _: &RuntimeSession) -> Result<(), AdapterError> {
        Err(AdapterError::unsupported("interrupt"))
    }
    fn subscribe_events(
        &mut self,
        session: &RuntimeSession,
    ) -> Result<RuntimeEventSubscription, AdapterError>;
    /// Must be idempotent and close the child process plus every event reader.
    fn close(&mut self, session: &RuntimeSession) -> Result<(), AdapterError>;
}
