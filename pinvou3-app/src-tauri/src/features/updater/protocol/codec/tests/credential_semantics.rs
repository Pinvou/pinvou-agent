#[allow(dead_code)]
#[path = "support/download_fixture.rs"]
mod download_fixture;

use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pinvou_upgrade_codec::{
    CredentialSemanticsError, Document, InputKind, assert_supplied_credential_semantics,
    decode_canonical_envelope,
};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
const MAX: u64 = 9007199254740991;
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(7)
        .unwrap()
        .to_owned()
}
fn fixtures() -> Vec<Value> {
    let mut vectors = serde_json::from_slice::<Value>(
        &std::fs::read(root().join("specs/upgrade/v1/vectors/crypto/envelopes.json")).unwrap(),
    )
    .unwrap()["vectors"]
        .as_array()
        .unwrap()
        .clone();
    download_fixture::replace_legacy_download(
        &mut vectors,
        &root().join("specs/upgrade/v1/reference/canonical-json.mjs"),
    );
    vectors
}
fn original(role: &str) -> (Value, u64) {
    let vector = fixtures().into_iter().find(|v| v["name"] == role).unwrap();
    (
        serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(vector["envelopeBase64Url"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap(),
        vector["now"].as_u64().unwrap(),
    )
}
fn credential(role: &str) -> bool {
    ![
        "root",
        "timestamp",
        "snapshot",
        "target",
        "release",
        "package",
    ]
    .contains(&role)
}
fn wire(value: &Value) -> Vec<u8> {
    Document::parse(&serde_json::to_vec(value).unwrap(), InputKind::Metadata)
        .unwrap()
        .canonical_bytes()
        .unwrap()
}
fn check(value: &Value, now: u64) -> Result<(), CredentialSemanticsError> {
    let envelope = decode_canonical_envelope(&wire(value), InputKind::Credential)
        .unwrap()
        .check_full_schema()
        .unwrap();
    let time = Document::parse(now.to_string().as_bytes(), InputKind::Metadata)
        .unwrap()
        .protocol_time_shape()
        .unwrap();
    assert_supplied_credential_semantics(&envelope, &time)
}
fn branches() -> Vec<(Value, u64)> {
    let (mut candidate, _) = original("decision");
    let claims = &mut candidate["signed"];
    let iat = claims["iat"].as_u64().unwrap();
    let opening = json!({"targetKey":claims["scope"]["targetKey"],"deploymentId":claims["update"]["endpointChain"]["deploymentId"],
        "deploymentRevision":claims["update"]["endpointChain"]["deploymentRevision"],"rolloutId":"qa-rollout","rolloutRevision":1,
        "releaseEnvelopeSha256":claims["update"]["endpointChain"]["releaseManifest"]["envelopeSha256"],"leafSalt":"0".repeat(64)});
    claims["update"]["endpointKind"] = json!("candidate");
    claims["update"]["rollout"] = json!({"rolloutId":"qa-rollout","revision":1,"state":"running","policyRevision":1,"opening":opening,"locator":"qa-locator","locatorExpiresAt":iat+1});
    // Locator now can be later than expiry: original semantics compares iat only.
    let (mut bridge, bnow) = original("decision");
    bridge["signed"]["update"]["hopKind"] = json!("bridge");
    bridge["signed"]["update"]["hopChain"]["deploymentId"] = json!("qa-bridge");
    bridge["signed"]["update"]["hopChain"]["deploymentState"] = json!("superseded");
    bridge["signed"]["update"]["bridge"] = json!({"eligibilityId":"qa-bridge","revision":1});
    let (mut ordinary, onow) = original("decision");
    ordinary["signed"]["update"]["hopChain"]["deploymentId"] = json!("qa-ordinary");
    ordinary["signed"]["update"]["ordinaryPathApproval"] =
        json!({"approvalId":"qa-ordinary","revision":1});
    vec![(candidate, iat + 2), (bridge, bnow), (ordinary, onow)]
}
fn lifetime(role: &str) -> u64 {
    match role {
        "decision" | "download" => 900000,
        "helper-cleanup" => 120000,
        "preinstall-transaction-event"
        | "install-transaction-event"
        | "activate-transaction-event" => 37 * 86400000,
        "telemetry-event" | "reconciliation-event" | "recovery-review-event" => 7 * 86400000,
        _ => 300000,
    }
}
fn near_safe_staged() -> (Value, u64) {
    let (mut value, _) = original("authorization-activate");
    let signed = &mut value["signed"];
    for key in ["iat", "nbf"] {
        signed[key] = json!(MAX - 200000);
    }
    signed["exp"] = json!(MAX);
    for key in ["root", "timestamp", "snapshot", "target"] {
        signed["metadataSet"][key]["expiresAt"] = json!(MAX);
    }
    for entry in signed["metadataSet"]["releases"].as_array_mut().unwrap() {
        entry["expiresAt"] = json!(MAX);
    }
    for key in ["endpointChain", "hopChain"] {
        signed["update"][key]["releaseManifest"]["expiresAt"] = json!(MAX);
        signed["update"][key]["installNotAfter"] = Value::Null;
        signed["update"][key]["supplyChain"]["effectiveExpiresAt"] = Value::Null;
    }
    signed["staged"]["stagedAt"] = json!(MAX - 30 * 86400000 + 1);
    signed["staged"]["stagedValidUntil"] = json!(MAX);
    (value, MAX - 1)
}
#[test]
fn thirteen_original_and_current_download_credentials_keep_exact_expiry_nbf_and_role_domain() {
    for vector in fixtures()
        .into_iter()
        .filter(|v| credential(v["name"].as_str().unwrap()))
    {
        let role = vector["name"].as_str().unwrap();
        let (value, now) = original(role);
        assert_eq!(check(&value, now), Ok(()));
        let mut maximum = value.clone();
        let iat = maximum["signed"]["iat"].as_u64().unwrap();
        maximum["signed"]["exp"] = json!(iat + lifetime(role));
        if role.ends_with("-event") {
            for key in ["transactionStartedAt", "sessionStartedAt", "taskStartedAt"] {
                if maximum["signed"].get(key).is_some() {
                    maximum["signed"][key] = json!(iat);
                }
            }
            maximum["signed"]["eventNotAfter"] = json!(iat + 1);
            if maximum["signed"].get("task").is_some() {
                maximum["signed"]["task"]["windowStart"] = json!(iat);
                maximum["signed"]["task"]["windowEnd"] = json!(iat + 86400000);
            }
        }
        assert_eq!(check(&maximum, now), Ok(()));
        maximum["signed"]["exp"] = json!(iat + lifetime(role) + 1);
        assert_eq!(
            check(&maximum, now),
            Err(CredentialSemanticsError::WindowInvalid)
        );

        let exp = value["signed"]["exp"].as_u64().unwrap();
        assert_eq!(
            check(&value, exp),
            Err(CredentialSemanticsError::WindowClosed)
        );
        let mut future = value.clone();
        future["signed"]["nbf"] = json!(now + 1);
        assert_eq!(
            check(&future, now),
            Err(CredentialSemanticsError::WindowClosed)
        );
    }
    let time = Document::parse(b"0", InputKind::Metadata)
        .unwrap()
        .protocol_time_shape()
        .unwrap();
    for vector in fixtures()
        .into_iter()
        .filter(|v| !credential(v["name"].as_str().unwrap()))
    {
        let raw = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let value = decode_canonical_envelope(&raw, InputKind::Metadata)
            .unwrap()
            .check_full_schema()
            .unwrap();
        assert_eq!(
            assert_supplied_credential_semantics(&value, &time),
            Err(CredentialSemanticsError::UnsupportedRole)
        );
    }
}
#[test]
fn original_preinstall_event_and_action_exceptions_are_not_silently_tightened() {
    let (mut preinstall, now) = original("authorization-preinstall");
    for key in ["endpointChain", "hopChain"] {
        preinstall["signed"]["update"][key]["installNotBefore"] = json!(now + 1000);
    }
    assert_eq!(check(&preinstall, now), Ok(()));
    let (mut install, now) = original("authorization-install");
    for key in ["endpointChain", "hopChain"] {
        install["signed"]["update"][key]["installNotBefore"] = json!(now + 1000);
    }
    assert_eq!(
        check(&install, now),
        Err(CredentialSemanticsError::InstallNotYet)
    );
    let (mut event, _) = original("telemetry-event");
    event["signed"]["eventNotAfter"] = json!(event["signed"]["nbf"].as_u64().unwrap() + 1);
    let now = event["signed"]["eventNotAfter"].as_u64().unwrap();
    assert_eq!(check(&event, now), Ok(()));
    let (mut action, now) = original("reconciliation-action");
    action["signed"]["task"]["windowStart"] = json!(now + 1000);
    action["signed"]["task"]["windowEnd"] = json!(now + 2000);
    assert_eq!(check(&action, now), Ok(()));
    let (mut event, _) = original("telemetry-event");
    for key in ["iat", "nbf", "sessionStartedAt"] {
        event["signed"][key] = json!(MAX - 1000);
    }
    event["signed"]["exp"] = json!(MAX);
    event["signed"]["eventNotAfter"] = json!(MAX);
    assert_eq!(check(&event, MAX - 1), Ok(()));
}
#[test]
fn candidate_bridge_path_and_long_version_bindings_match_full_jcs_without_authority() {
    let (overflow, now) = near_safe_staged();
    assert_eq!(
        check(&overflow, now),
        Err(CredentialSemanticsError::StagedWindow)
    );
    for (index, (value, now)) in branches().into_iter().enumerate() {
        let shape = decode_canonical_envelope(&wire(&value), InputKind::Credential)
            .unwrap()
            .check_full_schema();
        assert!(shape.is_ok(), "branch {index} schema failure");
        assert_eq!(check(&value, now), Ok(()));
    }
    let (mut value, now) = branches().remove(0);
    value["signed"]["update"]["rollout"]["opening"]["deploymentRevision"] = json!(2);
    assert_eq!(
        check(&value, now),
        Err(CredentialSemanticsError::OpeningBinding)
    );
    let (mut value, now) = original("decision");
    value["signed"]["update"]["hopChain"]["deploymentId"] = json!("qa-other");
    assert_eq!(
        check(&value, now),
        Err(CredentialSemanticsError::PathMissing)
    );
    let (mut value, now) = original("decision");
    value["signed"]["update"]["targetVersion"] = json!(format!("{}.0.0", "9".repeat(4096)));
    assert_eq!(check(&value, now), Ok(()));
    value["signed"]["currentVersion"] = value["signed"]["update"]["targetVersion"].clone();
    assert_eq!(
        check(&value, now),
        Err(CredentialSemanticsError::VersionNotHigher)
    );
}
fn leaf_paths(value: &Value, path: &mut Vec<String>, paths: &mut Vec<Vec<String>>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                path.push(key.clone());
                leaf_paths(child, path, paths);
                path.pop();
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                path.push(index.to_string());
                leaf_paths(child, path, paths);
                path.pop();
            }
        }
        _ => paths.push(path.clone()),
    }
}
fn at<'a>(value: &'a Value, path: &[String]) -> &'a Value {
    let mut value = value;
    for key in path {
        value = if value.is_array() {
            &value[key.parse::<usize>().unwrap()]
        } else {
            &value[key]
        };
    }
    value
}
fn at_mut<'a>(value: &'a mut Value, path: &[String]) -> &'a mut Value {
    let mut value = value;
    for key in path {
        value = if value.is_array() {
            &mut value[key.parse::<usize>().unwrap()]
        } else {
            &mut value[key]
        };
    }
    value
}
#[test]
fn original_node_credential_semantics_matches_all_leaf_windows_and_related_branches() {
    let mut scenarios = Vec::new();
    for vector in fixtures()
        .into_iter()
        .filter(|v| credential(v["name"].as_str().unwrap()))
    {
        scenarios.push(original(vector["name"].as_str().unwrap()));
    }
    scenarios.extend(branches());
    scenarios.push(near_safe_staged());
    let (mut long, now) = original("decision");
    long["signed"]["update"]["targetVersion"] = json!(format!("{}.0.0", "9".repeat(4096)));
    scenarios.push((long, now));
    let (mut preinstall, now) = original("authorization-preinstall");
    for key in ["endpointChain", "hopChain"] {
        preinstall["signed"]["update"][key]["installNotBefore"] = json!(now + 1000);
    }
    scenarios.push((preinstall, now));

    let (mut event, _) = original("telemetry-event");
    for key in ["iat", "nbf", "sessionStartedAt"] {
        event["signed"][key] = json!(MAX - 1000);
    }
    event["signed"]["exp"] = json!(MAX);
    event["signed"]["eventNotAfter"] = json!(MAX);
    scenarios.push((event, MAX - 1));
    let mut cases = Vec::new();
    for (original, now) in scenarios {
        let mut paths = Vec::new();
        leaf_paths(
            &original["signed"],
            &mut vec!["signed".to_owned()],
            &mut paths,
        );
        let mut values = vec![original.clone()];
        for path in paths {
            let replacements = match at(&original, &path) {
                Value::String(_) => vec![json!("different"), Value::Null],
                Value::Number(number) => vec![
                    json!(0),
                    json!(number.as_f64().unwrap() + 1.0),
                    json!(MAX),
                    Value::Null,
                ],
                Value::Bool(boolean) => vec![json!(!boolean), Value::Null],
                Value::Null => vec![json!({}), json!(1)],
                _ => unreachable!(),
            };
            for replacement in replacements {
                let mut value = original.clone();
                *at_mut(&mut value, &path) = replacement;
                values.push(value);
            }
        }
        // Explicit credential interval boundaries, not an invented download cap.
        let signed = &original["signed"];
        let mut times = vec![
            json!(now),
            json!(0),
            json!(-0.0),
            json!(-1),
            Value::Null,
            json!("1"),
            json!(1.5),
            json!(MAX),
            json!(MAX + 1),
        ];
        for field in ["iat", "nbf", "exp"] {
            let t = signed[field].as_u64().unwrap();
            for value in [t.saturating_sub(1), t, t + 1] {
                times.push(json!(value));
            }
        }
        for value in values {
            // Each independent leaf mutation is evaluated at original supplied now.
            let signed = &value["signed"];
            cases.push(json!({"raw":STANDARD.encode(wire(&value)),"nowRaw":STANDARD.encode(now.to_string()),
                "expected":{"role":signed["role"],"product":signed["product"],"component":signed["component"],"scope":signed["scope"]}}));
        }
        for time in times {
            cases.push(json!({"raw":STANDARD.encode(wire(&original)),"nowRaw":STANDARD.encode(serde_json::to_vec(&time).unwrap()),
            "expected":{"role":signed["role"],"product":signed["product"],"component":signed["component"],"scope":signed["scope"]}}));
        }
    }
    let expected: Vec<bool> = cases
        .iter()
        .map(|case| {
            let raw = STANDARD.decode(case["raw"].as_str().unwrap()).unwrap();
            let time = STANDARD.decode(case["nowRaw"].as_str().unwrap()).unwrap();
            let envelope = decode_canonical_envelope(&raw, InputKind::Credential).and_then(|e| {
                e.check_full_schema()
                    .map_err(|_| pinvou_upgrade_codec::EnvelopeShapeError::InvalidEnvelope)
            });
            let time = Document::parse(&time, InputKind::Metadata)
                .ok()
                .and_then(|d| d.protocol_time_shape().ok());
            match (envelope, time) {
                (Ok(envelope), Some(time)) => {
                    assert_supplied_credential_semantics(&envelope, &time).is_ok()
                }
                _ => false,
            }
        })
        .collect();
    let reference = root().join("specs/upgrade/v1/reference/signatures.mjs");
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("credential-semantics-oracle.mjs"),
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
        "original credential semantics oracle unavailable"
    );
    let actual: Vec<bool> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (a, b)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_eq!(a, b, "credential semantics case {index}");
    }
    eprintln!(
        "{} original Node credential semantics cases passed",
        cases.len()
    );
}
