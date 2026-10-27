//! Native Store component test in a separate process with an overridden HKLM.
//! This does not construct a source-installer capability or install a service.
use super::*;
use crate::{
    identity::{FileTime, LogonId},
    inventory::tests,
    registration::InstallerIdentity,
};
use std::sync::atomic::Ordering;

struct IsolatedRoot {
    root: Option<Key>,
    override_active: bool,
}
#[link(name = "ntdll")]
#[allow(unsafe_code)]
unsafe extern "system" {
    fn NtDeleteKey(key: HKEY) -> i32;
}
impl IsolatedRoot {
    #[allow(unsafe_code)]
    fn cleanup(&mut self) -> std::result::Result<(), &'static str> {
        if self.override_active {
            if unsafe { RegOverridePredefKey(HKEY_LOCAL_MACHINE, ptr::null_mut()) } != 0 {
                return Err("restore HKLM");
            }
            self.override_active = false;
        }
        if let Some(root) = self.root.as_ref() {
            if unsafe { RegDeleteTreeW(root.0, ptr::null()) } != 0 {
                return Err("clear retained test root");
            }
            if unsafe { NtDeleteKey(root.0) } != 0 {
                return Err("delete retained test root by handle");
            }
            drop(self.root.take());
        }
        Ok(())
    }
    fn finish(mut self) {
        self.cleanup().expect("cleanup owned registry test root");
    }
}
impl Drop for IsolatedRoot {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            eprintln!("best-effort test cleanup failed: {error}");
        }
    }
}

#[allow(unsafe_code)]
fn isolated_root() -> IsolatedRoot {
    use std::time::{SystemTime, UNIX_EPOCH};
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = wide(&format!(
        "PinvouMetadataComponent-{}-{unique}",
        std::process::id()
    ));
    let mut root = ptr::null_mut();
    let mut disposition = 0;
    let policy = NativeSecurity::registry(false).expect("native test-root policy");
    assert_eq!(
        unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_ALL_ACCESS,
                &policy.attributes(),
                &mut root,
                &mut disposition,
            )
        },
        0,
        "create test root"
    );
    if disposition != REG_CREATED_NEW_KEY {
        drop(Key(root));
        panic!("refuse existing test root");
    }
    let mut owned = IsolatedRoot {
        root: Some(Key(root)),
        override_active: false,
    };
    inspect_registry(root, true).expect("real protected test-root ACL");
    assert_eq!(
        unsafe { RegOverridePredefKey(HKEY_LOCAL_MACHINE, root) },
        0,
        "override HKLM"
    );
    owned.override_active = true;
    // Production checks the Software ancestor; create it with the unchanged
    // native policy. An unelevated token must fail here, never silently skip.
    let policy = NativeSecurity::registry(true).expect("native policy");
    let mut software = ptr::null_mut();
    assert_eq!(
        unsafe {
            RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                wide("Software").as_ptr(),
                0,
                ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_ALL_ACCESS | KEY_WOW64_64KEY,
                &policy.attributes(),
                &mut software,
                &mut disposition,
            )
        },
        0,
        "create protected Software: elevated administrator required"
    );
    let software = Key(software);
    assert_eq!(disposition, REG_CREATED_NEW_KEY);
    inspect_registry(software.0, true).expect("real protected registry ACL");
    drop(software);
    owned
}

pub(crate) fn run_isolated() {
    let _root = isolated_root();
    let seed = Seed::embedded().expect("signed test Seed and Root must be embedded");
    assert_eq!(seed.profile_kind, "lab");
    assert_eq!(seed.profile_id, "metadata-component-test");
    let input = tests::fixture_for_seed(&seed, vec![vec![tests::app()]]);
    let manifest = input.parse(&seed).expect("real inventory parser");
    let installer = InstallerIdentity {
        process_id: 1, creation_file_time: FileTime::from_native(0, 1),
        token_user_sid: "S-1-5-18".into(), logon_authentication_id: LogonId { high: 0, low: 1 },
        session_id: 1, source_nsis_sha256: "4".repeat(64),
        source_nsis_file_identity: serde_json::from_value(serde_json::json!({"volume":1,"indexHigh":0,"indexLow":1,"createdHigh":0,"createdLow":1})).unwrap(),
        manifest_sha256: json::sha256(&input.root), operation_id: "9".repeat(64),
    };
    let record =
        HostRegistration::initial(&seed, &manifest, "5".repeat(64), installer, None).unwrap();
    let store = Store::new(&seed);
    assert!(store.read(&seed).unwrap().is_none());
    INITIAL_COMMIT_FAULT.store(1, Ordering::SeqCst);
    assert_eq!(
        store.commit(&seed, None, &record, Some(&input)),
        Err(Error::Unavailable)
    );
    assert!(
        store.read(&seed).unwrap().is_none(),
        "abandoned TxR must publish no scope"
    );
    INITIAL_COMMIT_FAULT.store(2, Ordering::SeqCst);
    assert_eq!(
        store.commit(&seed, None, &record, Some(&input)),
        Err(Error::StateCommitUnknown)
    );
    let reopened = Store::new(&seed)
        .read(&seed)
        .unwrap()
        .expect("committed unknown must reopen");
    assert_eq!(reopened.digest().unwrap(), record.digest().unwrap());
    Store::new(&seed)
        .verify_initial_metadata(&seed, &reopened)
        .expect("recheck original initial intent");
    assert_eq!(
        store.commit(&seed, None, &record, Some(&input)),
        Err(Error::ScopeConflict),
        "must not reinitialize committed scope"
    );
    let private = store.key(true, None).unwrap().unwrap();
    let meta = open(private.0, "metadata-state-v1", true, None)
        .unwrap()
        .unwrap();
    write(&meta, "Head", b"damaged").unwrap();
    drop(meta);
    drop(private);
    assert!(
        store.verify_initial_metadata(&seed, &reopened).is_err(),
        "damaged initial state must not be claimed"
    );
    legacy_ready_is_readable(&store, &seed, &record);
    _root.finish();
    let _normal_root = isolated_root();
    let normal = Store::new(&seed);
    normal
        .commit(&seed, None, &record, Some(&input))
        .expect("normal first install commits and verifies");
    let committed = Store::new(&seed)
        .read(&seed)
        .unwrap()
        .expect("reopen normal commit");
    assert_eq!(committed.digest().unwrap(), record.digest().unwrap());
    normal
        .verify_initial_metadata(&seed, &committed)
        .expect("normal initial state remains valid");
    _normal_root.finish();
}

// Construct the old serialized lab shape under the same test-owned protected
// root. This checks Store compatibility, not the historical installer.
#[allow(unsafe_code)]
fn legacy_ready_is_readable(store: &Store, seed: &Seed, initial: &HostRegistration) {
    use crate::registration::{Phase, StartupAttestation};
    let mut old = initial.clone();
    old.known_owner_namespaces = vec!["bootstrap-v1".into()];
    let files = old.next(Phase::FilesVerified, "verify-files").unwrap();
    let mut intent = files
        .next(Phase::FilesVerified, "service-create-intent")
        .unwrap();
    intent.service_identity.creation_operation_id =
        Some(old.installer_identity.operation_id.clone());
    let mut configured = intent
        .next(Phase::ServiceConfigured, "service-configured")
        .unwrap();
    configured.service_identity.service_sid = Some("S-1-5-80-1-2-3-4-5".into());
    let mut started = configured
        .next(Phase::ServiceConfigured, "startup-incarnation")
        .unwrap();
    started.host_epoch = 1;
    started.startup_attestation = Some(StartupAttestation {
        process_id: 2,
        creation_file_time: FileTime::from_native(0, 2),
        instance_nonce: "6".repeat(64),
        helper_image_sha256: started.helper.sha256.clone(),
        manifest_sha256: started.genesis_manifest_sha256.clone(),
        host_epoch: 1,
        observed_registration_revision: configured.revision,
    });
    let ready = started.next(Phase::Ready, "ready").unwrap();
    for entry in [&old, &files, &intent, &configured, &started, &ready] {
        entry.validate(seed).unwrap();
    }
    ready.validate(seed).unwrap();
    let private = store.key(true, None).unwrap().unwrap();
    for name in [
        METADATA_BOOTSTRAP,
        METADATA_BOOTSTRAP_ROOT,
        PREPARATION_BOOTSTRAP,
    ] {
        assert_eq!(
            unsafe { RegDeleteValueW(private.0, wide(name).as_ptr()) },
            0
        );
    }
    assert_eq!(
        unsafe { RegDeleteTreeW(private.0, wide("metadata-state-v1").as_ptr()) },
        0
    );
    let first = open(private.0, "history\\0000000000000001", true, None)
        .unwrap()
        .unwrap();
    write(&first, "Record", &json::encode(&old, false).unwrap()).unwrap();
    drop(first);
    let tx = Handle::owned(unsafe {
        CreateTransaction(ptr::null_mut(), ptr::null_mut(), 0, 0, 0, 0, ptr::null())
    })
    .unwrap();
    let history = open(private.0, "history", true, Some(&tx))
        .unwrap()
        .unwrap();
    for entry in [&files, &intent, &configured, &started, &ready] {
        let revision = create(history.0, &revision_name(entry.revision), &tx, false, true).unwrap();
        write(&revision, "Record", &json::encode(entry, false).unwrap()).unwrap();
    }
    drop(history);
    assert_ne!(unsafe { CommitTransaction(tx.raw()) }, 0);
    drop(tx);
    write(&private, "Head", &json::encode(&ready, false).unwrap()).unwrap();
    drop(private);
    let public = store.public_key(None).unwrap().unwrap();
    write(
        &public,
        "Head",
        &json::encode(&ready.public(), false).unwrap(),
    )
    .unwrap();
    drop(public);
    let actual = store
        .read(seed)
        .unwrap()
        .expect("legacy Ready remains readable");
    assert_eq!(actual.digest().unwrap(), ready.digest().unwrap());
    assert!(matches!(
        store.original_root(seed, &actual),
        Err(Error::InitialTrustUnavailable)
    ));
    let private = store.key(false, None).unwrap().unwrap();
    assert!(
        open(private.0, "metadata-state-v1", false, None)
            .unwrap()
            .is_none(),
        "legacy read must not create metadata"
    );
}
