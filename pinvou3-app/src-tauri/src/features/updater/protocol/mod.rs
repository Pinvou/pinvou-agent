//! Upgrade V1 byte decoding only. No network, trust, time or installation rights.

pub(crate) fn decode_metadata(
    bytes: &[u8],
) -> Result<pinvou_upgrade_codec::Document, pinvou_upgrade_codec::Error> {
    pinvou_upgrade_codec::Document::parse(bytes, pinvou_upgrade_codec::InputKind::Metadata)
}

pub(crate) fn decode_credential(
    bytes: &[u8],
) -> Result<pinvou_upgrade_codec::Document, pinvou_upgrade_codec::Error> {
    pinvou_upgrade_codec::Document::parse(bytes, pinvou_upgrade_codec::InputKind::Credential)
}

pub(crate) fn compare_app_version_bytes(
    left: &[u8],
    right: &[u8],
) -> Result<std::cmp::Ordering, pinvou_upgrade_codec::VersionError> {
    let left = decode_metadata(left)
        .map_err(pinvou_upgrade_codec::VersionError::Json)?
        .app_version_shape()?;
    let right = decode_metadata(right)
        .map_err(pinvou_upgrade_codec::VersionError::Json)?
        .app_version_shape()?;
    Ok(pinvou_upgrade_codec::compare_app_versions(&left, &right))
}

pub(crate) fn decode_root_policy_shape(
    bytes: &[u8],
) -> Result<pinvou_upgrade_codec::RootPolicyShape, pinvou_upgrade_codec::RootPolicyError> {
    decode_metadata(bytes)
        .map_err(pinvou_upgrade_codec::RootPolicyError::Json)?
        .root_policy_shape()
}

pub(crate) fn decode_native_identity_shape(
    bytes: &[u8],
) -> Result<pinvou_upgrade_codec::NativeIdentityShape, pinvou_upgrade_codec::NativeIdentityError> {
    decode_metadata(bytes)
        .map_err(pinvou_upgrade_codec::NativeIdentityError::Json)?
        .native_identity_shape()
}

pub(crate) fn derive_consume_request_digest(
    bytes: &[u8],
) -> Result<pinvou_upgrade_codec::ConsumeRequestDigest, pinvou_upgrade_codec::ConsumeDigestError> {
    decode_metadata(bytes)
        .map_err(pinvou_upgrade_codec::ConsumeDigestError::Json)?
        .consume_request_digest()
}

pub(crate) fn derive_rollout_commitment(
    bytes: &[u8],
) -> Result<pinvou_upgrade_codec::RolloutCommitment, pinvou_upgrade_codec::RolloutCommitmentError> {
    decode_metadata(bytes)
        .map_err(pinvou_upgrade_codec::RolloutCommitmentError::Json)?
        .rollout_commitment()
}

pub(crate) fn derive_status_binding(
    bytes: &[u8],
) -> Result<pinvou_upgrade_codec::StatusBinding, pinvou_upgrade_codec::StatusBindingError> {
    decode_metadata(bytes)
        .map_err(pinvou_upgrade_codec::StatusBindingError::Json)?
        .status_binding()
}

pub(crate) fn derive_source_profile(
    bytes: &[u8],
) -> Result<pinvou_upgrade_codec::SourceProfile, pinvou_upgrade_codec::SourceProfileError> {
    decode_metadata(bytes)
        .map_err(pinvou_upgrade_codec::SourceProfileError::Json)?
        .source_profile()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_facade_compares_decimal_components_without_integer_overflow_or_normalization() {
        use std::cmp::Ordering;
        assert_eq!(
            compare_app_version_bytes(br#""9.999.999""#, br#""10.0.0""#),
            Ok(Ordering::Less)
        );
        assert_eq!(
            compare_app_version_bytes(
                br#""18446744073709551616.0.0""#,
                br#""18446744073709551615.0.0""#
            ),
            Ok(Ordering::Greater)
        );
        assert_eq!(
            compare_app_version_bytes(br#""\u0031.2.3""#, br#""1.2.3""#),
            Ok(Ordering::Equal)
        );
        assert_eq!(
            compare_app_version_bytes(br#""01.2.3""#, br#""1.2.3""#),
            Err(pinvou_upgrade_codec::VersionError::InvalidVersion)
        );
    }

    #[test]
    fn feature_facade_checks_root_policy_structure_without_anchor_or_point_trust() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let keys: Vec<_> = (0..3).map(|byte| {
            let text=URL_SAFE_NO_PAD.encode([byte;32]);
            let descriptor=pinvou_upgrade_codec::key_descriptor(&text).unwrap();
            serde_json::json!({"keyId":descriptor.key_id(),"algorithm":"ed25519","publicKey":descriptor.public_key()})
        }).collect();
        let ids: Vec<_> = keys.iter().map(|key| key["keyId"].clone()).collect();
        let mut claims = serde_json::json!({"protocolVersion":1,"role":"root","product":"pinvou","component":null,"scope":{},"version":1,"issuedAt":0,"expiresAt":0,"keys":keys,"roles":[{"role":"root","component":null,"channel":null,"targetKey":null,"threshold":2,"keyIds":ids}]});
        let shape = decode_root_policy_shape(&serde_json::to_vec_pretty(&claims).unwrap()).unwrap();
        assert_eq!(
            shape.canonical_bytes(),
            decode_metadata(&serde_json::to_vec(&claims).unwrap())
                .unwrap()
                .canonical_bytes()
                .unwrap()
        );
        claims["roles"][0]["threshold"] = serde_json::json!(1);
        assert!(matches!(
            decode_root_policy_shape(&serde_json::to_vec(&claims).unwrap()),
            Err(pinvou_upgrade_codec::RootPolicyError::InvalidPolicy)
        ));
    }

    #[test]
    fn feature_facade_compares_all_native_facts_not_only_package_bytes() {
        let vector: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/digests.json"
        )))
        .unwrap();
        let mut actual = vector["sourceFacts"]["application"].clone();
        let expected = decode_native_identity_shape(&serde_json::to_vec(&actual).unwrap()).unwrap();
        let same =
            decode_native_identity_shape(&serde_json::to_vec_pretty(&actual).unwrap()).unwrap();
        assert!(pinvou_upgrade_codec::assert_native_identity(&expected, &same).is_ok());
        actual["publisher"] = serde_json::json!("different-publisher");
        let changed = decode_native_identity_shape(&serde_json::to_vec(&actual).unwrap()).unwrap();
        assert_eq!(
            pinvou_upgrade_codec::assert_native_identity(&expected, &changed),
            Err(pinvou_upgrade_codec::NativeIdentityError::Mismatch)
        );
        actual["extra"] = serde_json::Value::Null;
        assert!(matches!(
            decode_native_identity_shape(&serde_json::to_vec(&actual).unwrap()),
            Err(pinvou_upgrade_codec::NativeIdentityError::InvalidIdentity)
        ));
    }

    #[test]
    fn feature_facade_binds_semantic_consume_request_without_transport_or_null_defaults() {
        let vector: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/digests.json"
        )))
        .unwrap();
        let mut request = vector["consumeRequest"].clone();
        let expected = vector["expectedConsumeRequestDigest"].as_str().unwrap();
        let result = derive_consume_request_digest(&serde_json::to_vec(&request).unwrap()).unwrap();
        assert_eq!(result.request_digest(), expected);
        assert_eq!(result.canonical_semantic()[0], b'{');
        request["headers"] = serde_json::json!({"ignored":true});
        request["recoverySecret"] = serde_json::Value::Null;
        assert_eq!(
            derive_consume_request_digest(&serde_json::to_vec(&request).unwrap())
                .unwrap()
                .request_digest(),
            expected
        );
        request.as_object_mut().unwrap().remove("backup");
        assert!(matches!(
            derive_consume_request_digest(&serde_json::to_vec(&request).unwrap()),
            Err(pinvou_upgrade_codec::ConsumeDigestError::InvalidRequest)
        ));
    }

    #[test]
    fn feature_facade_matches_empty_and_single_rollout_commitments() {
        let vector: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/digests.json"
        )))
        .unwrap();
        for (opening, field) in [
            (serde_json::Value::Null, "expectedEmptyCommitment"),
            (vector["opening"].clone(), "expectedOpeningCommitment"),
        ] {
            let result = derive_rollout_commitment(&serde_json::to_vec(&opening).unwrap()).unwrap();
            assert_eq!(result.commitment_hash(), vector[field].as_str().unwrap());
            assert_eq!(&result.input_bytes()[..22], b"pinvou-rollout-set-v1\0");
        }
        assert!(matches!(
            derive_rollout_commitment(b"[]"),
            Err(pinvou_upgrade_codec::RolloutCommitmentError::InvalidOpening)
        ));
    }

    #[test]
    fn feature_facade_binds_status_context_and_exact_raw_secret_without_authority() {
        let vector: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/digests.json"
        )))
        .unwrap();
        let mut binding = vector["statusBinding"].clone();
        let result = derive_status_binding(&serde_json::to_vec(&binding).unwrap()).unwrap();
        assert_eq!(
            result.binding_hash(),
            vector["expectedStatusBindingHash"].as_str().unwrap()
        );
        assert_eq!(&result.input_bytes()[..25], b"pinvou-consume-status-v1\0");
        assert_eq!(
            pinvou_upgrade_codec::recovery_handle_hash(&[1_u8; 32])
                .unwrap()
                .as_hex(),
            vector["consumeRequest"]["recoveryHandleHash"]
                .as_str()
                .unwrap()
        );
        binding["requestId"] = serde_json::json!("transport-extra");
        assert!(matches!(
            derive_status_binding(&serde_json::to_vec(&binding).unwrap()),
            Err(pinvou_upgrade_codec::StatusBindingError::InvalidBinding)
        ));
    }

    #[test]
    fn feature_facade_derives_stable_projection_without_dynamic_or_nested_epoch_confusion() {
        let vector: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/digests.json"
        )))
        .unwrap();
        let mut facts = vector["sourceFacts"].clone();
        let expected = vector["expectedSourceProfileId"].as_str().unwrap();
        assert_eq!(
            derive_source_profile(&serde_json::to_vec(&facts).unwrap())
                .unwrap()
                .source_profile_id(),
            expected
        );
        facts["ownerEpoch"] = serde_json::json!(7);
        assert_eq!(
            derive_source_profile(&serde_json::to_vec(&facts).unwrap())
                .unwrap()
                .source_profile_id(),
            expected
        );
        facts["helper"]["ownerEpoch"] = serde_json::json!(7);
        assert!(matches!(
            derive_source_profile(&serde_json::to_vec(&facts).unwrap()),
            Err(pinvou_upgrade_codec::SourceProfileError::InvalidStableFacts)
        ));
    }

    #[test]
    fn feature_facade_keeps_metadata_and_credential_profiles_distinct() {
        let mut input = vec![b' '; 65537];
        input[0] = b'0';
        assert_eq!(
            decode_metadata(&input).unwrap().canonical_bytes().unwrap(),
            b"0"
        );
        assert!(matches!(
            decode_credential(&input),
            Err(pinvou_upgrade_codec::Error::InputLimit)
        ));
    }

    #[test]
    fn feature_facade_uses_binary64_and_utf16_in_the_application_graph() {
        let document =
            decode_metadata("{\"\u{e000}\":9007199254740993,\"😀\":-0}".as_bytes()).unwrap();
        assert_eq!(
            document.canonical_bytes().unwrap(),
            "{\"😀\":0,\"\u{e000}\":9007199254740992}".as_bytes()
        );
    }

    #[test]
    fn feature_facade_binds_signed_bytes_and_public_descriptor_without_granting_trust() {
        let signed = decode_metadata(
            br#"{"protocolVersion":1,"role":"root","product":"pinvou","component":null,"scope":{}}"#,
        ).unwrap();
        let input = signed.signing_input().unwrap();
        assert_eq!(&input.as_bytes()[..25], b"pinvou-upgrade-signed-v1\0");
        let expected = br#"{"component":null,"product":"pinvou","protocolVersion":1,"role":"root","scope":{},"signed":{"component":null,"product":"pinvou","protocolVersion":1,"role":"root","scope":{}}}"#;
        assert_eq!(&input.as_bytes()[25..], expected);
        // Original public verification fixture; private signing material was discarded.
        let descriptor =
            pinvou_upgrade_codec::key_descriptor("_CJCOu9X5MQEeieRvPJYzMz_B7R5Ir1SBNpPoE9drIU")
                .unwrap();
        assert_eq!(
            descriptor.key_id(),
            "a2c128c7b29c794def8df2dc4dc1279073ebe6896f6aff073d862bb97537453c"
        );
    }
}
