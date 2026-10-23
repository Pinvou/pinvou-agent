//! Pure reference/transition tests. These never manufacture a protected byte
//! capability or claim native ACL, physical sealing, execution or E2E coverage.
use super::*;
use std::cell::Cell;

fn identity(values: [u32; 5]) -> Identity {
    serde_json::from_value(serde_json::json!({
        "volume": values[0], "indexHigh": values[1], "indexLow": values[2],
        "createdHigh": values[3], "createdLow": values[4],
    }))
    .unwrap()
}
fn reference() -> ObjectReference {
    ObjectReference {
        generation: ObjectGeneration {
            package_identity: identity([1, 2, 3, 4, 5]),
            native_identity: identity([6, 7, 8, 9, 10]),
            binding: ObjectBinding {
                root_claims_sha256: "a".repeat(64),
                package_envelope_sha256: "b".repeat(64),
                product: "pinvou".into(),
                component: "agent".into(),
                target_key: "windows-x86_64-nsis-perMachine".into(),
                profile_kind: "production".into(),
                profile_id: "stable-profile".into(),
                seed_sha256: "c".repeat(64),
                app_version: "1.2.3".into(),
                package_id: "package-id".into(),
                full_pack_size: 123,
                full_pack_sha256: "d".repeat(64),
                role: NativeRole::FinalInstaller,
                native_claim: b"signed-native-claim".to_vec(),
                native_size: 45,
                native_sha256: "e".repeat(64),
            },
        },
    }
}

#[test]
fn same_reference_allows_exactly_one_physical_transition_check() {
    let actual = reference();
    let expected = actual.clone();
    let calls = Cell::new(0);
    let result = check_transition(&actual, &expected, &mut || false, |stop| {
        calls.set(calls.get() + 1);
        poll(stop)
    });
    assert!(result.is_ok());
    assert_eq!(calls.get(), 1);
}

#[test]
fn every_actual_identity_field_including_creation_changes_the_generation() {
    let actual = reference();
    for native in [false, true] {
        for field in 0..5 {
            let mut values = if native {
                [6, 7, 8, 9, 10]
            } else {
                [1, 2, 3, 4, 5]
            };
            values[field] += 1;
            let mut other = actual.clone();
            if native {
                other.generation.native_identity = identity(values);
            } else {
                other.generation.package_identity = identity(values);
            }
            let calls = Cell::new(0);
            assert!(
                check_transition(&actual, &other, &mut || false, |_| {
                    calls.set(calls.get() + 1);
                    Ok(())
                }) == Err(Error::ImageRefused)
            );
            assert_eq!(
                calls.get(),
                0,
                "wrong physical generation must not reach action recheck"
            );
        }
    }
}

#[test]
fn metadata_binding_changes_cannot_reuse_a_physical_file_generation() {
    let changes: &[fn(&mut ObjectBinding)] = &[
        |v| v.root_claims_sha256 = "f".repeat(64),
        |v| v.package_envelope_sha256 = "f".repeat(64),
        |v| v.product = "other".into(),
        |v| v.component = "other".into(),
        |v| v.target_key = "linux-x86_64-deb-perMachine".into(),
        |v| v.profile_kind = "lab".into(),
        |v| v.profile_id = "other".into(),
        |v| v.seed_sha256 = "f".repeat(64),
        |v| v.app_version = "1.2.4".into(),
        |v| v.package_id = "other".into(),
        |v| v.full_pack_size += 1,
        |v| v.full_pack_sha256 = "f".repeat(64),
        |v| v.role = NativeRole::Launcher,
        |v| v.native_claim.push(0),
        |v| v.native_size += 1,
        |v| v.native_sha256 = "f".repeat(64),
    ];
    let actual = reference();
    for change in changes {
        let mut other = actual.clone();
        change(&mut other.generation.binding);
        assert!(actual.generation != other.generation);
        assert!(
            check_transition(&actual, &other, &mut || false, |_| {
                panic!("metadata mismatch must refuse before physical/action transition")
            }) == Err(Error::ImageRefused)
        );
    }
}

#[test]
fn cancellation_before_or_during_recheck_never_completes_a_transition() {
    let actual = reference();
    let expected = actual.clone();
    let calls = Cell::new(0);
    assert!(
        check_transition(&actual, &expected, &mut || true, |_| {
            calls.set(calls.get() + 1);
            Ok(())
        }) == Err(Error::StagingCanceled)
    );
    assert_eq!(calls.get(), 0);

    let canceled = Cell::new(false);
    let exposed_new_owner = Cell::new(false);
    let result = check_transition(&actual, &expected, &mut || canceled.get(), |stop| {
        canceled.set(true);
        assert!(stop());
        canceled.set(false);
        assert!(stop(), "one canceled observation must stay latched");
        Ok(())
    });
    if result.is_ok() {
        exposed_new_owner.set(true);
    }
    assert!(result == Err(Error::StagingCanceled));
    assert!(!exposed_new_owner.get());
}

#[test]
fn physical_recheck_failure_is_preserved_without_a_transition() {
    let actual = reference();
    assert!(
        check_transition(&actual, &actual.clone(), &mut || false, |_| Err(
            Error::SecurityRefused
        )) == Err(Error::SecurityRefused)
    );
}
