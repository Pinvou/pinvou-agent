//! Complete six-role metadata semantics at supplied time, never trusted acceptance.
use crate::{
    BoundedOutput, Document, EnvelopeShapeError, Error, SchemaCheckedEnvelope, SchemaError,
};
use serde_json::Value;
use std::collections::BTreeSet;
const DAY: f64 = 86_400_000.0;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RootWindowMode {
    #[default]
    Current,
    AllowExpired,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataSemanticsError {
    Json(Error),
    Envelope(EnvelopeShapeError),
    Schema(SchemaError),
    InvalidTime,
    UnsupportedRole,
    InvalidClaims,
    InvalidWindow,
    ClosedWindow,
    InvalidReference,
    ReferenceRole,
    DuplicateReference,
    HostArchitectures,
    DuplicateTarget,
    TargetBinding,
    BackupPolicy,
    DuplicateHelper,
    HelperTarget,
    HelperBinding,
    SourceRevision,
    IncrementalProduction,
    ProvenanceBinding,
    NativeFormat,
}
impl std::fmt::Display for MetadataSemanticsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Json(error) => return error.fmt(f),
            Self::Envelope(error) => return error.fmt(f),
            Self::Schema(error) => return error.fmt(f),
            Self::InvalidTime => "SUPPLIED_PROTOCOL_TIME_INVALID",
            Self::UnsupportedRole => "METADATA_SEMANTICS_ROLE_INVALID",
            Self::InvalidClaims => "METADATA_SEMANTICS_FIELDS_INVALID",
            Self::InvalidWindow => "TIME_WINDOW_INVALID",
            Self::ClosedWindow => "TIME_WINDOW_CLOSED",
            Self::InvalidReference => "REFERENCE_SCOPE_INVALID",
            Self::ReferenceRole => "REFERENCE_ROLE_INVALID",
            Self::DuplicateReference => "REFERENCE_DUPLICATE",
            Self::HostArchitectures => "HOST_ARCH_INVALID",
            Self::DuplicateTarget => "TARGET_DUPLICATE",
            Self::TargetBinding => "TARGET_BINDING_INVALID",
            Self::BackupPolicy => "BACKUP_POLICY_INVALID",
            Self::DuplicateHelper => "HELPER_DUPLICATE",
            Self::HelperTarget => "HELPER_TARGET_INVALID",
            Self::HelperBinding => "HELPER_BINDING_INVALID",
            Self::SourceRevision => "SOURCE_REVISION_INVALID",
            Self::IncrementalProduction => "INCREMENTAL_PRODUCTION_DISABLED",
            Self::ProvenanceBinding => "PROVENANCE_BINDING_INVALID",
            Self::NativeFormat => "NATIVE_FORMAT_INVALID",
        })
    }
}
impl std::error::Error for MetadataSemanticsError {}
/// Safe supplied millisecond value, not an attested trusted clock observation.
pub struct SuppliedProtocolTime {
    millis: f64,
}
impl SuppliedProtocolTime {
    pub fn as_millis(&self) -> f64 {
        self.millis
    }
}
impl Document {
    pub fn protocol_time_shape(&self) -> Result<SuppliedProtocolTime, MetadataSemanticsError> {
        let millis = self
            .value()
            .as_f64()
            .filter(|n| {
                n.is_finite() && (0.0..=9_007_199_254_740_991.0).contains(n) && n.fract() == 0.0
            })
            .ok_or(MetadataSemanticsError::InvalidTime)?;
        Ok(SuppliedProtocolTime { millis })
    }
}
/// Six metadata roles only. Complete Schema and canonical encoding are prechecked;
/// signature/key/Root consistency, trusted now, lineage, persistence and current
/// rights are not proved. Package always uses production incremental restrictions.
/// AllowExpired affects Root only and does not prove continuous Root traversal.
pub fn assert_supplied_metadata_semantics(
    envelope: &SchemaCheckedEnvelope,
    now: &SuppliedProtocolTime,
    root_mode: RootWindowMode,
) -> Result<(), MetadataSemanticsError> {
    let claims = envelope.envelope().signed().value();
    let role = text(&claims["role"])?;
    let maximum = match role {
        "root" => 365.0 * DAY,
        "timestamp" => DAY,
        "snapshot" => 7.0 * DAY,
        "target" => 30.0 * DAY,
        "release" => 90.0 * DAY,
        "package" => 0.0,
        _ => return Err(MetadataSemanticsError::UnsupportedRole),
    };
    references(claims)?;
    if role != "package" {
        let start = number(&claims["issuedAt"])?;
        let end = number(&claims["expiresAt"])?;
        ensure(
            end > start && end - start <= maximum,
            MetadataSemanticsError::InvalidWindow,
        )?;
        if role == "root" && root_mode == RootWindowMode::AllowExpired {
            ensure(now.millis >= start, MetadataSemanticsError::InvalidWindow)?;
        } else {
            ensure(
                now.millis >= start && now.millis < end,
                MetadataSemanticsError::ClosedWindow,
            )?;
        }
    }
    match role {
        "timestamp" => ensure(
            claims["snapshot"]["role"] == "snapshot",
            MetadataSemanticsError::ReferenceRole,
        ),
        "snapshot" => snapshot(claims),
        "target" => target(claims),
        "release" => release(claims),
        "package" => package(claims),
        _ => Ok(()),
    }
}
fn ensure(condition: bool, error: MetadataSemanticsError) -> Result<(), MetadataSemanticsError> {
    if condition { Ok(()) } else { Err(error) }
}
fn text(value: &Value) -> Result<&str, MetadataSemanticsError> {
    value.as_str().ok_or(MetadataSemanticsError::InvalidClaims)
}
fn number(value: &Value) -> Result<f64, MetadataSemanticsError> {
    value.as_f64().ok_or(MetadataSemanticsError::InvalidClaims)
}
fn array(value: &Value) -> Result<&[Value], MetadataSemanticsError> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or(MetadataSemanticsError::InvalidClaims)
}
fn canonical(value: &Value) -> Result<Vec<u8>, MetadataSemanticsError> {
    let mut output = BoundedOutput {
        bytes: Vec::new(),
        exceeded: false,
    };
    if serde_jcs::to_writer(&mut output, value).is_err() {
        return Err(MetadataSemanticsError::Json(if output.exceeded {
            Error::OutputLimit
        } else {
            Error::Canonicalization
        }));
    }
    Ok(output.bytes)
}
fn references(value: &Value) -> Result<(), MetadataSemanticsError> {
    match value {
        Value::Object(object) => {
            if object.contains_key("envelopeSha256") {
                let role = text(&value["role"])?;
                let fields: &[&str] = match role {
                    "target" => &["channel", "targetKey"],
                    "release" => &["releaseId"],
                    "package" => &["targetKey"],
                    _ => &[],
                };
                ensure(
                    crate::shape::closed(&value["scope"], fields).is_some(),
                    MetadataSemanticsError::InvalidReference,
                )?;
                if role == "package" {
                    ensure(
                        value["version"].is_null() && value["expiresAt"].is_null(),
                        MetadataSemanticsError::InvalidReference,
                    )?;
                } else {
                    ensure(
                        crate::shape::positive_safe_integer(&value["version"])
                            && value["expiresAt"].as_f64().is_some_and(|n| {
                                n.is_finite()
                                    && n.fract() == 0.0
                                    && n.abs() <= 9_007_199_254_740_991.0
                            }),
                        MetadataSemanticsError::InvalidReference,
                    )?;
                }
            }
            for child in object.values() {
                references(child)?;
            }
        }
        Value::Array(items) => {
            for child in items {
                references(child)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn snapshot(claims: &Value) -> Result<(), MetadataSemanticsError> {
    let mut identities = BTreeSet::new();
    for entry in array(&claims["entries"])? {
        let role = text(&entry["role"])?;
        ensure(
            ["target", "release", "package"].contains(&role),
            MetadataSemanticsError::ReferenceRole,
        )?;
        let mut identity = role.as_bytes().to_vec();
        identity.push(b':');
        if role == "package" {
            identity.extend_from_slice(text(&entry["envelopeSha256"])?.as_bytes());
        } else {
            identity.extend_from_slice(&canonical(&entry["scope"])?);
        }
        ensure(
            identities.insert(identity),
            MetadataSemanticsError::DuplicateReference,
        )?;
    }
    Ok(())
}
fn target(claims: &Value) -> Result<(), MetadataSemanticsError> {
    ensure(
        claims["baselineRelease"]["role"] == "release",
        MetadataSemanticsError::ReferenceRole,
    )?;
    for field in ["ordinaryPaths", "bridgePaths"] {
        for path in array(&claims[field])? {
            ensure(
                path["release"]["role"] == "release",
                MetadataSemanticsError::ReferenceRole,
            )?;
        }
    }
    let target = text(&claims["scope"]["targetKey"])?;
    let expected: &[&str] = if target.starts_with("macos-") {
        &["arm64", "x86_64"]
    } else if target.contains("-arm64-") {
        &["arm64"]
    } else {
        &["x86_64"]
    };
    let mut actual: Vec<&str> = array(&claims["hostArchitectures"])?
        .iter()
        .map(text)
        .collect::<Result<_, _>>()?;
    actual.sort_unstable();
    ensure(
        actual == expected,
        MetadataSemanticsError::HostArchitectures,
    )
}
fn release(claims: &Value) -> Result<(), MetadataSemanticsError> {
    let mut targets = BTreeSet::new();
    for target in array(&claims["targets"])? {
        ensure(
            targets.insert(text(&target["targetKey"])?),
            MetadataSemanticsError::DuplicateTarget,
        )?;
        ensure(
            target["packageManifest"]["role"] == "package"
                && target["packageManifest"]["scope"]["targetKey"] == target["targetKey"],
            MetadataSemanticsError::TargetBinding,
        )?;
        ensure(
            target["migrationMode"] != "irreversible" || target["backupPolicy"] == "required",
            MetadataSemanticsError::BackupPolicy,
        )?;
    }
    Ok(())
}
fn package(claims: &Value) -> Result<(), MetadataSemanticsError> {
    let mut helpers = BTreeSet::new();
    let mut selected = None;
    for helper in array(&claims["helpers"])? {
        ensure(
            helpers.insert(text(&helper["helperId"])?),
            MetadataSemanticsError::DuplicateHelper,
        )?;
        ensure(
            helper["targetKey"] == claims["scope"]["targetKey"]
                && helper["nativeIdentity"]["format"] == "helper",
            MetadataSemanticsError::HelperTarget,
        )?;
        if helper["helperId"] == claims["executionHelperId"] {
            selected = Some(&helper["nativeIdentity"]);
        }
    }
    let selected = selected.ok_or(MetadataSemanticsError::HelperBinding)?;
    ensure(
        canonical(selected)? == canonical(&claims["helper"])?,
        MetadataSemanticsError::HelperBinding,
    )?;
    let revision = &claims["provenance"]["sourceRevision"];
    ensure(
        text(&revision["value"])?.len()
            == if revision["algorithm"] == "git-sha1" {
                40
            } else {
                64
            },
        MetadataSemanticsError::SourceRevision,
    )?;
    ensure(
        array(&claims["incrementalPackages"])?.is_empty(),
        MetadataSemanticsError::IncrementalProduction,
    )?;
    ensure(
        claims["provenance"]["finalInstallerSha256"] == claims["finalInstaller"]["sha256"],
        MetadataSemanticsError::ProvenanceBinding,
    )?;
    let target = text(&claims["scope"]["targetKey"])?;
    let format = if target.starts_with("windows-") {
        "nsis"
    } else if target.starts_with("macos-") {
        "dmg"
    } else {
        "deb"
    };
    ensure(
        claims["finalInstaller"]["format"] == format
            && claims["helper"]["format"] == "helper"
            && claims["launcher"]["format"] == "launcher",
        MetadataSemanticsError::NativeFormat,
    )
}
