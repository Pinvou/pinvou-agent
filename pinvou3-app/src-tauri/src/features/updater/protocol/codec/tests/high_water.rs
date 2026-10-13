use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pinvou_upgrade_codec::{
    Document, HighWaterError, InputKind, assert_supplied_high_water, decode_canonical_envelope,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
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
fn canonical(value: &Value) -> Vec<u8> {
    Document::parse(&serde_json::to_vec(value).unwrap(), InputKind::Metadata)
        .unwrap()
        .canonical_bytes()
        .unwrap()
}
fn hash(raw: &[u8]) -> String {
    Sha256::digest(raw)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn record(raw: &[u8]) -> Value {
    let envelope: Value = serde_json::from_slice(raw).unwrap();
    let signed = &envelope["signed"];
    json!({"scope":{"role":signed["role"],"product":signed["product"],"component":signed["component"],"scope":signed["scope"]},
        "version":if signed["role"]=="release"{&signed["revision"]}else{&signed["version"]},"envelopeSha256":hash(raw)})
}
fn check(raw: &[u8], record: &[u8]) -> Result<(), HighWaterError> {
    let envelope = decode_canonical_envelope(raw, InputKind::Metadata).unwrap();
    let record = Document::parse(record, InputKind::Metadata).map_err(HighWaterError::Json)?;
    assert_supplied_high_water(&envelope, &record)
}
#[test]
fn original_full_envelope_bytes_bind_equal_versions_and_all_four_context_fields() {
    for vector in fixtures().into_iter().filter(|v| {
        matches!(
            v["name"].as_str(),
            Some("root" | "timestamp" | "snapshot" | "target" | "release")
        )
    }) {
        let raw = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let original = record(&raw);
        assert_eq!(check(&raw, &canonical(&original)), Ok(()));
        let mut changed: Value = serde_json::from_slice(&raw).unwrap();
        changed["signatures"][0]["signature"] = json!(URL_SAFE_NO_PAD.encode([0; 64]));
        assert_eq!(
            check(&canonical(&changed), &canonical(&original)),
            Err(HighWaterError::Equivocation)
        );
        for field in ["role", "product", "component", "scope"] {
            let mut changed = original.clone();
            changed["scope"][field] = json!(null);
            if changed["scope"] != original["scope"] {
                assert_eq!(
                    check(&raw, &canonical(&changed)),
                    Err(HighWaterError::ScopeMismatch)
                );
            }
        }
        let mut changed = original.clone();
        changed["scope"]["extra"] = json!(1);
        assert_eq!(
            check(&raw, &canonical(&changed)),
            Err(HighWaterError::ScopeMismatch)
        );
        let mut changed: Value = serde_json::from_slice(&raw).unwrap();
        changed["signed"]["scope"]["opaque"] = json!({"value":1});
        let changed = canonical(&changed);
        let current = record(&changed);
        assert_eq!(check(&changed, &canonical(&current)), Ok(()));
        assert_eq!(
            check(&changed, &canonical(&original)),
            Err(HighWaterError::ScopeMismatch)
        );
    }
}
#[test]
fn versioned_role_domain_uses_release_revision_without_fallback_or_record_default() {
    for vector in fixtures().into_iter().filter(|v| {
        matches!(
            v["name"].as_str(),
            Some("root" | "timestamp" | "snapshot" | "target" | "release")
        )
    }) {
        let raw = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let original = record(&raw);
        let mut value: Value = serde_json::from_slice(&raw).unwrap();
        let field = if value["signed"]["role"] == "release" {
            "revision"
        } else {
            "version"
        };
        for invalid in [
            Value::Null,
            json!(0),
            json!(-1),
            json!(1.5),
            json!("1"),
            json!(9007199254740992_u64),
        ] {
            value["signed"][field] = invalid;
            assert_eq!(
                check(&canonical(&value), &canonical(&original)),
                Err(HighWaterError::InvalidVersion)
            );
        }
        value["signed"].as_object_mut().unwrap().remove(field);
        value["signed"]["version"] = json!(1);
        if field == "revision" {
            assert_eq!(
                check(&canonical(&value), &canonical(&original)),
                Err(HighWaterError::InvalidVersion)
            );
        } else {
            value["signed"].as_object_mut().unwrap().remove("version");
            assert_eq!(
                check(&canonical(&value), &canonical(&original)),
                Err(HighWaterError::InvalidVersion)
            );
        }
        for bad in [Value::Null, json!([]), json!({}), json!("missing")] {
            assert_eq!(
                check(&raw, &canonical(&bad)),
                Err(HighWaterError::InvalidRecord)
            );
        }
    }
    for vector in fixtures().into_iter().filter(|v| {
        !matches!(
            v["name"].as_str(),
            Some("root" | "timestamp" | "snapshot" | "target" | "release")
        )
    }) {
        let raw = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let role = vector["name"].as_str().unwrap();
        let kind = if role == "package" {
            InputKind::Metadata
        } else {
            InputKind::Credential
        };
        let envelope = decode_canonical_envelope(&raw, kind).unwrap();
        assert_eq!(
            assert_supplied_high_water(
                &envelope,
                &Document::parse(b"{}", InputKind::Metadata).unwrap()
            ),
            Err(HighWaterError::UnsupportedRole)
        );
    }
}
#[test]
fn ignored_record_extras_keep_raw_resources_and_duplicate_rejection() {
    let vector = fixtures().remove(0);
    let raw = URL_SAFE_NO_PAD
        .decode(vector["envelopeBase64Url"].as_str().unwrap())
        .unwrap();
    let mut value = record(&raw);
    value["ignored"] = json!("a".repeat(65537));
    assert_eq!(
        check(&raw, &serde_json::to_vec(&value).unwrap()),
        Err(HighWaterError::Json(
            pinvou_upgrade_codec::Error::StringLimit
        ))
    );
    let text = String::from_utf8(canonical(&record(&raw))).unwrap();
    let duplicate = text.replace("\"version\":1", "\"version\":1,\"vers\\u0069on\":1");
    assert_eq!(
        check(&raw, duplicate.as_bytes()),
        Err(HighWaterError::Json(
            pinvou_upgrade_codec::Error::DuplicateKey
        ))
    );
    assert_eq!(
        check(&raw, &vec![b' '; 1048577]),
        Err(HighWaterError::Json(
            pinvou_upgrade_codec::Error::InputLimit
        ))
    );
    assert_eq!(
        check(&raw, b"\xff"),
        Err(HighWaterError::Json(
            pinvou_upgrade_codec::Error::InvalidUtf8
        ))
    );
    let mut value = record(&raw);
    value["ignored"] = json!({"also":"not a record field"});
    assert_eq!(check(&raw, &canonical(&value)), Ok(()));
}
#[test]
fn actual_original_node_high_water_matches_version_scope_and_exact_raw_hash_branches() {
    let mut cases = Vec::new();
    for vector in fixtures().into_iter().filter(|v| {
        matches!(
            v["name"].as_str(),
            Some("root" | "timestamp" | "snapshot" | "target" | "release")
        )
    }) {
        let raw = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let original_envelope: Value = serde_json::from_slice(&raw).unwrap();
        let original_record = record(&raw);
        let mut push = |raw: &[u8], record: &[u8]| {
            cases.push(json!({"raw":STANDARD.encode(raw),"record":STANDARD.encode(record)}))
        };
        push(&raw, &canonical(&original_record));
        for field in ["scope", "version", "envelopeSha256"] {
            let mut changed = original_record.clone();
            changed.as_object_mut().unwrap().remove(field);
            push(&raw, &canonical(&changed));
        }
        for field in ["role", "product", "component", "scope"] {
            for value in [
                Value::Null,
                json!("other"),
                json!({}),
                json!({"extra":1}),
                json!([]),
                json!(false),
            ] {
                let mut changed = original_record.clone();
                changed["scope"][field] = value;
                push(&raw, &canonical(&changed));
            }
        }
        let mut changed = original_record.clone();
        changed["scope"]["extra"] = json!(1);
        push(&raw, &canonical(&changed));
        for current in [1_u64, 2, 3, 9007199254740990, 9007199254740991] {
            let mut envelope = original_envelope.clone();
            let field = if envelope["signed"]["role"] == "release" {
                "revision"
            } else {
                "version"
            };
            envelope["signed"][field] = json!(current);
            let candidate = canonical(&envelope);
            let candidate_record = record(&candidate);
            for previous in [
                json!(1),
                json!(2),
                json!(3),
                json!(9007199254740991_u64),
                json!(9007199254740992_u64),
                json!(0),
                json!(-1),
                json!(1.5),
                json!("1"),
                Value::Null,
            ] {
                for sha in [
                    candidate_record["envelopeSha256"].clone(),
                    json!("0".repeat(64)),
                    json!(hash(&candidate).to_uppercase()),
                    json!("bad"),
                    Value::Null,
                    json!(1),
                ] {
                    let mut changed = candidate_record.clone();
                    changed["version"] = previous.clone();
                    changed["envelopeSha256"] = sha;
                    push(&candidate, &canonical(&changed));
                }
                let mut changed = candidate_record.clone();
                changed["version"] = previous;
                changed.as_object_mut().unwrap().remove("envelopeSha256");
                push(&candidate, &canonical(&changed));
            }
        }
        let mut envelope = original_envelope.clone();
        envelope["signatures"][0]["signature"] = json!(URL_SAFE_NO_PAD.encode([1; 64]));
        push(&canonical(&envelope), &canonical(&original_record));
        let record_text = String::from_utf8(canonical(&original_record)).unwrap();
        push(
            &raw,
            record_text
                .replace("\"version\":1", "\"version\":1.0")
                .as_bytes(),
        );
        push(
            &raw,
            record_text
                .replace("\"version\":1", "\"version\":1,\"vers\\u0069on\":1")
                .as_bytes(),
        );
        for value in [
            Value::Null,
            json!([]),
            json!({}),
            json!({"scope":original_record["scope"],"version":true}),
        ] {
            push(&raw, &canonical(&value));
        }
        let mut oversized = original_record.clone();
        oversized["ignored"] = json!("a".repeat(65537));
        push(&raw, &serde_json::to_vec(&oversized).unwrap());
    }
    let expected: Vec<bool> = cases
        .iter()
        .map(|case| {
            check(
                &STANDARD.decode(case["raw"].as_str().unwrap()).unwrap(),
                &STANDARD.decode(case["record"].as_str().unwrap()).unwrap(),
            )
            .is_ok()
        })
        .collect();
    let reference = root().join("specs/upgrade/v1/reference/signatures.mjs");
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("high-water-oracle.mjs"),
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
        "original high-water oracle unavailable"
    );
    let actual: Vec<bool> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (a, b)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_eq!(a, b, "case {index}");
    }
    eprintln!("{} original Node high-water cases passed", cases.len());
}
