use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pinvou_upgrade_codec::{Document, InputKind};
use pinvou_upgrade_verifier::{VerificationError, VerificationInputs, verify_with_supplied_root};
use serde_json::Value;

const VECTORS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../../../../specs/upgrade/v1/vectors/crypto/envelopes.json"
));
fn document(value: &Value) -> Document {
    Document::parse(&serde_json::to_vec(value).unwrap(), InputKind::Metadata).unwrap()
}
fn canonical(value: &Value) -> Vec<u8> {
    document(value).canonical_bytes().unwrap()
}
fn inventory() -> Value {
    serde_json::from_str(VECTORS).unwrap()
}

#[test]
fn all_twenty_original_envelopes_have_actual_signatures_and_original_bytes() {
    let vectors = inventory();
    let root = document(&vectors["trustedRoot"])
        .root_policy_shape()
        .unwrap();
    let mut total = 0;
    for vector in vectors["vectors"].as_array().unwrap() {
        let bytes = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let expected = document(&vector["expectedContext"]);
        let now = document(&vector["now"]).protocol_time_shape().unwrap();
        let result =
            verify_with_supplied_root(&bytes, VerificationInputs::new(&root, &expected, &now));
        let checked =
            result.unwrap_or_else(|error| panic!("original {} refused: {error}", vector["name"]));
        assert_eq!(checked.envelope().envelope().raw_bytes(), bytes);
        assert!(std::ptr::eq(checked.supplied_root(), &root));
        assert_eq!(checked.supplied_time_millis(), now.as_millis());
        total += checked.verified_signer_ids().len();
    }
    assert_eq!(total, 21);
}

#[test]
fn original_root_requires_two_actual_signers_and_refuses_unknown_extra() {
    let vectors = inventory();
    let root = document(&vectors["trustedRoot"])
        .root_policy_shape()
        .unwrap();
    let vector = vectors["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|vector| vector["name"] == "root")
        .unwrap();
    let bytes = URL_SAFE_NO_PAD
        .decode(vector["envelopeBase64Url"].as_str().unwrap())
        .unwrap();
    let original: Value = serde_json::from_slice(&bytes).unwrap();
    let expected = document(&vector["expectedContext"]);
    let now = document(&vector["now"]).protocol_time_shape().unwrap();
    let mut one = original.clone();
    one["signatures"].as_array_mut().unwrap().truncate(1);
    assert!(matches!(
        verify_with_supplied_root(
            &canonical(&one),
            VerificationInputs::new(&root, &expected, &now)
        ),
        Err(VerificationError::ThresholdUnmet)
    ));
    let mut extra = original.clone();
    let mut signature = original["signatures"][0].clone();
    signature["keyId"] = Value::String("f".repeat(64));
    extra["signatures"].as_array_mut().unwrap().push(signature);
    extra["signatures"]
        .as_array_mut()
        .unwrap()
        .sort_by(|a, b| a["keyId"].as_str().cmp(&b["keyId"].as_str()));
    assert!(matches!(
        verify_with_supplied_root(
            &canonical(&extra),
            VerificationInputs::new(&root, &expected, &now)
        ),
        Err(VerificationError::SigningKeyUnauthorized)
    ));
}

#[test]
fn changed_root_cannot_use_the_ordinary_envelope_entry() {
    let vectors = inventory();
    let root = document(&vectors["trustedRoot"])
        .root_policy_shape()
        .unwrap();
    let vector = vectors["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|vector| vector["name"] == "root")
        .unwrap();
    let bytes = URL_SAFE_NO_PAD
        .decode(vector["envelopeBase64Url"].as_str().unwrap())
        .unwrap();
    let mut changed: Value = serde_json::from_slice(&bytes).unwrap();
    changed["signed"]["version"] = Value::from(2);
    let expected = document(&vector["expectedContext"]);
    let now = document(&vector["now"]).protocol_time_shape().unwrap();
    assert!(matches!(
        verify_with_supplied_root(
            &canonical(&changed),
            VerificationInputs::new(&root, &expected, &now)
        ),
        Err(VerificationError::RootTransitionRequired)
    ));
}
