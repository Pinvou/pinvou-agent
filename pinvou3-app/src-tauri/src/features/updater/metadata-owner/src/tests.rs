use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pinvou_upgrade_codec::key_descriptor;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Value, json};

fn binding(scope: &str) -> BootstrapBinding {
    BootstrapBinding::new(
        scope,
        "pinvou",
        "agent",
        "windows-x86_64-nsis-perMachine",
        "production",
        "stable-installation",
        &"2".repeat(64),
    )
    .unwrap()
}
fn canonical(value: &Value) -> Vec<u8> {
    document(value).unwrap().canonical_bytes().unwrap()
}
fn signed_root(version: u64, issued_at: u64, expires_at: u64, signers: usize) -> Vec<u8> {
    // Deterministic test keys ONLY. They never become a compiled shipping Root,
    // installed publisher identity or a protected/current owner capability.
    let pairs: Vec<_> = (1..=3)
        .map(|byte| Ed25519KeyPair::from_seed_unchecked(&[byte; 32]).unwrap())
        .collect();
    let keys: Vec<_> = pairs
        .iter()
        .map(|pair| {
            let public = URL_SAFE_NO_PAD.encode(pair.public_key().as_ref());
            let key = key_descriptor(&public).unwrap();
            json!({"algorithm":"ed25519", "keyId":key.key_id(), "publicKey":key.public_key()})
        })
        .collect();
    let ids: Vec<_> = keys.iter().map(|key| key["keyId"].clone()).collect();
    let signed = json!({
        "protocolVersion":1, "role":"root", "product":"pinvou", "component":null,
        "scope":{}, "version":version, "issuedAt":issued_at, "expiresAt":expires_at,
        "keys":keys, "roles":[{"role":"root", "component":null, "channel":null,
            "targetKey":null, "threshold":2, "keyIds":ids}]
    });
    let input = document(&signed).unwrap().signing_input().unwrap();
    let mut signatures: Vec<_> = pairs.iter().zip(keys.iter()).take(signers).map(|(pair, key)| {
        json!({"keyId":key["keyId"], "signature":URL_SAFE_NO_PAD.encode(pair.sign(input.as_bytes()).as_ref())})
    }).collect();
    signatures.sort_by(|a, b| a["keyId"].as_str().cmp(&b["keyId"].as_str()));
    canonical(&json!({"signed":signed, "signatures":signatures}))
}
fn anchor(root: &[u8]) -> Result<BootstrapAnchor, Error> {
    BootstrapAnchor::new(
        &binding(&"1".repeat(64)),
        root,
        root.len() as u64,
        &sha256(root),
    )
}

#[test]
fn genuine_self_threshold_anchor_has_only_genesis_integrity_facts() {
    // The signed window is deliberately historical. No wall clock is read or
    // current metadata qualification asserted by the bootstrap initializer.
    let root = signed_root(7, 1000, 2000, 2);
    let expected = anchor(&root).unwrap();
    assert_eq!(expected.root_bytes(), root);
    let record: Value = serde_json::from_slice(expected.record_bytes()).unwrap();
    assert_eq!(record.as_object().unwrap().len(), 4);
    assert_eq!(record["initialRoot"]["version"], 7);
    assert_eq!(record["initialRoot"]["sha256"], sha256(&root));
    assert_eq!(
        record["integrityBasis"],
        "self-threshold-at-signed-issued-at"
    );
    for forbidden in [
        "currentMetadataSet",
        "accepted",
        "currentQualification",
        "trustedNow",
    ] {
        assert!(record.get(forbidden).is_none());
    }
    let readback = BootstrapAnchor::parse(
        expected.record_bytes(),
        &root,
        &binding(&"1".repeat(64)),
        root.len() as u64,
        &sha256(&root),
    )
    .unwrap();
    assert_eq!(readback.record_bytes(), expected.record_bytes());
    assert!(
        expected
            .check_existing(readback.record_bytes(), readback.root_bytes())
            .is_ok()
    );
}

#[test]
fn genuine_signatures_cannot_be_replaced_by_recomputed_pins_or_shape_checks() {
    assert_eq!(
        anchor(&signed_root(7, 1000, 2000, 1)).err(),
        Some(Error::InvalidAnchor)
    );
    let root = signed_root(7, 1000, 2000, 2);
    let mut corrupt: Value = serde_json::from_slice(&root).unwrap();
    corrupt["signatures"][0]["signature"] = "A".repeat(86).into();
    assert_eq!(
        anchor(&canonical(&corrupt)).err(),
        Some(Error::InvalidAnchor)
    );
    corrupt = serde_json::from_slice(&root).unwrap();
    corrupt["signed"]["version"] = 8.into();
    assert_eq!(
        anchor(&canonical(&corrupt)).err(),
        Some(Error::InvalidAnchor)
    );
    corrupt = serde_json::from_slice(&root).unwrap();
    let duplicate = corrupt["signatures"][0].clone();
    corrupt["signatures"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert_eq!(
        anchor(&canonical(&corrupt)).err(),
        Some(Error::InvalidAnchor)
    );
}

#[test]
fn signed_time_interval_and_root_protocol_shape_remain_mandatory() {
    for root in [
        signed_root(7, 1000, 1000, 2),
        signed_root(7, 2000, 1000, 2),
        signed_root(7, 0, 365 * 24 * 60 * 60 * 1000 + 1, 2),
        signed_root(0, 1000, 2000, 2),
    ] {
        assert_eq!(anchor(&root).err(), Some(Error::InvalidAnchor));
    }
    assert!(anchor(&signed_root(7, 0, 365 * 24 * 60 * 60 * 1000, 2)).is_ok());
    let root = signed_root(7, 1000, 2000, 2);
    let pretty =
        serde_json::to_vec_pretty(&serde_json::from_slice::<Value>(&root).unwrap()).unwrap();
    assert_eq!(anchor(&pretty).err(), Some(Error::InvalidAnchor));
}

#[test]
fn independent_pin_product_and_real_input_bounds_cannot_be_borrowed() {
    let root = signed_root(7, 1000, 2000, 2);
    let binding = binding(&"1".repeat(64));
    assert_eq!(
        BootstrapAnchor::new(&binding, &root, root.len() as u64 + 1, &sha256(&root)).err(),
        Some(Error::RootPinMismatch)
    );
    assert_eq!(
        BootstrapAnchor::new(&binding, &root, root.len() as u64, &"0".repeat(64)).err(),
        Some(Error::RootPinMismatch)
    );
    let wrong_product = BootstrapBinding::new(
        &"1".repeat(64),
        "other",
        "agent",
        "windows-x86_64-nsis-perMachine",
        "production",
        "stable-installation",
        &"2".repeat(64),
    )
    .unwrap();
    assert_eq!(
        BootstrapAnchor::new(&wrong_product, &root, root.len() as u64, &sha256(&root)).err(),
        Some(Error::InvalidAnchor)
    );
    assert_eq!(anchor(&[]).err(), Some(Error::RootPinMismatch));
    assert_eq!(
        anchor(&vec![b'x'; ROOT_MAXIMUM + 1]).err(),
        Some(Error::RootPinMismatch)
    );
}

#[test]
fn existing_records_and_original_root_are_immutable_and_scope_bound() {
    let root = signed_root(7, 1000, 2000, 2);
    let expected = anchor(&root).unwrap();
    let before = expected.record_bytes().to_vec();
    let successor = signed_root(8, 1000, 2000, 2);
    assert_eq!(
        expected.check_existing(&before, &successor),
        Err(Error::ExistingStateMismatch)
    );
    let changed = anchor(&successor).unwrap();
    assert!(changed.check_existing(&before, &root).is_err());
    for bad in [Vec::new(), b"{}".to_vec(), vec![b' '; RECORD_MAXIMUM + 1]] {
        assert!(expected.check_existing(&bad, &root).is_err());
    }
    let mut record: Value = serde_json::from_slice(&before).unwrap();
    record["binding"]["installationScopeId"] = "3".repeat(64).into();
    assert!(expected.check_existing(&canonical(&record), &root).is_err());
    record = serde_json::from_slice(&before).unwrap();
    record["currentQualification"] = true.into();
    assert!(expected.check_existing(&canonical(&record), &root).is_err());
    assert!(
        BootstrapAnchor::parse(
            &before,
            &root,
            &binding(&"3".repeat(64)),
            root.len() as u64,
            &sha256(&root)
        )
        .is_err()
    );
    assert_eq!(expected.record_bytes(), before);
}

#[test]
fn equal_root_bytes_cannot_borrow_another_bootstrap_binding() {
    // Real signature/parser DATA controls, not a registry/SCM Source fixture.
    // The same product-global Root legitimately appears under many component
    // bindings; only the exact original record may be accepted for a binding.
    let root = signed_root(7, 1000, 2000, 2);
    let expected_binding = binding(&"1".repeat(64));
    let original =
        BootstrapAnchor::new(&expected_binding, &root, root.len() as u64, &sha256(&root)).unwrap();
    for index in 0..6 {
        let scope = if index == 0 {
            "3".repeat(64)
        } else {
            "1".repeat(64)
        };
        let seed = if index == 5 {
            "4".repeat(64)
        } else {
            "2".repeat(64)
        };
        let other_binding = BootstrapBinding::new(
            &scope,
            "pinvou",
            if index == 1 { "plugin" } else { "agent" },
            if index == 2 {
                "linux-x86_64-deb-perMachine"
            } else {
                "windows-x86_64-nsis-perMachine"
            },
            if index == 3 { "lab" } else { "production" },
            if index == 4 {
                "another-installation"
            } else {
                "stable-installation"
            },
            &seed,
        )
        .unwrap();
        let other =
            BootstrapAnchor::new(&other_binding, &root, root.len() as u64, &sha256(&root)).unwrap();
        assert_eq!(other.root_bytes(), root);
        assert_eq!(
            BootstrapAnchor::parse(
                other.record_bytes(),
                &root,
                &expected_binding,
                root.len() as u64,
                &sha256(&root)
            )
            .err(),
            Some(Error::ExistingStateMismatch),
            "binding case {index}"
        );
        assert_eq!(
            BootstrapAnchor::parse(
                original.record_bytes(),
                &root,
                &other_binding,
                root.len() as u64,
                &sha256(&root)
            )
            .err(),
            Some(Error::ExistingStateMismatch),
            "reverse binding case {index}"
        );
    }
    // Missing/partial storage payloads cannot be interpreted as initial genesis.
    for missing in [&[][..], &b"{}"[..]] {
        assert!(
            BootstrapAnchor::parse(
                missing,
                &root,
                &expected_binding,
                root.len() as u64,
                &sha256(&root)
            )
            .is_err()
        );
        assert!(
            BootstrapAnchor::parse(
                original.record_bytes(),
                missing,
                &expected_binding,
                root.len() as u64,
                &sha256(&root)
            )
            .is_err()
        );
    }
}
