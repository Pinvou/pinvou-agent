use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pinvou_upgrade_codec::{
    Document, InputKind, MetadataReferenceError, ReleasePackageError,
    assert_supplied_release_package_binding, decode_canonical_envelope, derive_metadata_reference,
};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
const KEYS: [&str; 4] = [
    "windows-x86_64-nsis-perMachine",
    "linux-x86_64-deb-perMachine",
    "linux-arm64-deb-perMachine",
    "macos-universal-dmg-perMachine",
];
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
fn original(role: &str) -> Value {
    let v = fixtures().into_iter().find(|v| v["name"] == role).unwrap();
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
fn rebind(release: &mut Value, package: &Value) {
    let raw = wire(package);
    let reference = derive_metadata_reference(&raw).unwrap();
    reference.assert_original_envelope(&raw).unwrap();
    release["signed"]["targets"][0]["packageManifest"] =
        serde_json::from_slice(reference.canonical_bytes()).unwrap();
}
fn pair(key: &str) -> (Value, Value) {
    let mut release = original("release");
    let mut package = original("package");
    package["signed"]["scope"]["targetKey"] = json!(key);
    package["signed"]["activationModes"] = json!(["directInstall", "stagedRestart"]);
    release["signed"]["targets"][0]["targetKey"] = json!(key);
    release["signed"]["targets"][0]["activationModes"] = json!(["directInstall"]);
    release["signed"]["appVersion"] = package["signed"]["appVersion"].clone();
    release["signed"]["targets"][0]["launcher"] = package["signed"]["launcher"].clone();
    rebind(&mut release, &package);
    (release, package)
}
fn kind(raw: &[u8]) -> InputKind {
    let v: Value = serde_json::from_slice(raw).unwrap_or(Value::Null);
    if [
        "root",
        "timestamp",
        "snapshot",
        "target",
        "release",
        "package",
    ]
    .contains(&v["signed"]["role"].as_str().unwrap_or(""))
    {
        InputKind::Metadata
    } else {
        InputKind::Credential
    }
}
fn check_raw(release: &[u8], package: &[u8], key: &[u8]) -> Result<(), ReleasePackageError> {
    let r = decode_canonical_envelope(release, kind(release))
        .map_err(ReleasePackageError::Envelope)?
        .check_full_schema()
        .map_err(ReleasePackageError::Schema)?;
    let p = decode_canonical_envelope(package, kind(package))
        .map_err(ReleasePackageError::Envelope)?
        .check_full_schema()
        .map_err(ReleasePackageError::Schema)?;
    let k = Document::parse(key, InputKind::Metadata).map_err(ReleasePackageError::Json)?;
    assert_supplied_release_package_binding(&r, &p, &k)
}
fn check(release: &Value, package: &Value, key: &Value) -> Result<(), ReleasePackageError> {
    check_raw(
        &wire(release),
        &wire(package),
        &serde_json::to_vec(key).unwrap(),
    )
}
#[test]
fn all_known_target_relationships_keep_mode_subset_and_do_not_claim_context_or_native_authenticity()
{
    for key in KEYS {
        let (mut r, mut p) = pair(key);
        assert_eq!(check(&r, &p, &json!(key)), Ok(()));
        p["signed"]["activationModes"] = json!(["stagedRestart", "directInstall"]);
        r["signed"]["targets"][0]["activationModes"] = json!(["stagedRestart", "directInstall"]);
        rebind(&mut r, &p);
        assert_eq!(check(&r, &p, &json!(key)), Ok(()));
        p["signed"]["product"] = json!("another-product");
        p["signed"]["component"] = json!("engine");
        p["signatures"][0]["signature"] = json!(URL_SAFE_NO_PAD.encode([0; 64]));
        rebind(&mut r, &p);
        assert_eq!(check(&r, &p, &json!(key)), Ok(())); // Independent context/crypto remain outside.
    }
}
fn relationship_changes(current_key: &str) -> [(&'static str, Value); 8] {
    [
        (
            "/signed/scope/targetKey",
            json!(if current_key == KEYS[2] {
                KEYS[1]
            } else {
                KEYS[2]
            }),
        ),
        ("/signed/appVersion", json!("9.0.0")),
        ("/signed/activationModes", json!(["stagedRestart"])),
        ("/signed/launcher/size", json!(1)),
        ("/signed/launcher/sha256", json!("b".repeat(64))),
        ("/signed/launcher/identity", json!("another-launcher")),
        ("/signed/launcher/publisher", json!("another-publisher")),
        ("/signed/launcher/format", json!("helper")),
    ]
}
#[test]
fn every_package_relationship_is_tested_after_successful_complete_reference_rebinding() {
    let key = KEYS[1];
    let (r, p) = pair(key);
    let cases = relationship_changes(key);
    for (path, value) in cases {
        let mut package = p.clone();
        *package.pointer_mut(path).unwrap() = value;
        let mut release = r.clone();
        rebind(&mut release, &package);
        // Independently prove the six-field reference passes before testing the relation.
        let ref_doc = Document::parse(
            &serde_json::to_vec(&release["signed"]["targets"][0]["packageManifest"]).unwrap(),
            InputKind::Metadata,
        )
        .unwrap();
        ref_doc
            .metadata_reference_shape()
            .unwrap()
            .assert_original_envelope(&wire(&package))
            .unwrap();
        assert_eq!(
            check(&release, &package, &json!(key)),
            Err(ReleasePackageError::BindingMismatch),
            "{path}"
        );
        rebind(&mut release, &p);
        assert_eq!(check(&release, &p, &json!(key)), Ok(()));
    }
    let mut release = r.clone();
    release["signed"]["targets"][0]["activationModes"] = json!(["directInstall", "stagedRestart"]);
    let mut package = p.clone();
    package["signed"]["activationModes"] = json!(["directInstall"]);
    rebind(&mut release, &package);
    assert_eq!(
        check(&release, &package, &json!(key)),
        Err(ReleasePackageError::BindingMismatch)
    );
}
#[test]
fn original_complete_package_bytes_and_first_target_match_cannot_be_replaced_by_equivalent_body() {
    let key = KEYS[1];
    let (mut r, mut p) = pair(key);
    p["signatures"][0]["signature"] = json!(URL_SAFE_NO_PAD.encode([0; 64]));
    assert_eq!(
        check(&r, &p, &json!(key)),
        Err(ReleasePackageError::Reference(
            MetadataReferenceError::Mismatch
        ))
    );
    rebind(&mut r, &p);
    assert_eq!(check(&r, &p, &json!(key)), Ok(()));
    let mut duplicate = r["signed"]["targets"][0].clone();
    r["signed"]["targets"][0]["launcher"]["identity"] = json!("wrong-first");
    duplicate["releaseTargetId"] = json!("different-entry");
    r["signed"]["targets"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert_eq!(
        check(&r, &p, &json!(key)),
        Err(ReleasePackageError::BindingMismatch)
    ); // find, never any matching entry.
    for k in [Value::Null, json!({}), json!(1), json!("unknown-target")] {
        assert_eq!(check(&r, &p, &k), Err(ReleasePackageError::TargetInvalid));
    }
    assert_eq!(
        check(&r, &p, &json!(KEYS[2])),
        Err(ReleasePackageError::TargetMissing)
    );
    let wrong = original("root");
    assert_eq!(
        check(&wrong, &p, &json!(key)),
        Err(ReleasePackageError::RoleInvalid)
    );
    assert_eq!(
        check(&r, &wrong, &json!(key)),
        Err(ReleasePackageError::RoleInvalid)
    );
    let text = format!("\"{}\"", "x".repeat(65537));
    assert!(check_raw(&wire(&r), &wire(&p), text.as_bytes()).is_err());
}
fn leaves(v: &Value, path: &str, out: &mut Vec<String>) {
    match v {
        Value::Object(o) => {
            for (k, c) in o {
                leaves(c, &format!("{path}/{k}"), out);
            }
        }
        Value::Array(a) => {
            for (i, c) in a.iter().enumerate() {
                leaves(c, &format!("{path}/{i}"), out);
            }
        }
        _ => out.push(path.to_owned()),
    }
}
#[test]
fn actual_original_node_reference_and_relationship_fragment_matches_rebound_negative_and_positive_controls()
 {
    let mut cases = Vec::new();
    let mut add = |r: &Value, p: &Value, k: &Value| {
        let a = &r["signed"];
        let b = &p["signed"];
        cases.push(json!({"release":STANDARD.encode(wire(r)),"packageRaw":STANDARD.encode(wire(p)),"targetRaw":STANDARD.encode(serde_json::to_vec(k).unwrap()),
   "releaseExpected":{"role":a["role"],"product":a["product"],"component":a["component"],"scope":a["scope"]},
   "packageExpected":{"role":b["role"],"product":b["product"],"component":b["component"],"scope":b["scope"]}}));
    };
    for key in KEYS {
        let (r, p) = pair(key);
        assert_eq!(check(&r, &p, &json!(key)), Ok(()));
        add(&r, &p, &json!(key));
        for (path, value) in relationship_changes(key) {
            let mut package = p.clone();
            *package.pointer_mut(path).unwrap() = value;
            let mut release = r.clone();
            rebind(&mut release, &package);
            let reference = Document::parse(
                &serde_json::to_vec(&release["signed"]["targets"][0]["packageManifest"]).unwrap(),
                InputKind::Metadata,
            )
            .unwrap()
            .metadata_reference_shape()
            .unwrap();
            reference.assert_original_envelope(&wire(&package)).unwrap();
            assert_eq!(
                check(&release, &package, &json!(key)),
                Err(ReleasePackageError::BindingMismatch),
                "{key} {path}"
            );
            add(&release, &package, &json!(key));
            rebind(&mut release, &p);
            assert_eq!(check(&release, &p, &json!(key)), Ok(()));
            add(&release, &p, &json!(key));
        }
        let mut first = r.clone();
        let mut duplicate = first["signed"]["targets"][0].clone();
        duplicate["releaseTargetId"] = json!("different-entry");
        first["signed"]["targets"][0]["launcher"]["identity"] = json!("wrong-first");
        first["signed"]["targets"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        add(&first, &p, &json!(key));
        let mut paths = Vec::new();
        leaves(&p["signed"], "/signed", &mut paths);
        for path in paths {
            for value in [
                Value::Null,
                json!(false),
                json!(0),
                json!(1),
                json!(2),
                json!(9007199254740991u64),
                json!("different"),
                json!("9.0.0"),
                json!("b".repeat(64)),
                json!({}),
                json!([]),
            ] {
                let mut package = p.clone();
                *package.pointer_mut(&path).unwrap() = value;
                let mut release = r.clone();
                // Changes outside role/scope must also exercise post-reference relationships;
                // invalid projections remain Schema/shape negatives, with no hidden no-op.
                if let Ok(reference) = derive_metadata_reference(&wire(&package)) {
                    release["signed"]["targets"][0]["packageManifest"] =
                        serde_json::from_slice(reference.canonical_bytes()).unwrap();
                }
                add(&release, &package, &json!(key));
            }
        }
        let mut paths = Vec::new();
        leaves(&r["signed"]["targets"][0], "", &mut paths);
        for path in paths {
            for value in [
                Value::Null,
                json!(0),
                json!(1),
                json!(2),
                json!("different"),
                json!("9.0.0"),
                json!("b".repeat(64)),
                json!({}),
                json!([]),
            ] {
                let mut release = r.clone();
                *release["signed"]["targets"][0].pointer_mut(&path).unwrap() = value;
                add(&release, &p, &json!(key));
            }
        }
        for modes in [
            json!(["directInstall"]),
            json!(["stagedRestart"]),
            json!(["directInstall", "stagedRestart"]),
            json!(["stagedRestart", "directInstall"]),
        ] {
            let mut package = p.clone();
            package["signed"]["activationModes"] = modes;
            for target_modes in [
                json!(["directInstall"]),
                json!(["stagedRestart"]),
                json!(["directInstall", "stagedRestart"]),
                json!(["stagedRestart", "directInstall"]),
            ] {
                let mut release = r.clone();
                release["signed"]["targets"][0]["activationModes"] = target_modes;
                rebind(&mut release, &package);
                add(&release, &package, &json!(key));
            }
        }
        let mut context = p.clone();
        context["signed"]["product"] = json!("different-product");
        context["signed"]["component"] = json!("engine");
        let mut release = r.clone();
        rebind(&mut release, &context);
        add(&release, &context, &json!(key));
        let mut sig = p.clone();
        sig["signatures"][0]["signature"] = json!(URL_SAFE_NO_PAD.encode([0; 64]));
        add(&r, &sig, &json!(key));
        rebind(&mut release, &sig);
        add(&release, &sig, &json!(key));
        for target in [Value::Null, json!(1), json!({}), json!("unsupported")] {
            add(&r, &p, &target);
        }
    }
    let expected: Vec<bool> = cases
        .iter()
        .map(|c| {
            check_raw(
                &STANDARD.decode(c["release"].as_str().unwrap()).unwrap(),
                &STANDARD.decode(c["packageRaw"].as_str().unwrap()).unwrap(),
                &STANDARD.decode(c["targetRaw"].as_str().unwrap()).unwrap(),
            )
            .is_ok()
        })
        .collect();
    let reference = root().join("specs/upgrade/v1/reference/signatures.mjs");
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("release-package-oracle.mjs"),
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
        "original Release/Package binding oracle unavailable"
    );
    let actual: Vec<bool> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (i, (a, b)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(a, b, "Release/Package binding case {i}");
    }
    eprintln!(
        "{} actual original Node Release/Package binding cases passed",
        cases.len()
    );
}
