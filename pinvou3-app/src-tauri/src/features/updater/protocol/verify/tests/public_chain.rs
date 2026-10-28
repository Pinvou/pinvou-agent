use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{Document, InputKind};
use pinvou_upgrade_verifier::{
    PublicChainError, PublicChainInputs, verify_public_chain, verify_public_chain_interval,
};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path, process::Command, sync::OnceLock};

fn document(value: &Value) -> Document {
    Document::parse(&serde_json::to_vec(value).unwrap(), InputKind::Metadata).unwrap()
}
fn cases() -> &'static [Value] {
    static CASES: OnceLock<Vec<Value>> = OnceLock::new();
    CASES.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(7)
            .unwrap();
        let output = Command::new("node")
            .args([
                "--input-type=module",
                "-e",
                include_str!("public-chain-oracle.mjs"),
            ])
            .arg(root.join("specs/upgrade/v1/reference/signatures.mjs"))
            .output()
            .expect("Node is required for the complete public-chain oracle");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    })
}
fn check_group(group: &str, count: usize) {
    let mut actual = 0;
    for case in cases().iter().filter(|c| c["group"] == group) {
        actual += 1;
        let root = document(&case["trustedRoot"]).root_policy_shape().unwrap();
        let context = document(&case["context"]);
        let now = document(&case["now"]).protocol_time_shape().unwrap();
        let timestamp = STANDARD
            .decode(case["timestamp"].as_str().unwrap())
            .unwrap();
        let snapshot = STANDARD.decode(case["snapshot"].as_str().unwrap()).unwrap();
        let members: Vec<_> = case["members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| STANDARD.decode(m.as_str().unwrap()).unwrap())
            .collect();
        let borrowed: Vec<_> = members.iter().map(Vec::as_slice).collect();
        let denied: Vec<String> = serde_json::from_value(case["deniedKeyIds"].clone()).unwrap();
        let role_denied: BTreeMap<String, Vec<String>> =
            serde_json::from_value(case["deniedKeyIdsByRole"].clone()).unwrap();
        let mut inputs = PublicChainInputs::new(&root, &context, &now).with_denied_key_ids(&denied);
        for (role, ids) in &role_denied {
            inputs = inputs.with_role_denied_key_ids(role, ids);
        }
        match verify_public_chain(&timestamp, &snapshot, &borrowed, inputs) {
            Ok(checked) => {
                assert_eq!(
                    case["accepted"], true,
                    "unexpected chain acceptance {}",
                    case["name"]
                );
                assert!(std::ptr::eq(checked.supplied_root(), &root));
                assert_eq!(checked.supplied_time_millis(), now.as_millis());
                assert_eq!(checked.timestamp().envelope().raw_bytes(), timestamp);
                assert_eq!(checked.snapshot().envelope().raw_bytes(), snapshot);
                assert_eq!(checked.members().len(), members.len());
                for (checked, raw) in checked.members().iter().zip(&members) {
                    assert_eq!(checked.envelope().raw_bytes(), raw);
                }
            }
            Err(error) => {
                assert_eq!(
                    case["accepted"], false,
                    "unexpected chain refusal {}: {error}",
                    case["name"]
                );
                let code = case["code"].as_str().unwrap();
                let expected = match (case["name"].as_str().unwrap(), code) {
                    // Same necessary preallocation bound as the unchanged codec;
                    // an oversized actual list refuses before duplicate scanning.
                    ("duplicate-actual-member", _) => "PUBLIC_MEMBER_UNREFERENCED",
                    (_, "REFERENCE_MISMATCH") => "METADATA_REFERENCE_MISMATCH",
                    (_, "SCHEMA_INVALID") => "ENVELOPE_SCHEMA_INVALID",
                    (_, "ROLE_NOT_AUTHORIZED") => "ROLE_POLICY_NOT_DECLARED",
                    _ => code,
                };
                assert_eq!(
                    error.to_string(),
                    expected,
                    "wrong refusal {}",
                    case["name"]
                );
            }
        }
    }
    assert_eq!(actual, count, "changed public-chain group {group}");
}
#[test]
fn complete_reference_membership_and_public_reachability() {
    check_group("relationships", 34);
}
#[test]
fn global_and_role_denial_preserve_other_role_positive_controls() {
    check_group("denial", 9);
}
#[test]
fn actual_thresholds_all_extras_and_unsigned_changes() {
    check_group("signatures", 5);
}
#[test]
fn fresh_root_and_body_windows_are_required() {
    check_group("time", 3);
}

#[test]
fn interval_requires_every_instant_to_be_inside_all_fresh_windows() {
    let case = cases()
        .iter()
        .find(|case| case["name"] == "strong-signed-complete-positive")
        .unwrap();
    let root = document(&case["trustedRoot"]).root_policy_shape().unwrap();
    let context = document(&case["context"]);
    let timestamp = STANDARD
        .decode(case["timestamp"].as_str().unwrap())
        .unwrap();
    let snapshot = STANDARD.decode(case["snapshot"].as_str().unwrap()).unwrap();
    let members: Vec<_> = case["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| STANDARD.decode(item.as_str().unwrap()).unwrap())
        .collect();
    let borrowed: Vec<_> = members.iter().map(Vec::as_slice).collect();
    let mut claims = vec![case["trustedRoot"].clone()];
    claims.extend(
        [&timestamp, &snapshot]
            .into_iter()
            .chain(members.iter())
            .map(|bytes| serde_json::from_slice::<Value>(bytes).unwrap()["signed"].clone()),
    );
    let lower = claims
        .iter()
        .filter_map(|v| v["issuedAt"].as_i64())
        .max()
        .unwrap();
    let upper = claims
        .iter()
        .filter_map(|v| v["expiresAt"].as_i64())
        .min()
        .unwrap();
    assert!(lower > 0 && upper > lower);
    let time = |value| document(&Value::from(value)).protocol_time_shape().unwrap();
    let earliest = time(lower);
    let latest = time(upper - 1);
    let checked = verify_public_chain_interval(
        &timestamp,
        &snapshot,
        &borrowed,
        PublicChainInputs::new(&root, &context, &earliest),
        &latest,
    )
    .unwrap();
    assert_eq!(checked.earliest_millis(), lower as f64);
    assert_eq!(checked.latest_millis(), (upper - 1) as f64);
    assert_eq!(
        checked.chain().timestamp().envelope().raw_bytes(),
        timestamp
    );
    assert!(
        verify_public_chain_interval(
            &timestamp,
            &snapshot,
            &borrowed,
            PublicChainInputs::new(&root, &context, &earliest),
            &earliest
        )
        .is_ok()
    );
    let too_early = time(lower - 1);
    assert!(
        verify_public_chain_interval(
            &timestamp,
            &snapshot,
            &borrowed,
            PublicChainInputs::new(&root, &context, &too_early),
            &latest
        )
        .is_err()
    );
    let expired = time(upper);
    assert!(
        verify_public_chain_interval(
            &timestamp,
            &snapshot,
            &borrowed,
            PublicChainInputs::new(&root, &context, &earliest),
            &expired
        )
        .is_err()
    );
    assert!(matches!(
        verify_public_chain_interval(
            &timestamp,
            &snapshot,
            &borrowed,
            PublicChainInputs::new(&root, &context, &latest),
            &earliest
        ),
        Err(PublicChainError::InvalidInterval)
    ));
    let denied = vec![
        case["trustedRoot"]["roles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|role| role["role"] == "timestamp")
            .unwrap()["keyIds"][0]
            .as_str()
            .unwrap()
            .to_owned(),
    ];
    assert!(
        verify_public_chain_interval(
            &timestamp,
            &snapshot,
            &borrowed,
            PublicChainInputs::new(&root, &context, &earliest).with_denied_key_ids(&denied),
            &latest
        )
        .is_err()
    );
    assert!(
        verify_public_chain_interval(
            &timestamp,
            &snapshot,
            &borrowed,
            PublicChainInputs::new(&root, &context, &earliest)
                .with_role_denied_key_ids("timestamp", &denied),
            &latest
        )
        .is_err()
    );
    assert!(
        verify_public_chain_interval(
            &timestamp,
            &snapshot,
            &borrowed,
            PublicChainInputs::new(&root, &context, &earliest)
                .with_role_denied_key_ids("download", &denied),
            &latest
        )
        .is_ok()
    );
    assert!(
        verify_public_chain_interval(
            &timestamp,
            &snapshot,
            &borrowed[..borrowed.len() - 1],
            PublicChainInputs::new(&root, &context, &earliest),
            &latest
        )
        .is_err()
    );
    let mut altered = members.clone();
    altered[0].push(b' ');
    let altered_refs: Vec<_> = altered.iter().map(Vec::as_slice).collect();
    assert!(
        verify_public_chain_interval(
            &timestamp,
            &snapshot,
            &altered_refs,
            PublicChainInputs::new(&root, &context, &earliest),
            &latest
        )
        .is_err()
    );
}
#[test]
fn exact_policy_original_wire_and_production_package_rules() {
    check_group("wire_policy", 5);
}
