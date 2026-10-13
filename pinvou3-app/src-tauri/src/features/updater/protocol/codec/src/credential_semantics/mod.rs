//! Fourteen-role provided credential semantics only, never consumption or authority.
mod authorization;
mod event_task;
mod update;
use crate::{
    BoundedOutput, Document, EnvelopeShapeError, Error, MetadataSemanticsError,
    SchemaCheckedEnvelope, SchemaError, SuppliedProtocolTime,
};
use serde_json::Value;
const DAY: f64 = 86_400_000.0;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialSemanticsError {
    Json(Error),
    Envelope(EnvelopeShapeError),
    Schema(SchemaError),
    Time(MetadataSemanticsError),
    References(MetadataSemanticsError),
    InvalidFields,
    UnsupportedRole,
    WindowInvalid,
    WindowClosed,
    DecisionBinding,
    HostTarget,
    MetadataSet,
    MetadataIncomplete,
    SupplyExpired,
    UpgradeMode,
    BaselineBinding,
    EntityChain,
    PlanBinding,
    VersionNotHigher,
    EntityState,
    RolloutBinding,
    BridgeBinding,
    PathBinding,
    PathMissing,
    BackupPolicy,
    PackageBinding,
    ReleaseBinding,
    ReferenceRole,
    TargetBinding,
    InstallWindowInvalid,
    InstallWindowClosed,
    OpeningBinding,
    LocatorWindow,
    AuthorizationId,
    FreezeBinding,
    BackupBinding,
    StagedBinding,
    SlotBinding,
    UpgradePurpose,
    ActivationMode,
    InstallNotYet,
    StagedTransaction,
    StagedWindow,
    EventWindow,
    TaskBinding,
    TaskTarget,
    TaskProof,
    TaskWindow,
}
impl std::fmt::Display for CredentialSemanticsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Json(error) => return error.fmt(f),
            Self::Envelope(error) => return error.fmt(f),
            Self::Schema(error) => return error.fmt(f),
            Self::Time(error) => return error.fmt(f),
            Self::References(error) => return error.fmt(f),
            Self::InvalidFields => "CREDENTIAL_SEMANTICS_FIELDS_INVALID",
            Self::UnsupportedRole => "CREDENTIAL_SEMANTICS_ROLE_INVALID",
            Self::WindowInvalid => "TIME_WINDOW_INVALID",
            Self::WindowClosed => "TIME_WINDOW_CLOSED",
            Self::DecisionBinding => "DECISION_BINDING_INVALID",
            Self::HostTarget => "HOST_TARGET_INVALID",
            Self::MetadataSet => "METADATA_SET_INVALID",
            Self::MetadataIncomplete => "METADATA_SET_INCOMPLETE",
            Self::SupplyExpired => "SUPPLY_CHAIN_EXPIRED",
            Self::UpgradeMode => "UPGRADE_MODE_INVALID",
            Self::BaselineBinding => "BASELINE_BINDING_INVALID",
            Self::EntityChain => "ENTITY_CHAIN_MISMATCH",
            Self::PlanBinding => "PLAN_BINDING_INVALID",
            Self::VersionNotHigher => "TARGET_VERSION_NOT_HIGHER",
            Self::EntityState => "ENTITY_STATE_INVALID",
            Self::RolloutBinding => "ROLLOUT_BINDING_INVALID",
            Self::BridgeBinding => "BRIDGE_BINDING_INVALID",
            Self::PathBinding => "PATH_BINDING_INVALID",
            Self::PathMissing => "PATH_BINDING_MISSING",
            Self::BackupPolicy => "BACKUP_POLICY_INVALID",
            Self::PackageBinding => "PACKAGE_BINDING_INVALID",
            Self::ReleaseBinding => "RELEASE_BINDING_INVALID",
            Self::ReferenceRole => "REFERENCE_ROLE_INVALID",
            Self::TargetBinding => "TARGET_BINDING_INVALID",
            Self::InstallWindowInvalid => "INSTALL_WINDOW_INVALID",
            Self::InstallWindowClosed => "INSTALL_WINDOW_CLOSED",
            Self::OpeningBinding => "OPENING_BINDING_INVALID",
            Self::LocatorWindow => "LOCATOR_WINDOW_INVALID",
            Self::AuthorizationId => "AUTHORIZATION_ID_INVALID",
            Self::FreezeBinding => "FREEZE_BINDING_INVALID",
            Self::BackupBinding => "BACKUP_BINDING_INVALID",
            Self::StagedBinding => "STAGED_BINDING_INVALID",
            Self::SlotBinding => "SLOT_BINDING_INVALID",
            Self::UpgradePurpose => "UPGRADE_PURPOSE_INVALID",
            Self::ActivationMode => "ACTIVATION_MODE_INVALID",
            Self::InstallNotYet => "INSTALL_NOT_YET_VALID",
            Self::StagedTransaction => "STAGED_TRANSACTION_REUSED",
            Self::StagedWindow => "STAGED_WINDOW_INVALID",
            Self::EventWindow => "EVENT_WINDOW_INVALID",
            Self::TaskBinding => "TASK_BINDING_INVALID",
            Self::TaskTarget => "TASK_TARGET_INVALID",
            Self::TaskProof => "TASK_PROOF_MISSING",
            Self::TaskWindow => "TASK_WINDOW_INVALID",
        })
    }
}
impl std::error::Error for CredentialSemanticsError {}
/// Full frozen Schema/encoding must precede these original provided-claim rules.
/// This is not a signature, trusted time/source/context, consume, state transition,
/// persistent acceptance or current execution-right certificate.
pub fn assert_supplied_credential_semantics(
    envelope: &SchemaCheckedEnvelope,
    time: &SuppliedProtocolTime,
) -> Result<(), CredentialSemanticsError> {
    let claims = envelope.envelope().signed().value();
    let role = text(&claims["role"])?;
    let now = time.as_millis();
    let lifetime = match role {
        "decision" | "download" => 900_000.0,
        "authorization-preinstall"
        | "authorization-install"
        | "authorization-activate"
        | "reconciliation-action"
        | "recovery-review-action" => 300_000.0,
        "helper-cleanup" => 120_000.0,
        "telemetry-event" | "reconciliation-event" | "recovery-review-event" => 7.0 * DAY,
        "preinstall-transaction-event"
        | "install-transaction-event"
        | "activate-transaction-event" => 37.0 * DAY,
        _ => return Err(CredentialSemanticsError::UnsupportedRole),
    };
    crate::metadata_semantics::references(claims).map_err(CredentialSemanticsError::References)?;
    let iat = number(&claims["iat"])?;
    let exp = number(&claims["exp"])?;
    let nbf = number(&claims["nbf"])?;
    rule(
        exp > iat && exp - iat <= lifetime,
        CredentialSemanticsError::WindowInvalid,
    )?;
    rule(
        now >= iat && now < exp,
        CredentialSemanticsError::WindowClosed,
    )?;
    rule(
        nbf >= iat && nbf < exp && now >= nbf,
        CredentialSemanticsError::WindowClosed,
    )?;
    if role == "decision" {
        let available = claims["updateAvailable"]
            .as_bool()
            .ok_or(CredentialSemanticsError::InvalidFields)?;
        rule(
            available != claims["update"].is_null() && available == claims["reason"].is_null(),
            CredentialSemanticsError::DecisionBinding,
        )?;
    }
    if let Some(set) = nonnull(claims, "metadataSet") {
        metadata_set(set, claims, now)?;
    }
    if let Some(update) = nonnull(claims, "update") {
        update::check(update, claims, now)?;
    }
    if role.starts_with("authorization-") {
        authorization::check(claims, now)?;
    }
    if role == "download" {
        rule(
            claims["packageId"] == claims["update"]["hopChain"]["packageId"]
                && same(&claims["package"], &claims["update"]["hopChain"]["package"])?,
            CredentialSemanticsError::PackageBinding,
        )?;
    }
    event_task::check(claims, role)
}
fn metadata_set(set: &Value, claims: &Value, now: f64) -> Result<(), CredentialSemanticsError> {
    let target = text(&claims["scope"]["targetKey"])?;
    let parts: Vec<_> = target.split('-').collect();
    let os = parts
        .first()
        .copied()
        .ok_or(CredentialSemanticsError::InvalidFields)?;
    let architectures: &[&str] = if os == "macos" {
        &["arm64", "x86_64"]
    } else {
        std::slice::from_ref(
            parts
                .get(1)
                .ok_or(CredentialSemanticsError::InvalidFields)?,
        )
    };
    rule(
        claims["host"]["os"] == os && architectures.contains(&text(&claims["host"]["arch"])?),
        CredentialSemanticsError::HostTarget,
    )?;
    for role in ["root", "timestamp", "snapshot", "target"] {
        rule(
            set[role]["role"] == role && now < number(&set[role]["expiresAt"])?,
            CredentialSemanticsError::MetadataSet,
        )?;
    }
    let scope = serde_json::json!({"channel":claims["scope"]["channel"],"targetKey":claims["scope"]["targetKey"]});
    rule(
        same(&set["target"]["scope"], &scope)?,
        CredentialSemanticsError::MetadataSet,
    )?;
    let releases = array(&set["releases"])?;
    for reference in releases {
        rule(
            reference["role"] == "release" && now < number(&reference["expiresAt"])?,
            CredentialSemanticsError::MetadataSet,
        )?;
    }
    if let Some(update) = nonnull(claims, "update") {
        for chain in [&update["endpointChain"], &update["hopChain"]] {
            let mut found = false;
            for reference in releases {
                if same(reference, &chain["releaseManifest"])? {
                    found = true;
                    break;
                }
            }
            rule(found, CredentialSemanticsError::MetadataIncomplete)?;
            let expiry = &chain["supplyChain"]["effectiveExpiresAt"];
            rule(
                expiry.is_null() || now < number(expiry)?,
                CredentialSemanticsError::SupplyExpired,
            )?;
        }
    }
    Ok(())
}
fn nonnull<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.get(name).filter(|v| !v.is_null())
}
fn rule(condition: bool, error: CredentialSemanticsError) -> Result<(), CredentialSemanticsError> {
    if condition { Ok(()) } else { Err(error) }
}
fn text(value: &Value) -> Result<&str, CredentialSemanticsError> {
    value
        .as_str()
        .ok_or(CredentialSemanticsError::InvalidFields)
}
fn number(value: &Value) -> Result<f64, CredentialSemanticsError> {
    value
        .as_f64()
        .ok_or(CredentialSemanticsError::InvalidFields)
}
fn array(value: &Value) -> Result<&[Value], CredentialSemanticsError> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or(CredentialSemanticsError::InvalidFields)
}
fn canonical(value: &Value) -> Result<Vec<u8>, CredentialSemanticsError> {
    let mut output = BoundedOutput {
        bytes: Vec::new(),
        exceeded: false,
    };
    if serde_jcs::to_writer(&mut output, value).is_err() {
        return Err(CredentialSemanticsError::Json(if output.exceeded {
            Error::OutputLimit
        } else {
            Error::Canonicalization
        }));
    }
    Ok(output.bytes)
}
fn same(left: &Value, right: &Value) -> Result<bool, CredentialSemanticsError> {
    Ok(canonical(left)? == canonical(right)?)
}
fn higher(target: &Value, source: &Value) -> Result<bool, CredentialSemanticsError> {
    // Both strings are already full-Schema/resource checked. This private
    // subtree construction introduces no public unbounded Document entrance.
    let target = Document {
        value: target.clone(),
    }
    .app_version_shape()
    .map_err(|_| CredentialSemanticsError::InvalidFields)?;
    let source = Document {
        value: source.clone(),
    }
    .app_version_shape()
    .map_err(|_| CredentialSemanticsError::InvalidFields)?;
    Ok(crate::compare_app_versions(&target, &source).is_gt())
}
