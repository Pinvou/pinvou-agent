//! Shared graph kernel. A source returns actual full-Schema objects, never a
//! Boolean approval. Each loaded object's complete reference/context is rechecked.
use crate::{
    Document, InputKind, MetadataReferenceError, MetadataReferenceShape, PublicGraphError as E,
    SchemaCheckedEnvelope,
};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};

pub(crate) fn check_prefix(
    timestamp: &SchemaCheckedEnvelope,
    snapshot: &SchemaCheckedEnvelope,
    context: &Document,
) -> Result<usize, E> {
    let ctx = context.value().as_object().ok_or(E::InvalidContext)?;
    let product = ctx
        .get("product")
        .filter(|v| crate::shape::identifier(v))
        .ok_or(E::InvalidContext)?;
    let component = ctx
        .get("component")
        .filter(|v| crate::shape::identifier(v))
        .ok_or(E::InvalidContext)?;
    if timestamp.envelope().signed().value()["role"] != "timestamp"
        || snapshot.envelope().signed().value()["role"] != "snapshot"
    {
        return Err(E::InvalidRole);
    }
    for (object, role) in [(timestamp, "timestamp"), (snapshot, "snapshot")] {
        let expected = Document {
            value: json!({"role":role,"product":product,"component":component,"scope":{}}),
        };
        crate::assert_supplied_expected_context(object, &expected).map_err(E::Context)?;
    }
    reference(&timestamp.envelope().signed().value()["snapshot"])?
        .assert_original_envelope(snapshot.envelope().raw_bytes())
        .map_err(E::Reference)?;
    Ok(snapshot.envelope().signed().value()["entries"]
        .as_array()
        .ok_or(E::InvalidFields)?
        .len())
}

/// Bounded per-Snapshot reference inventory with on-demand original objects.
/// This proves relationships only, not signatures, custody or persistent acceptance.
pub fn assert_supplied_public_graph_source(
    timestamp: &SchemaCheckedEnvelope,
    snapshot: &SchemaCheckedEnvelope,
    references: &[MetadataReferenceShape],
    context: &Document,
    mut read: impl FnMut(usize) -> Result<SchemaCheckedEnvelope, E>,
) -> Result<(), E> {
    check_prefix(timestamp, snapshot, context)?;
    let ctx = context.value().as_object().ok_or(E::InvalidContext)?;
    let product = ctx
        .get("product")
        .filter(|v| crate::shape::identifier(v))
        .ok_or(E::InvalidContext)?;
    let component = ctx
        .get("component")
        .filter(|v| crate::shape::identifier(v))
        .ok_or(E::InvalidContext)?;
    if timestamp.envelope().signed().value()["role"] != "timestamp"
        || snapshot.envelope().signed().value()["role"] != "snapshot"
    {
        return Err(E::InvalidRole);
    }
    let bind = |object: &SchemaCheckedEnvelope, role: &str, scope: &Value| {
        let expected = Document {
            value: json!({"role":role,"product":product,"component":component,"scope":scope}),
        };
        crate::assert_supplied_expected_context(object, &expected).map_err(E::Context)
    };
    bind(timestamp, "timestamp", &json!({}))?;
    bind(snapshot, "snapshot", &json!({}))?;
    reference(&timestamp.envelope().signed().value()["snapshot"])?
        .assert_original_envelope(snapshot.envelope().raw_bytes())
        .map_err(E::Reference)?;
    let entries = snapshot.envelope().signed().value()["entries"]
        .as_array()
        .ok_or(E::InvalidFields)?;
    if references.len() > entries.len() {
        return Err(E::MemberUnreferenced);
    }
    let mut available = HashMap::new();
    let mut declared = Vec::with_capacity(references.len());
    for (index, item) in references.iter().enumerate() {
        let doc = Document::parse(item.canonical_bytes(), InputKind::Metadata).map_err(E::Json)?;
        let hash = doc.value()["envelopeSha256"]
            .as_str()
            .ok_or(E::InvalidFields)?
            .to_owned();
        if available.insert(hash, index).is_some() {
            return Err(E::MemberDuplicate);
        }
        declared.push(doc);
    }
    let mut public = BTreeSet::new();
    for item in entries {
        let index = lookup(item, &available).ok_or(E::MemberMissing)?;
        if reference(item)?.canonical_bytes() != references[index].canonical_bytes() {
            return Err(E::Reference(MetadataReferenceError::Mismatch));
        }
        public.insert(index);
    }
    if available.len() != public.len() {
        return Err(E::MemberUnreferenced);
    }
    let mut load = |index: usize| -> Result<SchemaCheckedEnvelope, E> {
        let object = read(index)?;
        references[index]
            .assert_original_envelope(object.envelope().raw_bytes())
            .map_err(E::Reference)?;
        let fields = declared[index].value();
        bind(
            &object,
            fields["role"].as_str().ok_or(E::InvalidFields)?,
            &fields["scope"],
        )?;
        Ok(object)
    };
    // Authenticate every supplied member, including one that is ultimately
    // rejected as unreachable. Source failure never yields a partial graph.
    for &index in &public {
        load(index)?;
    }
    let mut permitted = BTreeSet::new();
    let mut releases = BTreeSet::new();
    let mut targets = 0;
    for &index in &public {
        if declared[index].value()["role"] != "target" {
            continue;
        }
        let object = load(index)?;
        let target = object.envelope().signed().value();
        targets += 1;
        permitted.insert(index);
        let ordinary = target["ordinaryPaths"].as_array().ok_or(E::InvalidFields)?;
        let bridge = target["bridgePaths"].as_array().ok_or(E::InvalidFields)?;
        for item in std::iter::once(&target["baselineRelease"])
            .chain(ordinary.iter().map(|p| &p["release"]))
            .chain(bridge.iter().map(|p| &p["release"]))
        {
            let ri = lookup(item, &available).ok_or(E::ReleaseMissing)?;
            if !public.contains(&ri) || declared[ri].value()["role"] != "release" {
                return Err(E::ReleaseMissing);
            }
            let release = load(ri)?;
            let signed = release.envelope().signed().value();
            if !signed["targets"]
                .as_array()
                .ok_or(E::InvalidFields)?
                .iter()
                .any(|t| t["targetKey"] == target["scope"]["targetKey"])
            {
                return Err(E::TargetMismatch);
            }
            reference(item)?
                .assert_original_envelope(release.envelope().raw_bytes())
                .map_err(E::Reference)?;
            permitted.insert(ri);
            releases.insert(ri);
        }
    }
    if targets == 0 {
        return Err(E::TargetMissing);
    }
    for index in releases {
        let object = load(index)?;
        let release = object.envelope().signed().value();
        for target in release["targets"].as_array().ok_or(E::InvalidFields)? {
            let item = &target["packageManifest"];
            let pi = lookup(item, &available).ok_or(E::PackageMissing)?;
            if !public.contains(&pi) || declared[pi].value()["role"] != "package" {
                return Err(E::PackageMissing);
            }
            let package = load(pi)?;
            reference(item)?
                .assert_original_envelope(package.envelope().raw_bytes())
                .map_err(E::Reference)?;
            crate::release_package::assert_target_binding(
                release,
                target,
                package.envelope().signed().value(),
            )
            .map_err(E::Package)?;
            permitted.insert(pi);
        }
    }
    if public.iter().any(|index| !permitted.contains(index)) {
        return Err(E::CandidateLeak);
    }
    Ok(())
}
fn reference(value: &Value) -> Result<MetadataReferenceShape, E> {
    Document {
        value: value.clone(),
    }
    .metadata_reference_shape()
    .map_err(E::Reference)
}
fn lookup(value: &Value, available: &HashMap<String, usize>) -> Option<usize> {
    value["envelopeSha256"]
        .as_str()
        .and_then(|hash| available.get(hash))
        .copied()
}
