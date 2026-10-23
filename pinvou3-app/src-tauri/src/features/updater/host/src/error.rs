//! Fixed nonsecret diagnostics. Native code and sensitive paths never enter Display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    Unavailable,
    ProfileDisabled,
    InitialTrustUnavailable,
    SourceUnavailable,
    InstallerContextRefused,
    ScopeConflict,
    SecurityRefused,
    StateCorrupt,
    StateCommitUnknown,
    ServiceIdentityRefused,
    PeerRefused,
    IpcInvalid,
    IpcConflict,
    Busy,
    ImageRefused,
    HandoffOutcomeUnknown,
    Referenced,
    RepairRequired,
    OperationUnsupported,
    SetupIncomplete,
    PrerequisiteUnavailable,
    OptionalLaunchUnavailable,
    RestartCanceled,
    RestartNotReady,
    RestartConflict,
    StagingCanceled,
}
impl Error {
    pub(crate) fn from_code(code: &str) -> Option<Self> {
        const ALL: &[Error] = &[
            Error::Unsupported,
            Error::Unavailable,
            Error::ProfileDisabled,
            Error::InitialTrustUnavailable,
            Error::SourceUnavailable,
            Error::InstallerContextRefused,
            Error::ScopeConflict,
            Error::SecurityRefused,
            Error::StateCorrupt,
            Error::StateCommitUnknown,
            Error::ServiceIdentityRefused,
            Error::PeerRefused,
            Error::IpcInvalid,
            Error::IpcConflict,
            Error::Busy,
            Error::ImageRefused,
            Error::HandoffOutcomeUnknown,
            Error::Referenced,
            Error::RepairRequired,
            Error::OperationUnsupported,
            Error::SetupIncomplete,
            Error::PrerequisiteUnavailable,
            Error::OptionalLaunchUnavailable,
            Error::RestartCanceled,
            Error::RestartNotReady,
            Error::RestartConflict,
            Error::StagingCanceled,
        ];
        ALL.iter().copied().find(|error| error.code() == code)
    }
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unsupported => "HOST_UNSUPPORTED",
            Self::Unavailable => "HOST_UNAVAILABLE",
            Self::ProfileDisabled => "HOST_PROFILE_DISABLED",
            Self::InitialTrustUnavailable => "HOST_INITIAL_TRUST_UNAVAILABLE",
            Self::SourceUnavailable => "HOST_SOURCE_UNAVAILABLE",
            Self::InstallerContextRefused => "HOST_INSTALLER_CONTEXT_REFUSED",
            Self::ScopeConflict => "HOST_SCOPE_CONFLICT",
            Self::SecurityRefused => "HOST_SECURITY_REFUSED",
            Self::StateCorrupt => "HOST_STATE_CORRUPT",
            Self::StateCommitUnknown => "HOST_STATE_COMMIT_UNKNOWN",
            Self::ServiceIdentityRefused => "HOST_SERVICE_IDENTITY_REFUSED",
            Self::PeerRefused => "HOST_PEER_REFUSED",
            Self::IpcInvalid => "HOST_IPC_INVALID",
            Self::IpcConflict => "HOST_IPC_CONFLICT",
            Self::Busy => "HOST_BUSY",
            Self::ImageRefused => "HOST_IMAGE_REFUSED",
            Self::HandoffOutcomeUnknown => "HOST_HANDOFF_OUTCOME_UNKNOWN",
            Self::Referenced => "HOST_REFERENCED",
            Self::RepairRequired => "HOST_REPAIR_REQUIRED",
            Self::OperationUnsupported => "HOST_OPERATION_UNSUPPORTED",
            Self::SetupIncomplete => "HOST_SETUP_INCOMPLETE",
            Self::PrerequisiteUnavailable => "HOST_PREREQUISITE_UNAVAILABLE",
            Self::OptionalLaunchUnavailable => "HOST_OPTIONAL_LAUNCH_UNAVAILABLE",
            Self::RestartCanceled => "HOST_RESTART_CANCELED",
            Self::RestartNotReady => "HOST_RESTART_NOT_READY",
            Self::RestartConflict => "HOST_RESTART_CONFLICT",
            Self::StagingCanceled => "HOST_BYTE_STAGING_CANCELED",
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for Error {}
pub(crate) type Result<T> = std::result::Result<T, Error>;
