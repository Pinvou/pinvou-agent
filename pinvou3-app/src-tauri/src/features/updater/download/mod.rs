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

pub(crate) fn check_file_bytes<'a>(
    file: &mut std::fs::File,
    target: pinvou_upgrade_transfer::Artifact<'a>,
    cancelled: impl FnMut() -> bool,
) -> Result<pinvou_upgrade_byte_check::ByteMatch<'a>, pinvou_upgrade_byte_check::ByteCheckError> {
    pinvou_upgrade_byte_check::check_file(file, target, cancelled)
}

pub(crate) fn copy_checked_file_bytes<'a>(
    source: &mut std::fs::File,
    destination: &mut std::fs::File,
    target: pinvou_upgrade_transfer::Artifact<'a>,
    cancelled: impl FnMut() -> bool,
) -> Result<pinvou_upgrade_byte_check::ByteMatch<'a>, pinvou_upgrade_byte_check::ByteCheckError> {
    pinvou_upgrade_byte_check::copy_and_check(source, destination, target, cancelled)
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

    #[test]
    fn application_byte_check_graph_scans_and_copies_original_handles() {
        use std::io::{Seek, SeekFrom, Write};
        let path =
            std::env::temp_dir().join(format!("pinvou-app-byte-check-{}", uuid::Uuid::new_v4()));
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(b"abc").unwrap();
        file.seek(SeekFrom::Start(2)).unwrap();
        let target = Artifact::new(
            "package-1",
            3,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        )
        .unwrap();
        assert!(
            check_file_bytes(&mut file, target, || false)
                .unwrap()
                .target()
                == target
        );
        file.seek(SeekFrom::Start(1)).unwrap();
        file.write_all(b"x").unwrap();
        assert!(matches!(
            check_file_bytes(&mut file, target, || false),
            Err(pinvou_upgrade_byte_check::ByteCheckError::HashMismatch)
        ));
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(b"abc").unwrap();
        let destination_path =
            std::env::temp_dir().join(format!("pinvou-app-byte-copy-{}", uuid::Uuid::new_v4()));
        let mut destination = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&destination_path)
            .unwrap();
        assert!(
            copy_checked_file_bytes(&mut file, &mut destination, target, || false)
                .unwrap()
                .target()
                == target
        );
        drop(destination);
        std::fs::remove_file(destination_path).unwrap();
        drop(file);
        std::fs::remove_file(path).unwrap();
    }

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
