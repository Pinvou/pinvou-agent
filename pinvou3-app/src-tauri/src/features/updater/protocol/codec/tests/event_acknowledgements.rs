use pinvou_upgrade_codec::{
    ClientRequestError, ClientRequestOperation as Op, Document, Error,
    EventAcknowledgementError as AckError, InputKind,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, process::Command, sync::OnceLock};

fn fixtures() -> &'static Vec<Value> {
    static FIXTURES: OnceLock<Vec<Value>> = OnceLock::new();
    FIXTURES.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(7)
            .unwrap();
        let output = Command::new("node")
            .args([
                "--input-type=module",
                "-e",
                include_str!("event-acknowledgement-fixtures.mjs"),
            ])
            .arg(root.join("specs/upgrade/v1/reference/canonical-json.mjs"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cases: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(cases.len(), 6);
        cases
    })
}
fn document(value: &Value) -> Document {
    Document::parse(&serde_json::to_vec(value).unwrap(), InputKind::Metadata).unwrap()
}
fn check(request: &Value, response: &Value, op: Op, status: u16) -> Result<(), AckError> {
    document(request).assert_supplied_event_acknowledgements(&document(response), op, status)
}
fn check_raw(request: &[u8], response: &[u8], op: Op) -> Result<(), AckError> {
    let request = Document::parse(request, InputKind::Metadata).map_err(AckError::Json)?;
    let response = Document::parse(response, InputKind::Metadata).map_err(AckError::Json)?;
    request.assert_supplied_event_acknowledgements(&response, op, 200)
}
fn operation(case: &Value) -> Op {
    case["operation"].as_str().unwrap().parse().unwrap()
}
fn assert_old_contracts(request: &Value, response: &Value, op: Op) {
    assert_eq!(
        document(request).assert_supplied_client_request_contract(op),
        Ok(())
    );
    assert_eq!(
        document(response).assert_supplied_client_response_contract(op),
        Ok(())
    );
}

#[test]
fn six_original_batches_preserve_duplicate_positions_dispositions_and_inputs() {
    for case in fixtures() {
        let (request, response, op) = (&case["request"], &case["response"], operation(case));
        let before = case.clone();
        assert_old_contracts(request, response, op);
        assert_eq!(check(request, response, op, 200), Ok(()));
        // Two identical A events have indistinguishable wire identities; their
        // valid dispositions may differ without creating an ordering rule.
        let mut indistinguishable = response.clone();
        indistinguishable["acknowledgements"]
            .as_array_mut()
            .unwrap()
            .swap(0, 2);
        assert_old_contracts(request, &indistinguishable, op);
        assert_eq!(check(request, &indistinguishable, op, 200), Ok(()));
        for disposition in ["committed", "replayed", "late-diagnostic"] {
            let mut changed = response.clone();
            changed["requestId"] = json!("a-different-server-request");
            for ack in changed["acknowledgements"].as_array_mut().unwrap() {
                ack["disposition"] = json!(disposition);
            }
            assert_eq!(check(request, &changed, op, 200), Ok(()));
        }
        // A new invocation with another credential JTI does not share an ID set.
        let mut independent = request.clone();
        independent["credential"]["signed"]["jti"] = json!("other-credential");
        assert_eq!(check(&independent, response, op, 200), Ok(()));
        assert_eq!(case, &before);
    }
}

#[test]
fn old_valid_receipts_cannot_acknowledge_missing_extra_reordered_or_foreign_events() {
    for case in fixtures() {
        let (request, response, op) = (&case["request"], &case["response"], operation(case));
        for variant in 0..4 {
            let mut changed = response.clone();
            let acks = changed["acknowledgements"].as_array_mut().unwrap();
            match variant {
                0 => {
                    acks.pop();
                }
                1 => acks.push(acks[0].clone()),
                2 => acks.swap(1, 2),
                _ => acks[1]["eventId"] = json!("foreign-event"),
            }
            assert_old_contracts(request, &changed, op);
            assert_eq!(
                check(request, &changed, op, 200),
                Err(AckError::AcknowledgementMismatch)
            );
            assert_eq!(check(request, response, op, 200), Ok(()));
        }
        let mut empty = response.clone();
        empty["acknowledgements"] = json!([]);
        assert!(matches!(
            check(request, &empty, op, 200),
            Err(AckError::Response(_))
        ));
    }
}

#[test]
fn same_key_conflicts_compare_whole_canonical_events_after_old_contract_positives() {
    for case in fixtures() {
        let (request, response, op) = (&case["request"], &case["response"], operation(case));
        for variant in 0..4 {
            let mut changed = request.clone();
            let duplicate = &mut changed["events"][2];
            match variant {
                0 => duplicate["sequence"] = json!(3),
                1 => duplicate["occurredAt"] = json!(duplicate["occurredAt"].as_u64().unwrap() + 1),
                2 => duplicate["fromState"] = json!("different-from"),
                _ => {
                    duplicate["facts"]["verificationResult"] = json!("verified");
                    let bytes = document(&duplicate["facts"]).canonical_bytes().unwrap();
                    duplicate["factsSha256"] = json!(
                        Sha256::digest(bytes)
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<String>()
                    );
                }
            }
            assert_old_contracts(&changed, response, op);
            assert_eq!(
                check(&changed, response, op, 200),
                Err(AckError::EventConflict)
            );
            assert_eq!(check(request, response, op, 200), Ok(()));
        }
        let mut reordered = request.clone();
        let original = reordered["events"][2].as_object().unwrap().clone();
        let target = reordered["events"][2].as_object_mut().unwrap();
        target.clear();
        for (key, value) in original.into_iter().rev() {
            target.insert(key, value);
        }
        assert_eq!(check(&reordered, response, op, 200), Ok(()));
    }
}

#[test]
fn only_six_event_operations_and_http200_can_return_a_binding() {
    for case in fixtures() {
        let (request, response, op) = (&case["request"], &case["response"], operation(case));
        for other in Op::ALL {
            if !fixtures().iter().any(|c| operation(c) == other) {
                assert_eq!(
                    check(request, response, other, 200),
                    Err(AckError::UnsupportedOperation)
                );
            } else if other != op {
                assert!(matches!(
                    check(request, response, other, 200),
                    Err(AckError::Request(_))
                ));
            }
        }
        let error = json!({"protocolVersion":1,"requestId":"request-error",
            "code":"SERVICE_UNAVAILABLE","retryable":true});
        assert_eq!(
            document(&error).assert_supplied_client_http_response_contract(op, 503),
            Ok(())
        );
        assert_eq!(
            check(request, &error, op, 503),
            Err(AckError::InvalidStatus)
        );
        assert!(matches!(
            check(request, &error, op, 200),
            Err(AckError::Response(_))
        ));
        for status in [0, 199, 201, 202, 204, 299, 301, 400, 409, 503, 600] {
            assert_eq!(
                check(request, response, op, status),
                Err(AckError::InvalidStatus)
            );
        }
        let mut changed = request.clone();
        changed["events"][0]["executionPurpose"] = json!("wrong-purpose");
        assert_eq!(
            check(&changed, response, op, 200),
            Err(AckError::Request(ClientRequestError::InvalidContract))
        );
        let mut invalid = response.clone();
        invalid["acknowledgements"][0]["disposition"] = json!("pending");
        assert!(matches!(
            check(request, &invalid, op, 200),
            Err(AckError::Response(_))
        ));
        assert_eq!(check(request, response, op, 200), Ok(()));
    }
}

#[test]
fn both_raw_profiles_and_nested_credential_limits_remain_enforced() {
    for case in fixtures() {
        let (request, response, op) = (&case["request"], &case["response"], operation(case));
        let request_bytes = serde_json::to_vec(request).unwrap();
        let response_bytes = serde_json::to_vec(response).unwrap();
        for (index, value) in [request, response].into_iter().enumerate() {
            let mut raw = serde_json::to_vec(value).unwrap();
            raw.resize(1024 * 1024, b' ');
            let check_side = |bytes: &[u8]| {
                if index == 0 {
                    check_raw(bytes, &response_bytes, op)
                } else {
                    check_raw(&request_bytes, bytes, op)
                }
            };
            assert_eq!(check_side(&raw), Ok(()));
            raw.push(b' ');
            assert_eq!(check_side(&raw), Err(AckError::Json(Error::InputLimit)));
            let duplicate =
                serde_json::to_string(value)
                    .unwrap()
                    .replacen("{", r#"{"protocolVersion":1,"#, 1);
            assert_eq!(
                check_side(duplicate.as_bytes()),
                Err(AckError::Json(Error::DuplicateKey))
            );
            assert_eq!(check_side(b"{"), Err(AckError::Json(Error::InvalidJson)));
        }
        let mut changed = request.clone();
        let signature = changed["credential"]["signatures"][0].clone();
        let initial = document(&changed["credential"])
            .canonical_bytes()
            .unwrap()
            .len();
        let per = document(&signature).canonical_bytes().unwrap().len() + 1;
        let count = 1 + (65536 - initial) / per;
        changed["credential"]["signatures"] = json!(vec![signature.clone(); count]);
        assert!(
            document(&changed["credential"])
                .canonical_bytes()
                .unwrap()
                .len()
                <= 65536
        );
        assert_eq!(check(&changed, response, op, 200), Ok(()));
        changed["credential"]["signatures"]
            .as_array_mut()
            .unwrap()
            .push(signature);
        assert!(
            document(&changed["credential"])
                .canonical_bytes()
                .unwrap()
                .len()
                > 65536
        );
        assert_eq!(
            check(&changed, response, op, 200),
            Err(AckError::Request(ClientRequestError::CredentialTooLarge))
        );
        let mut invalid = request.clone();
        invalid["extra"] = Value::Null;
        assert_eq!(
            check(&invalid, response, op, 200),
            Err(AckError::Request(ClientRequestError::InvalidContract))
        );
        let mut invalid = response.clone();
        invalid["extra"] = Value::Null;
        assert!(matches!(
            check(request, &invalid, op, 200),
            Err(AckError::Response(_))
        ));
        assert_eq!(check(request, response, op, 200), Ok(()));
    }
}
