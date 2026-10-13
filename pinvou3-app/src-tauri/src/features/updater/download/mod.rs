//! Structural byte-transfer facade; network and protected cache ownership remain
//! separate. A returned plan proves no final verification or installation right.

pub(crate) fn plan_file_response<'a>(
    request: pinvou_upgrade_transfer::RequestPlan<'a>,
    status: u16,
    fields: impl IntoIterator<Item = (&'a str, &'a [u8])>,
) -> Result<pinvou_upgrade_transfer::ResponseDecision<'a>, pinvou_upgrade_transfer::TransferError> {
    pinvou_upgrade_transfer::plan_response(
        request,
        status,
        pinvou_upgrade_transfer::ResponseHeaders::from_fields(fields)?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinvou_upgrade_transfer::{
        Artifact, PrefixState, ResponseDecision, SourceContinuity, StrongEtag, TransferError,
        WriteMode, plan_request,
    };

    #[test]
    fn application_graph_replaces_ignored_range_and_latches_inexact_body() {
        let target = Artifact::new("package-1", 10, &"a".repeat(64)).unwrap();
        let prefix = PrefixState::new(
            Some(target),
            4,
            Some(StrongEtag::parse(b"\"original\"").unwrap()),
        )
        .unwrap();
        let request = plan_request(target, Some(prefix), SourceContinuity::SameImmutableEntity);
        let response =
            plan_file_response(request, 200, [("Content-Length", b"10".as_slice())]).unwrap();
        let ResponseDecision::Transfer(plan) = response else {
            panic!("expected a replacement body plan");
        };
        assert_eq!(plan.write_mode(), WriteMode::ReplaceFromZero);
        assert_eq!(plan.expected_response_length(), 10);
        let mut progress = plan.receive();
        progress.observe_bytes(9).unwrap();
        assert!(matches!(
            progress.finish(),
            Err(TransferError::BodyTruncated)
        ));
        assert_eq!(progress.observe_bytes(1), Err(TransferError::BodyTruncated));
        assert!(matches!(
            progress.finish(),
            Err(TransferError::BodyTruncated)
        ));
    }
}
