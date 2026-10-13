use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{ConsumeDigestError, Document, Error, InputKind};
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
    fixture()["consumeRequest"].clone()
}
fn full() -> Value {
    let mut value = base();
    value["backup"] = json!({"recordId":"backup-1","freezeEpoch":1,"sha256":"a".repeat(64),"dataScopeDigest":"a".repeat(64),"preparationEpoch":1});
    value["staged"] = json!({"preinstallTransactionId":"preinstall-1","stagedRevision":1,"slotIdentity":"slot-1","stagedAt":0,"stagedValidUntil":0});
    value["preinstallSlot"] = json!({"slotIdentity":"slot-1","slotRevision":1,"state":"inactive"});
    let plan = value["plans"]["installation"].clone();
    for name in [
        "migration",
        "backup",
        "helperMigration",
        "launcherMigration",
        "takeover",
    ] {
        value["plans"][name] = plan.clone();
    }
    value
}
fn wire(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn document(bytes: &[u8]) -> Document {
    Document::parse(bytes, InputKind::Metadata).unwrap()
}
fn hash(value: &Value) -> String {
    document(&wire(value))
        .consume_request_digest()
        .unwrap()
        .request_digest()
        .to_owned()
}
fn leaves(value: &Value, path: &str, result: &mut Vec<String>) {
    if let Some(object) = value.as_object() {
        for (key, child) in object {
            leaves(child, &format!("{path}/{key}"), result);
        }
    } else {
        result.push(path.to_owned());
    }
}
fn objects(value: &Value, path: &str, result: &mut Vec<String>) {
    if let Some(object) = value.as_object() {
        result.push(path.to_owned());
        for (key, child) in object {
            objects(child, &format!("{path}/{key}"), result);
        }
    }
}
#[test]
fn frozen_digest_is_plain_complete_semantic_jcs_with_exact_top_level_exclusions() {
    let original = base();
    let result = document(&wire(&original)).consume_request_digest().unwrap();
    assert_eq!(
        result.request_digest(),
        fixture()["expectedConsumeRequestDigest"].as_str().unwrap()
    );
    assert_eq!(
        result.canonical_semantic(),
        document(&wire(&original)).canonical_bytes().unwrap()
    );
    assert_eq!(result.canonical_semantic()[0], b'{');
    let mut changed = original.clone();
    for name in [
        "consumeRequestDigest",
        "recoverySecret",
        "requestId",
        "headers",
    ] {
        changed[name] = json!({"arbitrary": "excluded-control"});
    }
    let excluded = document(&wire(&changed)).consume_request_digest().unwrap();
    assert_eq!(excluded.canonical_semantic(), result.canonical_semantic());
    assert_eq!(excluded.request_digest(), result.request_digest());
    assert!(!String::from_utf8_lossy(excluded.canonical_semantic()).contains("excluded-control"));
    assert!(changed.get("recoverySecret").is_some()); // No erasure of the original caller data is claimed.
    assert_eq!(
        document(&serde_json::to_vec_pretty(&changed).unwrap())
            .consume_request_digest()
            .unwrap()
            .canonical_semantic(),
        result.canonical_semantic()
    );
    let mut fields: Vec<_> = changed
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| format!("{}:{}", serde_json::to_string(key).unwrap(), value))
        .collect();
    fields.reverse();
    assert_eq!(
        document(format!("{{{}}}", fields.join(",")).as_bytes())
            .consume_request_digest()
            .unwrap()
            .canonical_semantic(),
        result.canonical_semantic()
    );
}
#[test]
fn every_variable_semantic_leaf_binds_without_invented_cross_field_guards() {
    let value = full();
    let original = hash(&value);
    let mut paths = Vec::new();
    leaves(&value, "", &mut paths);
    for path in paths {
        if path == "/protocolVersion" || path == "/preinstallSlot/state" {
            continue;
        }
        let mut changed = value.clone();
        let leaf = changed.pointer_mut(&path).unwrap();
        *leaf = if leaf.is_number() {
            json!(leaf.as_u64().unwrap() + 1)
        } else if path == "/purpose" {
            json!("activate")
        } else if leaf.as_str().is_some_and(|text| text.len() == 64) {
            json!("0".repeat(64))
        } else {
            json!("different")
        };
        assert_ne!(hash(&changed), original, "unbound semantic leaf {path}");
    }
    for purpose in ["preinstall", "install", "activate"] {
        let mut changed = value.clone();
        changed["purpose"] = json!(purpose);
        // This is shape math, not a predicate proving that these facts satisfy a purpose.
        assert!(document(&wire(&changed)).consume_request_digest().is_ok());
    }
    let mut reversed_time = value.clone();
    reversed_time["staged"]["stagedAt"] = json!(2);
    assert!(
        document(&wire(&reversed_time))
            .consume_request_digest()
            .is_ok()
    );
    for path in [
        "/freezeEpoch",
        "/backup",
        "/staged",
        "/preinstallSlot",
        "/plans/migration",
        "/plans/backup",
        "/plans/helperMigration",
        "/plans/launcherMigration",
        "/plans/takeover",
    ] {
        let mut changed = value.clone();
        *changed.pointer_mut(path).unwrap() = Value::Null;
        assert_ne!(hash(&changed), original, "unbound nullable value {path}");
    }
}
#[test]
fn excluded_fields_still_have_original_duplicate_and_resource_checks() {
    let raw = String::from_utf8(wire(&base())).unwrap();
    let duplicate = format!(
        "{},\"recoverySecret\":null,\"recoverySecr\\u0065t\":null}}",
        &raw[..raw.len() - 1]
    );
    assert!(matches!(
        Document::parse(duplicate.as_bytes(), InputKind::Metadata),
        Err(Error::DuplicateKey)
    ));
    let too_long = format!(
        "{},\"recoverySecret\":\"{}\"}}",
        &raw[..raw.len() - 1],
        "a".repeat(65537)
    );
    assert!(matches!(
        Document::parse(too_long.as_bytes(), InputKind::Metadata),
        Err(Error::StringLimit)
    ));
    let deep = format!(
        "{},\"headers\":{}null{}}}",
        &raw[..raw.len() - 1],
        "[".repeat(33),
        "]".repeat(33)
    );
    assert!(matches!(
        Document::parse(deep.as_bytes(), InputKind::Metadata),
        Err(Error::DepthLimit)
    ));
    let members = format!(
        "{},\"headers\":[{}]}}",
        &raw[..raw.len() - 1],
        vec!["null"; 4097].join(",")
    );
    assert!(matches!(
        Document::parse(members.as_bytes(), InputKind::Metadata),
        Err(Error::MemberLimit)
    ));
    let mut oversized = wire(&base());
    oversized.resize(1024 * 1024 + 1, b' ');
    assert!(matches!(
        Document::parse(&oversized, InputKind::Metadata),
        Err(Error::InputLimit)
    ));
    assert_eq!(
        ConsumeDigestError::InvalidRequest.to_string(),
        "CONSUME_REQUEST_SCHEMA_INVALID"
    );
}
#[test]
fn original_node_ajv_matches_complete_nested_schema_and_digest() {
    let value = full();
    let mut cases = vec![
        wire(&base()),
        wire(&value),
        b"null".to_vec(),
        b"[]".to_vec(),
        b"{}".to_vec(),
        b"false".to_vec(),
    ];
    let mut object_paths = Vec::new();
    objects(&value, "", &mut object_paths);
    for path in &object_paths {
        for key in value.pointer(path).unwrap().as_object().unwrap().keys() {
            let mut missing = value.clone();
            missing
                .pointer_mut(path)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(key);
            cases.push(wire(&missing));
            for replacement in [
                Value::Null,
                json!(false),
                json!([]),
                json!({}),
                json!(""),
                json!(1),
            ] {
                let mut changed = value.clone();
                changed.pointer_mut(path).unwrap()[key] = replacement;
                cases.push(wire(&changed));
            }
        }
        for key in [
            "extra",
            "headers",
            "requestId",
            "consumeRequestDigest",
            "recoverySecret",
        ] {
            let mut changed = value.clone();
            changed.pointer_mut(path).unwrap()[key] = json!("excluded-only-at-root");
            cases.push(wire(&changed));
        }
    }
    let mut paths = Vec::new();
    leaves(&value, "", &mut paths);
    for path in &paths {
        if value.pointer(path).unwrap().is_number() {
            for number in [
                -1.0,
                -0.0,
                0.0,
                0.5,
                1.0,
                1.5,
                9007199254740991.0,
                9007199254740992.0,
            ] {
                let mut changed = value.clone();
                *changed.pointer_mut(path).unwrap() = json!(number);
                cases.push(wire(&changed));
            }
        } else {
            for text in [
                "a",
                "A._:-9",
                "_bad",
                "a b",
                "\u{00e9}",
                &"a".repeat(128),
                &"a".repeat(129),
                &"0".repeat(64),
                &"A".repeat(64),
                &"a".repeat(63),
                &"a".repeat(65),
            ] {
                let mut changed = value.clone();
                *changed.pointer_mut(path).unwrap() = json!(text);
                cases.push(wire(&changed));
            }
            for ending in ["\n", "\r", "\r\n", "\u{2028}", "\u{2029}"] {
                let mut changed = value.clone();
                *changed.pointer_mut(path).unwrap() = json!(format!(
                    "{}{ending}",
                    value.pointer(path).unwrap().as_str().unwrap()
                ));
                cases.push(wire(&changed));
            }
        }
    }
    for purpose in ["preinstall", "install", "activate", "cleanup", "Install"] {
        let mut changed = value.clone();
        changed["purpose"] = json!(purpose);
        cases.push(wire(&changed));
    }
    let raw = String::from_utf8(wire(&value)).unwrap();
    for (field, old) in [
        ("ownerEpoch", "1"),
        ("stagedAt", "0"),
        ("protocolVersion", "1"),
    ] {
        for spelling in [
            "1.0",
            "1e0",
            "1.0000000000000001",
            "9007199254740990.5",
            "9007199254740991.1",
            "9007199254740991.5",
            "1e309",
        ] {
            cases.push(
                raw.replace(
                    &format!("\"{field}\":{old}"),
                    &format!("\"{field}\":{spelling}"),
                )
                .into_bytes(),
            );
        }
    }
    cases.push(
        raw.replacen(
            "\"ownerEpoch\":1",
            "\"ownerEpoch\":1,\"ownerEp\\u006fch\":2",
            1,
        )
        .into_bytes(),
    );
    for excluded in [
        "consumeRequestDigest",
        "recoverySecret",
        "requestId",
        "headers",
    ] {
        for ignored in [
            Value::Null,
            json!("ignored"),
            json!([]),
            json!({"ignored":true}),
        ] {
            let mut changed = value.clone();
            changed[excluded] = ignored;
            cases.push(wire(&changed));
        }
    }
    let samples: Vec<_> = cases.iter().map(|bytes| STANDARD.encode(bytes)).collect();
    let expected: Vec<Value> = cases
        .iter()
        .map(|bytes| {
            Document::parse(bytes, InputKind::Metadata)
                .ok()
                .and_then(|doc| doc.consume_request_digest().ok())
                .map(|result| {
                    json!([
                        result.request_digest(),
                        STANDARD.encode(result.canonical_semantic())
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
            include_str!("consume-oracle.mjs"),
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
        "original consume digest oracle unavailable"
    );
    let actual: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        assert!(
            actual == expected,
            "consume digest oracle mismatch at case {index}"
        );
    }
    eprintln!(
        "{} original Node/Ajv consume digest cases passed",
        expected.len()
    );
}
