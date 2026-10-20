use crate::*;
use std::{fs, os::windows::fs::OpenOptionsExt, path::PathBuf};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

struct Fixture {
    root: PathBuf,
    signed: PathBuf,
    unsigned: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "pinvou-native-evidence-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let signed = root.join("signed.dll");
        let unsigned = root.join("unsigned.exe");
        // Genuine system signed bytes, not a test certificate or forged return.
        // Copying avoids the system image's legitimate WinSxS hardlink aliases.
        let system = PathBuf::from(std::env::var_os("SystemRoot").expect("Windows fixture root"))
            .join("System32/kernel32.dll");
        fs::copy(system, &signed).unwrap();
        fs::copy(std::env::current_exe().unwrap(), &unsigned).unwrap();
        Self {
            root,
            signed,
            unsigned,
        }
    }
    fn expected(path: &Path) -> ExpectedFile {
        let bytes = fs::read(path).unwrap();
        ExpectedFile::new(bytes.len() as u64, &hash(&bytes)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}
fn observe(path: &Path) -> Observation {
    observe_path(path, &Fixture::expected(path), &AtomicBool::new(false)).unwrap()
}
fn raw_reader(path: &Path) -> File {
    fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(path)
        .unwrap()
}

#[test]
fn actual_hfile_overrides_opposite_signed_path_and_null_path_matches() {
    let f = Fixture::new();
    let signed = super::lock(&raw_reader(&f.signed)).unwrap();
    let unsigned = super::lock(&raw_reader(&f.unsigned)).unwrap();
    let ordinary = super::wintrust::inspect(&signed, &AtomicBool::new(false)).unwrap();
    let wrong_path = super::wintrust::supplied_path_for_test(&signed, &f.unsigned).unwrap();
    assert_eq!(ordinary[0].native_status, wrong_path[0].native_status);
    assert_eq!(ordinary.len(), wrong_path.len());
    for (original, alternate) in ordinary.iter().zip(&wrong_path) {
        assert_eq!(original.index, alternate.index);
        assert_eq!(original.native_status, alternate.native_status);
        assert_eq!(original.verification, alternate.verification);
        let fingerprints = |s: &SignatureEvidence| {
            s.signer.as_ref().map(|c| {
                c.certificates
                    .iter()
                    .map(|c| c.certificate_sha256.clone())
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(fingerprints(original), fingerprints(alternate));
    }
    assert_ne!(ordinary[0].verification, Verification::Unsigned);
    // Cache-only verification may legitimately be Unknown with no signer on a
    // cold machine. Object choice is checked against the actual NULL-path
    // baseline above, including optional certificate fingerprints, rather than
    // assuming a cached complete chain. The inverse must remain unsigned.
    let inverse = super::wintrust::supplied_path_for_test(&unsigned, &f.signed).unwrap();
    assert_eq!(inverse.len(), 1);
    assert_eq!(inverse[0].native_status, "800b0100");
    assert_eq!(inverse[0].verification, Verification::Unsigned);
    assert_eq!(
        super::wintrust::inspect(&unsigned, &AtomicBool::new(false)).unwrap()[0].verification,
        Verification::Unsigned
    );
    // Each State has been moved into a Vec and dropped after explicit CLOSE in
    // the collector. No native state or path pointer is returned to the caller.
}

#[test]
fn actual_tamper_rehashed_reaches_provider_and_byte_identity_refusals_release() {
    let f = Fixture::new();
    let expected = Fixture::expected(&f.signed);
    let wrong = ExpectedFile::new(expected.size, &"0".repeat(64)).unwrap();
    assert!(matches!(
        observe_path(&f.signed, &wrong, &AtomicBool::new(false)),
        Err(Error::ByteMismatch)
    ));
    let wrong_size = ExpectedFile::new(expected.size + 1, &expected.sha256).unwrap();
    assert!(matches!(
        observe_path(&f.signed, &wrong_size, &AtomicBool::new(false)),
        Err(Error::ByteMismatch)
    ));
    let wrong_identity = Fixture::expected(&f.signed)
        .with_identity_sha256(&"0".repeat(64))
        .unwrap();
    assert!(matches!(
        observe_path(&f.signed, &wrong_identity, &AtomicBool::new(false)),
        Err(Error::IdentityMismatch)
    ));
    let signature_count = observe(&f.signed).report().signatures.len();
    let original = fs::read(&f.signed).unwrap();
    let mut changed = original.clone();
    changed[0x1000] ^= 1;
    fs::write(&f.signed, &changed).unwrap();
    // Recomputed expectations prevent the SHA gate masking signature failure.
    let damaged = observe(&f.signed);
    assert_eq!(
        damaged.report().signatures[0].verification,
        Verification::InvalidSignature
    );
    assert_eq!(damaged.report().signatures[0].native_status, "80096010");
    assert_eq!(damaged.report().signatures.len(), signature_count);
    for (index, s) in damaged.report().signatures.iter().enumerate() {
        assert_eq!(s.index, index as u32);
        assert_eq!(s.native_status, "80096010");
        assert_eq!(s.verification, Verification::InvalidSignature);
    }
    drop(damaged);
    fs::write(&f.signed, &original).unwrap();
    let restored = observe(&f.signed);
    assert_ne!(
        restored.report().signatures[0].verification,
        Verification::InvalidSignature
    );
    let identity = restored.report().identity_sha256.clone();
    drop(restored);
    assert!(
        observe_path(
            &f.signed,
            &Fixture::expected(&f.signed)
                .with_identity_sha256(&identity)
                .unwrap(),
            &AtomicBool::new(false)
        )
        .is_ok()
    );
}

#[test]
fn reopen_file_rejects_existing_writer_and_observation_holds_write_delete_exclusion() {
    let f = Fixture::new();
    let read = raw_reader(&f.signed);
    let writer = fs::OpenOptions::new()
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(&f.signed)
        .unwrap();
    assert!(matches!(
        observe_file(
            &read,
            &Fixture::expected(&f.signed),
            &AtomicBool::new(false)
        ),
        Err(Error::Busy)
    ));
    drop(writer);
    let observation = observe_file(
        &read,
        &Fixture::expected(&f.signed),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(fs::OpenOptions::new().write(true).open(&f.signed).is_err());
    assert!(fs::rename(&f.signed, f.root.join("replaced.dll")).is_err());
    assert!(
        !observation.report().publisher_qualification
            && !observation.report().current_qualification
            && !observation.report().installation_rights
    );
    drop(observation);
    drop(read);
    let bytes = fs::read(&f.signed).unwrap();
    fs::write(&f.signed, bytes).unwrap();
}

#[test]
fn cancellation_links_and_ancestor_aliases_are_refused_without_touching_target() {
    let f = Fixture::new();
    let original = fs::read(&f.signed).unwrap();
    assert!(matches!(
        observe_path(
            &f.signed,
            &Fixture::expected(&f.signed),
            &AtomicBool::new(true)
        ),
        Err(Error::Cancelled)
    ));
    let alias = f.root.join("alias.dll");
    fs::hard_link(&f.signed, &alias).unwrap();
    assert!(matches!(
        observe_path(
            &f.signed,
            &Fixture::expected(&f.signed),
            &AtomicBool::new(false)
        ),
        Err(Error::UnsafeObject)
    ));
    fs::remove_file(alias).unwrap();
    let junction = f.root.join("junction");
    let target = f.root.join("target");
    fs::create_dir(&target).unwrap();
    fs::copy(&f.signed, target.join("image.dll")).unwrap();
    let child = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&junction)
        .arg(&target)
        .output()
        .unwrap();
    assert!(child.status.success());
    assert!(matches!(
        observe_path(
            &junction.join("image.dll"),
            &Fixture::expected(&f.signed),
            &AtomicBool::new(false)
        ),
        Err(Error::UnsafeObject)
    ));
    assert!(matches!(
        observe_path(
            &junction,
            &Fixture::expected(&f.signed),
            &AtomicBool::new(false)
        ),
        Err(Error::UnsafeObject)
    ));
    fs::remove_dir(junction).unwrap();
    assert_eq!(fs::read(&f.signed).unwrap(), original);
    assert!(observe(&f.signed).report().size > 0);
}

#[test]
fn certificate_chain_completeness_never_treats_missing_revocation_as_verified() {
    // Pure predicate fixture, not a forged native verification result. Actual
    // complete certificate/timestamp chains have a separate strict opt-in test.
    let leaf = CertificateEvidence {
        certificate_sha256: "1".repeat(64),
        encoded_bytes: 1,
        not_before_file_time: "9007199254740993".into(),
        not_after_file_time: "18446744073709551615".into(),
        provider_error: 0,
        chain_error_status: Some(0),
        chain_info_status: Some(0),
        revocation_error: Some(0),
        trusted_root: false,
    };
    let root = CertificateEvidence {
        certificate_sha256: "2".repeat(64),
        // Informational self-signed status is not an error; root revocation is
        // excluded by the production policy, while non-root evidence is required.
        chain_info_status: Some(0x8),
        revocation_error: None,
        trusted_root: true,
        ..leaf.clone()
    };
    let complete = ChainEvidence {
        signer_type: 0,
        provider_error: 0,
        verification_as_of_file_time: "9007199254740993".into(),
        chain_error_status: Some(0),
        chain_info_status: Some(0),
        certificates: vec![leaf, root],
    };
    assert!(super::provider::complete(&complete));
    // All serialized FILETIME values retain exact decimal strings beyond f64.
    let value = serde_json::to_value(&complete).unwrap();
    assert_eq!(value["verificationAsOfFileTime"], "9007199254740993");
    assert_eq!(
        value["certificates"][0]["notBeforeFileTime"],
        "9007199254740993"
    );
    assert_eq!(
        value["certificates"][0]["notAfterFileTime"],
        "18446744073709551615"
    );
    let mut missing = complete.clone();
    missing.chain_error_status = None;
    assert!(!super::provider::complete(&missing));
    let mut missing = complete.clone();
    missing.certificates[0].revocation_error = None;
    assert!(!super::provider::complete(&missing));
    let mut missing = complete.clone();
    missing.certificates[0].chain_error_status = Some(0x40);
    assert!(!super::provider::complete(&missing));
    let mut missing = complete.clone();
    missing.certificates.last_mut().unwrap().trusted_root = false;
    assert!(!super::provider::complete(&missing));
    let mut missing = complete.clone();
    missing.provider_error = 1;
    assert!(!super::provider::complete(&missing));
    let mut missing = complete.clone();
    missing.certificates[0].provider_error = 1;
    assert!(!super::provider::complete(&missing));
    let mut missing = complete.clone();
    missing.certificates[0].revocation_error = Some(1);
    assert!(!super::provider::complete(&missing));
    let mut missing = complete.clone();
    missing.certificates.clear();
    assert!(!super::provider::complete(&missing));
    assert!(super::provider::complete(&complete));
}

#[test]
#[ignore = "requires a genuine embedded PE whose complete chains/revocation are already cached; no network fallback"]
fn cache_only_strict_signature_and_timestamp_positive() {
    let f = Fixture::new();
    let observed = observe(&f.signed);
    assert!(
        observed.report().signatures.len() >= 2,
        "strict positive includes actual dual embedded signature enumeration"
    );
    for (i, s) in observed.report().signatures.iter().enumerate() {
        assert_eq!(s.index, i as u32);
    }
    let signature = observed
        .report()
        .signatures
        .iter()
        .find(|s| s.verification == Verification::Verified)
        .expect("one complete provider signature/chain positive");
    assert_eq!(signature.native_status, "00000000");
    assert_eq!(signature.verification, Verification::Verified);
    assert!(super::provider::complete(
        signature.signer.as_ref().unwrap()
    ));
    assert!(!signature.timestamps.is_empty());
    assert!(
        signature
            .timestamps
            .iter()
            .all(|s| s.signer_type == 16 && super::provider::complete(s))
    );
}

#[test]
#[allow(unsafe_code)]
fn actual_live_writable_mapping_is_refused_after_writer_handle_closes() {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_WRITE, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
    };
    let f = Fixture::new();
    let expected = Fixture::expected(&f.signed);
    let writer = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(&f.signed)
        .unwrap();
    // SAFETY: owned fixture file and exact fixed SDK parameters. Every view and
    // section is released below before assertions or fixture deletion.
    let section = unsafe {
        CreateFileMappingW(
            writer.as_raw_handle(),
            std::ptr::null(),
            PAGE_READWRITE,
            0,
            0,
            std::ptr::null(),
        )
    };
    assert!(!section.is_null());
    // SAFETY: adopt the newly created section exactly once; cleanup also runs
    // if view creation or a later assertion fails.
    let section = unsafe { OwnedHandle::from_raw_handle(section) };
    let view = unsafe { MapViewOfFile(section.as_raw_handle(), FILE_MAP_WRITE, 0, 0, 0) };
    assert!(!view.Value.is_null());
    struct View(windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS);
    impl Drop for View {
        fn drop(&mut self) {
            // SAFETY: one owned live view, released before the section/file.
            unsafe { UnmapViewOfFile(self.0) };
        }
    }
    let view = View(view);
    drop(writer);
    let read = raw_reader(&f.signed);
    let result = observe_file(&read, &expected, &AtomicBool::new(false));
    drop(view);
    drop(section);
    assert!(matches!(result, Err(Error::Busy)));
    assert!(observe_file(&read, &expected, &AtomicBool::new(false)).is_ok());
}

#[test]
fn same_bytes_in_another_object_fail_real_identity_and_retained_handle_survives_path_swap() {
    let f = Fixture::new();
    let observed = observe(&f.signed);
    let identity = observed.report().identity_sha256.clone();
    drop(observed);
    let expected = Fixture::expected(&f.signed)
        .with_identity_sha256(&identity)
        .unwrap();
    let copy = f.root.join("copy.dll");
    fs::copy(&f.signed, &copy).unwrap();
    assert!(matches!(
        observe_path(&copy, &expected, &AtomicBool::new(false)),
        Err(Error::IdentityMismatch)
    ));
    let original = raw_reader(&f.signed);
    let retained = f.root.join("retained.dll");
    fs::rename(&f.signed, &retained).unwrap();
    fs::copy(&f.unsigned, &f.signed).unwrap();
    let still_original = observe_file(&original, &expected, &AtomicBool::new(false)).unwrap();
    assert_ne!(
        still_original.report().signatures[0].verification,
        Verification::Unsigned
    );
    assert_eq!(
        observe(&f.signed).report().signatures[0].verification,
        Verification::Unsigned
    );
    drop(still_original);
    drop(original);
    fs::remove_file(&f.signed).unwrap();
    fs::rename(&retained, &f.signed).unwrap();
    assert!(observe_path(&f.signed, &expected, &AtomicBool::new(false)).is_ok());
}
#[test]
fn cancellation_after_genuine_wintrust_call_closes_before_result() {
    let f = Fixture::new();
    let original = raw_reader(&f.signed);
    let locked = super::lock(&original).unwrap();
    let flag = AtomicBool::new(false);
    assert!(matches!(
        super::wintrust::cancel_after_actual_native_call(&locked, &flag),
        Err(Error::Cancelled)
    ));
    drop(locked);
    drop(original);
    // The actual call has returned, no detached thread/provider state escapes.
    assert!(fs::OpenOptions::new().write(true).open(&f.signed).is_ok());
    assert!(observe(&f.signed).report().size > 0);
}
