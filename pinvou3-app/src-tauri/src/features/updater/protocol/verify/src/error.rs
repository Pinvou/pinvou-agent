use pinvou_upgrade_codec::{
    BindingError, ContextBindingError, CredentialSemanticsError, EnvelopeShapeError, Error,
    HighWaterError, MetadataSemanticsError, PolicyLookupError, RootPolicyError, SchemaError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationError {
    Json(Error),
    Envelope(EnvelopeShapeError),
    Schema(SchemaError),
    Context(ContextBindingError),
    Policy(PolicyLookupError),
    Time(MetadataSemanticsError),
    Metadata(MetadataSemanticsError),
    Credential(CredentialSemanticsError),
    HighWater(HighWaterError),
    Binding(BindingError),
    Root(RootPolicyError),
    RootContinuityInvalid,
    ActiveReadSetInvalid,
    RootTransitionRequired,
    SigningKeyDenied,
    SigningKeyUnauthorized,
    InvalidDescriptor,
    InvalidSignature,
    ThresholdUnmet,
    CredentialKeyBinding,
}

impl std::fmt::Display for VerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Json(e) => return e.fmt(f),
            Self::Envelope(e) => return e.fmt(f),
            Self::Schema(e) => return e.fmt(f),
            Self::Context(e) => return e.fmt(f),
            Self::Policy(e) => return e.fmt(f),
            Self::Time(e) | Self::Metadata(e) => return e.fmt(f),
            Self::Credential(e) => return e.fmt(f),
            Self::HighWater(e) => return e.fmt(f),
            Self::Binding(e) => return e.fmt(f),
            Self::Root(e) => return e.fmt(f),
            Self::RootContinuityInvalid => "ROOT_CONTINUITY_INVALID",
            Self::ActiveReadSetInvalid => "ACTIVE_READ_SET_INVALID",
            Self::RootTransitionRequired => "ROOT_TRANSITION_REQUIRED",
            Self::SigningKeyDenied => "SIGNING_KEY_DENIED",
            Self::SigningKeyUnauthorized => "SIGNING_KEY_NOT_AUTHORIZED",
            Self::InvalidDescriptor => "SIGNING_KEY_DESCRIPTOR_INVALID",
            Self::InvalidSignature => "SIGNATURE_INVALID",
            Self::ThresholdUnmet => "SIGNATURE_THRESHOLD_UNMET",
            Self::CredentialKeyBinding => "CREDENTIAL_KEY_BINDING_INVALID",
        })
    }
}
impl std::error::Error for VerificationError {}
