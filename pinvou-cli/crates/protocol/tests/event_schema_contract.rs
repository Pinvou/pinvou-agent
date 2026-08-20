use pinvou_protocol::{RateClass, ResourceRef, RuntimeEventEnvelope, RuntimeEventKind, StreamId};

const FIXTURES: &[(&str, RateClass, StreamId)] = &[
    ("attachment.started", RateClass::R0, StreamId::Control),
    ("attachment.ended", RateClass::R0, StreamId::Control),
    ("turn.started", RateClass::R0, StreamId::Control),
    ("turn.ended", RateClass::R0, StreamId::Control),
    ("approval.requested", RateClass::R0, StreamId::Control),
    ("approval.resolved", RateClass::R0, StreamId::Control),
    ("input.requested", RateClass::R0, StreamId::Control),
    ("input.resolved", RateClass::R0, StreamId::Control),
    ("error.raised", RateClass::R0, StreamId::Control),
    ("resource.ref_created", RateClass::R0, StreamId::Control),
    ("stream.aborted", RateClass::R0, StreamId::Control),
    ("stream.gap", RateClass::R0, StreamId::Control),
    ("text.delta", RateClass::R1, StreamId::Main),
    ("thinking.delta", RateClass::R1, StreamId::Main),
    ("plan.delta", RateClass::R1, StreamId::Main),
    ("message.completed", RateClass::R1, StreamId::Main),
    ("tool.call.started", RateClass::R1, StreamId::Main),
    ("tool.call.args_delta", RateClass::R1, StreamId::Main),
    ("tool.call.output_delta", RateClass::R1, StreamId::Main),
    ("tool.call.completed", RateClass::R1, StreamId::Main),
    ("file.change.completed", RateClass::R1, StreamId::Main),
    ("usage.reported", RateClass::R1, StreamId::Main),
    ("log.record", RateClass::R2, StreamId::Main),
    ("diagnostic.gap", RateClass::R2, StreamId::Main),
    ("progress.tick", RateClass::R3, StreamId::Main),
    ("resource.sample", RateClass::R3, StreamId::Main),
    ("vendor", RateClass::R1, StreamId::Main),
];

#[test]
fn every_v1_kind_has_a_byte_stable_round_trip_fixture() {
    assert_eq!(
        RuntimeEventKind::ALL
            .into_iter()
            .map(RuntimeEventKind::as_str)
            .collect::<Vec<_>>(),
        FIXTURES
            .iter()
            .map(|(kind, _, _)| *kind)
            .collect::<Vec<_>>()
    );
    for (kind, rate_class, stream_id) in FIXTURES {
        let path = format!(
            "{}/tests/fixtures/events/{kind}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let mut bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        let envelope = RuntimeEventEnvelope::from_json_slice(&bytes)
            .unwrap_or_else(|error| panic!("{path}: {error}"));
        assert_eq!(envelope.kind(), *kind);
        assert_eq!(envelope.rate_class(), *rate_class);
        assert_eq!(envelope.stream_id(), *stream_id);
        assert_eq!(envelope.to_json_vec().unwrap(), bytes, "{path}");
    }
}

#[test]
fn schema_validation_is_fail_closed() {
    let original = std::fs::read(format!(
        "{}/tests/fixtures/events/text.delta.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&original).unwrap();

    for (field, replacement) in [
        ("protocol_version", serde_json::json!(2)),
        ("schema_version", serde_json::json!(2)),
        ("stream_id", serde_json::json!("control")),
        ("rate_class", serde_json::json!("R0")),
    ] {
        let mut malformed = value.clone();
        malformed[field] = replacement;
        assert!(
            RuntimeEventEnvelope::from_value(malformed).is_err(),
            "{field}"
        );
    }
    for required_nullable in ["work_id", "collaborative_run_id"] {
        let mut malformed = value.clone();
        malformed.as_object_mut().unwrap().remove(required_nullable);
        assert!(RuntimeEventEnvelope::from_value(malformed).is_err());
    }
    let mut empty_turn = value.clone();
    empty_turn["turn_id"] = serde_json::json!("");
    assert!(RuntimeEventEnvelope::from_value(empty_turn).is_err());
}

#[test]
fn source_span_must_be_ordered() {
    let mut value: serde_json::Value = serde_json::from_slice(
        &std::fs::read(format!(
            "{}/tests/fixtures/events/text.delta.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    value["source_span"] = serde_json::json!({"start": 9, "end": 8});
    assert!(RuntimeEventEnvelope::from_value(value).is_err());
}

#[test]
fn stage_one_rejects_delegation_ids_and_resource_remote_paths() {
    let event_path = |name: &str| {
        format!(
            "{}/tests/fixtures/events/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        )
    };
    let mut event: serde_json::Value =
        serde_json::from_slice(&std::fs::read(event_path("text.delta")).unwrap()).unwrap();
    event["work_id"] = serde_json::json!("work_future");
    assert!(RuntimeEventEnvelope::from_value(event).is_err());

    let mut resource: serde_json::Value =
        serde_json::from_slice(&std::fs::read(event_path("resource.ref_created")).unwrap())
            .unwrap();
    resource["payload"]["ref"]["remote_path"] = serde_json::json!("/secret/path");
    assert!(RuntimeEventEnvelope::from_value(resource).is_err());
}

#[test]
fn payload_types_vendor_class_and_nested_paths_are_fail_closed() {
    let load = |name: &str| -> serde_json::Value {
        serde_json::from_slice(
            &std::fs::read(format!(
                "{}/tests/fixtures/events/{name}.json",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap(),
        )
        .unwrap()
    };

    let mut text = load("text.delta");
    text["payload"]["content"] = serde_json::Value::Null;
    assert!(RuntimeEventEnvelope::from_value(text).is_err());

    let mut error = load("error.raised");
    error["payload"]["fatal"] = serde_json::json!("true");
    assert!(RuntimeEventEnvelope::from_value(error).is_err());

    let mut vendor = load("vendor");
    vendor["rate_class"] = serde_json::json!("R0");
    vendor["stream_id"] = serde_json::json!("control");
    assert!(RuntimeEventEnvelope::from_value(vendor).is_err());

    let mut nested_path = load("vendor");
    nested_path["vendor_extension"]["raw"]["remote_path"] = serde_json::json!("C:/secret");
    assert!(RuntimeEventEnvelope::from_value(nested_path).is_err());

    let mut top_level_extension = load("vendor");
    top_level_extension["future"] = serde_json::json!({"remote_path": "C:/secret"});
    assert!(RuntimeEventEnvelope::from_value(top_level_extension).is_err());

    let resource = load("resource.ref_created")["payload"]["ref"].clone();
    let mut nested_resource_extension = resource;
    nested_resource_extension["future"] = serde_json::json!({"remote_path": "/secret"});
    assert!(serde_json::from_value::<ResourceRef>(nested_resource_extension).is_err());

    let mut truncated_log = load("log.record");
    truncated_log["payload"]
        .as_object_mut()
        .unwrap()
        .remove("original_len");
    assert!(RuntimeEventEnvelope::from_value(truncated_log).is_err());
}

#[test]
fn unconstrained_json_values_and_boolean_json_schema_are_accepted() {
    let load = |name: &str| -> serde_json::Value {
        serde_json::from_slice(
            &std::fs::read(format!(
                "{}/tests/fixtures/events/{name}.json",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap(),
        )
        .unwrap()
    };

    let mut input = load("input.resolved");
    input["payload"]["value"] = serde_json::Value::Null;
    assert!(RuntimeEventEnvelope::from_value(input).is_ok());

    let mut tool = load("tool.call.completed");
    tool["payload"]["result"] = serde_json::Value::Null;
    assert!(RuntimeEventEnvelope::from_value(tool).is_ok());

    let mut request = load("input.requested");
    request["payload"]["schema"] = serde_json::json!(false);
    assert!(RuntimeEventEnvelope::from_value(request).is_ok());
}
