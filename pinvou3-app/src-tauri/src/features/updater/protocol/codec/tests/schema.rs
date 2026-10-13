use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pinvou_upgrade_codec::{Document, InputKind, SchemaError, decode_canonical_envelope};
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
fn wire(value: &Value) -> Vec<u8> {
    Document::parse(&serde_json::to_vec(value).unwrap(), InputKind::Metadata)
        .unwrap()
        .canonical_bytes()
        .unwrap()
}
fn accepted(raw: &[u8], role: &str) -> bool {
    decode_canonical_envelope(raw, kind(role))
        .and_then(|e| {
            e.check_full_schema()
                .map_err(|_| pinvou_upgrade_codec::EnvelopeShapeError::InvalidEnvelope)
        })
        .is_ok()
}
#[test]
fn all_twenty_full_schemas_preserve_original_bytes_without_signature_or_time_validation() {
    for vector in fixtures() {
        let raw = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let role = vector["name"].as_str().unwrap();
        let shape = decode_canonical_envelope(&raw, kind(role))
            .unwrap()
            .check_full_schema()
            .unwrap();
        assert_eq!(shape.envelope().raw_bytes(), raw);
        let mut value: Value = serde_json::from_slice(&raw).unwrap();
        value["signatures"][0]["signature"] = json!(URL_SAFE_NO_PAD.encode([0; 64]));
        // Schema success deliberately does not attest the signature.
        assert!(accepted(&wire(&value), role));
        if value["signed"].get("expiresAt").is_some() {
            value["signed"]["issuedAt"] = json!(9007199254740991_u64);
            value["signed"]["expiresAt"] = json!(0);
            assert!(accepted(&wire(&value), role));
        }
        let minimal = json!({"signed":{"protocolVersion":1,"role":role,"product":"pinvou","component":if role=="root"{Value::Null}else{json!("app")},"scope":{}},
            "signatures":[{"keyId":"0".repeat(64),"signature":URL_SAFE_NO_PAD.encode([0;64])}]});
        let encoding = decode_canonical_envelope(&wire(&minimal), kind(role)).unwrap();
        assert!(matches!(
            encoding.check_full_schema(),
            Err(SchemaError::InvalidEnvelope)
        ));
    }
}
// Mutate real valid instances according to actual committed Schema paths.
// No validator algorithm is copied: both Rust and original Node evaluate cases.
fn resolve<'a>(schema: &'a Value, common: &'a Value) -> &'a Value {
    if let Some(reference) = schema["$ref"].as_str() {
        let prefix = "urn:pinvou:upgrade:v1:common#";
        assert!(reference.starts_with(prefix));
        common.pointer(&reference[prefix.len()..]).unwrap()
    } else {
        schema
    }
}
fn at_mut<'a>(value: &'a mut Value, path: &[String]) -> &'a mut Value {
    let mut current = value;
    for key in path {
        current = if current.is_array() {
            &mut current[key.parse::<usize>().unwrap()]
        } else {
            &mut current[key]
        };
    }
    current
}
fn variants(base: &Value, path: &[String], schema: &Value, common: &Value, cases: &mut Vec<Value>) {
    let schema = resolve(schema, common);
    let mut current = base;
    for key in path {
        current = if current.is_array() {
            &current[key.parse::<usize>().unwrap()]
        } else {
            &current[key]
        };
    }
    for replacement in [
        Value::Null,
        json!(false),
        json!(0),
        json!(1.5),
        json!(""),
        json!([]),
        json!({}),
    ] {
        let mut changed = base.clone();
        *at_mut(&mut changed, path) = replacement;
        cases.push(changed);
    }
    match current {
        Value::Object(object) => {
            if let Some(required) = schema["required"].as_array() {
                for name in required {
                    let mut changed = base.clone();
                    at_mut(&mut changed, path)
                        .as_object_mut()
                        .unwrap()
                        .remove(name.as_str().unwrap());
                    cases.push(changed);
                }
            }
            if schema["additionalProperties"] == json!(false) {
                let mut changed = base.clone();
                at_mut(&mut changed, path)["__unexpected"] = json!(true);
                cases.push(changed);
            }
            if let Some(properties) = schema["properties"].as_object() {
                for (name, child_schema) in properties {
                    if object.contains_key(name) {
                        let mut child_path = path.to_vec();
                        child_path.push(name.clone());
                        variants(base, &child_path, child_schema, common, cases);
                    }
                }
            }
        }
        Value::Array(array) => {
            if !array.is_empty() {
                let mut changed = base.clone();
                at_mut(&mut changed, path)
                    .as_array_mut()
                    .unwrap()
                    .push(array[0].clone());
                cases.push(changed);
                let mut child_path = path.to_vec();
                child_path.push("0".to_owned());
                if schema.get("items").is_some() {
                    variants(base, &child_path, &schema["items"], common, cases);
                }
            }
        }
        Value::String(text) => {
            let replacements = if schema.get("pattern").is_some() {
                vec![
                    format!("{text}\n"),
                    format!("{text}\r"),
                    format!("{text}\r\n"),
                    format!("{text}\u{2028}"),
                    format!("{text}\u{2029}"),
                    ".".to_owned(),
                    "..".to_owned(),
                    "../x".to_owned(),
                    "x/../y".to_owned(),
                    "x//y".to_owned(),
                    "x/./y".to_owned(),
                    "\u{1f600}".to_owned(),
                    "x".repeat(128),
                    "x".repeat(129),
                ]
            } else {
                vec!["\u{1f600}".to_owned(), "x".to_owned()]
            };
            for replacement in replacements {
                let mut changed = base.clone();
                *at_mut(&mut changed, path) = json!(replacement);
                cases.push(changed);
            }
        }
        Value::Number(_) => {
            for number in [
                json!(-1),
                json!(-0.0),
                json!(1),
                json!(9007199254740991_u64),
                json!(9007199254740992_u64),
                json!(1e100),
                json!(1e-100),
            ] {
                let mut changed = base.clone();
                *at_mut(&mut changed, path) = number;
                cases.push(changed);
            }
        }
        _ => {}
    }
    if let Some(branches) = schema["anyOf"].as_array() {
        for branch in branches {
            let branch = resolve(branch, common);
            if (current.is_object() && branch["type"] == "object")
                || (current.is_array() && branch["type"] == "array")
                || (current.is_string() && branch["type"] == "string")
                || (current.is_number()
                    && (branch["type"] == "integer" || branch["type"] == "number"))
            {
                variants(base, path, branch, common, cases);
            }
        }
    }
}
#[test]
fn original_node_full_schema_parser_matches_all_nested_required_and_scalar_mutations() {
    let common: Value = serde_json::from_slice(
        &std::fs::read(root().join("specs/upgrade/v1/schemas/common.json")).unwrap(),
    )
    .unwrap();
    let mut cases = Vec::new();
    for vector in fixtures() {
        let role = vector["name"].as_str().unwrap();
        let raw = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let value: Value = serde_json::from_slice(&raw).unwrap();
        let group = if kind(role) == InputKind::Metadata {
            "metadata"
        } else {
            "credentials"
        };
        let schema: Value = serde_json::from_slice(
            &std::fs::read(root().join(format!("specs/upgrade/v1/schemas/{group}/{role}.json")))
                .unwrap(),
        )
        .unwrap();
        let mut values = vec![value.clone()];
        variants(&value, &[], &schema, &common, &mut values);
        for value in values {
            let expected = json!({"role":value["signed"]["role"],"product":value["signed"]["product"],"component":value["signed"]["component"],"scope":value["signed"]["scope"]});
            cases.push(
                json!({"raw":STANDARD.encode(wire(&value)),"expected":expected,"profileRole":role}),
            );
        }
        // Original parser preconditions are part of the comparison, not AJV alone.
        for bad in [
            format!("{}\n", String::from_utf8(raw.clone()).unwrap()).into_bytes(),
            serde_json::to_vec_pretty(&value).unwrap(),
        ] {
            cases.push(json!({"raw":STANDARD.encode(bad),"expected":vector["expectedContext"],"profileRole":role}));
        }
    }
    let expected: Vec<bool> = cases
        .iter()
        .map(|case| {
            accepted(
                &STANDARD.decode(case["raw"].as_str().unwrap()).unwrap(),
                case["profileRole"].as_str().unwrap(),
            )
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
        "original full-schema oracle unavailable"
    );
    let actual: Vec<bool> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (a, b)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_eq!(a, b, "schema case {index}");
    }
    eprintln!(
        "{} original Node full-schema parser cases passed",
        cases.len()
    );
}
