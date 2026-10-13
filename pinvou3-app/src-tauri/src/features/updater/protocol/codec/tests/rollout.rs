use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{Document, Error, InputKind, RolloutCommitmentError};
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
    fixture()["opening"].clone()
}
fn wire(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn document(bytes: &[u8]) -> Document {
    Document::parse(bytes, InputKind::Metadata).unwrap()
}
fn hash(value: &Value) -> String {
    document(&wire(value))
        .rollout_commitment()
        .unwrap()
        .commitment_hash()
        .to_owned()
}
#[test]
fn frozen_empty_and_single_opening_match_exact_fixed_array_preimages() {
    let vector = fixture();
    let empty = document(b"null").rollout_commitment().unwrap();
    assert_eq!(empty.input_bytes(), b"pinvou-rollout-set-v1\0[]");
    assert_eq!(
        empty.commitment_hash(),
        vector["expectedEmptyCommitment"].as_str().unwrap()
    );
    let single = document(&wire(&base())).rollout_commitment().unwrap();
    assert_eq!(
        single.commitment_hash(),
        vector["expectedOpeningCommitment"].as_str().unwrap()
    );
    let canonical = format!(
        r#"[{{"deploymentId":"deployment-2","deploymentRevision":1,"leafSalt":"{}","releaseEnvelopeSha256":"{}","rolloutId":"rollout-1","rolloutRevision":1,"targetKey":"linux-x86_64-deb-perMachine"}}]"#,
        "b".repeat(64),
        "a".repeat(64)
    );
    assert_eq!(&single.input_bytes()[..22], b"pinvou-rollout-set-v1\0");
    assert_eq!(&single.input_bytes()[22..], canonical.as_bytes());
    assert_eq!(
        document(&serde_json::to_vec_pretty(&base()).unwrap())
            .rollout_commitment()
            .unwrap()
            .input_bytes(),
        single.input_bytes()
    );
    let mut fields: Vec<_> = base()
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| format!("{}:{}", serde_json::to_string(key).unwrap(), value))
        .collect();
    fields.reverse();
    assert_eq!(
        document(format!("{{{}}}", fields.join(",")).as_bytes())
            .rollout_commitment()
            .unwrap()
            .input_bytes(),
        single.input_bytes()
    );
}
#[test]
fn all_seven_fields_bind_and_zero_salt_does_not_claim_entropy() {
    let original = hash(&base());
    for field in base().as_object().unwrap().keys() {
        let mut changed = base();
        changed[field] = match field.as_str() {
            "targetKey" => json!("windows-x86_64-nsis-perMachine"),
            "deploymentRevision" | "rolloutRevision" => json!(2),
            "releaseEnvelopeSha256" | "leafSalt" => json!("0".repeat(64)),
            _ => json!("different"),
        };
        assert_ne!(hash(&changed), original, "unbound field {field}");
    }
    let mut zero = base();
    zero["leafSalt"] = json!("0".repeat(64));
    assert!(document(&wire(&zero)).rollout_commitment().is_ok());
}
#[test]
fn null_is_not_missing_empty_object_or_caller_wrapped_array() {
    for value in [
        json!([]),
        json!({}),
        json!([base()]),
        json!([base(), base()]),
        json!(false),
        json!(0),
        json!("null"),
    ] {
        assert!(matches!(
            document(&wire(&value)).rollout_commitment(),
            Err(RolloutCommitmentError::InvalidOpening)
        ));
    }
    for field in base().as_object().unwrap().keys() {
        let mut missing = base();
        missing.as_object_mut().unwrap().remove(field);
        assert!(matches!(
            document(&wire(&missing)).rollout_commitment(),
            Err(RolloutCommitmentError::InvalidOpening)
        ));
        missing[field] = Value::Null;
        assert!(matches!(
            document(&wire(&missing)).rollout_commitment(),
            Err(RolloutCommitmentError::InvalidOpening)
        ));
    }
    let mut extra = base();
    extra["commitment"] = Value::Null;
    assert!(matches!(
        document(&wire(&extra)).rollout_commitment(),
        Err(RolloutCommitmentError::InvalidOpening)
    ));
    let raw = String::from_utf8(wire(&base())).unwrap();
    let duplicate = raw.replacen(
        "\"rolloutRevision\":1",
        "\"rolloutRevision\":1,\"rolloutRevi\\u0073ion\":2",
        1,
    );
    assert!(matches!(
        Document::parse(duplicate.as_bytes(), InputKind::Metadata),
        Err(Error::DuplicateKey)
    ));
    assert_eq!(
        RolloutCommitmentError::InvalidOpening.to_string(),
        "ROLLOUT_OPENING_INVALID"
    );
}
#[test]
fn original_node_ajv_matches_shape_binary64_preimage_and_hash() {
    let mut cases = vec![
        b"null".to_vec(),
        b"[]".to_vec(),
        b"{}".to_vec(),
        b"false".to_vec(),
        b"0".to_vec(),
        wire(&base()),
        wire(&json!([base()])),
    ];
    for field in base().as_object().unwrap().keys() {
        let mut missing = base();
        missing.as_object_mut().unwrap().remove(field);
        cases.push(wire(&missing));
        for value in [Value::Null, json!(1), json!(false), json!([]), json!({})] {
            let mut changed = base();
            changed[field] = value;
            cases.push(wire(&changed));
        }
    }
    for field in ["deploymentId", "rolloutId"] {
        for text in [
            "",
            "a",
            "A._:-9",
            "_bad",
            "a b",
            "\u{00e9}",
            &"a".repeat(128),
            &"a".repeat(129),
        ] {
            let mut changed = base();
            changed[field] = json!(text);
            cases.push(wire(&changed));
        }
    }
    for field in ["releaseEnvelopeSha256", "leafSalt"] {
        for text in [
            "a".repeat(63),
            "A".repeat(64),
            "g".repeat(64),
            "0".repeat(64),
            "a".repeat(65),
        ] {
            let mut changed = base();
            changed[field] = json!(text);
            cases.push(wire(&changed));
        }
    }
    for ending in ["\n", "\r", "\r\n", "\u{2028}", "\u{2029}"] {
        for field in ["deploymentId", "leafSalt", "targetKey"] {
            let mut changed = base();
            changed[field] = json!(format!("{}{ending}", changed[field].as_str().unwrap()));
            cases.push(wire(&changed));
        }
    }
    let schema: Value = serde_json::from_slice(
        &fs::read(root().join("specs/upgrade/v1/schemas/common.json")).unwrap(),
    )
    .unwrap();
    for target in schema["$defs"]["opening"]["properties"]["targetKey"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .cloned()
        .chain([
            json!("linux-x86_64-deb"),
            json!("Linux-x86_64-deb-perMachine"),
            json!(""),
        ])
    {
        let mut changed = base();
        changed["targetKey"] = target;
        cases.push(wire(&changed));
    }
    let raw = String::from_utf8(wire(&base())).unwrap();
    for field in ["deploymentRevision", "rolloutRevision"] {
        for number in [
            "-0",
            "0",
            "-1",
            "1",
            "1.0",
            "1e0",
            "0.5",
            "1.5",
            "9007199254740991",
            "9007199254740992",
            "9007199254740990.5",
            "9007199254740991.1",
            "9007199254740991.5",
            "1.0000000000000001",
            "1e309",
        ] {
            cases.push(
                raw.replace(&format!("\"{field}\":1"), &format!("\"{field}\":{number}"))
                    .into_bytes(),
            );
        }
    }
    for field in ["headers", "requestId", "rolloutCommitment", "extra"] {
        let mut changed = base();
        changed[field] = Value::Null;
        cases.push(wire(&changed));
    }
    cases.push(
        raw.replacen(
            "\"rolloutRevision\":1",
            "\"rolloutRevision\":1,\"rolloutRevi\\u0073ion\":2",
            1,
        )
        .into_bytes(),
    );
    let samples: Vec<_> = cases.iter().map(|bytes| STANDARD.encode(bytes)).collect();
    let expected: Vec<Value> = cases
        .iter()
        .map(|bytes| {
            Document::parse(bytes, InputKind::Metadata)
                .ok()
                .and_then(|doc| doc.rollout_commitment().ok())
                .map(|result| {
                    json!([
                        result.commitment_hash(),
                        STANDARD.encode(result.input_bytes())
                    ])
                })
                .unwrap_or(Value::Null)
        })
        .collect();
    let reference = root()
        .join("specs/upgrade/v1/reference/digests.mjs")
        .to_string_lossy()
        .into_owned();
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("rollout-oracle.mjs"),
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
        "original rollout oracle unavailable"
    );
    let actual: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        assert!(
            actual == expected,
            "rollout oracle mismatch at case {index}"
        );
    }
    eprintln!("{} original Node/Ajv rollout cases passed", expected.len());
}
