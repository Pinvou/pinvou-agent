use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pinvou_upgrade_codec::{
    Document, InputKind, PublicGraphError, ReleasePackageError, RootWindowMode,
    SchemaCheckedEnvelope, assert_supplied_metadata_semantics, assert_supplied_public_graph,
    decode_canonical_envelope, derive_metadata_reference,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(7)
        .unwrap()
        .to_owned()
}
fn original(role: &str) -> Value {
    let vectors: Value = serde_json::from_slice(
        &std::fs::read(root().join("specs/upgrade/v1/vectors/crypto/envelopes.json")).unwrap(),
    )
    .unwrap();
    let v = vectors["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == role)
        .unwrap();
    serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(v["envelopeBase64Url"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap()
}
fn wire(v: &Value) -> Vec<u8> {
    Document::parse(&serde_json::to_vec(v).unwrap(), InputKind::Metadata)
        .unwrap()
        .canonical_bytes()
        .unwrap()
}
fn reference(v: &Value) -> Value {
    serde_json::from_slice(
        derive_metadata_reference(&wire(v))
            .unwrap()
            .canonical_bytes(),
    )
    .unwrap()
}
fn graph() -> (Value, Value, Vec<Value>) {
    let mut package = original("package");
    package["signed"]["activationModes"] = json!(["directInstall", "stagedRestart"]);
    let mut release = original("release");
    release["signed"]["targets"][0]["packageManifest"] = reference(&package);
    release["signed"]["targets"][0]["launcher"] = package["signed"]["launcher"].clone();
    let mut target = original("target");
    target["signed"]["baselineRelease"] = reference(&release);
    let members = vec![target, release, package];
    let mut snapshot = original("snapshot");
    snapshot["signed"]["entries"] = json!(members.iter().map(reference).collect::<Vec<_>>());
    let mut timestamp = original("timestamp");
    timestamp["signed"]["snapshot"] = reference(&snapshot);
    (timestamp, snapshot, members)
}
fn rebuild(t: &mut Value, s: &mut Value, m: &[Value]) {
    s["signed"]["entries"] = json!(m.iter().map(reference).collect::<Vec<_>>());
    t["signed"]["snapshot"] = reference(s);
}
fn checked(raw: &[u8]) -> SchemaCheckedEnvelope {
    decode_canonical_envelope(raw, InputKind::Metadata)
        .unwrap()
        .check_full_schema()
        .unwrap()
}
fn check(t: &Value, s: &Value, m: &[Value], ctx: &Value) -> Result<(), PublicGraphError> {
    let t = checked(&wire(t));
    let s = checked(&wire(s));
    let m: Vec<_> = m.iter().map(|v| checked(&wire(v))).collect();
    let ctx = Document::parse(&serde_json::to_vec(ctx).unwrap(), InputKind::Metadata).unwrap();
    assert_supplied_public_graph(&t, &s, &m, &ctx)
}
fn ctx() -> Value {
    json!({"product":"pinvou","component":"app"})
}
#[test]
fn complete_references_and_supplied_context_bind_without_a_trusted_graph_result() {
    let (t, s, m) = graph();
    assert_eq!(check(&t, &s, &m, &ctx()), Ok(()));
    let mut extra = ctx();
    extra["ignored"] = json!({"value":1});
    assert_eq!(check(&t, &s, &m, &extra), Ok(()));
    for context in [
        json!({}),
        json!({"product":null,"component":"app"}),
        json!({"product":"pinvou","component":null}),
    ] {
        assert_eq!(
            check(&t, &s, &m, &context),
            Err(PublicGraphError::InvalidContext)
        );
    }
    let mut wrong = ctx();
    wrong["component"] = json!("engine");
    assert!(matches!(
        check(&t, &s, &m, &wrong),
        Err(PublicGraphError::Context(_))
    ));
    let mut timestamp = t.clone();
    timestamp["signed"]["snapshot"]["size"] = json!(1);
    assert!(matches!(
        check(&timestamp, &s, &m, &ctx()),
        Err(PublicGraphError::Reference(_))
    ));
    let mut missing = m.clone();
    missing.pop();
    assert_eq!(
        check(&t, &s, &missing, &ctx()),
        Err(PublicGraphError::MemberMissing)
    );
}
#[test]
fn every_concrete_release_target_is_checked_even_when_schema_only_targets_share_one_key() {
    let (mut t, mut s, mut m) = graph();
    let mut second = m[1]["signed"]["targets"][0].clone();
    second["releaseTargetId"] = json!("second-target");
    m[1]["signed"]["targets"]
        .as_array_mut()
        .unwrap()
        .push(second);
    m[0]["signed"]["baselineRelease"] = reference(&m[1]);
    rebuild(&mut t, &mut s, &m);
    assert_eq!(check(&t, &s, &m, &ctx()), Ok(())); // Pure Schema domain; B17 separately refuses duplicates.
    m[1]["signed"]["targets"][1]["launcher"]["publisher"] = json!("different-publisher");
    m[0]["signed"]["baselineRelease"] = reference(&m[1]);
    rebuild(&mut t, &mut s, &m);
    // Prove complete preceding references and the first target are still correct.
    derive_metadata_reference(&wire(&m[1]))
        .unwrap()
        .assert_original_envelope(&wire(&m[1]))
        .unwrap();
    assert_eq!(
        m[1]["signed"]["targets"][0]["launcher"],
        m[2]["signed"]["launcher"]
    );
    assert_eq!(
        check(&t, &s, &m, &ctx()),
        Err(PublicGraphError::Package(
            ReleasePackageError::BindingMismatch
        ))
    );
    m[1]["signed"]["targets"][1]["launcher"] = m[2]["signed"]["launcher"].clone();
    m[0]["signed"]["baselineRelease"] = reference(&m[1]);
    rebuild(&mut t, &mut s, &m);
    assert_eq!(check(&t, &s, &m, &ctx()), Ok(()));
}
#[test]
fn duplicate_reference_map_semantics_and_4096_derived_index_bound_do_not_add_an_aggregate_cap() {
    let (mut t, mut s, m) = graph();
    let repeated = s["signed"]["entries"][2].clone();
    while s["signed"]["entries"].as_array().unwrap().len() < 4096 {
        s["signed"]["entries"]
            .as_array_mut()
            .unwrap()
            .push(repeated.clone());
    }
    t["signed"]["snapshot"] = reference(&s);
    assert_eq!(check(&t, &s, &m, &ctx()), Ok(())); // Original pure Map overwrite, not B17 reference uniqueness.
    let mut duplicates = m.clone();
    duplicates.push(m[2].clone());
    assert_eq!(
        check(&t, &s, &duplicates, &ctx()),
        Err(PublicGraphError::MemberDuplicate)
    );
    let mut omitted = s.clone();
    omitted["signed"]["entries"]
        .as_array_mut()
        .unwrap()
        .retain(|r| r["role"] != "target");
    t["signed"]["snapshot"] = reference(&omitted);
    assert_eq!(
        check(&t, &omitted, &m, &ctx()),
        Err(PublicGraphError::MemberUnreferenced)
    );
    let mut over = s.clone();
    over["signed"]["entries"]
        .as_array_mut()
        .unwrap()
        .push(repeated);
    assert_eq!(
        Document::parse(&serde_json::to_vec(&over).unwrap(), InputKind::Metadata).err(),
        Some(pinvou_upgrade_codec::Error::MemberLimit),
    ); // 4097 raw members.
}
#[test]
fn actual_node_full_export_with_fresh_authorized_signed_inputs_matches_rust_provided_composition() {
    let reference = root().join("specs/upgrade/v1/reference/signatures.mjs");
    let output = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("public-graph-cases.mjs"),
            reference.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "actual signed Node graph oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cases: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert!(cases.len() >= 28);
    assert!(
        cases
            .iter()
            .any(|c| c["name"] == "strong-signed-complete-positive" && c["accepted"] == true)
    );
    for case in &cases {
        let t = checked(
            &STANDARD
                .decode(case["timestamp"].as_str().unwrap())
                .unwrap(),
        );
        let s = checked(&STANDARD.decode(case["snapshot"].as_str().unwrap()).unwrap());
        let m: Vec<_> = case["members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| checked(&STANDARD.decode(b.as_str().unwrap()).unwrap()))
            .collect();
        let time = Document::parse(case["now"].to_string().as_bytes(), InputKind::Metadata)
            .unwrap()
            .protocol_time_shape()
            .unwrap();
        // This composed comparison includes metadata semantics. Actual Node already
        // preflighted every exact signed envelope against real Root policies/time.
        // Rust still performs no cryptographic validation and creates no trust result.
        for e in std::iter::once(&t)
            .chain(std::iter::once(&s))
            .chain(m.iter())
        {
            assert_supplied_metadata_semantics(e, &time, RootWindowMode::Current).unwrap();
        }
        let context = Document::parse(
            &serde_json::to_vec(&case["context"]).unwrap(),
            InputKind::Metadata,
        )
        .unwrap();
        let actual = assert_supplied_public_graph(&t, &s, &m, &context).is_ok();
        assert_eq!(
            actual,
            case["accepted"].as_bool().unwrap(),
            "graph case {}",
            case["name"]
        );
    }
    eprintln!(
        "{} actually signed and independently preflighted Node public graph cases passed",
        cases.len()
    );
}
