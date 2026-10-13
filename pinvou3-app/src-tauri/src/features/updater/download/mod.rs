//! Byte-transfer facade. The restricted GET adapter and structural planner do
//! not own protected cache writes, qualification or installation rights.

pub(crate) async fn fetch_file<'a>(
    url: &str,
    request: pinvou_upgrade_transfer::RequestPlan<'a>,
    policy: &dyn pinvou_upgrade_file_http::DestinationPolicy,
    resolver: &dyn pinvou_upgrade_file_http::AddressResolver,
) -> Result<pinvou_upgrade_file_http::FetchResult<'a>, pinvou_upgrade_file_http::FileHttpError> {
    pinvou_upgrade_file_http::fetch(url, request, policy, resolver).await
}

pub(crate) fn plan_file_response<'target, 'headers>(
    request: pinvou_upgrade_transfer::RequestPlan<'target>,
    status: u16,
    fields: impl IntoIterator<Item = (&'headers str, &'headers [u8])>,
) -> Result<
    pinvou_upgrade_transfer::ResponseDecision<'target>,
    pinvou_upgrade_transfer::TransferError,
> {
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

    #[tokio::test]
    async fn application_http_graph_refuses_credential_bearing_file_urls() {
        struct Deny;
        impl pinvou_upgrade_file_http::DestinationPolicy for Deny {
            fn permits_origin(&self, _: &pinvou_upgrade_file_http::FileOrigin) -> bool {
                false
            }
            fn permits_address(
                &self,
                _: &pinvou_upgrade_file_http::FileOrigin,
                _: std::net::SocketAddr,
            ) -> bool {
                false
            }
        }
        let hash = "a".repeat(64);
        let target = Artifact::new("package-1", 10, &hash).unwrap();
        let result = fetch_file(
            "https://@file.test.invalid/file",
            plan_request(target, None, SourceContinuity::Unproven),
            &Deny,
            &pinvou_upgrade_file_http::SystemResolver,
        )
        .await;
        assert!(matches!(
            result,
            Err(pinvou_upgrade_file_http::FileHttpError::InvalidUrl)
        ));
    }

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
