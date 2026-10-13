use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pinvou_upgrade_codec::{
    Document, Error, InputKind, MetadataReferenceError, MetadataReferenceShape,
    derive_metadata_reference,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
const ROLES: [&str; 6] = [
    "root",
    "timestamp",
    "snapshot",
    "target",
    "release",
    "package",
];
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(7)
        .unwrap()
        .to_owned()
}
fn wire(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn shape(bytes: &[u8]) -> Result<MetadataReferenceShape, MetadataReferenceError> {
    Document::parse(bytes, InputKind::Metadata)
        .map_err(MetadataReferenceError::Json)?
        .metadata_reference_shape()
}
fn reference(role: &str) -> Value {
    let scope = match role {
        "target" => json!({"channel":"custom-channel","targetKey":"custom-target"}),
        "release" => json!({"releaseId":"release-1"}),
        "package" => json!({"targetKey":"custom-target"}),
        _ => json!({}),
    };
    json!({"role":role,"scope":scope,"version":if role=="package"{Value::Null}else{json!(1)},
        "expiresAt":if role=="package"{Value::Null}else{json!(0)},"size":100,"envelopeSha256":"a".repeat(64)})
}
fn envelope(role: &str) -> Value {
    let reference = reference(role);
    let mut signed = json!({"role":role,"scope":reference["scope"]});
    if role == "release" {
        signed["revision"] = json!(1);
    } else {
        signed["version"] = json!(1);
    }
    if role != "package" {
        signed["expiresAt"] = json!(0);
    }
    // Deliberately minimal unsigned supplied claims, never a trusted envelope.
    json!({"signed":signed})
}
fn ref_value(bytes: &[u8]) -> Value {
    serde_json::from_slice(derive_metadata_reference(bytes).unwrap().canonical_bytes()).unwrap()
}
#[test]
fn six_reference_role_shapes_keep_generic_scope_identifiers_and_nullable_package_fields() {
    for role in ROLES {
        let value = reference(role);
        let expected = shape(&wire(&value)).unwrap();
        assert_eq!(
            shape(&serde_json::to_vec_pretty(&value).unwrap())
                .unwrap()
                .canonical_bytes(),
            expected.canonical_bytes()
        );
    }
    for (role, scope) in [
        ("root", json!({"targetKey":"x"})),
        ("timestamp", json!({"channel":"x"})),
        ("snapshot", json!({"releaseId":"x"})),
        ("target", json!({"targetKey":"x"})),
        ("release", json!({})),
        ("package", json!({})),
    ] {
        let mut value = reference(role);
        value["scope"] = scope;
        assert!(shape(&wire(&value)).is_err());
    }
    for field in ["version", "expiresAt"] {
        let mut value = reference("package");
        value[field] = json!(1);
        assert!(shape(&wire(&value)).is_err());
        let mut value = reference("root");
        value[field] = Value::Null;
        assert!(shape(&wire(&value)).is_err());
    }
}
#[test]
fn original_six_metadata_fixtures_bind_raw_bytes_and_every_reference_field() {
    let vectors: Value = serde_json::from_slice(
        &fs::read(root().join("specs/upgrade/v1/vectors/crypto/envelopes.json")).unwrap(),
    )
    .unwrap();
    let mut checked = 0;
    for vector in vectors["vectors"].as_array().unwrap() {
        if !ROLES.contains(&vector["expectedContext"]["role"].as_str().unwrap()) {
            continue;
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let expected = derive_metadata_reference(&bytes).unwrap();
        let value: Value = serde_json::from_slice(expected.canonical_bytes()).unwrap();
        assert_eq!(value["envelopeSha256"], vector["expectedEnvelopeSha256"]);
        assert_eq!(value["size"], json!(bytes.len()));
        assert!(expected.assert_original_envelope(&bytes).is_ok());
        let pretty = serde_json::to_vec_pretty(
            &Document::parse(&bytes, InputKind::Metadata)
                .unwrap()
                .value(),
        )
        .unwrap();
        assert_eq!(
            expected.assert_original_envelope(&pretty),
            Err(MetadataReferenceError::Mismatch)
        );
        checked += 1;
    }
    assert_eq!(checked, 6);
    let raw = wire(&envelope("root"));
    let original = ref_value(&raw);
    for field in ["role", "version", "expiresAt", "size", "envelopeSha256"] {
        let mut changed = original.clone();
        changed[field] = match field {
            "role" => json!("timestamp"),
            "envelopeSha256" => json!("0".repeat(64)),
            "size" => json!(original["size"].as_u64().unwrap() + 1),
            _ => json!(2),
        };
        let expected = shape(&wire(&changed)).unwrap();
        assert_eq!(
            expected.assert_original_envelope(&raw),
            Err(MetadataReferenceError::Mismatch)
        );
    }
    let raw = wire(&envelope("target"));
    let mut changed = ref_value(&raw);
    changed["scope"]["targetKey"] = json!("other-target");
    assert_eq!(
        shape(&wire(&changed))
            .unwrap()
            .assert_original_envelope(&raw),
        Err(MetadataReferenceError::Mismatch)
    );
}
fn same_length_controls() -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut controls = vec![];
    for field in ["opaque", "signatures"] {
        let mut original = envelope("root");
        original[field] = json!("x");
        let mut changed = original.clone();
        changed[field] = json!("y");
        controls.push((wire(&original), wire(&changed)));
    }
    controls.push((
        br#"{"signed":{"role":"root","scope":{},"version":1,"expiresAt":0}}"#.to_vec(),
        br#"{"signed":{"expiresAt":0,"version":1,"scope":{},"role":"root"}}"#.to_vec(),
    ));
    controls
}
#[test]
fn raw_projection_preserves_selected_fields_and_all_extra_byte_resource_checks() {
    for (original, changed) in same_length_controls() {
        assert_eq!(original.len(), changed.len());
        let expected = derive_metadata_reference(&original).unwrap();
        let mut left = ref_value(&original);
        let mut right = ref_value(&changed);
        assert_ne!(left["envelopeSha256"], right["envelopeSha256"]);
        left.as_object_mut().unwrap().remove("envelopeSha256");
        right.as_object_mut().unwrap().remove("envelopeSha256");
        assert_eq!(left, right);
        assert_eq!(
            expected.assert_original_envelope(&changed),
            Err(MetadataReferenceError::Mismatch)
        );
    }
    let mut release = envelope("release");
    release["signed"]["version"] = json!(999);
    assert_eq!(ref_value(&wire(&release))["version"], json!(1));
    release["signed"]
        .as_object_mut()
        .unwrap()
        .remove("revision");
    assert!(derive_metadata_reference(&wire(&release)).is_err());
    let mut package = envelope("package");
    package["signed"]["version"] = json!({"ignored":"value"});
    assert_eq!(ref_value(&wire(&package))["version"], Value::Null);
    package["signed"]["expiresAt"] = json!(1);
    assert!(derive_metadata_reference(&wire(&package)).is_err());
    let original = wire(&envelope("root"));
    let expected = derive_metadata_reference(&original).unwrap();
    let mut changed = envelope("root");
    changed["signed"]["extra"] = json!({"envelopeSha256":"ignored","other":"byte bound"});
    let changed_wire = wire(&changed);
    assert!(derive_metadata_reference(&changed_wire).is_ok());
    assert_eq!(
        expected.assert_original_envelope(&changed_wire),
        Err(MetadataReferenceError::Mismatch)
    );
    changed["extra"] = json!("x".repeat(65_537));
    assert!(matches!(
        derive_metadata_reference(&wire(&changed)),
        Err(MetadataReferenceError::Json(Error::StringLimit))
    ));
    assert!(matches!(derive_metadata_reference(br#"{"signed":{"role":"root","scope":{},"version":1,"expiresAt":0},"extra":0,"e\u0078tra":1}"#),Err(MetadataReferenceError::Json(Error::DuplicateKey))));
    let raw = wire(&envelope("root"));
    let mut exact = vec![b' '; 1_048_576 - raw.len()];
    exact.extend_from_slice(&raw);
    assert_eq!(ref_value(&exact)["size"], json!(1_048_576));
    exact.push(b' ');
    assert!(matches!(
        derive_metadata_reference(&exact),
        Err(MetadataReferenceError::Json(Error::InputLimit))
    ));
}
#[test]
fn original_node_schema_scope_derivation_and_complete_reference_match_agree() {
    let mut cases: Vec<Value> = vec![];
    for role in ROLES {
        let base = reference(role);
        cases.push(json!({"mode":"shape","input":STANDARD.encode(wire(&base))}));
        let raw = wire(&envelope(role));
        let expected = ref_value(&raw);
        cases.push(json!({"mode":"derive","input":STANDARD.encode(&raw)}));
        cases.push(json!({"mode":"match","input":STANDARD.encode(&raw),"expected":STANDARD.encode(wire(&expected))}));
        cases.push(json!({"mode":"match","input":STANDARD.encode(serde_json::to_vec_pretty(&envelope(role)).unwrap()),"expected":STANDARD.encode(wire(&expected))}));
        for field in base.as_object().unwrap().keys() {
            let mut missing = base.clone();
            missing.as_object_mut().unwrap().remove(field);
            cases.push(json!({"mode":"shape","input":STANDARD.encode(wire(&missing))}));
            for bad in [
                Value::Null,
                json!(false),
                json!(0),
                json!(-1),
                json!(1.5),
                json!(""),
                json!("bad\n"),
                json!([]),
                json!({}),
            ] {
                let mut value = base.clone();
                value[field] = bad;
                cases.push(json!({"mode":"shape","input":STANDARD.encode(wire(&value))}));
            }
        }
        for scope in [
            json!({"channel":"x","targetKey":"y"}),
            json!({"releaseId":"x"}),
            json!({"targetKey":"y"}),
            json!({}),
            json!({"channel":"x","targetKey":"y","extra":"z"}),
            json!({"channel":1,"targetKey":"y"}),
        ] {
            let mut value = base.clone();
            value["scope"] = scope;
            cases.push(json!({"mode":"shape","input":STANDARD.encode(wire(&value))}));
        }
        let mut extra = base.clone();
        extra["extra"] = Value::Null;
        cases.push(json!({"mode":"shape","input":STANDARD.encode(wire(&extra))}));
        for field in ["role", "scope", "version", "revision", "expiresAt"] {
            for bad in [
                Value::Null,
                json!(false),
                json!(0),
                json!(1.5),
                json!("unknown"),
                json!([]),
                json!({}),
            ] {
                let mut raw = envelope(role);
                raw["signed"][field] = bad;
                cases.push(json!({"mode":"derive","input":STANDARD.encode(wire(&raw))}));
            }
            let mut raw = envelope(role);
            raw["signed"].as_object_mut().unwrap().remove(field);
            cases.push(json!({"mode":"derive","input":STANDARD.encode(wire(&raw))}));
        }
    }
    let base = reference("root");
    for field in ["version", "expiresAt", "size"] {
        for number in [
            "-0",
            "0",
            "1",
            "1.00000000000000001",
            "1048576",
            "1048577",
            "9007199254740991",
            "9007199254740991.1",
            "9007199254740992",
            "1e400",
        ] {
            let raw = String::from_utf8(wire(&base)).unwrap();
            let old = base[field].to_string();
            let bytes = raw
                .replace(
                    &format!("\"{field}\":{old}"),
                    &format!("\"{field}\":{number}"),
                )
                .into_bytes();
            cases.push(json!({"mode":"shape","input":STANDARD.encode(bytes)}));
        }
    }
    let raw = wire(&envelope("root"));
    let mut exact = vec![b' '; 1_048_576 - raw.len()];
    exact.extend_from_slice(&raw);
    cases.push(json!({"mode":"derive","input":STANDARD.encode(&exact)}));
    exact.push(b' ');
    cases.push(json!({"mode":"derive","input":STANDARD.encode(&exact)}));
    for raw in [br#"{"signed":{"role":"root","scope":{},"version":1,"expiresAt":0},"extra":0,"e\u0078tra":1}"#.to_vec(),vec![b'{',0xff,b'}'],b"null".to_vec()] {cases.push(json!({"mode":"derive","input":STANDARD.encode(raw)}));}
    let vectors: Value = serde_json::from_slice(
        &fs::read(root().join("specs/upgrade/v1/vectors/crypto/envelopes.json")).unwrap(),
    )
    .unwrap();
    for vector in vectors["vectors"].as_array().unwrap() {
        let raw = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        cases.push(json!({"mode":"derive","input":STANDARD.encode(raw)}));
    }
    for (original, changed) in same_length_controls() {
        cases.push(json!({"mode":"derive","input":STANDARD.encode(&original)}));
        cases.push(json!({"mode":"derive","input":STANDARD.encode(&changed)}));
        cases.push(json!({"mode":"match","input":STANDARD.encode(&changed),"expected":STANDARD.encode(wire(&ref_value(&original)))}));
    }
    let expected: Vec<_> = cases
        .iter()
        .map(|case| {
            let bytes = STANDARD.decode(case["input"].as_str().unwrap()).unwrap();
            let result = if case["mode"] == "shape" {
                shape(&bytes)
            } else {
                derive_metadata_reference(&bytes)
            };
            if case["mode"] == "match" {
                let declared = STANDARD.decode(case["expected"].as_str().unwrap()).unwrap();
                match shape(&declared)
                    .and_then(|expected| expected.assert_original_envelope(&bytes))
                {
                    Ok(()) => json!(true),
                    Err(MetadataReferenceError::Mismatch) => json!(false),
                    Err(_) => Value::Null,
                }
            } else {
                result
                    .map(|value| json!(STANDARD.encode(value.canonical_bytes())))
                    .unwrap_or(Value::Null)
            }
        })
        .collect();
    let reference = root()
        .join("specs/upgrade/v1/reference/signatures.mjs")
        .to_string_lossy()
        .into_owned();
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("reference-oracle.mjs"),
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
        .write_all(&serde_json::to_vec(&cases).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "original reference oracle unavailable"
    );
    let actual: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (a, b)) in actual.iter().zip(&expected).enumerate() {
        assert!(a == b, "reference oracle mismatch at case {index}");
    }
    eprintln!(
        "{} original Node/Ajv metadata reference cases passed",
        expected.len()
    );
}
