use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{Document, Error, InputKind, SourceProfileError};
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
fn facts() -> Value {
    fixture()["sourceFacts"].clone()
}
fn wire(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn document(bytes: &[u8]) -> Document {
    Document::parse(bytes, InputKind::Metadata).unwrap()
}
fn id(value: &Value) -> String {
    document(&wire(value))
        .source_profile()
        .unwrap()
        .source_profile_id()
        .to_owned()
}
fn expected(bytes: &[u8]) -> Option<(String, String)> {
    Document::parse(bytes, InputKind::Metadata)
        .ok()?
        .source_profile()
        .ok()
        .map(|result| {
            (
                result.source_profile_id().to_owned(),
                STANDARD.encode(result.canonical_projection()),
            )
        })
}
#[test]
fn frozen_stable_projection_and_reordered_whitespace_have_identical_jcs_and_digest() {
    let fixture = fixture();
    let source = fixture["sourceFacts"].clone();
    let result = document(&wire(&source)).source_profile().unwrap();
    assert_eq!(
        result.source_profile_id(),
        fixture["expectedSourceProfileId"].as_str().unwrap()
    );
    assert_eq!(
        result.canonical_projection(),
        serde_jcs::to_vec(&source).unwrap()
    );
    let mut fields: Vec<_> = source.as_object().unwrap().iter().collect();
    fields.reverse();
    let ordered = fields
        .into_iter()
        .map(|(key, value)| {
            format!(
                "{} : {}",
                serde_json::to_string(key).unwrap(),
                serde_json::to_string(value).unwrap()
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    assert_eq!(
        document(format!(" {{ {ordered} }} ").as_bytes())
            .source_profile()
            .unwrap()
            .source_profile_id(),
        result.source_profile_id()
    );
    assert!(!result.canonical_projection().starts_with(b"pinvou-"));
}
#[test]
fn outer_dynamic_and_self_fields_are_excluded_but_every_stable_fact_is_bound() {
    let original = facts();
    let expected = id(&original);
    let mut dynamic = original.clone();
    for key in [
        "sourceProfileId",
        "registryVersion",
        "ownerEpoch",
        "freezeEpoch",
        "transactionId",
        "permissionState",
        "policyId",
        "approval",
        "unrecognizedExtra",
    ] {
        dynamic[key] = json!({"arbitrary":"excluded", "nested":null});
    }
    assert_eq!(id(&dynamic), expected);
    for (pointer, value) in stable_mutations() {
        let mut changed = dynamic.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        assert_ne!(id(&changed), expected, "unbound stable field {pointer}");
    }
}
fn stable_mutations() -> Vec<(&'static str, Value)> {
    let mut changes = vec![
        ("/product", json!("other")),
        ("/component", json!("other")),
        ("/targetKey", json!("windows-x86_64-nsis-perMachine")),
        ("/canonicalAppVersion", json!("2.0.0")),
        ("/installation/format", json!("nsis")),
        ("/installation/ownershipProtocol", json!("owner-v2")),
        ("/dataMigration/dataFormat", json!("data-v2")),
        ("/dataMigration/migrationProtocol", json!("migration-v2")),
    ];
    for (pointer, value) in [
        ("/application/size", json!(124)),
        ("/application/sha256", json!("b".repeat(64))),
        ("/application/identity", json!("app-2")),
        ("/application/publisher", json!("publisher-2")),
        ("/application/format", json!("helper")),
        ("/helper/size", json!(124)),
        ("/helper/sha256", json!("b".repeat(64))),
        ("/helper/identity", json!("helper-2")),
        ("/helper/publisher", json!("publisher-2")),
        ("/helper/format", json!("deb")),
        ("/launcher/size", json!(124)),
        ("/launcher/sha256", json!("b".repeat(64))),
        ("/launcher/identity", json!("launcher-2")),
        ("/launcher/publisher", json!("publisher-2")),
        ("/launcher/format", json!("deb")),
    ] {
        changes.push((pointer, value));
    }
    changes
}
fn shape_samples() -> Vec<Vec<u8>> {
    let mut samples = vec![wire(&facts())];
    let common: Value = serde_json::from_slice(
        &fs::read(root().join("specs/upgrade/v1/schemas/common.json")).unwrap(),
    )
    .unwrap();
    for target in common["$defs"]["stableFacts"]["properties"]["targetKey"]["enum"]
        .as_array()
        .unwrap()
    {
        let mut changed = facts();
        changed["targetKey"] = target.clone();
        samples.push(wire(&changed));
    }
    for format in common["$defs"]["nativeIdentity"]["properties"]["format"]["enum"]
        .as_array()
        .unwrap()
    {
        for object in ["application", "helper", "launcher"] {
            let mut changed = facts();
            changed[object]["format"] = format.clone();
            samples.push(wire(&changed));
        }
    }
    for format in
        common["$defs"]["stableFacts"]["properties"]["installation"]["properties"]["format"]["enum"]
            .as_array()
            .unwrap()
    {
        let mut changed = facts();
        changed["installation"]["format"] = format.clone();
        samples.push(wire(&changed));
    }

    for (pointer, value) in stable_mutations() {
        let mut changed = facts();
        *changed.pointer_mut(pointer).unwrap() = value;
        samples.push(wire(&changed));
    }
    for field in [
        "product",
        "component",
        "targetKey",
        "canonicalAppVersion",
        "application",
        "helper",
        "launcher",
        "installation",
        "dataMigration",
    ] {
        let mut missing = facts();
        missing.as_object_mut().unwrap().remove(field);
        samples.push(wire(&missing));
        for value in [Value::Null, json!(false), json!([]), json!({})] {
            let mut changed = facts();
            changed[field] = value;
            samples.push(wire(&changed));
        }
    }
    for object in [
        "application",
        "helper",
        "launcher",
        "installation",
        "dataMigration",
    ] {
        let names: Vec<_> = facts()[object]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        for name in names {
            let mut changed = facts();
            changed[object].as_object_mut().unwrap().remove(&name);
            samples.push(wire(&changed));
            let mut changed = facts();
            changed[object][&name] = Value::Null;
            samples.push(wire(&changed));
        }
        let mut changed = facts();
        changed[object]["ownerEpoch"] = json!(1);
        samples.push(wire(&changed));
    }
    for token in [
        "",
        "_bad",
        "a b",
        "\u{00e9}",
        "A",
        "A._:-9",
        "123",
        &"a".repeat(128),
        &"a".repeat(129),
    ] {
        for pointer in ["/product", "/helper/publisher", "/dataMigration/dataFormat"] {
            let mut changed = facts();
            *changed.pointer_mut(pointer).unwrap() = json!(token);
            samples.push(wire(&changed));
        }
    }
    for ending in ["\n", "\r", "\r\n", "\u{2028}", "\u{2029}"] {
        for pointer in ["/product", "/canonicalAppVersion", "/application/sha256"] {
            let mut changed = facts();
            let text = format!(
                "{}{ending}",
                changed.pointer(pointer).unwrap().as_str().unwrap()
            );
            *changed.pointer_mut(pointer).unwrap() = json!(text);
            samples.push(wire(&changed));
        }
    }
    for version in [
        "0.0.0",
        "01.0.0",
        "1.2",
        "1.2.3.4",
        "1.0.0-beta",
        "1.0.0+build",
        "1..0",
        "-1.0.0",
        "\u{0661}.0.0",
        &format!("{}.0.0", "9".repeat(200)),
        &format!("{}.0.0", "9".repeat(65532)),
    ] {
        let mut changed = facts();
        changed["canonicalAppVersion"] = json!(version);
        samples.push(wire(&changed));
    }
    for size in [
        json!(0),
        json!(-0.0),
        json!(-1),
        json!(1),
        json!(1.5),
        json!(9007199254740990_u64),
        json!(9007199254740991_u64),
        json!(9007199254740992_u64),
        json!("123"),
        json!(true),
    ] {
        let mut changed = facts();
        changed["application"]["size"] = size;
        samples.push(wire(&changed));
    }
    for hash in [
        "a".repeat(63),
        "A".repeat(64),
        "g".repeat(64),
        "0".repeat(64),
        "a".repeat(65),
    ] {
        let mut changed = facts();
        changed["application"]["sha256"] = json!(hash);
        samples.push(wire(&changed));
    }
    for (pointer, value) in [
        ("/targetKey", json!("linux-x86_64-deb-perUser")),
        ("/installation/installScope", json!("perUser")),
        ("/installation/format", json!("helper")),
        ("/helper/format", json!("zip")),
    ] {
        let mut changed = facts();
        *changed.pointer_mut(pointer).unwrap() = value;
        samples.push(wire(&changed));
    }
    // Long numeric version and otherwise mismatched role/format are legal Schema
    // shapes; actual platform/Registry policy must not be invented here.
    let mut dynamic = facts();
    dynamic["arbitrary"] = json!({"nested":{"extra":1}});
    samples.push(wire(&dynamic));
    for size in [
        "1.0",
        "1e0",
        "9007199254740990.5",
        "9007199254740991.4",
        "9007199254740991.5",
        "1e309",
    ] {
        let text = String::from_utf8(wire(&facts())).unwrap().replacen(
            "\"size\":123",
            &format!("\"size\":{size}"),
            1,
        );
        samples.push(text.into_bytes());
    }
    samples
}
#[test]
fn closed_nested_shapes_and_safe_binary64_sizes_refuse_invalid_facts() {
    for object in [
        "application",
        "helper",
        "launcher",
        "installation",
        "dataMigration",
    ] {
        let mut changed = facts();
        changed[object]["ownerEpoch"] = json!(1);
        assert!(matches!(
            document(&wire(&changed)).source_profile(),
            Err(SourceProfileError::InvalidStableFacts)
        ));
    }
    for size in [0_u64, 9007199254740992] {
        let mut changed = facts();
        changed["application"]["size"] = json!(size);
        assert!(matches!(
            document(&wire(&changed)).source_profile(),
            Err(SourceProfileError::InvalidStableFacts)
        ));
    }
    let mut changed = facts();
    changed["canonicalAppVersion"] = json!(format!("{}.0.0", "9".repeat(65532)));
    assert!(document(&wire(&changed)).source_profile().is_ok());
    changed["canonicalAppVersion"] = json!(format!("{}.0.0", "9".repeat(65533)));
    assert!(matches!(
        Document::parse(&wire(&changed), InputKind::Metadata),
        Err(Error::StringLimit)
    ));
}
#[test]
fn duplicate_keys_and_resources_in_excluded_outer_facts_still_fail_before_projection() {
    let text = String::from_utf8(wire(&facts())).unwrap();
    let duplicate = text.replacen(
        "\"product\":\"pinvou\"",
        "\"product\":\"pinvou\",\"pro\\u0064uct\":\"pinvou\"",
        1,
    );
    assert!(matches!(
        Document::parse(duplicate.as_bytes(), InputKind::Metadata),
        Err(Error::DuplicateKey)
    ));
    let duplicate = format!(
        "{},\"ownerEpoch\":1,\"owner\\u0045poch\":2}}",
        &text[..text.len() - 1]
    );
    assert!(matches!(
        Document::parse(duplicate.as_bytes(), InputKind::Metadata),
        Err(Error::DuplicateKey)
    ));
    for arrays in [31, 32] {
        let input = format!(
            "{},\"ignored\":{}0{}}}",
            &text[..text.len() - 1],
            "[".repeat(arrays),
            "]".repeat(arrays)
        );
        if arrays == 31 {
            assert!(document(input.as_bytes()).source_profile().is_ok());
        } else {
            assert!(matches!(
                Document::parse(input.as_bytes(), InputKind::Metadata),
                Err(Error::DepthLimit)
            ));
        }
    }
    let mut ignored = facts();
    ignored["dynamic"] = json!("x".repeat(65537));
    assert!(matches!(
        Document::parse(&wire(&ignored), InputKind::Metadata),
        Err(Error::StringLimit)
    ));
    for index in 0..4088 {
        ignored[format!("ignored-{index}")] = Value::Null;
    }
    ignored.as_object_mut().unwrap().remove("dynamic");
    assert!(matches!(
        Document::parse(&wire(&ignored), InputKind::Metadata),
        Err(Error::MemberLimit)
    ));
    let mut bytes = wire(&facts());
    bytes.resize(1024 * 1024 + 1, b' ');
    assert!(matches!(
        Document::parse(&bytes, InputKind::Metadata),
        Err(Error::InputLimit)
    ));
}
#[test]
fn frozen_node_ajv_reference_agrees_on_all_schema_and_projection_variants() {
    let mut samples = shape_samples();
    let text = String::from_utf8(wire(&facts())).unwrap();
    samples.push(
        format!(
            "{},\"ownerEpoch\":1,\"ownerEpoch\":2}}",
            &text[..text.len() - 1]
        )
        .into_bytes(),
    );
    samples.push(b"null".to_vec());
    samples.push(b"[]".to_vec());
    let results: Vec<_> = samples.iter().map(|bytes| expected(bytes)).collect();
    let reference = root()
        .join("specs/upgrade/v1/reference/digests.mjs")
        .to_string_lossy()
        .into_owned();
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("source-oracle.mjs"),
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
            &serde_json::to_vec(
                &samples
                    .iter()
                    .map(|bytes| STANDARD.encode(bytes))
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "original source-profile oracle unavailable"
    );
    let actual: Vec<Option<(String, String)>> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), results.len());
    for (index, (actual, expected)) in actual.iter().zip(&results).enumerate() {
        assert!(
            actual == expected,
            "source-profile oracle mismatch at case {index}"
        );
    }
    eprintln!(
        "{} original Node/Ajv source-profile cases passed",
        results.len()
    );
}
