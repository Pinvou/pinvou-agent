use super::*;
use pinvou_upgrade_codec::{Error, VersionError, derive_metadata_reference};
use serde_json::{Value, json};
use std::{
    io::{Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

struct Fixtures {
    base: PathBuf,
    root: PathBuf,
    cases: Vec<Value>,
}
impl Fixtures {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().canonicalize().unwrap();
        let stem = format!(
            "pinvou-package-proof-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let root = base.join(stem);
        std::fs::create_dir(&root).unwrap();
        assert_eq!(root.canonicalize().unwrap().parent(), Some(base.as_path()));
        let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo = crate_root.ancestors().nth(7).unwrap();
        let output = Command::new("python")
            .arg(crate_root.join("tests/generate_vectors.py"))
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "actual ZIP fixture generation failed"
        );
        let output = Command::new("node")
            .arg(crate_root.join("tests/package-oracle.mjs"))
            .arg(repo.join("specs/upgrade/v1/reference/canonical-json.mjs"))
            .arg(root.join("physical-vectors.json"))
            .arg(root.join("signed-vectors.json"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "actual original Package fixture preflight failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cases =
            serde_json::from_slice(&std::fs::read(root.join("signed-vectors.json")).unwrap())
                .unwrap();
        Self { base, root, cases }
    }
    fn baseline(&self) -> &Value {
        &self.cases[0]
    }
}
impl Drop for Fixtures {
    fn drop(&mut self) {
        // Only the exact created synthetic directory, still directly under the
        // resolved temp base, is eligible. Never traverse a replaced link/root.
        if let Ok(resolved) = self.root.canonicalize()
            && resolved == self.root
            && resolved.parent() == Some(self.base.as_path())
            && self
                .root
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("pinvou-package-proof-")
            && std::fs::symlink_metadata(&self.root).is_ok_and(|m| !m.file_type().is_symlink())
        {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}
fn doc(value: &Value) -> Document {
    Document::parse(&serde_json::to_vec(value).unwrap(), InputKind::Metadata).unwrap()
}
fn canonical(value: &Value) -> Vec<u8> {
    doc(value).canonical_bytes().unwrap()
}
fn run<'a>(
    source: &mut File,
    case: &Value,
    artifact: Artifact<'a>,
    cancelled: impl FnMut() -> bool,
) -> Result<PackageInventory<'a>, PackageCheckError> {
    let reference = doc(&case["reference"]).metadata_reference_shape().unwrap();
    let context = doc(&case["expectedContext"]);
    let version = doc(&case["expectedVersion"]).app_version_shape().unwrap();
    let time = doc(&case["time"]).protocol_time_shape().unwrap();
    inspect_full_pack(
        source,
        PackageInspection {
            artifact,
            manifest_bytes: case["manifest"].as_str().unwrap().as_bytes(),
            reference: &reference,
            expected_context: &context,
            expected_version: &version,
            supplied_time: &time,
            limits: ArchiveLimits::new(
                64 * 1024 * 1024,
                128 * 1024 * 1024,
                4096,
                8 * 1024 * 1024,
                8 * 1024 * 1024,
                100_000,
            )
            .unwrap(),
        },
        cancelled,
    )
}
fn artifact(case: &Value) -> Artifact<'static> {
    Artifact::new(
        "package-2",
        case["artifact"]["size"].as_u64().unwrap(),
        case["artifact"]["sha256"].as_str().unwrap(),
    )
    .unwrap()
}
#[test]
fn actual_store_deflate_large_alias_and_missing_native_bytes_are_fully_bound() {
    let fixtures = Fixtures::new();
    assert_eq!(fixtures.cases.len(), 14);
    for case in &fixtures.cases {
        let mut source = File::open(case["path"].as_str().unwrap()).unwrap();
        let result = run(&mut source, case, artifact(case), || false);
        if case["expected"] == "positive" {
            let inventory = result.unwrap();
            assert_eq!(inventory.target().size(), artifact(case).size());
            assert!(inventory.members().len() >= 4);
        } else {
            assert!(
                matches!(result, Err(PackageCheckError::NativeMemberMissing)),
                "{}",
                case["name"]
            );
        }
    }
}
#[test]
fn independent_context_reference_version_and_artifact_reject_before_file_reads_and_restore() {
    let fixtures = Fixtures::new();
    let base = fixtures.baseline();
    let mut source = File::open(base["path"].as_str().unwrap()).unwrap();
    for field in ["role", "product", "component", "scope"] {
        let mut changed = base.clone();
        changed["expectedContext"][field] = match field {
            "scope" => json!({"targetKey":"linux-arm64-deb-perMachine"}),
            "role" => json!("release"),
            _ => json!("other-identity"),
        };
        source.seek(SeekFrom::Start(7)).unwrap();
        assert!(matches!(
            run(&mut source, &changed, artifact(base), || false),
            Err(PackageCheckError::Context(_))
        ));
        assert_eq!(source.stream_position().unwrap(), 7);
        run(&mut source, base, artifact(base), || false).unwrap();
    }
    let mut changed = base.clone();
    changed["expectedVersion"] = json!("3.0.0");
    source.seek(SeekFrom::Start(7)).unwrap();
    assert!(matches!(
        run(&mut source, &changed, artifact(base), || false),
        Err(PackageCheckError::VersionMismatch)
    ));
    assert_eq!(source.stream_position().unwrap(), 7);
    run(&mut source, base, artifact(base), || false).unwrap();
    let mut changed = base.clone();
    changed["reference"]["envelopeSha256"] = json!("b".repeat(64));
    source.seek(SeekFrom::Start(7)).unwrap();
    assert!(matches!(
        run(&mut source, &changed, artifact(base), || false),
        Err(PackageCheckError::Reference(
            MetadataReferenceError::Mismatch
        ))
    ));
    assert_eq!(source.stream_position().unwrap(), 7);
    run(&mut source, base, artifact(base), || false).unwrap();
    for expected in [
        Artifact::new(
            "other-package",
            artifact(base).size(),
            base["artifact"]["sha256"].as_str().unwrap(),
        )
        .unwrap(),
        Artifact::new(
            "package-2",
            artifact(base).size() + 1,
            base["artifact"]["sha256"].as_str().unwrap(),
        )
        .unwrap(),
        Artifact::new("package-2", artifact(base).size(), &"b".repeat(64)).unwrap(),
    ] {
        source.seek(SeekFrom::Start(7)).unwrap();
        assert!(matches!(
            run(&mut source, base, expected, || false),
            Err(PackageCheckError::ArtifactMismatch)
        ));
        assert_eq!(source.stream_position().unwrap(), 7);
        run(&mut source, base, artifact(base), || false).unwrap();
    }
}
#[test]
fn canonical_and_full_package_semantics_are_required_but_success_is_not_signature_trust() {
    let fixtures = Fixtures::new();
    let base = fixtures.baseline();
    let mut source = File::open(base["path"].as_str().unwrap()).unwrap();
    let mut changed = base.clone();
    changed["manifest"] = json!(format!("{} ", base["manifest"].as_str().unwrap()));
    assert!(matches!(
        run(&mut source, &changed, artifact(base), || false),
        Err(PackageCheckError::Envelope(
            EnvelopeShapeError::NotCanonical
        ))
    ));
    let mut envelope: Value = serde_json::from_str(base["manifest"].as_str().unwrap()).unwrap();
    envelope["signed"]["helpers"][1]["targetKey"] = json!("linux-arm64-deb-perMachine");
    let bytes = canonical(&envelope);
    changed["manifest"] = json!(String::from_utf8(bytes.clone()).unwrap());
    changed["reference"] =
        serde_json::from_slice(derive_metadata_reference(&bytes).unwrap().canonical_bytes())
            .unwrap();
    assert!(matches!(
        run(&mut source, &changed, artifact(base), || false),
        Err(PackageCheckError::Semantics(
            MetadataSemanticsError::HelperTarget
        ))
    ));
    let mut wrong_role = base.clone();
    wrong_role["manifest"] = base["rootManifest"].clone();
    let bytes = wrong_role["manifest"].as_str().unwrap().as_bytes();
    wrong_role["reference"] =
        serde_json::from_slice(derive_metadata_reference(bytes).unwrap().canonical_bytes())
            .unwrap();
    source.seek(SeekFrom::Start(7)).unwrap();
    assert!(matches!(
        run(&mut source, &wrong_role, artifact(base), || false),
        Err(PackageCheckError::WrongRole)
    ));
    assert_eq!(source.stream_position().unwrap(), 7);
    run(&mut source, base, artifact(base), || false).unwrap();
    // Deliberately invalid Ed25519 signature, with a matching fresh supplied
    // reference, is an encoding-valid structural input. No verifier is invoked.
    let mut envelope: Value = serde_json::from_str(base["manifest"].as_str().unwrap()).unwrap();
    envelope["signatures"][0]["signature"] = json!("A".repeat(86));
    let bytes = canonical(&envelope);
    changed["manifest"] = json!(String::from_utf8(bytes.clone()).unwrap());
    changed["reference"] =
        serde_json::from_slice(derive_metadata_reference(&bytes).unwrap().canonical_bytes())
            .unwrap();
    run(&mut source, &changed, artifact(base), || false).unwrap();
    run(&mut source, base, artifact(base), || false).unwrap();
    assert!(matches!(
        doc(&json!("02.0.0")).app_version_shape(),
        Err(VersionError::InvalidVersion)
    ));
    assert!(matches!(
        Document::parse(b"{\"signed\":{},\"signed\":{}}", InputKind::Metadata),
        Err(Error::DuplicateKey)
    ));
}
#[test]
fn cancellation_and_current_file_mismatch_return_no_partial_inventory() {
    let fixtures = Fixtures::new();
    let base = fixtures.baseline();
    let mut source = File::open(base["path"].as_str().unwrap()).unwrap();
    source.seek(SeekFrom::Start(7)).unwrap();
    assert!(matches!(
        run(&mut source, base, artifact(base), || true),
        Err(PackageCheckError::Archive(ArchiveError::Cancelled))
    ));
    assert_eq!(source.stream_position().unwrap(), 7);
    let mut count = 0;
    assert!(matches!(
        run(&mut source, base, artifact(base), || {
            count += 1;
            count >= 12
        }),
        Err(PackageCheckError::Archive(ArchiveError::Cancelled))
    ));
    run(&mut source, base, artifact(base), || false).unwrap();
    let mut full_count = 0;
    run(&mut source, base, artifact(base), || {
        full_count += 1;
        false
    })
    .unwrap();
    assert!(full_count > 12);
    let mut final_count = 0;
    assert!(matches!(
        run(&mut source, base, artifact(base), || {
            final_count += 1;
            final_count == full_count
        }),
        Err(PackageCheckError::Archive(ArchiveError::Cancelled))
    ));
    assert_eq!(final_count, full_count);
    run(&mut source, base, artifact(base), || false).unwrap();
    let mut wrong = File::open(fixtures.cases[1]["path"].as_str().unwrap()).unwrap();
    assert!(matches!(
        run(&mut wrong, base, artifact(base), || false),
        Err(PackageCheckError::Archive(ArchiveError::PackageBytes(_)))
    ));
    run(&mut source, base, artifact(base), || false).unwrap();
}
