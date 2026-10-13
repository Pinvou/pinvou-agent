use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pinvou_upgrade_codec::{BindingError, Document, Error, InputKind, key_descriptor};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(7)
        .unwrap()
        .to_owned()
}
fn document(input: &[u8]) -> Document {
    Document::parse(input, InputKind::Metadata).unwrap()
}
fn base() -> Value {
    serde_json::json!({"protocolVersion":1,"role":"root","product":"pinvou",
        "component":null,"scope":{},"unknown":{"content":"bound"}})
}
fn input(value: &Value) -> Vec<u8> {
    document(&serde_jcs::to_vec(value).unwrap())
        .signing_input()
        .unwrap()
        .as_bytes()
        .to_vec()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn original_twenty_signing_inputs_and_public_descriptor_ids() {
    let vectors: Value = serde_json::from_slice(
        &fs::read(root().join("specs/upgrade/v1/vectors/crypto/envelopes.json")).unwrap(),
    )
    .unwrap();
    let entries = vectors["vectors"].as_array().unwrap();
    assert_eq!(entries.len(), 20);
    for vector in entries {
        let wire = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let envelope = document(&wire);
        assert!(
            hex(&input(&envelope.value()["signed"]))
                == vector["expectedSigningInputHex"].as_str().unwrap()
        );
    }
    let keys = vectors["trustedRoot"]["keys"].as_array().unwrap();
    assert_eq!(keys.len(), 4);
    for key in keys {
        let public_key = key["publicKey"].as_str().unwrap();
        let descriptor = key_descriptor(public_key).unwrap();
        assert_eq!(descriptor.public_key(), public_key);
        assert_eq!(descriptor.key_id(), key["keyId"].as_str().unwrap());
    }
}

#[test]
fn complete_signed_object_and_required_bindings_are_preserved() {
    let value = base();
    let bytes = input(&value);
    assert_eq!(&bytes[..25], b"pinvou-upgrade-signed-v1\0");
    assert_ne!(&bytes[..25], b"pinvou-upgrade-signed-v1\\");
    let wrapper: Value = serde_json::from_slice(&bytes[25..]).unwrap();
    assert_eq!(wrapper["signed"], value);
    assert!(wrapper["component"].is_null());
    for field in ["protocolVersion", "role", "product", "component", "scope"] {
        assert_eq!(wrapper[field], value[field]);
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(matches!(
            document(&serde_jcs::to_vec(&missing).unwrap()).signing_input(),
            Err(BindingError::MissingClaims)
        ));
        let mut changed = value.clone();
        changed[field] = serde_json::json!({"different":true});
        let changed_bytes = input(&changed);
        assert_ne!(bytes, changed_bytes);
        let changed_wrapper: Value = serde_json::from_slice(&changed_bytes[25..]).unwrap();
        assert_eq!(changed_wrapper[field], changed[field]);
    }
    let mut changed = value.clone();
    changed["unknown"]["content"] = Value::String("changed".into());
    assert_ne!(bytes, input(&changed));
    for wire in [b"null".as_slice(), b"[]", b"{}", b"1"] {
        assert!(matches!(
            document(wire).signing_input(),
            Err(BindingError::MissingClaims)
        ));
    }
    let envelope = serde_json::json!({"signed":value,"signatures":[]});
    assert!(matches!(
        document(&serde_jcs::to_vec(&envelope).unwrap()).signing_input(),
        Err(BindingError::MissingClaims)
    ));
    assert_eq!(
        BindingError::MissingClaims.to_string(),
        "SIGNED_BINDING_FIELDS_REQUIRED"
    );
}

#[test]
fn wrapper_depth_is_rechecked_including_duplicated_scope() {
    for field in ["unknown", "scope"] {
        for (arrays, accepted) in [(30, true), (31, false)] {
            let nested = format!("{}0{}", "[".repeat(arrays), "]".repeat(arrays));
            let mut value = base();
            value[field] = serde_json::from_str(&nested).unwrap();
            let parsed = document(&serde_jcs::to_vec(&value).unwrap());
            assert!(parsed.canonical_bytes().is_ok());
            let result = parsed.signing_input();
            if accepted {
                assert!(result.is_ok());
            } else {
                assert!(matches!(result, Err(BindingError::Json(Error::DepthLimit))));
            }
        }
    }
}

fn boundary_input(extra: usize) -> Vec<u8> {
    // Binary64 zero would serialize as 0.0 through ordinary serde_json. Direct
    // JCS must accept the exact output boundary without that larger intermediary.
    let groups = (0..500000)
        .collect::<Vec<_>>()
        .chunks(4096)
        .map(|group| format!("[{}]", vec!["0"; group.len()].join(",")))
        .collect::<Vec<_>>()
        .join(",");
    let build = |filler: usize| {
        format!("{{\"protocolVersion\":1,\"role\":\"r\",\"product\":\"p\",\"component\":null,\"scope\":{{}},\"rows\":[{groups}],\"padding\":\"{}\"}}","a".repeat(filler)).into_bytes()
    };
    let empty = build(0);
    let canonical_size = document(&empty).signing_input().unwrap().as_bytes().len() - 25;
    let filler = 1048576 - canonical_size;
    assert!(filler + extra <= 65536);
    build(filler + extra)
}

#[test]
fn wrapper_canonical_payload_limit_excludes_fixed_domain_bytes() {
    let exact = document(&boundary_input(0));
    assert!(exact.canonical_bytes().unwrap().len() < 1048576);
    let bytes = exact.signing_input().unwrap();
    assert_eq!(bytes.as_bytes().len(), 1048576 + 25);
    assert_eq!(&bytes.as_bytes()[..25], b"pinvou-upgrade-signed-v1\0");
    let excess = document(&boundary_input(1));
    assert!(excess.canonical_bytes().is_ok());
    assert!(matches!(
        excess.signing_input(),
        Err(BindingError::Json(Error::OutputLimit))
    ));
}

#[test]
fn descriptor_encoding_is_canonical_and_does_not_claim_point_validity() {
    for bytes in [[0u8; 32], [255u8; 32], {
        let mut a = [0u8; 32];
        a[0] = 1;
        a
    }] {
        let encoded = URL_SAFE_NO_PAD.encode(bytes);
        let descriptor = key_descriptor(&encoded).unwrap();
        assert_eq!(descriptor.public_key(), encoded);
        assert_eq!(descriptor.key_id().len(), 64);
        assert!(
            descriptor
                .key_id()
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        );
    }
    let canonical = URL_SAFE_NO_PAD.encode([0u8; 32]);
    let mut bad_tail = canonical.clone();
    bad_tail.pop();
    bad_tail.push('B');
    for malformed in [
        String::new(),
        format!("{canonical}="),
        format!("{canonical} "),
        format!("+{}", &canonical[1..]),
        format!("/{}", &canonical[1..]),
        format!("é{}", &canonical[2..]),
        URL_SAFE_NO_PAD.encode([0u8; 31]),
        URL_SAFE_NO_PAD.encode([0u8; 33]),
        bad_tail,
    ] {
        assert!(matches!(
            key_descriptor(&malformed),
            Err(BindingError::InvalidKeyEncoding)
        ));
    }
    assert_eq!(
        BindingError::InvalidKeyEncoding.to_string(),
        "KEY_ENCODING_INVALID"
    );
}

#[test]
fn original_node_signing_and_descriptor_oracle() {
    use base64::engine::general_purpose::STANDARD;
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let Ok(reference) = std::env::var("PINVOU_UPGRADE_SIGNATURE_REFERENCE") else {
        eprintln!("original Node signature oracle not configured; no oracle verification claimed");
        return;
    };
    let vectors: Value = serde_json::from_slice(
        &fs::read(root().join("specs/upgrade/v1/vectors/crypto/envelopes.json")).unwrap(),
    )
    .unwrap();
    let mut wires = Vec::new();
    for vector in vectors["vectors"].as_array().unwrap() {
        let wire = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        wires.push(serde_jcs::to_vec(&document(&wire).value()["signed"]).unwrap());
    }
    wires.push(serde_jcs::to_vec(&base()).unwrap());
    for field in ["protocolVersion", "role", "product", "component", "scope"] {
        let mut missing = base();
        missing.as_object_mut().unwrap().remove(field);
        wires.push(serde_jcs::to_vec(&missing).unwrap());
    }
    for arrays in [30, 31] {
        let nested = format!("{}0{}", "[".repeat(arrays), "]".repeat(arrays));
        let mut value = base();
        value["scope"] = serde_json::from_str(&nested).unwrap();
        wires.push(serde_jcs::to_vec(&value).unwrap());
    }
    wires.push(boundary_input(0));
    wires.push(boundary_input(1));
    let mut keys = vectors["trustedRoot"]["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| key["publicKey"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    keys.push(URL_SAFE_NO_PAD.encode([0u8; 32]));
    let canonical = URL_SAFE_NO_PAD.encode([0u8; 32]);
    let mut bad_tail = canonical.clone();
    bad_tail.pop();
    bad_tail.push('B');
    keys.extend([
        String::new(),
        format!("{canonical}="),
        format!("{canonical} "),
        format!("+{}", &canonical[1..]),
        format!("/{}", &canonical[1..]),
        format!("é{}", &canonical[2..]),
        URL_SAFE_NO_PAD.encode([0u8; 31]),
        URL_SAFE_NO_PAD.encode([0u8; 33]),
        bad_tail,
    ]);
    let mut samples = wires
        .iter()
        .map(|wire| serde_json::json!({"signed":STANDARD.encode(wire)}))
        .collect::<Vec<_>>();
    samples.extend(keys.iter().map(|key| serde_json::json!({"publicKey":key})));
    let expected = wires
        .iter()
        .map(|wire| {
            document(wire)
                .signing_input()
                .ok()
                .map(|input| STANDARD.encode(input.as_bytes()))
        })
        .chain(keys.iter().map(|key| {
            key_descriptor(key)
                .ok()
                .map(|descriptor| descriptor.key_id().to_owned())
        }))
        .collect::<Vec<_>>();
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("signing-oracle.mjs"),
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
        .write_all(
            &serde_json::to_vec(&serde_json::json!({
        "samples":samples,"root":vectors["trustedRoot"]}))
            .unwrap(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "original signature oracle unavailable"
    );
    let actual: Vec<Option<String>> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
        assert!(
            actual == expected,
            "original signing/descriptor oracle mismatch at case {index}"
        );
    }
}
