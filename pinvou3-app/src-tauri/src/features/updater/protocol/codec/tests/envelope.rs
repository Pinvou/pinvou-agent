use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pinvou_upgrade_codec::{Document, EnvelopeShapeError, InputKind, decode_canonical_envelope};
use serde_json::{Value, json};
use std::{
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
fn fixtures() -> Vec<Value> {
    serde_json::from_slice::<Value>(
        &std::fs::read(root().join("specs/upgrade/v1/vectors/crypto/envelopes.json")).unwrap(),
    )
    .unwrap()["vectors"]
        .as_array()
        .unwrap()
        .clone()
}
fn kind(role: &str) -> InputKind {
    if [
        "root",
        "timestamp",
        "snapshot",
        "target",
        "release",
        "package",
    ]
    .contains(&role)
    {
        InputKind::Metadata
    } else {
        InputKind::Credential
    }
}
fn canonical(value: &Value) -> Vec<u8> {
    Document::parse(&serde_json::to_vec(value).unwrap(), InputKind::Metadata)
        .unwrap()
        .canonical_bytes()
        .unwrap()
}
fn minimal(role: &str) -> Value {
    json!({"signed":{"protocolVersion":1,"role":role,"product":"pinvou","component":if role=="root" {Value::Null} else {json!("app")},"scope":{}},
        "signatures":[{"keyId":"0".repeat(64),"signature":URL_SAFE_NO_PAD.encode([0;64])}]})
}
#[test]
fn frozen_envelopes_retain_original_bytes_and_typed_signatures_without_verification() {
    for vector in fixtures() {
        let raw = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let role = vector["name"].as_str().unwrap();
        let envelope = decode_canonical_envelope(&raw, kind(role)).unwrap();
        let value: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(envelope.raw_bytes(), raw);
        assert_eq!(
            envelope.signed().canonical_bytes().unwrap(),
            canonical(&value["signed"])
        );
        for (record, expected) in envelope
            .signatures()
            .iter()
            .zip(value["signatures"].as_array().unwrap())
        {
            assert_eq!(record.key_id(), expected["keyId"].as_str().unwrap());
            assert_eq!(
                record.as_bytes().as_slice(),
                URL_SAFE_NO_PAD
                    .decode(expected["signature"].as_str().unwrap())
                    .unwrap()
            );
        }
        assert!(matches!(
            decode_canonical_envelope(
                &raw,
                if kind(role) == InputKind::Metadata {
                    InputKind::Credential
                } else {
                    InputKind::Metadata
                }
            ),
            Err(EnvelopeShapeError::WrongProfile)
        ));
    }
    // Minimal role-specific-invalid bodies and zero/weak signature bytes are
    // positive shape facts only, never accepted cryptographic envelopes.
    for role in ["root", "target", "decision"] {
        assert!(decode_canonical_envelope(&canonical(&minimal(role)), kind(role)).is_ok());
    }
    let original = minimal("root");
    for field in ["protocolVersion", "role", "product", "component", "scope"] {
        let mut missing = original.clone();
        missing["signed"].as_object_mut().unwrap().remove(field);
        assert!(matches!(
            decode_canonical_envelope(&canonical(&missing), InputKind::Metadata),
            Err(EnvelopeShapeError::InvalidContext)
        ));
    }
    for (field, value) in [
        ("protocolVersion", json!(2)),
        ("role", json!("unknown")),
        ("product", json!("invalid/")),
        ("component", json!("app")),
        ("scope", json!([])),
    ] {
        let mut changed = original.clone();
        changed["signed"][field] = value;
        assert!(matches!(
            decode_canonical_envelope(&canonical(&changed), InputKind::Metadata),
            Err(EnvelopeShapeError::InvalidContext)
        ));
    }
    for component in [Value::Null, json!(""), json!(7)] {
        let mut changed = minimal("target");
        changed["signed"]["component"] = component;
        assert!(decode_canonical_envelope(&canonical(&changed), InputKind::Metadata).is_err());
    }
}
#[test]
fn original_canonical_spelling_and_complete_raw_bounds_are_enforced() {
    let value = minimal("root");
    let bytes = canonical(&value);
    let mut newline = bytes.clone();
    newline.push(b'\n');
    let original = String::from_utf8(bytes.clone()).unwrap();
    for raw in [
        serde_json::to_vec_pretty(&value).unwrap(),
        newline,
        original
            .replace("\"protocolVersion\":1", "\"protocolVersion\":1.0")
            .into_bytes(),
        original.replace("pinvou", "pinv\\u006fu").into_bytes(),
        format!(
            "{{\"signed\":{},\"signatures\":{}}}",
            String::from_utf8(canonical(&value["signed"])).unwrap(),
            String::from_utf8(canonical(&value["signatures"])).unwrap()
        )
        .into_bytes(),
    ] {
        assert!(matches!(
            decode_canonical_envelope(&raw, InputKind::Metadata),
            Err(EnvelopeShapeError::NotCanonical)
        ));
    }
    let duplicate = original.replace(
        "\"product\":\"pinvou\"",
        "\"product\":\"pinvou\",\"prod\\u0075ct\":\"pinvou\"",
    );
    assert!(matches!(
        decode_canonical_envelope(duplicate.as_bytes(), InputKind::Metadata),
        Err(EnvelopeShapeError::Json(
            pinvou_upgrade_codec::Error::DuplicateKey
        ))
    ));
    assert!(matches!(
        decode_canonical_envelope(b"\xff", InputKind::Metadata),
        Err(EnvelopeShapeError::Json(
            pinvou_upgrade_codec::Error::InvalidUtf8
        ))
    ));
    let mut oversized = value.clone();
    oversized["signed"]["ignored"] = json!("a".repeat(65537));
    assert!(matches!(
        decode_canonical_envelope(
            &serde_json::to_vec(&oversized).unwrap(),
            InputKind::Metadata
        ),
        Err(EnvelopeShapeError::Json(
            pinvou_upgrade_codec::Error::StringLimit
        ))
    ));
    for (role, maximum) in [("root", 1048576), ("decision", 65536)] {
        let mut value = minimal(role);
        // Start below the limit, then measure exact punctuation before padding.
        value["signed"]["padding"] = json!([]);
        while maximum - canonical(&value).len() > 65539 {
            value["signed"]["padding"]
                .as_array_mut()
                .unwrap()
                .push(json!("a".repeat(65500)));
        }
        value["signed"]["padding"]
            .as_array_mut()
            .unwrap()
            .push(json!(""));
        let mut raw = canonical(&value);
        // Account for the exact array/member punctuation measured on the wire.
        let delta = maximum - raw.len();
        let last = value["signed"]["padding"]
            .as_array_mut()
            .unwrap()
            .last_mut()
            .unwrap();
        *last = json!(format!("{}{}", last.as_str().unwrap(), "a".repeat(delta)));
        raw = canonical(&value);
        assert_eq!(raw.len(), maximum);
        assert!(decode_canonical_envelope(&raw, kind(role)).is_ok());
        raw.push(b' ');
        assert!(matches!(
            decode_canonical_envelope(&raw, kind(role)),
            Err(EnvelopeShapeError::Json(
                pinvou_upgrade_codec::Error::InputLimit
            ))
        ));
    }
    let mut deeply = minimal("root");
    let mut nested = json!(null);
    for _ in 0..33 {
        nested = json!([nested]);
    }
    deeply["signed"]["ignored"] = nested;
    assert!(matches!(
        decode_canonical_envelope(&serde_json::to_vec(&deeply).unwrap(), InputKind::Metadata),
        Err(EnvelopeShapeError::Json(
            pinvou_upgrade_codec::Error::DepthLimit
        ))
    ));
}
#[test]
fn signatures_require_strict_original_order_and_full_container_bounds() {
    let mut value = minimal("root");
    value["signatures"]=json!((0..4096).map(|index|json!({"keyId":format!("{index:064x}"),"signature":URL_SAFE_NO_PAD.encode([0;64])})).collect::<Vec<_>>());
    let bytes = canonical(&value);
    let envelope = decode_canonical_envelope(&bytes, InputKind::Metadata).unwrap();
    assert_eq!(envelope.signatures().len(), 4096);
    assert_eq!(
        envelope.signatures()[4095].key_id(),
        format!("{:064x}", 4095)
    );
    let first = value["signatures"][0].clone();
    value["signatures"].as_array_mut().unwrap().push(first);
    assert!(matches!(
        decode_canonical_envelope(&serde_json::to_vec(&value).unwrap(), InputKind::Metadata),
        Err(EnvelopeShapeError::Json(
            pinvou_upgrade_codec::Error::MemberLimit
        ))
    ));
}
#[test]
fn original_node_parser_and_encoding_helpers_match_unchanged_valid_body_domain() {
    let mut cases = Vec::new();
    for vector in fixtures() {
        let raw = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let envelope: Value = serde_json::from_slice(&raw).unwrap();
        let expected = vector["expectedContext"].clone();
        let mut push =
            |raw: Vec<u8>| cases.push(json!({"raw":STANDARD.encode(raw),"expected":expected}));
        push(raw.clone());
        let mut trailing = raw.clone();
        trailing.push(b'\n');
        push(trailing);
        push(serde_json::to_vec_pretty(&envelope).unwrap());
        for signatures in [
            json!([]),
            json!(null),
            json!({}),
            json!([{}]),
            json!([null]),
        ] {
            let mut changed = envelope.clone();
            changed["signatures"] = signatures;
            push(canonical(&changed));
        }
        let mut changed = envelope.clone();
        changed["extra"] = json!(true);
        push(canonical(&changed));
        let mut changed = envelope.clone();
        changed.as_object_mut().unwrap().remove("signatures");
        push(canonical(&changed));
        for field in ["keyId", "signature"] {
            let mut changed = envelope.clone();
            changed["signatures"][0]
                .as_object_mut()
                .unwrap()
                .remove(field);
            push(canonical(&changed));
            for wrong in [
                Value::Null,
                json!(1),
                json!(""),
                json!(" "),
                json!("A".repeat(64)),
                json!("0".repeat(63)),
                json!("0".repeat(65)),
                json!("a".repeat(86) + "="),
                json!("/".repeat(86)),
                json!("a".repeat(85)),
                json!("a".repeat(87)),
            ] {
                let mut changed = envelope.clone();
                changed["signatures"][0][field] = wrong;
                push(canonical(&changed));
            }
        }
        let mut changed = envelope.clone();
        changed["signatures"][0]["extra"] = json!(0);
        push(canonical(&changed));
        let first = envelope["signatures"][0].clone();
        let mut second = first.clone();
        second["keyId"] = json!("f".repeat(64));
        for records in [
            json!([first.clone(), second.clone()]),
            json!([second, first.clone()]),
            json!([first.clone(), first]),
        ] {
            let mut changed = envelope.clone();
            changed["signatures"] = records;
            push(canonical(&changed));
        }
        // All possible six-bit final symbols exercise the four legal pad-bit
        // values and sixty noncanonical alternatives for a 64-byte signature.
        let prefix = "A".repeat(85);
        for last in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_" {
            let mut changed = envelope.clone();
            changed["signatures"][0]["signature"] = json!(format!("{prefix}{}", *last as char));
            push(canonical(&changed));
        }
        let text = String::from_utf8(raw).unwrap();
        push(text.replacen("\"keyId\":", "\"keyId\":\"", 1).into_bytes());
        push(
            text.replacen(
                "\"signatures\":",
                "\"signatures\":[],\"signat\\u0075res\":",
                1,
            )
            .into_bytes(),
        );
    }
    let expected: Vec<bool> = cases
        .iter()
        .map(|case| {
            let raw = STANDARD.decode(case["raw"].as_str().unwrap()).unwrap();
            decode_canonical_envelope(&raw, kind(case["expected"]["role"].as_str().unwrap()))
                .is_ok()
        })
        .collect();
    let reference = root().join("specs/upgrade/v1/reference/signatures.mjs");
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("envelope-oracle.mjs"),
            reference.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&cases).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "original envelope oracle unavailable"
    );
    let actual: Vec<bool> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (a, b)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_eq!(a, b, "case {index}");
    }
    eprintln!(
        "{} original Node envelope+encoding cases passed",
        cases.len()
    );
}
