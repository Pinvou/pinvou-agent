//! Supplied public metadata graph relationships, never an authenticated graph.
use crate::{
    ContextBindingError, Document, EnvelopeShapeError, Error, MetadataReferenceError,
    MetadataReferenceShape, ReleasePackageError, SchemaCheckedEnvelope, SchemaError,
};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicGraphError {
    Json(Error),
    Envelope(EnvelopeShapeError),
    Schema(SchemaError),
    Context(ContextBindingError),
    Reference(MetadataReferenceError),
    Package(ReleasePackageError),
    InvalidContext,
    InvalidRole,
    InvalidFields,
    MemberDuplicate,
    MemberMissing,
    MemberUnreferenced,
    ReleaseMissing,
    TargetMismatch,
    TargetMissing,
    PackageMissing,
    CandidateLeak,
}
impl std::fmt::Display for PublicGraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => e.fmt(f),
            Self::Envelope(e) => e.fmt(f),
            Self::Schema(e) => e.fmt(f),
            Self::Context(e) => e.fmt(f),
            Self::Reference(e) => e.fmt(f),
            Self::Package(e) => e.fmt(f),
            Self::InvalidContext => f.write_str("PUBLIC_CONTEXT_INVALID"),
            Self::InvalidRole => f.write_str("PUBLIC_ROLE_INVALID"),
            Self::InvalidFields => f.write_str("PUBLIC_FIELDS_INVALID"),
            Self::MemberDuplicate => f.write_str("MEMBER_DUPLICATE"),
            Self::MemberMissing => f.write_str("PUBLIC_MEMBER_MISSING"),
            Self::MemberUnreferenced => f.write_str("PUBLIC_MEMBER_UNREFERENCED"),
            Self::ReleaseMissing => f.write_str("PUBLIC_RELEASE_MISSING"),
            Self::TargetMismatch => f.write_str("PUBLIC_TARGET_MISMATCH"),
            Self::TargetMissing => f.write_str("PUBLIC_TARGET_MISSING"),
            Self::PackageMissing => f.write_str("PUBLIC_PACKAGE_MISSING"),
            Self::CandidateLeak => f.write_str("PUBLIC_CANDIDATE_LEAK"),
        }
    }
}
impl std::error::Error for PublicGraphError {}
struct Member<'a> {
    envelope: &'a SchemaCheckedEnvelope,
    reference: MetadataReferenceShape,
    hash: String,
}
impl<'a> Member<'a> {
    fn new(envelope: &'a SchemaCheckedEnvelope) -> Result<Self, PublicGraphError> {
        // This is the immutable factory's own complete original raw bytes.
        // Derive once, keeping only the bounded six-field canonical reference.
        let reference = crate::derive_metadata_reference(envelope.envelope().raw_bytes())
            .map_err(PublicGraphError::Reference)?;
        let parsed = Document::parse(reference.canonical_bytes(), crate::InputKind::Metadata)
            .map_err(PublicGraphError::Json)?;
        let hash = parsed.value()["envelopeSha256"]
            .as_str()
            .ok_or(PublicGraphError::InvalidFields)?
            .to_owned();
        Ok(Self {
            envelope,
            reference,
            hash,
        })
    }
    fn signed(&self) -> &Value {
        self.envelope.envelope().signed().value()
    }
}
/// Exact public-graph supplied context/reference/reachability relationships.
/// Full Schema/encoding factories precede all inputs. Caller context supplies
/// product/component explicitly; extras remain raw-bounded but are ignored.
/// This does not verifyEnvelope, validate trusted time/Root/key policies, check
/// supplied metadata semantics, persist acceptance or select an execution object.
pub fn assert_supplied_public_graph(
    timestamp: &SchemaCheckedEnvelope,
    snapshot: &SchemaCheckedEnvelope,
    members: &[SchemaCheckedEnvelope],
    context: &Document,
) -> Result<(), PublicGraphError> {
    let context = context
        .value()
        .as_object()
        .ok_or(PublicGraphError::InvalidContext)?;
    let product = context
        .get("product")
        .filter(|v| crate::shape::identifier(v))
        .ok_or(PublicGraphError::InvalidContext)?;
    let component = context
        .get("component")
        .filter(|v| crate::shape::identifier(v))
        .ok_or(PublicGraphError::InvalidContext)?;
    if timestamp.envelope().signed().value()["role"] != "timestamp"
        || snapshot.envelope().signed().value()["role"] != "snapshot"
    {
        return Err(PublicGraphError::InvalidRole);
    }
    bind_context(timestamp, "timestamp", &json!({}), product, component)?;
    bind_context(snapshot, "snapshot", &json!({}), product, component)?;
    let snapshot_member = Member::new(snapshot)?;
    matches_reference(
        &timestamp.envelope().signed().value()["snapshot"],
        &snapshot_member,
    )?;
    let entries = snapshot.envelope().signed().value()["entries"]
        .as_array()
        .ok_or(PublicGraphError::InvalidFields)?;
    // Original available.size == publicObjects.size implies this necessary bound.
    // Do not introduce an independent aggregate-byte, duration or retry cap.
    if members.len() > entries.len() {
        return Err(PublicGraphError::MemberUnreferenced);
    }
    let indexed: Vec<_> = members.iter().map(Member::new).collect::<Result<_, _>>()?;
    let mut available = HashMap::new();
    for (index, member) in indexed.iter().enumerate() {
        if available.insert(member.hash.as_str(), index).is_some() {
            return Err(PublicGraphError::MemberDuplicate);
        }
    }
    let mut public = BTreeSet::new();
    for reference in entries {
        let hash = reference["envelopeSha256"]
            .as_str()
            .ok_or(PublicGraphError::InvalidFields)?;
        let index = *available.get(hash).ok_or(PublicGraphError::MemberMissing)?;
        let member = &indexed[index];
        matches_reference(reference, member)?;
        let role = reference["role"]
            .as_str()
            .ok_or(PublicGraphError::InvalidFields)?;
        bind_context(
            member.envelope,
            role,
            &reference["scope"],
            product,
            component,
        )?;
        // Repeated identical references retain the original Map overwrite effect.
        public.insert(index);
    }
    if available.len() != public.len() {
        return Err(PublicGraphError::MemberUnreferenced);
    }
    let mut permitted = BTreeSet::new();
    let mut releases = BTreeSet::new();
    let mut target_count = 0;
    for &index in &public {
        let target = indexed[index].signed();
        if target["role"] != "target" {
            continue;
        }
        target_count += 1;
        permitted.insert(index);
        let ordinary = target["ordinaryPaths"]
            .as_array()
            .ok_or(PublicGraphError::InvalidFields)?;
        let bridge = target["bridgePaths"]
            .as_array()
            .ok_or(PublicGraphError::InvalidFields)?;
        let references = std::iter::once(&target["baselineRelease"])
            .chain(ordinary.iter().map(|p| &p["release"]))
            .chain(bridge.iter().map(|p| &p["release"]));
        for reference in references {
            let release_index =
                lookup(reference, &available).ok_or(PublicGraphError::ReleaseMissing)?;
            let release = &indexed[release_index];
            if !public.contains(&release_index) || release.signed()["role"] != "release" {
                return Err(PublicGraphError::ReleaseMissing);
            }
            let targets = release.signed()["targets"]
                .as_array()
                .ok_or(PublicGraphError::InvalidFields)?;
            if !targets
                .iter()
                .any(|entry| entry["targetKey"] == target["scope"]["targetKey"])
            {
                return Err(PublicGraphError::TargetMismatch);
            }
            matches_reference(reference, release)?;
            permitted.insert(release_index);
            releases.insert(release_index);
        }
    }
    if target_count == 0 {
        return Err(PublicGraphError::TargetMissing);
    }
    // Repeated reachability of an immutable Release repeats the same pure checks
    // in the original. Check each unique index once instead of an unbounded Vec.
    for index in releases {
        let release = indexed[index].signed();
        let targets = release["targets"]
            .as_array()
            .ok_or(PublicGraphError::InvalidFields)?;
        for target in targets {
            let reference = &target["packageManifest"];
            let package_index =
                lookup(reference, &available).ok_or(PublicGraphError::PackageMissing)?;
            let package = &indexed[package_index];
            if !public.contains(&package_index) || package.signed()["role"] != "package" {
                return Err(PublicGraphError::PackageMissing);
            }
            matches_reference(reference, package)?;
            crate::release_package::assert_target_binding(release, target, package.signed())
                .map_err(PublicGraphError::Package)?;
            permitted.insert(package_index);
        }
    }
    if public.iter().any(|index| !permitted.contains(index)) {
        return Err(PublicGraphError::CandidateLeak);
    }
    Ok(())
}
fn lookup(reference: &Value, available: &HashMap<&str, usize>) -> Option<usize> {
    reference["envelopeSha256"]
        .as_str()
        .and_then(|hash| available.get(hash))
        .copied()
}
fn matches_reference(reference: &Value, member: &Member<'_>) -> Result<(), PublicGraphError> {
    // A small closed full-Schema-checked reference projection only.
    let declared = Document {
        value: reference.clone(),
    }
    .metadata_reference_shape()
    .map_err(PublicGraphError::Reference)?;
    if declared.canonical_bytes() != member.reference.canonical_bytes() {
        return Err(PublicGraphError::Reference(
            MetadataReferenceError::Mismatch,
        ));
    }
    Ok(())
}
fn bind_context(
    envelope: &SchemaCheckedEnvelope,
    role: &str,
    scope: &Value,
    product: &Value,
    component: &Value,
) -> Result<(), PublicGraphError> {
    let expected = Document {
        value: json!({"role":role,"product":product,"component":component,"scope":scope}),
    };
    crate::assert_supplied_expected_context(envelope, &expected).map_err(PublicGraphError::Context)
}
