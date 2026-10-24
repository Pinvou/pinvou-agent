#[allow(dead_code)]
#[path = "support/download_fixture.rs"]
mod download_fixture;

use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pinvou_upgrade_codec::{
    ContextBindingError, Document, InputKind, assert_supplied_expected_context,
    decode_canonical_envelope,
};
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
    let mut vectors = serde_json::from_slice::<Value>(
        &std::fs::read(root().join("specs/upgrade/v1/vectors/crypto/envelopes.json")).unwrap(),
    )
    .unwrap()["vectors"]
        .as_array()
        .unwrap()
        .clone();
    download_fixture::replace_legacy_download(
        &mut vectors,
        &root().join("specs/upgrade/v1/reference/canonical-json.mjs"),
    );
    vectors
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
fn check(raw: &[u8], role: &str, expected: &[u8]) -> Result<(), ContextBindingError> {
    let envelope = decode_canonical_envelope(raw, kind(role))
        .unwrap()
        .check_full_schema()
        .unwrap();
    let expected =
        Document::parse(expected, InputKind::Metadata).map_err(ContextBindingError::Json)?;
    assert_supplied_expected_context(&envelope, &expected)
}
fn reordered(value: &Value) -> Value {
    let mut result = value.clone();
    for field in ["scope", ""] {
        let object = if field.is_empty() {
            &mut result
        } else {
            &mut result[field]
        };
        let reversed = object
            .as_object()
            .unwrap()
            .iter()
            .rev()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        *object = Value::Object(reversed);
    }
    result
}
fn maximum_expected_context(value: &Value) -> Vec<u8> {
    let mut value = value.clone();
    value["ignored"] = json!([]);
    while 1048576 - wire(&value).len() > 65539 {
        value["ignored"]
            .as_array_mut()
            .unwrap()
            .push(json!("a".repeat(65500)));
    }
    value["ignored"].as_array_mut().unwrap().push(json!(""));
    let remaining = 1048576 - wire(&value).len();
    *value["ignored"].as_array_mut().unwrap().last_mut().unwrap() = json!("a".repeat(remaining));
    let bytes = wire(&value);
    assert_eq!(bytes.len(), 1048576);
    bytes
}
fn wire(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}
#[test]
fn independent_contexts_match_all_twenty_current_roles_and_require_all_properties() {
    for vector in fixtures() {
        let role = vector["name"].as_str().unwrap();
        let raw = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let original = &vector["expectedContext"];
        assert_eq!(check(&raw, role, &wire(original)), Ok(()));
        assert_eq!(check(&raw, role, &wire(&reordered(original))), Ok(()));
        assert_eq!(
            check(&raw, role, &serde_json::to_vec_pretty(original).unwrap()),
            Ok(())
        );
        for field in ["role", "product", "component", "scope"] {
            let mut changed = original.clone();
            changed.as_object_mut().unwrap().remove(field);
            assert_eq!(
                check(&raw, role, &wire(&changed)),
                Err(ContextBindingError::InvalidExpected)
            );
        }
        let mut extra = original.clone();
        extra["ignored"] = json!({"also":"not context"});
        assert_eq!(check(&raw, role, &wire(&extra)), Ok(()));
        for scalar in [Value::Null, json!([]), json!("context"), json!(1)] {
            assert_eq!(
                check(&raw, role, &wire(&scalar)),
                Err(ContextBindingError::InvalidExpected)
            );
        }
    }
}
#[test]
fn complete_scope_binds_installation_channel_revision_and_each_original_leaf() {
    for vector in fixtures() {
        let role = vector["name"].as_str().unwrap();
        let raw = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let original = &vector["expectedContext"];
        for field in ["role", "product", "component", "scope"] {
            let mut changed = original.clone();
            changed[field] = json!("different");
            assert_eq!(
                check(&raw, role, &wire(&changed)),
                Err(ContextBindingError::Mismatch)
            );
        }
        let mut changed = original.clone();
        changed["scope"]["unexpected"] = json!(1);
        assert_eq!(
            check(&raw, role, &wire(&changed)),
            Err(ContextBindingError::Mismatch)
        );
        for key in original["scope"].as_object().unwrap().keys() {
            let mut changed = original.clone();
            changed["scope"].as_object_mut().unwrap().remove(key);
            assert_eq!(
                check(&raw, role, &wire(&changed)),
                Err(ContextBindingError::Mismatch)
            );
            let mut changed = original.clone();
            changed["scope"][key] = json!("different");
            assert_eq!(
                check(&raw, role, &wire(&changed)),
                Err(ContextBindingError::Mismatch)
            );
        }
        // Equal finite binary64 channelRevision and reordered scope remain equal.
        if original["scope"].get("channelRevision").is_some() {
            let text = String::from_utf8(wire(original)).unwrap();
            let integer = original["scope"]["channelRevision"].as_u64().unwrap();
            let text = text.replace(
                &format!("\"channelRevision\":{integer}"),
                &format!("\"channelRevision\":{integer}.0"),
            );
            assert_eq!(check(&raw, role, text.as_bytes()), Ok(()));
        }
    }
}
#[test]
fn expected_document_extras_cannot_bypass_original_resources_or_escaped_duplicates() {
    let vector = fixtures().remove(0);
    let raw = URL_SAFE_NO_PAD
        .decode(vector["envelopeBase64Url"].as_str().unwrap())
        .unwrap();
    let maximum = maximum_expected_context(&vector["expectedContext"]);
    assert_eq!(check(&raw, "root", &maximum), Ok(()));
    let mut above = maximum.clone();
    above.push(b' ');
    assert_eq!(
        check(&raw, "root", &above),
        Err(ContextBindingError::Json(
            pinvou_upgrade_codec::Error::InputLimit
        ))
    );
    let mut expected = vector["expectedContext"].clone();
    expected["ignored"] = json!("a".repeat(65536));
    assert_eq!(check(&raw, "root", &wire(&expected)), Ok(()));
    expected["ignored"] = json!("a".repeat(65537));
    assert_eq!(
        check(&raw, "root", &wire(&expected)),
        Err(ContextBindingError::Json(
            pinvou_upgrade_codec::Error::StringLimit
        ))
    );
    let text = String::from_utf8(wire(&vector["expectedContext"])).unwrap();
    let duplicate = text.replace(
        "\"component\":null",
        "\"component\":null,\"comp\\u006fnent\":null",
    );
    assert_eq!(
        check(&raw, "root", duplicate.as_bytes()),
        Err(ContextBindingError::Json(
            pinvou_upgrade_codec::Error::DuplicateKey
        ))
    );
    assert_eq!(
        check(&raw, "root", b"\xff"),
        Err(ContextBindingError::Json(
            pinvou_upgrade_codec::Error::InvalidUtf8
        ))
    );
    assert_eq!(
        check(&raw, "root", &vec![b' '; 1048577]),
        Err(ContextBindingError::Json(
            pinvou_upgrade_codec::Error::InputLimit
        ))
    );
    let mut nested = json!(null);
    for _ in 0..33 {
        nested = json!([nested]);
    }
    expected["ignored"] = nested;
    assert_eq!(
        check(&raw, "root", &wire(&expected)),
        Err(ContextBindingError::Json(
            pinvou_upgrade_codec::Error::DepthLimit
        ))
    );
}
#[test]
fn actual_original_node_parser_matches_independent_expected_context_variants() {
    let mut cases = Vec::new();
    for vector in fixtures() {
        let role = vector["name"].as_str().unwrap();
        let raw = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let original = &vector["expectedContext"];
        let mut push = |expected: Vec<u8>| {
            cases.push(json!({"raw":STANDARD.encode(&raw),"role":role,"expectedRaw":STANDARD.encode(expected)}))
        };
        push(wire(original));
        push(wire(&reordered(original)));
        if role == "root" {
            let maximum = maximum_expected_context(original);
            push(maximum.clone());
            let mut above = maximum;
            above.push(b' ');
            push(above);
        }
        push(serde_json::to_vec_pretty(original).unwrap());
        for field in ["role", "product", "component", "scope"] {
            let mut changed = original.clone();
            changed.as_object_mut().unwrap().remove(field);
            push(wire(&changed));
            for value in [
                Value::Null,
                json!("root"),
                json!("decision"),
                json!("other"),
                json!(" "),
                json!(1),
                json!(false),
                json!([]),
                json!({}),
            ] {
                let mut changed = original.clone();
                changed[field] = value;
                push(wire(&changed));
            }
        }
        let mut changed = original.clone();
        changed["scope"]["extra"] = json!(true);
        push(wire(&changed));
        for key in original["scope"].as_object().unwrap().keys() {
            let mut changed = original.clone();
            changed["scope"].as_object_mut().unwrap().remove(key);
            push(wire(&changed));
            for value in [
                Value::Null,
                json!(1),
                json!(1.0),
                json!(0),
                json!(1.5),
                json!("other"),
                json!("1"),
                json!(false),
                json!({}),
                json!([]),
            ] {
                let mut changed = original.clone();
                changed["scope"][key] = value;
                push(wire(&changed));
            }
        }
        let mut changed = original.clone();
        changed["ignored"] = json!({"also":"not part of context"});
        push(wire(&changed));
        let mut changed = original.clone();
        changed["ignored"] = json!("a".repeat(65537));
        push(wire(&changed));
        for value in [Value::Null, json!([]), json!("context"), json!(7)] {
            push(wire(&value));
        }
        let text = String::from_utf8(wire(original)).unwrap();
        push(text.replace("pinvou", "pinv\\u006fu").into_bytes());
        push(
            text.replace(
                "\"product\":\"pinvou\"",
                "\"product\":\"pinvou\",\"prod\\u0075ct\":\"pinvou\"",
            )
            .into_bytes(),
        );
        if original["scope"].get("channelRevision").is_some() {
            let integer = original["scope"]["channelRevision"].as_u64().unwrap();
            push(
                text.replace(
                    &format!("\"channelRevision\":{integer}"),
                    &format!("\"channelRevision\":{integer}.0"),
                )
                .into_bytes(),
            );
        }
    }
    let expected: Vec<bool> = cases
        .iter()
        .map(|case| {
            check(
                &STANDARD.decode(case["raw"].as_str().unwrap()).unwrap(),
                case["role"].as_str().unwrap(),
                &STANDARD
                    .decode(case["expectedRaw"].as_str().unwrap())
                    .unwrap(),
            )
            .is_ok()
        })
        .collect();
    let reference = root().join("specs/upgrade/v1/reference/signatures.mjs");
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("context-oracle.mjs"),
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
        "original expected-context oracle unavailable"
    );
    let actual: Vec<bool> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (a, b)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_eq!(a, b, "context case {index}");
    }
    eprintln!(
        "{} original Node independent expected-context cases passed",
        cases.len()
    );
}
