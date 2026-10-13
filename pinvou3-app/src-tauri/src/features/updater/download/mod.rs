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

pub(crate) fn fetch_cache_once_with_observer_on_worker<'a>(
    runtime: &tokio::runtime::Handle,
    request: pinvou_upgrade_file_http::CacheFetchRequest<'_, 'a>,
    file: &mut std::fs::File,
    cancelled: impl FnMut() -> bool,
    observer: &mut dyn pinvou_upgrade_file_http::CacheObserver,
) -> Result<
    pinvou_upgrade_file_http::CacheFetchOutcome<'a>,
    pinvou_upgrade_file_http::CacheReceiveError,
> {
    pinvou_upgrade_file_http::fetch_cache_once_with_observer_on_worker(
        runtime, request, file, cancelled, observer,
    )
}

pub(crate) fn fetch_cache_once_on_worker<'a>(
    runtime: &tokio::runtime::Handle,
    request: pinvou_upgrade_file_http::CacheFetchRequest<'_, 'a>,
    file: &mut std::fs::File,
    cancelled: impl FnMut() -> bool,
) -> Result<
    pinvou_upgrade_file_http::CacheFetchOutcome<'a>,
    pinvou_upgrade_file_http::CacheReceiveError,
> {
    pinvou_upgrade_file_http::fetch_cache_once_on_worker(runtime, request, file, cancelled)
}

pub(crate) fn receive_file_body_on_worker<'a>(
    runtime: &tokio::runtime::Handle,
    body: pinvou_upgrade_file_http::FileBody<'a>,
    file: &mut std::fs::File,
    permission: pinvou_upgrade_byte_check::CacheWritePermission,
    cancelled: impl FnMut() -> bool,
) -> Result<pinvou_upgrade_transfer::SegmentReceipt<'a>, pinvou_upgrade_file_http::CacheReceiveError>
{
    pinvou_upgrade_file_http::receive_cache_body_on_worker(
        runtime, body, file, permission, cancelled,
    )
}

pub(crate) fn start_cache_segment<'file, 'target>(
    file: &'file mut std::fs::File,
    plan: pinvou_upgrade_transfer::BodyPlan<'target>,
    permission: pinvou_upgrade_byte_check::CacheWritePermission,
    cancelled: impl FnMut() -> bool,
) -> Result<
    pinvou_upgrade_byte_check::FileSegmentWriter<'file, 'target>,
    pinvou_upgrade_byte_check::SegmentWriteError,
> {
    pinvou_upgrade_byte_check::FileSegmentWriter::start(file, plan, permission, cancelled)
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

pub(crate) fn inspect_package_bytes<'a>(
    file: &mut std::fs::File,
    target: pinvou_upgrade_transfer::Artifact<'a>,
    limits: pinvou_upgrade_archive::ArchiveLimits,
    cancelled: impl FnMut() -> bool,
) -> Result<pinvou_upgrade_archive::PackageInventory<'a>, pinvou_upgrade_archive::ArchiveError> {
    pinvou_upgrade_archive::inspect_file(file, target, limits, cancelled)
}

pub(crate) fn materialize_package_member<'a>(
    source: &mut std::fs::File,
    destination: &mut std::fs::File,
    target: pinvou_upgrade_transfer::Artifact<'a>,
    expected: pinvou_upgrade_archive::ExpectedMember<'_>,
    limits: pinvou_upgrade_archive::ArchiveLimits,
    cancelled: impl FnMut() -> bool,
) -> Result<pinvou_upgrade_archive::MaterializedMember<'a>, pinvou_upgrade_archive::ArchiveError> {
    pinvou_upgrade_archive::materialize_member(
        source,
        destination,
        target,
        expected,
        limits,
        cancelled,
    )
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
    fn application_cache_writer_graph_replaces_and_verifies_actual_file() {
        use std::io::Write;
        let path =
            std::env::temp_dir().join(format!("pinvou-app-cache-write-{}", uuid::Uuid::new_v4()));
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.write_all(b"old cache bytes").unwrap();
        let target = Artifact::new(
            "package-1",
            3,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        )
        .unwrap();
        let request = plan_request(target, None, SourceContinuity::Unproven);
        let ResponseDecision::Transfer(plan) =
            plan_file_response(request, 200, [("Content-Length", b"3".as_slice())]).unwrap()
        else {
            panic!("expected replacement plan");
        };
        let mut writer = start_cache_segment(
            &mut file,
            plan,
            pinvou_upgrade_byte_check::CacheWritePermission::ReplaceExisting,
            || false,
        )
        .unwrap();
        writer.write(b"abc", || false).unwrap();
        // Structural receipt fixture; this test is not a network EOF E2E.
        let mut transport = plan.receive();
        transport.observe_bytes(3).unwrap();
        let receipt = writer
            .finish_after_network_eof(transport.finish().unwrap(), || false)
            .unwrap();
        assert!(receipt.target() == target);
        assert!(
            check_file_bytes(&mut file, target, || false)
                .unwrap()
                .target()
                == target
        );
        drop(file);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn application_archive_graph_checks_real_store_deflate_and_tail() {
        use std::io::Write;
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("archive/tests/fixtures/zip-vectors.json")).unwrap();
        for case in ["store", "deflate", "empty-file", "compressed-tail"] {
            let value = vectors
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["case"] == case)
                .unwrap();
            let hex = value["hex"].as_str().unwrap();
            let bytes: Vec<u8> = hex
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect();
            let path =
                std::env::temp_dir().join(format!("pinvou-app-zip-check-{}", uuid::Uuid::new_v4()));
            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            file.write_all(&bytes).unwrap();
            let target = Artifact::new(
                "package-1",
                bytes.len() as u64,
                value["sha256"].as_str().unwrap(),
            )
            .unwrap();
            let limits = pinvou_upgrade_archive::ArchiveLimits::new(
                100000, 100000, 10, 10000, 10000, 1000000,
            )
            .unwrap();
            let result = inspect_package_bytes(&mut file, target, limits, || false);
            if case == "compressed-tail" {
                assert!(matches!(
                    result,
                    Err(pinvou_upgrade_archive::ArchiveError::CompressedTail)
                ));
            } else {
                let inventory = result.unwrap();
                assert!(inventory.target() == target);
                assert_eq!(inventory.members().len(), 1);
                assert_eq!(
                    inventory.members()[0].name(),
                    value["members"][0]["name"].as_str().unwrap()
                );
                assert_eq!(
                    inventory.members()[0].size(),
                    value["members"][0]["size"].as_u64().unwrap()
                );
                let member = &value["members"][0];
                let digest: Vec<_> = member["sha256"]
                    .as_str()
                    .unwrap()
                    .as_bytes()
                    .chunks_exact(2)
                    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                    .collect();
                let expected = pinvou_upgrade_archive::ExpectedMember::new(
                    member["name"].as_str().unwrap(),
                    member["size"].as_u64().unwrap(),
                    digest.try_into().unwrap(),
                )
                .unwrap();
                let destination_path = std::env::temp_dir()
                    .join(format!("pinvou-app-zip-member-{}", uuid::Uuid::new_v4()));
                let mut destination = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(&destination_path)
                    .unwrap();
                let materialized = materialize_package_member(
                    &mut file,
                    &mut destination,
                    target,
                    expected,
                    limits,
                    || false,
                )
                .unwrap();
                assert!(materialized.package() == target);
                assert_eq!(
                    materialized.member().size(),
                    member["size"].as_u64().unwrap()
                );
                assert_eq!(
                    destination.metadata().unwrap().len(),
                    member["size"].as_u64().unwrap()
                );
                drop(destination);
                std::fs::remove_file(destination_path).unwrap();
            }
            drop(file);
            std::fs::remove_file(path).unwrap();
        }
    }

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
