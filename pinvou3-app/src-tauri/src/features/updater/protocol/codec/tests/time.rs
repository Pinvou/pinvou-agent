use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pinvou_upgrade_codec::{Document, InputKind, key_descriptor};
use serde_json::{Value, json};
fn document(value: &Value) -> Document {
    Document::parse(&serde_json::to_vec(value).unwrap(), InputKind::Credential).unwrap()
}
fn request() -> Value {
    json!({"protocolVersion":1,"product":"pinvou","component":"app",
        "requestNonce":URL_SAFE_NO_PAD.encode([3;32]),"root":{"version":1,"sha256":"a".repeat(64)}})
}
#[test]
fn time_request_and_body_are_closed_canonical_nonce_safe_integer_shapes_only() {
    let original = request();
    let parsed = document(&original).time_request_shape().unwrap();
    assert_eq!(parsed.product(), "pinvou");
    assert_eq!(parsed.component(), "app");
    assert_eq!(
        parsed.request_nonce(),
        original["requestNonce"].as_str().unwrap()
    );
    assert_eq!(parsed.root_version(), 1);
    assert_eq!(parsed.root_sha256(), "a".repeat(64));
    for name in [
        "protocolVersion",
        "product",
        "component",
        "requestNonce",
        "root",
    ] {
        let mut value = original.clone();
        value.as_object_mut().unwrap().remove(name);
        assert!(document(&value).time_request_shape().is_err(), "{name}");
    }
    for nonce in [
        "A".repeat(42),
        format!("{}B", "A".repeat(42)),
        format!("{}=", "A".repeat(43)),
        "/".repeat(43),
    ] {
        let mut value = original.clone();
        value["requestNonce"] = json!(nonce);
        assert!(document(&value).time_request_shape().is_err());
    }
    for invalid in [
        Value::Null,
        json!(0),
        json!(1.5),
        json!(9_007_199_254_740_992_u64),
    ] {
        let mut value = original.clone();
        value["root"]["version"] = invalid;
        assert!(document(&value).time_request_shape().is_err());
    }
    let mut body = original;
    body["role"] = json!("time");
    body["scope"] = json!({});
    body["purpose"] = json!("trusted-time.v1");
    body["serverTime"] = json!(1000);
    body["uncertaintyMs"] = json!(0);
    assert_eq!(
        document(&body)
            .time_response_shape()
            .unwrap()
            .server_time_millis(),
        1000
    );
    assert!(document(&body).time_request_shape().is_err());
    for field in ["serverTime", "uncertaintyMs"] {
        for invalid in [
            json!(-1),
            json!(0.5),
            json!(9_007_199_254_740_992_u64),
            json!("1"),
        ] {
            let mut value = body.clone();
            value[field] = invalid;
            assert!(document(&value).time_response_shape().is_err());
        }
    }
    for (field, invalid) in [
        ("scope", json!({"channel":null})),
        ("purpose", json!("decision")),
        ("role", json!("timestamp")),
        ("extra", json!(true)),
    ] {
        let mut value = body.clone();
        value[field] = invalid;
        assert!(document(&value).time_response_shape().is_err());
    }
    let raw = serde_json::to_string(&request()).unwrap().replace(
        "\"product\":\"pinvou\"",
        "\"product\":\"pinvou\",\"pr\\u006fduct\":\"pinvou\"",
    );
    assert!(Document::parse(raw.as_bytes(), InputKind::Credential).is_err());
}
#[test]
fn time_role_is_component_exact_with_no_channel_or_target_wildcard() {
    let keys: Vec<_> = (1_u8..=3)
        .map(|seed| {
            let public = URL_SAFE_NO_PAD.encode([seed; 32]);
            let key = key_descriptor(&public).unwrap();
            json!({"keyId":key.key_id(),"algorithm":"ed25519","publicKey":public})
        })
        .collect();
    let mut root = json!({"protocolVersion":1,"role":"root","product":"pinvou","component":null,"scope":{},
        "version":1,"issuedAt":0,"expiresAt":1000,"keys":keys,
        "roles":[{"role":"root","component":null,"channel":null,"targetKey":null,"threshold":2,"keyIds":keys.iter().map(|k|k["keyId"].clone()).collect::<Vec<_>>()},
        {"role":"time","component":"app","channel":null,"targetKey":null,"threshold":1,"keyIds":[keys[0]["keyId"]]}]});
    assert!(document(&root).root_policy_shape().is_ok());
    for (field, value) in [
        ("component", Value::Null),
        ("channel", json!("stable")),
        ("targetKey", json!("linux-x86_64-deb-perMachine")),
    ] {
        let original = root["roles"][1][field].clone();
        root["roles"][1][field] = value;
        assert!(document(&root).root_policy_shape().is_err());
        root["roles"][1][field] = original;
    }
    root["roles"].as_array_mut().unwrap().pop();
    assert!(document(&root).root_policy_shape().is_ok()); // Old roots remain structurally valid, without time authority.
}

#[test]
fn time_api_routes_closed_data_without_credential_time_or_unsigned_wrapper() {
    use pinvou_upgrade_codec::{ClientRequestOperation as Op, ClientResponseOperation};
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(7)
        .unwrap()
        .join("specs/upgrade/v1/vectors/time/anchor.json");
    let vectors: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for vector in vectors["cases"].as_array().unwrap() {
        let request = vector["request"].clone();
        let response: Value =
            serde_json::from_str(vector["responseBytes"].as_str().unwrap()).unwrap();
        assert_eq!("time".parse(), Ok(Op::Time));
        assert!(
            document(&request)
                .assert_supplied_client_request_contract(Op::Time)
                .is_ok()
        );
        assert!(
            document(&response)
                .assert_supplied_client_response_contract(ClientResponseOperation::Time)
                .is_ok()
        );
        assert!(
            document(&response)
                .assert_supplied_client_request_contract(Op::Time)
                .is_err()
        );
        assert!(
            document(&request)
                .assert_supplied_client_response_contract(Op::Time)
                .is_err()
        );
        for field in ["requestId", "iat", "exp"] {
            let mut malformed = response.clone();
            malformed[field] = json!("untrusted");
            assert!(
                document(&malformed)
                    .assert_supplied_client_response_contract(Op::Time)
                    .is_err()
            );
        }
        for field in ["signed", "signatures"] {
            let mut malformed = response.clone();
            malformed.as_object_mut().unwrap().remove(field);
            assert!(
                document(&malformed)
                    .assert_supplied_client_response_contract(Op::Time)
                    .is_err()
            );
        }
        for value in ["app\n", "app\r", "app\u{2028}", "app\u{2029}"] {
            let mut malformed = response.clone();
            malformed["signed"]["product"] = json!(value);
            assert!(
                document(&malformed)
                    .assert_supplied_client_response_contract(Op::Time)
                    .is_err()
            );
        }
        let mut malformed = response.clone();
        malformed["signed"]["requestNonce"] = json!(format!("{}B", "A".repeat(42)));
        assert!(
            document(&malformed)
                .assert_supplied_client_response_contract(Op::Time)
                .is_err()
        );
        let mut malformed = request.clone();
        malformed["requestNonce"] = json!(format!("{}B", "A".repeat(42)));
        assert!(
            document(&malformed)
                .assert_supplied_client_request_contract(Op::Time)
                .is_err()
        );
    }
}
