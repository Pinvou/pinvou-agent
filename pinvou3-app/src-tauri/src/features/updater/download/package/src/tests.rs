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

fn owned_source(
    fixtures: &Fixtures,
    case: &Value,
) -> Option<pinvou_upgrade_cache_object::ReadOnlyCacheHandle> {
    use std::io::Write;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = fixtures.root.join(format!(
        "source-cache-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    match pinvou_upgrade_cache_object::create_exclusive_cache(&path) {
        Err(pinvou_upgrade_cache_object::CacheObjectError::Unsupported) => {
            assert!(!path.exists());
            None
        }
        Ok(mut source) => {
            source
                .file_mut()
                .write_all(&std::fs::read(case["path"].as_str().unwrap()).unwrap())
                .unwrap();
            let view = source.read_only_duplicate().unwrap();
            drop(source); // Tests retire the original writer; the production API does not.
            Some(view)
        }
        Err(_) => panic!("synthetic cache source unavailable"),
    }
}
fn fresh_destination(
    fixtures: &Fixtures,
) -> (PathBuf, pinvou_upgrade_cache_object::ExclusiveCacheFile) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = fixtures.root.join(format!(
        "destination-cache-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let file = pinvou_upgrade_cache_object::create_exclusive_cache(&path).unwrap();
    (path, file)
}
fn materialize<'a>(
    source: &pinvou_upgrade_cache_object::ReadOnlyCacheHandle,
    destination: &mut pinvou_upgrade_cache_object::ExclusiveCacheFile,
    case: &Value,
    artifact: Artifact<'a>,
    kind: NativeMemberKind<'_>,
    name: &str,
    cancelled: impl FnMut() -> bool,
) -> Result<ObservedNativeMember<'a>, NativeMaterializeError> {
    let reference = doc(&case["reference"]).metadata_reference_shape().unwrap();
    let context = doc(&case["expectedContext"]);
    let version = doc(&case["expectedVersion"]).app_version_shape().unwrap();
    let time = doc(&case["time"]).protocol_time_shape().unwrap();
    materialize_native_member(
        source,
        destination,
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
        kind,
        name,
        cancelled,
    )
}
#[test]
fn actual_declared_native_roles_copy_to_fresh_objects_and_readonly_duplicates_preserve_all_fields()
{
    use std::io::{Read, Write};
    let fixtures = Fixtures::new();
    for name in ["store", "deflate", "large-33MiB"] {
        let case = fixtures
            .cases
            .iter()
            .find(|case| case["name"] == name)
            .unwrap();
        let Some(source) = owned_source(&fixtures, case) else {
            return;
        };
        let envelope: Value = serde_json::from_str(case["manifest"].as_str().unwrap()).unwrap();
        for (kind, path, value) in [
            (
                NativeMemberKind::FinalInstaller,
                "payload/installer.deb",
                &envelope["signed"]["finalInstaller"],
            ),
            (
                NativeMemberKind::ExecutionHelper,
                "helpers/current-helper",
                &envelope["signed"]["helper"],
            ),
            (
                NativeMemberKind::Launcher,
                "launcher/stable-launcher",
                &envelope["signed"]["launcher"],
            ),
            (
                NativeMemberKind::HelperArtifact("helper-extra"),
                "helpers/other-helper",
                &envelope["signed"]["helpers"][1]["nativeIdentity"],
            ),
        ] {
            let (output, mut destination) = fresh_destination(&fixtures);
            let result = materialize(
                &source,
                &mut destination,
                case,
                artifact(case),
                kind,
                path,
                || false,
            )
            .unwrap();
            assert_eq!(result.member().package().size(), artifact(case).size());
            assert_eq!(result.member().member().name(), path);
            assert_eq!(
                result.identity().canonical_bytes(),
                doc(value)
                    .native_identity_shape()
                    .unwrap()
                    .canonical_bytes()
            );
            assert_eq!(
                result.file().file().metadata().unwrap().len(),
                value["size"].as_u64().unwrap()
            );
            drop(destination);
            assert!(
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&output)
                    .is_err()
            );
            assert!(std::fs::remove_file(&output).is_err());
            let mut reader = result.file().file();
            reader.seek(SeekFrom::Start(0)).unwrap();
            let mut hasher = sha2::Sha256::new();
            use sha2::Digest;
            let mut buffer = [0_u8; 65536];
            let mut size = 0;
            loop {
                let count = reader.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                };
                size += count;
                hasher.update(&buffer[..count]);
            }
            assert_eq!(size as u64, value["size"].as_u64().unwrap());
            let actual: String = hasher
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(actual, value["sha256"].as_str().unwrap());
            let mut readonly = result.file().file();
            assert!(readonly.write_all(b"write denied").is_err());
            assert!(result.file().file().set_len(0).is_err());
            drop(result);
            assert!(std::fs::remove_file(output).is_ok());
        }
    }
}
#[test]
fn invalid_native_selector_locator_context_and_named_member_do_not_write_and_can_restore() {
    let fixtures = Fixtures::new();
    let case = fixtures.baseline();
    let Some(source) = owned_source(&fixtures, case) else {
        return;
    };
    let (_, mut destination) = fresh_destination(&fixtures);
    let mut view = source.file();
    view.seek(SeekFrom::Start(7)).unwrap();
    assert!(matches!(
        materialize(
            &source,
            &mut destination,
            case,
            artifact(case),
            NativeMemberKind::HelperArtifact("not-declared"),
            "helpers/other-helper",
            || false
        ),
        Err(NativeMaterializeError::InvalidSelector)
    ));
    assert_eq!(source.file().stream_position().unwrap(), 7);
    assert!(matches!(
        materialize(
            &source,
            &mut destination,
            case,
            artifact(case),
            NativeMemberKind::FinalInstaller,
            "../unsafe",
            || false
        ),
        Err(NativeMaterializeError::Archive(
            ArchiveError::InvalidExpectedMember
        ))
    ));
    assert_eq!(source.file().stream_position().unwrap(), 7);
    let mut context = case.clone();
    context["expectedContext"]["product"] = json!("other-product");
    assert!(matches!(
        materialize(
            &source,
            &mut destination,
            &context,
            artifact(case),
            NativeMemberKind::FinalInstaller,
            "payload/installer.deb",
            || false
        ),
        Err(NativeMaterializeError::Package(PackageCheckError::Context(
            _
        )))
    ));
    assert_eq!(source.file().stream_position().unwrap(), 7);
    assert!(matches!(
        materialize(
            &source,
            &mut destination,
            case,
            artifact(case),
            NativeMemberKind::FinalInstaller,
            "helpers/current-helper",
            || false
        ),
        Err(NativeMaterializeError::Archive(
            ArchiveError::MemberMismatch
        ))
    ));
    assert_eq!(destination.file_mut().metadata().unwrap().len(), 0);
    materialize(
        &source,
        &mut destination,
        case,
        artifact(case),
        NativeMemberKind::FinalInstaller,
        "payload/installer.deb",
        || false,
    )
    .unwrap();
}
#[test]
fn native_copy_refuses_existing_destination_and_different_actual_source_without_clobber() {
    use std::io::Write;
    let fixtures = Fixtures::new();
    let case = fixtures.baseline();
    let Some(source) = owned_source(&fixtures, case) else {
        return;
    };
    let (_, mut existing) = fresh_destination(&fixtures);
    existing
        .file_mut()
        .write_all(b"existing-owner-bytes")
        .unwrap();
    assert!(matches!(
        materialize(
            &source,
            &mut existing,
            case,
            artifact(case),
            NativeMemberKind::FinalInstaller,
            "payload/installer.deb",
            || false
        ),
        Err(NativeMaterializeError::Archive(
            ArchiveError::DestinationNotEmpty
        ))
    ));
    assert_eq!(existing.file_mut().metadata().unwrap().len(), 20);
    let (_, mut destination) = fresh_destination(&fixtures);
    let wrong = owned_source(&fixtures, &fixtures.cases[1]).unwrap();
    assert!(matches!(
        materialize(
            &wrong,
            &mut destination,
            case,
            artifact(case),
            NativeMemberKind::FinalInstaller,
            "payload/installer.deb",
            || false
        ),
        Err(NativeMaterializeError::Package(PackageCheckError::Archive(
            ArchiveError::PackageBytes(_)
        )))
    ));
    assert_eq!(destination.file_mut().metadata().unwrap().len(), 0);
    materialize(
        &source,
        &mut destination,
        case,
        artifact(case),
        NativeMemberKind::FinalInstaller,
        "payload/installer.deb",
        || false,
    )
    .unwrap();
}
#[test]
fn native_copy_initial_partial_and_last_cancel_leave_no_result_and_owner_keeps_untrusted_bytes() {
    let fixtures = Fixtures::new();
    let case = fixtures.baseline();
    let Some(source) = owned_source(&fixtures, case) else {
        return;
    };
    let (_, mut destination) = fresh_destination(&fixtures);
    assert!(matches!(
        materialize(
            &source,
            &mut destination,
            case,
            artifact(case),
            NativeMemberKind::FinalInstaller,
            "payload/installer.deb",
            || true
        ),
        Err(NativeMaterializeError::Archive(ArchiveError::Cancelled))
    ));
    assert_eq!(destination.file_mut().metadata().unwrap().len(), 0);
    let mut count = 0;
    materialize(
        &source,
        &mut destination,
        case,
        artifact(case),
        NativeMemberKind::FinalInstaller,
        "payload/installer.deb",
        || {
            count += 1;
            false
        },
    )
    .unwrap();
    let total = count;
    let (_, mut final_cancel) = fresh_destination(&fixtures);
    let mut count = 0;
    assert!(matches!(
        materialize(
            &source,
            &mut final_cancel,
            case,
            artifact(case),
            NativeMemberKind::FinalInstaller,
            "payload/installer.deb",
            || {
                count += 1;
                count == total
            }
        ),
        Err(NativeMaterializeError::Archive(ArchiveError::Cancelled))
    ));
    assert_eq!(count, total);
    assert!(final_cancel.file_mut().metadata().unwrap().len() > 0);
    let (_, mut partial) = fresh_destination(&fixtures);
    let observer = partial.file_mut().try_clone().unwrap();
    assert!(matches!(
        materialize(
            &source,
            &mut partial,
            case,
            artifact(case),
            NativeMemberKind::FinalInstaller,
            "payload/installer.deb",
            || observer.metadata().unwrap().len() > 0
        ),
        Err(NativeMaterializeError::Archive(ArchiveError::Cancelled))
    ));
    let partial_length = observer.metadata().unwrap().len();
    assert!(
        partial_length > 0
            && partial_length < case["natives"]["finalInstaller"]["size"].as_u64().unwrap()
    );
}
