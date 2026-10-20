//! Supplied public metadata graph relationships, never an authenticated graph.
use crate::{
    ContextBindingError, Document, EnvelopeShapeError, Error, MetadataReferenceError,
    ReleasePackageError, SchemaCheckedEnvelope, SchemaError,
};
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
/// Exact original graph kernel with an in-memory source. The additional source
/// entry preserves these relationships while permitting bounded database reads.
pub fn assert_supplied_public_graph(
    timestamp: &SchemaCheckedEnvelope,
    snapshot: &SchemaCheckedEnvelope,
    members: &[SchemaCheckedEnvelope],
    context: &Document,
) -> Result<(), PublicGraphError> {
    let count = crate::public_graph_source::check_prefix(timestamp, snapshot, context)?;
    if members.len() > count {
        return Err(PublicGraphError::MemberUnreferenced);
    }
    let references = members
        .iter()
        .map(|member| {
            crate::derive_metadata_reference(member.envelope().raw_bytes())
                .map_err(PublicGraphError::Reference)
        })
        .collect::<Result<Vec<_>, _>>()?;
    crate::assert_supplied_public_graph_source(timestamp, snapshot, &references, context, |index| {
        crate::decode_canonical_envelope(
            members[index].envelope().raw_bytes(),
            crate::InputKind::Metadata,
        )
        .map_err(PublicGraphError::Envelope)?
        .check_full_schema()
        .map_err(PublicGraphError::Schema)
    })
}
