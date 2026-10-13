use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{
    Document, Error, InputKind, NativeIdentityError, NativeIdentityShape, assert_native_identity,
};
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
fn base() -> Value {
    let fixture: Value = serde_json::from_slice(
        &fs::read(root().join("specs/upgrade/v1/vectors/crypto/digests.json")).unwrap(),
    )
    .unwrap();
    fixture["sourceFacts"]["application"].clone()
}
fn wire(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn shape(bytes: &[u8]) -> Result<NativeIdentityShape, NativeIdentityError> {
    Document::parse(bytes, InputKind::Metadata)
        .map_err(NativeIdentityError::Json)?
        .native_identity_shape()
}
#[test]
fn complete_canonical_identity_matches_reordered_and_equivalent_number_spellings() {
    let expected = shape(&wire(&base())).unwrap();
    let canonical = format!(
        r#"{{"format":"deb","identity":"app","publisher":"test-publisher","sha256":"{}","size":123}}"#,
        "a".repeat(64)
    );
    assert_eq!(expected.canonical_bytes(), canonical.as_bytes());
    let pretty = serde_json::to_vec_pretty(&base()).unwrap();
    assert_eq!(
        assert_native_identity(&expected, &shape(&pretty).unwrap()),
        Ok(())
    );
    let reordered = format!(
        r#"{{"size":123,"sha256":"{}","publisher":"test-publisher","identity":"app","format":"deb"}}"#,
        "a".repeat(64)
    );
    assert_eq!(
        assert_native_identity(&expected, &shape(reordered.as_bytes()).unwrap()),
        Ok(())
    );
    for spelling in ["123", "123.0", "1.23e2"] {
        let changed = canonical.replace("\"size\":123", &format!("\"size\":{spelling}"));
        assert_eq!(
            assert_native_identity(&expected, &shape(changed.as_bytes()).unwrap()),
            Ok(())
        );
    }
}
#[test]
fn all_five_fields_participate_without_cross_platform_or_role_rules() {
    let expected = shape(&wire(&base())).unwrap();
    for field in ["size", "sha256", "identity", "publisher", "format"] {
        let mut changed = base();
        changed[field] = match field {
            "size" => json!(124),
            "sha256" => json!("0".repeat(64)),
            "format" => json!("helper"),
            _ => json!("different"),
        };
        let actual = shape(&wire(&changed)).unwrap();
        assert_eq!(
            assert_native_identity(&expected, &actual),
            Err(NativeIdentityError::Mismatch),
            "unbound field {field}"
        );
        assert_eq!(
            assert_native_identity(&actual, &expected),
            Err(NativeIdentityError::Mismatch)
        );
        assert_eq!(assert_native_identity(&actual, &actual), Ok(()));
    }
    // An identity named app with format helper is shape-valid. Neither operand
    // attests a platform probe or authenticity; comparison does not add policy.
    assert_eq!(
        NativeIdentityError::Mismatch.to_string(),
        "NATIVE_IDENTITY_MISMATCH"
    );
    assert_eq!(
        NativeIdentityError::InvalidIdentity.to_string(),
        "NATIVE_IDENTITY_SCHEMA_INVALID"
    );
    let raw = String::from_utf8(wire(&base())).unwrap();
    let duplicate = raw.replacen(
        "\"identity\":\"app\"",
        "\"identity\":\"app\",\"ident\\u0069ty\":\"other\"",
        1,
    );
    assert!(matches!(
        shape(duplicate.as_bytes()),
        Err(NativeIdentityError::Json(Error::DuplicateKey))
    ));
}
#[test]
fn original_node_matches_both_operands_shape_equality_and_canonical_bytes() {
    let valid = wire(&base());
    let mut cases = vec![
        valid.clone(),
        b"null".to_vec(),
        b"[]".to_vec(),
        b"{}".to_vec(),
    ];
    for field in base().as_object().unwrap().keys() {
        let mut missing = base();
        missing.as_object_mut().unwrap().remove(field);
        cases.push(wire(&missing));
        for value in [
            Value::Null,
            json!(false),
            json!(1),
            json!(""),
            json!([]),
            json!({}),
        ] {
            let mut changed = base();
            changed[field] = value;
            cases.push(wire(&changed));
        }
    }
    let schema: Value = serde_json::from_slice(
        &fs::read(root().join("specs/upgrade/v1/schemas/common.json")).unwrap(),
    )
    .unwrap();
    for format in schema["$defs"]["nativeIdentity"]["properties"]["format"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .cloned()
        .chain([json!("zip"), json!("DEB"), json!("")])
    {
        let mut changed = base();
        changed["format"] = format;
        cases.push(wire(&changed));
    }
    for field in ["identity", "publisher"] {
        for text in [
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
    for hash in [
        "0".repeat(64),
        "A".repeat(64),
        "g".repeat(64),
        "a".repeat(63),
        "a".repeat(65),
    ] {
        let mut changed = base();
        changed["sha256"] = json!(hash);
        cases.push(wire(&changed));
    }
    for ending in ["\n", "\r", "\r\n", "\u{2028}", "\u{2029}"] {
        for field in ["identity", "publisher", "sha256", "format"] {
            let mut changed = base();
            changed[field] = json!(format!("{}{ending}", changed[field].as_str().unwrap()));
            cases.push(wire(&changed));
        }
    }
    let raw = String::from_utf8(valid.clone()).unwrap();
    for spelling in [
        "-0",
        "0",
        "-1",
        "0.5",
        "1",
        "1.0",
        "1e0",
        "123",
        "123.0",
        "1.23e2",
        "9007199254740991",
        "9007199254740992",
        "9007199254740990.5",
        "9007199254740991.1",
        "9007199254740991.5",
        "1.0000000000000001",
        "1e309",
    ] {
        cases.push(
            raw.replace("\"size\":123", &format!("\"size\":{spelling}"))
                .into_bytes(),
        );
    }
    for extra in ["role", "targetKey", "extra", "headers"] {
        let mut changed = base();
        changed[extra] = Value::Null;
        cases.push(wire(&changed));
    }
    cases.push(
        raw.replacen(
            "\"identity\":\"app\"",
            "\"identity\":\"app\",\"ident\\u0069ty\":\"other\"",
            1,
        )
        .into_bytes(),
    );
    let pairs: Vec<_> = cases
        .iter()
        .flat_map(|bytes| {
            [
                (bytes.clone(), bytes.clone()),
                (valid.clone(), bytes.clone()),
                (bytes.clone(), valid.clone()),
            ]
        })
        .collect();
    let samples: Vec<_> = pairs
        .iter()
        .map(|(expected, actual)| json!([STANDARD.encode(expected), STANDARD.encode(actual)]))
        .collect();
    let expected: Vec<Value> = pairs
        .iter()
        .map(
            |(expected, actual)| match (shape(expected), shape(actual)) {
                (Ok(expected), Ok(actual)) => json!([
                    assert_native_identity(&expected, &actual).is_ok(),
                    STANDARD.encode(expected.canonical_bytes()),
                    STANDARD.encode(actual.canonical_bytes())
                ]),
                _ => Value::Null,
            },
        )
        .collect();
    let reference = root()
        .join("specs/upgrade/v1/reference/digests.mjs")
        .to_string_lossy()
        .into_owned();
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("native-oracle.mjs"),
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
        "original native identity oracle unavailable"
    );
    let actual: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        assert!(
            actual == expected,
            "native identity oracle mismatch at case {index}"
        );
    }
    eprintln!(
        "{} original Node/Ajv native identity pairs passed",
        expected.len()
    );
}
