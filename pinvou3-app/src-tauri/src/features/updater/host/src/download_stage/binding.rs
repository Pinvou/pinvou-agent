//! Original compiled anchor and Package byte binding; no current-time owner.
use super::DownloadedPackageSource;
use crate::{
    error::{Error, Result},
    profile::{ByteReference, Seed},
};
use pinvou_upgrade_codec::{Document, InputKind, decode_canonical_envelope};
use pinvou_upgrade_verifier::{
    VerificationInputs, verify_archived_package, verify_with_supplied_root,
};

fn document(bytes: &[u8]) -> Result<Document> {
    Document::parse(bytes, InputKind::Metadata).map_err(|_| Error::ImageRefused)
}

pub(crate) struct BoundPackage {
    pub(crate) document: Document,
    pub(crate) expected: Document,
    pub(crate) envelope_sha256: String,
    pub(crate) root_claims_sha256: String,
}

pub(crate) fn bind_package(
    source: &DownloadedPackageSource<'_>,
    seed: &Seed,
) -> Result<BoundPackage> {
    // Expectations belong to the compiled host, never the ordinary cache.
    let expected = document(
        &serde_json::to_vec(&serde_json::json!({
            "role": "package", "product": seed.product, "component": seed.component,
            "scope": {"targetKey": seed.target_key}
        }))
        .map_err(|_| Error::ImageRefused)?,
    )?;
    let bytes = expected
        .canonical_bytes()
        .map_err(|_| Error::ImageRefused)?;
    let checked = verify_bound_package(
        HostAnchor {
            bytes: seed.original_root_bytes(),
            reference: &seed.original_root,
            product: &seed.product,
            component: &seed.component,
            target: &seed.target_key,
        },
        PackageInput {
            root_claims_pin: source.root_claims_pin,
            envelope: source.envelope,
            expected: &bytes,
            target: &seed.target_key,
            historical_time: source.historical_time,
        },
    )?;
    Ok(BoundPackage {
        document: checked,
        expected,
        envelope_sha256: crate::json::sha256(source.envelope),
        root_claims_sha256: source.root_claims_pin.to_owned(),
    })
}

// Private decomposition makes the real signature boundary testable. Runtime
// callers can only enter through bind_package and the compiled Seed above.
struct HostAnchor<'a> {
    bytes: &'a [u8],
    reference: &'a ByteReference,
    product: &'a str,
    component: &'a str,
    target: &'a str,
}
struct PackageInput<'a> {
    root_claims_pin: &'a str,
    envelope: &'a [u8],
    expected: &'a [u8],
    target: &'a str,
    historical_time: u64,
}

fn verify_bound_package(host: HostAnchor<'_>, source: PackageInput<'_>) -> Result<Document> {
    if !host.reference.matches(host.bytes) {
        return Err(Error::InitialTrustUnavailable);
    }
    let original = decode_canonical_envelope(host.bytes, InputKind::Metadata)
        .and_then(|envelope| {
            envelope
                .check_full_schema()
                .map_err(|_| pinvou_upgrade_codec::EnvelopeShapeError::InvalidEnvelope)
        })
        .map_err(|_| Error::InitialTrustUnavailable)?;
    let root = original
        .envelope()
        .signed()
        .root_policy_shape()
        .map_err(|_| Error::InitialTrustUnavailable)?;
    // The job pins canonical Root CLAIMS, while Seed pins the original envelope.
    // These are intentionally different pins; never compare their raw digests.
    if source.root_claims_pin != crate::json::sha256(root.canonical_bytes()) {
        return Err(Error::InitialTrustUnavailable);
    }
    let expected_root = document(
        &serde_json::to_vec(&serde_json::json!({
            "role": "root", "product": host.product, "component": null, "scope": {}
        }))
        .map_err(|_| Error::InitialTrustUnavailable)?,
    )?;
    let issued = document(
        original.envelope().signed().value()["issuedAt"]
            .to_string()
            .as_bytes(),
    )?
    .protocol_time_shape()
    .map_err(|_| Error::InitialTrustUnavailable)?;
    // Same deterministic self-threshold integrity basis as the bootstrap owner;
    // signed issuedAt is never promoted to an observed trusted current clock.
    verify_with_supplied_root(
        host.bytes,
        VerificationInputs::new(&root, &expected_root, &issued),
    )
    .map_err(|_| Error::InitialTrustUnavailable)?;

    let expected = document(source.expected)?;
    require_host_context(expected.value(), host.product, host.component, host.target)?;
    if source.target != host.target {
        return Err(Error::ImageRefused);
    }
    let time = document(source.historical_time.to_string().as_bytes())?
        .protocol_time_shape()
        .map_err(|_| Error::ImageRefused)?;
    verify_archived_package(source.envelope, &root, &expected, &time)
        .map_err(|_| Error::ImageRefused)?;
    document(source.envelope)
}

fn require_host_context(
    expected: &serde_json::Value,
    product: &str,
    component: &str,
    target: &str,
) -> Result<()> {
    if expected["role"] != "package"
        || expected["product"] != product
        || expected["component"] != component
        || expected["scope"]["targetKey"] != target
    {
        return Err(Error::ImageRefused);
    }
    // The original verifier binds the COMPLETE retained scope. It remains signed
    // historical context, not an assertion that this installation is eligible.
    Ok(())
}

/// The signed native identity has no source path. Infer only a unique regular
/// member with exact declared bytes; aliases/duplicate matches refuse.
pub(crate) fn unique_member<'a>(
    entries: impl IntoIterator<Item = (&'a str, u64, [u8; 32])>,
    size: u64,
    sha256: [u8; 32],
) -> Result<String> {
    let mut found = None;
    for (name, actual_size, actual_hash) in entries {
        if actual_size == size && actual_hash == sha256 {
            if found.is_some() {
                return Err(Error::ImageRefused);
            }
            found = Some(name.to_owned());
        }
    }
    found.ok_or(Error::ImageRefused)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_context_cannot_select_another_host_or_target() {
        let expected = serde_json::json!({"role":"package","product":"pinvou","component":"agent",
            "scope":{"targetKey":"windows-x86_64-nsis-perMachine","channel":"stable"}});
        assert!(
            require_host_context(
                &expected,
                "pinvou",
                "agent",
                "windows-x86_64-nsis-perMachine"
            )
            .is_ok()
        );
        for (product, component, target) in [
            ("other", "agent", "windows-x86_64-nsis-perMachine"),
            ("pinvou", "other", "windows-x86_64-nsis-perMachine"),
            ("pinvou", "agent", "linux-x86_64-deb-perMachine"),
        ] {
            assert!(require_host_context(&expected, product, component, target).is_err());
        }
        assert!(
            require_host_context(
                &serde_json::json!({}),
                "pinvou",
                "agent",
                "windows-x86_64-nsis-perMachine"
            )
            .is_err()
        );
    }
    #[test]
    fn native_locator_requires_exact_bytes_and_one_member() {
        let hash = [7; 32];
        assert_eq!(
            unique_member([("app/installer.exe", 8, hash)], 8, hash).unwrap(),
            "app/installer.exe"
        );
        assert!(unique_member([("a.exe", 8, hash), ("b.exe", 8, hash)], 8, hash).is_err());
        assert!(unique_member([("a.exe", 9, hash)], 8, hash).is_err());
        assert!(unique_member([("a.exe", 8, [8; 32])], 8, hash).is_err());
    }
}

#[cfg(test)]
mod signature_tests;
