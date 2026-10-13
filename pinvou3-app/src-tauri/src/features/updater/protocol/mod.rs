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

#[cfg(test)]
mod tests {
    use super::*;

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
