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
