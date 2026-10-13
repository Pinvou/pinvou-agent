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
}
