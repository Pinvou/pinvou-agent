use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pinvou_upgrade_codec::{Document, InputKind};
use pinvou_upgrade_verifier::{
    SuppliedTimeObservation, TimeVerificationError, TimeVerificationInputs, VerificationError,
    VerificationInputs, verify_time_response, verify_with_supplied_root,
};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Value, json};

fn document(value: &Value) -> Document {
    Document::parse(&serde_json::to_vec(value).unwrap(), InputKind::Metadata).unwrap()
}
struct Key {
    pair: Ed25519KeyPair,
    descriptor: Value,
}
fn key(seed: u8) -> Key {
    let pair = Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap();
    let public = URL_SAFE_NO_PAD.encode(pair.public_key().as_ref());
    let descriptor = pinvou_upgrade_codec::key_descriptor(&public).unwrap();
    Key {
        pair,
        descriptor: json!({"keyId":descriptor.key_id(),"algorithm":"ed25519","publicKey":public}),
    }
}
fn signed(claims: &Value, keys: &[&Key]) -> Vec<u8> {
    let body = document(claims);
    let input = body.signing_input().unwrap();
    let mut records: Vec<_> = keys
        .iter()
        .map(|key| {
            json!({
                "keyId":key.descriptor["keyId"],
                "signature":URL_SAFE_NO_PAD.encode(key.pair.sign(input.as_bytes()).as_ref()),
            })
        })
        .collect();
    records.sort_by(|a, b| {
        a["keyId"]
            .as_str()
            .unwrap()
            .cmp(b["keyId"].as_str().unwrap())
    });
    document(&json!({"signed":claims,"signatures":records}))
        .canonical_bytes()
        .unwrap()
}
fn digest(raw: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, raw)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
struct Fixture {
    old_keys: Vec<Key>,
    new_keys: Vec<Key>,
    time_key: Key,
    initial_claims: Value,
    final_claims: Value,
    response_claims: Value,
    initial: Vec<u8>,
    successor: Vec<u8>,
    request: Value,
    response: Vec<u8>,
}
fn fixture() -> Fixture {
    let old_keys: Vec<_> = (1..=3).map(key).collect();
    let new_keys: Vec<_> = (4..=6).map(key).collect();
    let time_key = key(7);
    let root = |version, issued, expires, keys: &[Key]| {
        json!({
            "protocolVersion":1,"role":"root","product":"pinvou","component":null,"scope":{},
            "version":version,"issuedAt":issued,"expiresAt":expires,
            "keys":keys.iter().map(|k| k.descriptor.clone()).collect::<Vec<_>>(),
            "roles":[{"role":"root","component":null,"channel":null,"targetKey":null,
                      "threshold":2,"keyIds":keys.iter().map(|k|k.descriptor["keyId"].clone()).collect::<Vec<_>>()}],
        })
    };
    let initial_claims = root(1, 0, 1, &old_keys);
    let mut final_claims = root(2, 90_000, 200_000, &new_keys);
    final_claims["keys"]
        .as_array_mut()
        .unwrap()
        .push(time_key.descriptor.clone());
    final_claims["roles"].as_array_mut().unwrap().push(json!({
        "role":"time","component":"app","channel":null,"targetKey":null,
        "threshold":1,"keyIds":[time_key.descriptor["keyId"]],
    }));
    let initial = signed(&initial_claims, &[&old_keys[0], &old_keys[1]]);
    let successor = signed(
        &final_claims,
        &[&old_keys[0], &old_keys[1], &new_keys[0], &new_keys[1]],
    );
    let reference = json!({"version":2,"sha256":digest(&successor)});
    let nonce = URL_SAFE_NO_PAD.encode([11; 32]);
    let request = json!({"protocolVersion":1,"product":"pinvou","component":"app",
        "requestNonce":nonce,"root":reference});
    let response_claims = json!({"protocolVersion":1,"role":"time","product":"pinvou","component":"app",
        "scope":{},"purpose":"trusted-time.v1","requestNonce":nonce,"root":reference,
        "serverTime":100_000,"uncertaintyMs":5});
    let response = signed(&response_claims, &[&time_key]);
    Fixture {
        old_keys,
        new_keys,
        time_key,
        initial_claims,
        final_claims,
        response_claims,
        initial,
        successor,
        request,
        response,
    }
}
fn observation() -> SuppliedTimeObservation {
    SuppliedTimeObservation {
        sent_monotonic_millis: 10,
        received_monotonic_millis: 20,
        max_round_trip_millis: 10,
        max_uncertainty_millis: 5,
        trusted_time_floor_millis: 99_995,
    }
}
fn check(
    f: &Fixture,
    root: &[u8],
    successors: &[&[u8]],
    response: &[u8],
    obs: SuppliedTimeObservation,
    deny: &[String],
) -> Result<(), TimeVerificationError> {
    let request = document(&f.request).time_request_shape().unwrap();
    verify_time_response(
        response,
        TimeVerificationInputs::new(root, successors, &request, obs, deny),
    )
    .map(|_| ())
}
#[test]
fn dedicated_time_checks_real_rotation_and_preserves_the_complete_final_root_identity() {
    let f = fixture();
    let request = document(&f.request).time_request_shape().unwrap();
    let steps = [f.successor.as_slice()];
    let checked = verify_time_response(
        &f.response,
        TimeVerificationInputs::new(&f.initial, &steps, &request, observation(), &[]),
    )
    .unwrap();
    assert_eq!(
        (
            checked.earliest_millis(),
            checked.latest_millis(),
            checked.as_of_monotonic_millis()
        ),
        (99_995, 100_015, 20)
    );
    assert_eq!(checked.root_sha256(), digest(&f.successor));
    assert_eq!(checked.root_version(), 2);
    assert_eq!(checked.checked_root_steps(), 1);
    assert_eq!(checked.verified_signer_ids().len(), 1);
    assert_eq!(
        checked.envelope().envelope().raw_bytes(),
        f.response.as_slice()
    );
    // The protected final Root contains old+new cross-signatures. It is not
    // newly reauthenticated under only its new root key policy.
    assert!(check(&f, &f.successor, &[], &f.response, observation(), &[]).is_ok());
    let root = document(&f.final_claims).root_policy_shape().unwrap();
    let expected =
        document(&json!({"role":"time","product":"pinvou","component":"app","scope":{}}));
    let now = document(&json!(100_000)).protocol_time_shape().unwrap();
    assert!(matches!(
        verify_with_supplied_root(&f.response, VerificationInputs::new(&root, &expected, &now)),
        Err(VerificationError::TimeResponseRequired)
    ));
}
#[test]
fn time_never_accepts_bad_nonce_root_binding_signatures_deny_or_one_sided_rotation() {
    let f = fixture();
    for field in ["requestNonce", "product", "component", "root"] {
        let mut claims = f.response_claims.clone();
        claims[field] = if field == "root" {
            json!({"version":2,"sha256":"f".repeat(64)})
        } else {
            json!(if field == "requestNonce" {
                URL_SAFE_NO_PAD.encode([12; 32])
            } else {
                "other".to_owned()
            })
        };
        assert!(
            check(
                &f,
                &f.initial,
                &[&f.successor],
                &signed(&claims, &[&f.time_key]),
                observation(),
                &[]
            )
            .is_err(),
            "{field}"
        );
    }
    let denied = [f.time_key.descriptor["keyId"].as_str().unwrap().to_owned()];
    assert!(matches!(
        check(
            &f,
            &f.initial,
            &[&f.successor],
            &f.response,
            observation(),
            &denied
        ),
        Err(TimeVerificationError::Verification(
            VerificationError::SigningKeyDenied
        ))
    ));
    for keys in [
        vec![&f.old_keys[0], &f.old_keys[1]],
        vec![&f.new_keys[0], &f.new_keys[1]],
    ] {
        assert!(matches!(
            check(
                &f,
                &f.initial,
                &[&signed(&f.final_claims, &keys)],
                &f.response,
                observation(),
                &[]
            ),
            Err(TimeVerificationError::Verification(
                VerificationError::ThresholdUnmet
            ))
        ));
    }
    let bad = signed(&f.response_claims, &[&f.new_keys[0]]);
    assert!(check(&f, &f.initial, &[&f.successor], &bad, observation(), &[]).is_err());
    let mut raw: Value = serde_json::from_slice(&f.response).unwrap();
    raw["signatures"][0]["signature"] = json!(URL_SAFE_NO_PAD.encode([0; 64]));
    assert!(
        check(
            &f,
            &f.initial,
            &[&f.successor],
            &document(&raw).canonical_bytes().unwrap(),
            observation(),
            &[]
        )
        .is_err()
    );
    // No time role in the protected old Root means no borrowed timestamp role.
    let mut old = f.request.clone();
    old["root"] = json!({"version":1,"sha256":digest(&f.initial)});
    let request = document(&old).time_request_shape().unwrap();
    let mut claims = f.response_claims.clone();
    claims["root"] = old["root"].clone();
    assert!(
        verify_time_response(
            &signed(&claims, &[&f.time_key]),
            TimeVerificationInputs::new(&f.initial, &[], &request, observation(), &[])
        )
        .is_err()
    );
}
#[test]
fn safe_interval_limits_and_every_original_root_window_are_rechecked_without_a_supplied_now() {
    let f = fixture();
    for obs in [
        SuppliedTimeObservation {
            received_monotonic_millis: 9,
            ..observation()
        },
        SuppliedTimeObservation {
            max_round_trip_millis: 9,
            ..observation()
        },
        SuppliedTimeObservation {
            max_uncertainty_millis: 4,
            ..observation()
        },
        SuppliedTimeObservation {
            trusted_time_floor_millis: 99_996,
            ..observation()
        },
        SuppliedTimeObservation {
            received_monotonic_millis: 9_007_199_254_740_992,
            ..observation()
        },
    ] {
        assert!(check(&f, &f.initial, &[&f.successor], &f.response, obs, &[]).is_err());
    }
    for (server, uncertainty) in [(0, 1), (9_007_199_254_740_991_u64, 0)] {
        let mut claims = f.response_claims.clone();
        claims["serverTime"] = json!(server);
        claims["uncertaintyMs"] = json!(uncertainty);
        assert!(
            check(
                &f,
                &f.initial,
                &[&f.successor],
                &signed(&claims, &[&f.time_key]),
                observation(),
                &[]
            )
            .is_err()
        );
    }
    for (issued, expires) in [
        (99_996_u64, 200_000_u64),
        (90_000, 100_015),
        (0, 365 * 86_400_000 + 1),
    ] {
        let mut claims = f.final_claims.clone();
        claims["issuedAt"] = json!(issued);
        claims["expiresAt"] = json!(expires);
        let next = signed(
            &claims,
            &[
                &f.old_keys[0],
                &f.old_keys[1],
                &f.new_keys[0],
                &f.new_keys[1],
            ],
        );
        let mut changed = fixture();
        changed.request["root"]["sha256"] = json!(digest(&next));
        let mut response = f.response_claims.clone();
        response["root"] = changed.request["root"].clone();
        assert!(
            check(
                &changed,
                &f.initial,
                &[&next],
                &signed(&response, &[&f.time_key]),
                observation(),
                &[]
            )
            .is_err()
        );
    }
    let mut old = f.initial_claims.clone();
    old["issuedAt"] = json!(99_996);
    old["expiresAt"] = json!(100_001);
    assert!(
        check(
            &f,
            &signed(&old, &[&f.old_keys[0], &f.old_keys[1]]),
            &[&f.successor],
            &f.response,
            observation(),
            &[]
        )
        .is_err()
    );
    let zero = SuppliedTimeObservation {
        received_monotonic_millis: 10,
        max_round_trip_millis: 0,
        max_uncertainty_millis: 0,
        ..observation()
    };
    let mut claims = f.response_claims.clone();
    claims["uncertaintyMs"] = json!(0);
    assert!(
        check(
            &f,
            &f.initial,
            &[&f.successor],
            &signed(&claims, &[&f.time_key]),
            zero,
            &[]
        )
        .is_ok()
    );
    let oversized = vec![f.successor.as_slice(); 4097];
    assert!(check(&f, &f.initial, &oversized, &f.response, observation(), &[]).is_err());
}

#[test]
fn generated_public_node_vectors_match_the_dedicated_interval_and_rotated_root() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(7)
        .unwrap();
    // Root generates these public-only originals with the reviewed Node tool.
    // Missing vectors are a test prerequisite failure, never a skipped positive.
    let raw = std::fs::read(root.join("specs/upgrade/v1/vectors/time/anchor.json"))
        .expect("generated public time vectors required");
    let vectors: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(vectors["protocolVersion"], 1);
    let cases = vectors["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 2);
    for case in cases {
        let request = document(&case["request"]).time_request_shape().unwrap();
        let successors: Vec<&[u8]> = case["rootSuccessors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().as_bytes())
            .collect();
        let denied: Vec<String> = case["deniedKeyIds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        let observation = SuppliedTimeObservation {
            sent_monotonic_millis: case["sentMonotonicMs"].as_u64().unwrap(),
            received_monotonic_millis: case["receivedMonotonicMs"].as_u64().unwrap(),
            max_round_trip_millis: case["maxRoundTripMs"].as_u64().unwrap(),
            max_uncertainty_millis: case["maxUncertaintyMs"].as_u64().unwrap(),
            trusted_time_floor_millis: case["trustedTimeFloor"].as_u64().unwrap(),
        };
        let checked = verify_time_response(
            case["responseBytes"].as_str().unwrap().as_bytes(),
            TimeVerificationInputs::new(
                case["trustedRootBytes"].as_str().unwrap().as_bytes(),
                &successors,
                &request,
                observation,
                &denied,
            ),
        )
        .unwrap();
        let expected = &case["expected"];
        assert_eq!(
            checked.earliest_millis(),
            expected["earliest"].as_u64().unwrap()
        );
        assert_eq!(
            checked.latest_millis(),
            expected["latest"].as_u64().unwrap()
        );
        assert_eq!(
            checked.as_of_monotonic_millis(),
            expected["asOfMonotonicMs"].as_u64().unwrap()
        );
        assert_eq!(
            checked.root_version(),
            expected["root"]["version"].as_u64().unwrap()
        );
        assert_eq!(
            checked.root_sha256(),
            expected["root"]["sha256"].as_str().unwrap()
        );
        assert_eq!(checked.verified_signer_ids().len(), 2);
    }
}
