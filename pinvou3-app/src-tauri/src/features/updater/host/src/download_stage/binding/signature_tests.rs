use super::*;
use serde_json::Value;
use std::{path::Path, process::Command, sync::OnceLock};

fn fixture() -> &'static Value {
    static FIXTURE: OnceLock<Value> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let reference = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(6)
            .unwrap()
            .join("specs/upgrade/v1/reference/canonical-json.mjs");
        let output = Command::new("node")
            .args(["--input-type=module", "-e", include_str!("fixture.mjs")])
            .arg(reference)
            .output()
            .unwrap();
        assert!(output.status.success(), "public signing fixture failed");
        serde_json::from_slice(&output.stdout).unwrap()
    })
}
fn text(key: &str) -> &'static str {
    fixture()[key].as_str().unwrap()
}
fn anchor() -> ByteReference {
    ByteReference {
        size: text("root").len() as u64,
        sha256: crate::json::sha256(text("root").as_bytes()),
    }
}
fn host(reference: &ByteReference) -> HostAnchor<'_> {
    HostAnchor {
        bytes: text("root").as_bytes(),
        reference,
        product: "pinvou",
        component: "app",
        target: "windows-x86_64-nsis-perMachine",
    }
}
fn source<'a>() -> PackageInput<'a> {
    PackageInput {
        root_claims_pin: text("claimsPin"),
        envelope: text("package").as_bytes(),
        expected: text("expected").as_bytes(),
        target: "windows-x86_64-nsis-perMachine",
        historical_time: fixture()["now"].as_u64().unwrap(),
    }
}
fn canonical(value: &Value) -> Vec<u8> {
    document(&serde_json::to_vec(value).unwrap())
        .unwrap()
        .canonical_bytes()
        .unwrap()
}

#[test]
fn genuine_root_threshold_and_package_signature_bind_distinct_pins() {
    let pin = anchor();
    assert_ne!(pin.sha256, text("claimsPin"));
    assert!(verify_bound_package(host(&pin), source()).is_ok());
}

#[test]
fn wrong_anchor_claim_pin_and_insufficient_root_threshold_refuse() {
    let pin = anchor();
    let wrong_pin = "0".repeat(64);
    let mut input = source();
    input.root_claims_pin = &wrong_pin;
    assert!(matches!(
        verify_bound_package(host(&pin), input),
        Err(Error::InitialTrustUnavailable)
    ));
    let mut wrong = host(&pin);
    wrong.bytes = text("otherRoot").as_bytes();
    assert!(matches!(
        verify_bound_package(wrong, source()),
        Err(Error::InitialTrustUnavailable)
    ));
    let mut root: Value = serde_json::from_str(text("root")).unwrap();
    root["signatures"].as_array_mut().unwrap().truncate(1);
    let body = canonical(&root);
    let new_pin = ByteReference {
        size: body.len() as u64,
        sha256: crate::json::sha256(&body),
    };
    let mut insufficient = host(&new_pin);
    insufficient.bytes = &body;
    assert!(matches!(
        verify_bound_package(insufficient, source()),
        Err(Error::InitialTrustUnavailable)
    ));
}

#[test]
fn changed_host_scope_or_package_signature_cannot_be_staged() {
    let pin = anchor();
    for field in ["product", "component", "target"] {
        let mut wrong = host(&pin);
        match field {
            "product" => wrong.product = "other",
            "component" => wrong.component = "other",
            _ => wrong.target = "linux-x86_64-deb-perMachine",
        }
        assert!(verify_bound_package(wrong, source()).is_err());
    }
    let mut expected: Value = serde_json::from_str(text("expected")).unwrap();
    expected["scope"]["targetKey"] = Value::String("linux-x86_64-deb-perMachine".into());
    let encoded = canonical(&expected);
    let mut wrong = source();
    wrong.expected = &encoded;
    assert!(verify_bound_package(host(&pin), wrong).is_err());
    let mut package: Value = serde_json::from_str(text("package")).unwrap();
    package["signatures"][0]["signature"] = Value::String("A".repeat(86));
    let encoded = canonical(&package);
    let mut wrong = source();
    wrong.envelope = &encoded;
    assert!(matches!(
        verify_bound_package(host(&pin), wrong),
        Err(Error::ImageRefused)
    ));
}

#[test]
fn historical_root_expiry_does_not_grant_current_rights_or_erase_package_bytes() {
    let pin = anchor();
    let mut historical = source();
    historical.historical_time = fixture()["expiresAt"].as_u64().unwrap() + 1;
    assert!(verify_bound_package(host(&pin), historical).is_ok());
    let mut premature = source();
    premature.historical_time = 0;
    assert!(matches!(
        verify_bound_package(host(&pin), premature),
        Err(Error::ImageRefused)
    ));
}
