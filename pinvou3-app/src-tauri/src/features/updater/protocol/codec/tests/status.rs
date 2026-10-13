use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{Document, Error, InputKind, StatusBindingError, recovery_handle_hash};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(7)
        .unwrap()
        .to_owned()
}
fn fixture() -> Value {
    serde_json::from_slice(
        &fs::read(root().join("specs/upgrade/v1/vectors/crypto/digests.json")).unwrap(),
    )
    .unwrap()
}
fn base() -> Value {
    fixture()["statusBinding"].clone()
}
fn wire(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn document(bytes: &[u8]) -> Document {
    Document::parse(bytes, InputKind::Metadata).unwrap()
}
fn hash(value: &Value) -> String {
    document(&wire(value))
        .status_binding()
        .unwrap()
        .binding_hash()
        .to_owned()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
#[test]
fn frozen_status_binding_has_exact_domain_nul_input_and_hash() {
    let vector = fixture();
    let binding = document(&wire(&base())).status_binding().unwrap();
    assert_eq!(
        hex(binding.input_bytes()),
        vector["expectedStatusInputHex"].as_str().unwrap()
    );
    assert_eq!(
        binding.binding_hash(),
        vector["expectedStatusBindingHash"].as_str().unwrap()
    );
    assert_ne!(
        binding.binding_hash(),
        vector["expectedEscapedNulHash"].as_str().unwrap()
    );
    assert_eq!(&binding.input_bytes()[..25], b"pinvou-consume-status-v1\0");
    let pretty = serde_json::to_vec_pretty(&base()).unwrap();
    assert_eq!(
        document(&pretty).status_binding().unwrap().binding_hash(),
        binding.binding_hash()
    );
    let mut fields: Vec<_> = base()
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    fields.reverse();
    let reordered = fields
        .iter()
        .map(|(k, v)| {
            format!(
                "{}:{}",
                serde_json::to_string(k).unwrap(),
                serde_json::to_string(v).unwrap()
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(
        document(format!("{{{reordered}}}").as_bytes())
            .status_binding()
            .unwrap()
            .input_bytes(),
        binding.input_bytes()
    );
}
#[test]
fn every_original_binding_field_changes_the_hash_and_all_three_purposes_are_valid() {
    let original = hash(&base());
    for field in base().as_object().unwrap().keys() {
        let mut changed = base();
        changed[field] = if field.ends_with("Hash") || field.ends_with("Digest") {
            json!("f".repeat(64))
        } else if field == "purpose" {
            json!("activate")
        } else {
            json!("different")
        };
        assert_ne!(hash(&changed), original, "unbound field {field}");
    }
    for purpose in ["preinstall", "install", "activate"] {
        let mut changed = base();
        changed["purpose"] = json!(purpose);
        assert!(document(&wire(&changed)).status_binding().is_ok());
    }
}
#[test]
fn status_root_is_strictly_closed_with_no_missing_null_or_transport_defaults() {
    for field in base().as_object().unwrap().keys() {
        let mut missing = base();
        missing.as_object_mut().unwrap().remove(field);
        assert!(matches!(
            document(&wire(&missing)).status_binding(),
            Err(StatusBindingError::InvalidBinding)
        ));
        for value in [Value::Null, json!(1), json!(false), json!([]), json!({})] {
            let mut changed = base();
            changed[field] = value;
            assert!(matches!(
                document(&wire(&changed)).status_binding(),
                Err(StatusBindingError::InvalidBinding)
            ));
        }
    }
    for field in [
        "headers",
        "requestId",
        "recoverySecret",
        "statusBindingHash",
        "arbitrary",
    ] {
        let mut changed = base();
        changed[field] = Value::Null;
        assert!(matches!(
            document(&wire(&changed)).status_binding(),
            Err(StatusBindingError::InvalidBinding)
        ));
    }
    let raw = String::from_utf8(wire(&base())).unwrap();
    let duplicate = raw.replacen(
        "\"purpose\":\"install\"",
        "\"purpose\":\"install\",\"pur\\u0070ose\":\"activate\"",
        1,
    );
    assert!(matches!(
        Document::parse(duplicate.as_bytes(), InputKind::Metadata),
        Err(Error::DuplicateKey)
    ));
}
#[test]
fn recovery_hash_uses_only_the_exact_borrowed_raw_32_bytes() {
    let secret = [1_u8; 32];
    let result = recovery_handle_hash(&secret).unwrap();
    assert_eq!(
        result.as_hex(),
        fixture()["consumeRequest"]["recoveryHandleHash"]
            .as_str()
            .unwrap()
    );
    assert_eq!(secret, [1_u8; 32]);
    assert_ne!(
        recovery_handle_hash(&[0_u8; 32]).unwrap().as_hex(),
        result.as_hex()
    );
    for length in [0, 1, 31, 33, 64] {
        assert!(matches!(
            recovery_handle_hash(&vec![1; length]),
            Err(StatusBindingError::InvalidSecretLength)
        ));
    }
    assert!(matches!(
        recovery_handle_hash(STANDARD.encode(secret).as_bytes()),
        Err(StatusBindingError::InvalidSecretLength)
    ));
    assert_eq!(
        StatusBindingError::InvalidSecretLength.to_string(),
        "RECOVERY_SECRET_SIZE_INVALID"
    );
    // All-zero bytes are length-valid; this function certifies no entropy.
    assert!(recovery_handle_hash(&[0_u8; 32]).is_ok());
}
#[test]
fn actual_node_reference_agrees_on_binding_shape_preimage_hash_and_secret_lengths() {
    let mut bindings = vec![wire(&base())];
    for field in base().as_object().unwrap().keys() {
        let mut missing = base();
        missing.as_object_mut().unwrap().remove(field);
        bindings.push(wire(&missing));
        for value in [Value::Null, json!(1), json!(false), json!([]), json!({})] {
            let mut changed = base();
            changed[field] = value;
            bindings.push(wire(&changed));
        }
    }
    for purpose in [
        "preinstall",
        "install",
        "activate",
        "cleanup",
        "reconcile",
        "Install",
        "",
    ] {
        let mut changed = base();
        changed["purpose"] = json!(purpose);
        bindings.push(wire(&changed));
    }
    for value in [
        "",
        "a",
        "A._:-9",
        "_bad",
        "a b",
        "\u{00e9}",
        &"a".repeat(128),
        &"a".repeat(129),
    ] {
        for field in [
            "product",
            "component",
            "installationScopeId",
            "authorizationJti",
            "consumeKey",
            "transactionId",
        ] {
            let mut changed = base();
            changed[field] = json!(value);
            bindings.push(wire(&changed));
        }
    }
    for value in [
        "a".repeat(63),
        "A".repeat(64),
        "g".repeat(64),
        "0".repeat(64),
        "a".repeat(65),
    ] {
        for field in ["recoveryHandleHash", "consumeRequestDigest"] {
            let mut changed = base();
            changed[field] = json!(value);
            bindings.push(wire(&changed));
        }
    }
    for ending in ["\n", "\r", "\r\n", "\u{2028}", "\u{2029}"] {
        for field in ["product", "consumeRequestDigest"] {
            let mut changed = base();
            changed[field] = json!(format!("{}{ending}", changed[field].as_str().unwrap()));
            bindings.push(wire(&changed));
        }
    }
    for field in [
        "headers",
        "requestId",
        "recoverySecret",
        "statusBindingHash",
        "extra",
    ] {
        let mut changed = base();
        changed[field] = json!("excluded-in-other-digests");
        bindings.push(wire(&changed));
    }
    let raw = String::from_utf8(wire(&base())).unwrap();
    bindings.push(format!("{},\"purpose\":\"activate\"}}", &raw[..raw.len() - 1]).into_bytes());
    bindings.push(b"null".to_vec());
    bindings.push(b"[]".to_vec());
    let mut samples: Vec<_> = bindings
        .iter()
        .map(|bytes| json!({"binding":STANDARD.encode(bytes)}))
        .collect();
    let mut expected: Vec<Value> = bindings
        .iter()
        .map(|bytes| {
            Document::parse(bytes, InputKind::Metadata)
                .ok()
                .and_then(|doc| doc.status_binding().ok())
                .map(|value| json!([value.binding_hash(), STANDARD.encode(value.input_bytes())]))
                .unwrap_or(Value::Null)
        })
        .collect();
    for length in [0, 1, 31, 32, 33, 64] {
        let secret = vec![1; length];
        samples.push(json!({"secret":STANDARD.encode(&secret)}));
        expected.push(
            recovery_handle_hash(&secret)
                .map(|hash| json!(hash.as_hex()))
                .unwrap_or(Value::Null),
        );
    }
    let reference = root()
        .join("specs/upgrade/v1/reference/digests.mjs")
        .to_string_lossy()
        .into_owned();
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("status-oracle.mjs"),
            &reference,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&samples).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "original status/secret oracle unavailable"
    );
    let actual: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        assert!(
            actual == expected,
            "status/secret oracle mismatch at case {index}"
        );
    }
    eprintln!(
        "{} original Node/Ajv status/secret cases passed",
        expected.len()
    );
}
