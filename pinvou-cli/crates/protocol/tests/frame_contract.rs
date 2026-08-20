use pinvou_protocol::{
    FrameError, HelloClient, HelloServer, IpcMessage, MAX_FRAME_LEN, decode_frame,
    decode_length_prefix, encode_frame, read_frame,
};

#[test]
fn frame_is_u32_little_endian_followed_by_utf8_json() {
    let message = IpcMessage::request(
        serde_json::json!(7),
        "health",
        serde_json::json!({"probe": true}),
    )
    .unwrap();
    let encoded = encode_frame(&message).unwrap();
    assert_eq!(
        u32::from_le_bytes(encoded[..4].try_into().unwrap()) as usize,
        encoded.len() - 4
    );
    assert_eq!(decode_frame::<IpcMessage>(&encoded).unwrap(), message);
}

#[test]
fn oversized_declared_length_is_rejected_from_the_prefix_alone() {
    let prefix = ((MAX_FRAME_LEN + 1) as u32).to_le_bytes();
    assert_eq!(
        decode_length_prefix(prefix),
        Err(FrameError::FrameTooLarge {
            declared: MAX_FRAME_LEN + 1,
            maximum: MAX_FRAME_LEN,
        })
    );
    assert_eq!(
        decode_length_prefix((MAX_FRAME_LEN as u32).to_le_bytes()),
        Ok(MAX_FRAME_LEN)
    );
}

#[test]
fn malformed_frames_fail_without_panicking() {
    assert_eq!(
        decode_frame::<IpcMessage>(&[1, 0, 0]),
        Err(FrameError::MissingLengthPrefix)
    );
    assert!(matches!(
        decode_frame::<IpcMessage>(&[2, 0, 0, 0, 0xff, 0xff]),
        Err(FrameError::InvalidUtf8)
    ));
    assert!(matches!(
        decode_frame::<IpcMessage>(&[4, 0, 0, 0, b'{', b'}']),
        Err(FrameError::LengthMismatch { .. })
    ));
    assert_eq!(
        decode_frame::<serde_json::Value>(&[2, 0, 0, 0, b'x', b'x']),
        Err(FrameError::InvalidJson)
    );

    for seed in 0_u32..1024 {
        let mut bytes = seed.wrapping_mul(2_654_435_761).to_le_bytes().to_vec();
        bytes.extend_from_slice(&seed.rotate_left(13).to_le_bytes());
        let _ = decode_frame::<serde_json::Value>(&bytes);
    }
}

#[test]
fn encoding_stops_when_json_exceeds_the_frame_limit() {
    let oversized = serde_json::json!({"data": "x".repeat(MAX_FRAME_LEN)});
    assert!(matches!(
        encode_frame(&oversized),
        Err(FrameError::FrameTooLarge { .. })
    ));
}

#[test]
fn streaming_reader_rejects_an_oversized_prefix_before_reading_a_body() {
    struct PrefixOnly {
        prefix: std::io::Cursor<[u8; 4]>,
    }
    impl std::io::Read for PrefixOnly {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.prefix.position() < 4 {
                self.prefix.read(buffer)
            } else {
                panic!("oversized frame body must never be read")
            }
        }
    }

    let mut reader = PrefixOnly {
        prefix: std::io::Cursor::new(((MAX_FRAME_LEN + 1) as u32).to_le_bytes()),
    };
    assert!(matches!(
        read_frame::<_, serde_json::Value>(&mut reader),
        Err(FrameError::FrameTooLarge { .. })
    ));
}

#[test]
fn ipc_v1_combinations_and_hello_challenge_are_fail_closed() {
    let valid = [
        IpcMessage::request(serde_json::json!(1), "health", serde_json::json!({})).unwrap(),
        IpcMessage::response(serde_json::json!(1), serde_json::json!({"ok": true})).unwrap(),
        IpcMessage::event("runtime", serde_json::json!({})).unwrap(),
        IpcMessage::ack("runtime", serde_json::json!({"cursor": 2})).unwrap(),
        IpcMessage::error(
            Some(serde_json::json!(1)),
            serde_json::json!({"code": "failed"}),
        )
        .unwrap(),
    ];
    for message in valid {
        assert_eq!(
            decode_frame::<IpcMessage>(&encode_frame(&message).unwrap()).unwrap(),
            message
        );
    }

    let invalid = serde_json::json!({
        "v": 2, "id": 1, "kind": "req", "method": "health", "payload": {}
    });
    assert!(decode_frame::<IpcMessage>(&encode_frame(&invalid).unwrap()).is_err());
    assert!(IpcMessage::request(serde_json::Value::Null, "health", serde_json::json!({})).is_err());
    assert!(IpcMessage::request(serde_json::json!(1), "", serde_json::json!({})).is_err());

    let client = HelloClient::new(serde_json::json!({"name": "pinvou"})).unwrap();
    let server = HelloServer::new("instance_1").unwrap();
    let client_json = serde_json::to_value(&client).unwrap();
    let server_json = serde_json::to_value(&server).unwrap();
    assert_eq!(client_json["kind"], "hello");
    assert_eq!(server_json["protocol_version"], 1);
    assert_eq!(
        serde_json::from_value::<HelloClient>(client_json).unwrap(),
        client
    );
    assert_eq!(
        serde_json::from_value::<HelloServer>(server_json).unwrap(),
        server
    );
    assert!(
        serde_json::from_value::<HelloClient>(serde_json::json!({
            "kind": "hello", "protocol_version": 2, "client_info": {}
        }))
        .is_err()
    );
    assert!(HelloClient::new(serde_json::json!("not-an-object")).is_err());
    assert!(HelloServer::new("").is_err());
}
