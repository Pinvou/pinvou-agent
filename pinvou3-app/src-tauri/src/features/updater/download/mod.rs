//! Byte-transfer facade. The restricted GET adapter and structural planner do
//! not own protected cache writes, qualification or installation rights.

pub(crate) fn create_exclusive_cache(
    path: &std::path::Path,
) -> Result<
    pinvou_upgrade_cache_object::ExclusiveCacheFile,
    pinvou_upgrade_cache_object::CacheObjectError,
> {
    pinvou_upgrade_cache_object::create_exclusive_cache(path)
}

pub(crate) fn duplicate_cache_read_handle(
    cache: &pinvou_upgrade_cache_object::ExclusiveCacheFile,
) -> Result<
    pinvou_upgrade_cache_object::ReadOnlyCacheHandle,
    pinvou_upgrade_cache_object::CacheObjectError,
> {
    cache.read_only_duplicate()
}

pub(crate) fn check_read_only_cache_bytes<'a>(
    view: &pinvou_upgrade_cache_object::ReadOnlyCacheHandle,
    target: pinvou_upgrade_transfer::Artifact<'a>,
    cancelled: impl FnMut() -> bool,
) -> Result<pinvou_upgrade_byte_check::ByteMatch<'a>, pinvou_upgrade_byte_check::ByteCheckError> {
    pinvou_upgrade_byte_check::check_read_only_cache(view, target, cancelled)
}

pub(crate) fn copy_read_only_cache_bytes<'a>(
    view: &pinvou_upgrade_cache_object::ReadOnlyCacheHandle,
    destination: &mut pinvou_upgrade_cache_object::ExclusiveCacheFile,
    target: pinvou_upgrade_transfer::Artifact<'a>,
    cancelled: impl FnMut() -> bool,
) -> Result<pinvou_upgrade_byte_check::ByteMatch<'a>, pinvou_upgrade_byte_check::ByteCheckError> {
    pinvou_upgrade_byte_check::copy_read_only_cache(view, destination, target, cancelled)
}

pub(crate) async fn fetch_file<'a>(
    url: &str,
    request: pinvou_upgrade_transfer::RequestPlan<'a>,
    policy: &dyn pinvou_upgrade_file_http::DestinationPolicy,
    resolver: &dyn pinvou_upgrade_file_http::AddressResolver,
) -> Result<pinvou_upgrade_file_http::FetchResult<'a>, pinvou_upgrade_file_http::FileHttpError> {
    pinvou_upgrade_file_http::fetch(url, request, policy, resolver).await
}

pub(crate) fn fetch_and_check_cache_once_on_worker<'a>(
    runtime: &tokio::runtime::Handle,
    request: pinvou_upgrade_file_http::CacheFetchRequest<'_, 'a>,
    file: &mut std::fs::File,
    cancelled: impl FnMut() -> bool,
    validation: &mut dyn pinvou_upgrade_file_http::CacheValidationOwner,
) -> Result<
    pinvou_upgrade_file_http::CheckedCacheOutcome<'a>,
    pinvou_upgrade_file_http::CacheCheckError,
> {
    pinvou_upgrade_file_http::fetch_and_check_cache_once_on_worker(
        runtime, request, file, cancelled, validation,
    )
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
    fn application_read_only_copy_checks_new_exclusive_target_and_refuses_nonempty_retry() {
        use std::io::Write;
        let root = std::env::temp_dir();
        let input = root.join(format!(
            "pinvou-app-readonly-copy-input-{}",
            uuid::Uuid::new_v4()
        ));
        let output = root.join(format!(
            "pinvou-app-readonly-copy-output-{}",
            uuid::Uuid::new_v4()
        ));
        match create_exclusive_cache(&input) {
            Err(pinvou_upgrade_cache_object::CacheObjectError::Unsupported) => {
                assert!(!input.exists());
                assert!(!output.exists());
            }
            Ok(mut source) => {
                source.file_mut().write_all(b"0123456789").unwrap();
                let view = duplicate_cache_read_handle(&source).unwrap();
                drop(source);
                let mut destination = create_exclusive_cache(&output).unwrap();
                let original = Artifact::new(
                    "app-cache-1",
                    10,
                    "84d89877f0d4041efb6bf91a16f0248f2fd573e6af05c19f96bedb9f882f7882",
                )
                .unwrap();
                assert!(
                    copy_read_only_cache_bytes(&view, &mut destination, original, || false)
                        .unwrap()
                        .target()
                        == original
                );
                assert!(matches!(
                    copy_read_only_cache_bytes(&view, &mut destination, original, || false),
                    Err(pinvou_upgrade_byte_check::ByteCheckError::DestinationNotEmpty)
                ));
                drop(destination);
                drop(view);
                assert_eq!(std::fs::read(&output).unwrap(), b"0123456789");
                assert_eq!(std::fs::read(&input).unwrap(), b"0123456789");
                std::fs::remove_file(input).unwrap();
                std::fs::remove_file(output).unwrap();
            }
            Err(_) => panic!("cache object creation unavailable on this test host"),
        }
    }

    #[test]
    fn application_read_only_cache_scan_matches_actual_bytes_and_rejects_original_hash_mismatch() {
        use std::io::Write;
        let path =
            std::env::temp_dir().join(format!("pinvou-app-readonly-scan-{}", uuid::Uuid::new_v4()));
        match create_exclusive_cache(&path) {
            Err(pinvou_upgrade_cache_object::CacheObjectError::Unsupported) => {
                assert!(!path.exists())
            }
            Ok(mut source) => {
                source.file_mut().write_all(b"0123456789").unwrap();
                let view = duplicate_cache_read_handle(&source).unwrap();
                drop(source);
                let target = Artifact::new(
                    "app-cache-1",
                    10,
                    "84d89877f0d4041efb6bf91a16f0248f2fd573e6af05c19f96bedb9f882f7882",
                )
                .unwrap();
                assert!(
                    check_read_only_cache_bytes(&view, target, || false)
                        .unwrap()
                        .target()
                        == target
                );
                let wrong = Artifact::new("app-cache-1", 10, &"0".repeat(64)).unwrap();
                assert!(matches!(
                    check_read_only_cache_bytes(&view, wrong, || false),
                    Err(pinvou_upgrade_byte_check::ByteCheckError::HashMismatch)
                ));
                drop(view);
                assert_eq!(std::fs::read(&path).unwrap(), b"0123456789");
                std::fs::remove_file(path).unwrap();
            }
            Err(_) => panic!("cache object creation unavailable on this test host"),
        }
    }

    #[test]
    fn application_read_only_cache_duplicate_denies_writes_and_keeps_original_sharing() {
        use std::io::{Read, Seek, SeekFrom, Write};
        let path = std::env::temp_dir().join(format!(
            "pinvou-app-cache-read-only-{}",
            uuid::Uuid::new_v4()
        ));
        match create_exclusive_cache(&path) {
            Err(pinvou_upgrade_cache_object::CacheObjectError::Unsupported) => {
                assert!(!path.exists())
            }
            Ok(mut source) => {
                source.file_mut().write_all(b"cache bytes").unwrap();
                let reader = duplicate_cache_read_handle(&source).unwrap();
                drop(source);
                let mut view = reader.file();
                view.seek(SeekFrom::Start(0)).unwrap();
                let mut bytes = vec![];
                view.read_to_end(&mut bytes).unwrap();
                assert_eq!(bytes, b"cache bytes");
                assert!(view.write_all(b"wrong").is_err());
                assert!(reader.file().set_len(0).is_err());
                assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
                drop(reader);
                assert_eq!(std::fs::read(&path).unwrap(), b"cache bytes");
                std::fs::remove_file(path).unwrap();
            }
            Err(_) => panic!("cache object unavailable on this test host"),
        }
    }

    #[test]
    fn application_cache_object_facade_preserves_exclusive_handle_or_reports_unsupported() {
        use std::io::Write;
        let path =
            std::env::temp_dir().join(format!("pinvou-app-cache-object-{}", uuid::Uuid::new_v4()));
        match create_exclusive_cache(&path) {
            Err(pinvou_upgrade_cache_object::CacheObjectError::Unsupported) => {
                assert!(!path.exists())
            }
            Ok(mut cache) => {
                cache.file_mut().write_all(b"owned cache bytes").unwrap();
                assert!(std::fs::OpenOptions::new().read(true).open(&path).is_err());
                assert!(std::fs::remove_file(&path).is_err());
                drop(cache);
                assert_eq!(std::fs::read(&path).unwrap(), b"owned cache bytes");
                std::fs::remove_file(&path).unwrap();
            }
            Err(_) => panic!("cache object creation unavailable on this test host"),
        }
    }

    #[test]
    fn application_checked_cache_uses_actual_file_even_after_complete_length_projection() {
        use pinvou_upgrade_file_http::{
            CacheCheckError, CacheFetchRequest, CacheValidationOwner, CheckedCacheOutcome,
            DestinationPolicy, FileOrigin, ReplacementPermission, SystemResolver,
            ValidationDecision,
        };
        use std::io::Write;
        struct Deny;
        impl DestinationPolicy for Deny {
            fn permits_origin(&self, _: &FileOrigin) -> bool {
                false
            }
            fn permits_address(&self, _: &FileOrigin, _: std::net::SocketAddr) -> bool {
                false
            }
        }
        struct Owner {
            begun: usize,
        }
        impl CacheValidationOwner for Owner {
            fn begin_or_resume(&mut self, target: Artifact<'_>) -> ValidationDecision {
                assert_eq!(target.package_id(), "app-cache-1");
                self.begun += 1;
                ValidationDecision::Continue // Scripted owner, not real time certification.
            }
            fn poll(&mut self, _: Artifact<'_>) -> ValidationDecision {
                ValidationDecision::Continue
            }
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let handle = runtime.handle().clone();
        runtime.block_on(async move {
            tokio::task::spawn_blocking(move || {
                let path = std::env::temp_dir()
                    .join(format!("pinvou-app-checked-cache-{}", uuid::Uuid::new_v4()));
                let mut file = std::fs::OpenOptions::new()
                    .create_new(true)
                    .read(true)
                    .write(true)
                    .open(&path)
                    .unwrap();
                file.write_all(b"0123456789").unwrap();
                let target = Artifact::new(
                    "app-cache-1",
                    10,
                    "84d89877f0d4041efb6bf91a16f0248f2fd573e6af05c19f96bedb9f882f7882",
                )
                .unwrap();
                let plan = plan_request(
                    target,
                    Some(PrefixState::new(None, 10, None).unwrap()),
                    SourceContinuity::Unproven,
                );
                let mut owner = Owner { begun: 0 };
                let request = CacheFetchRequest::new(
                    "invalid unused URL",
                    plan,
                    &Deny,
                    &SystemResolver,
                    ReplacementPermission::Refuse,
                );
                let result = fetch_and_check_cache_once_on_worker(
                    &handle,
                    request,
                    &mut file,
                    || false,
                    &mut owner,
                )
                .unwrap();
                let CheckedCacheOutcome::Complete(matched) = result else {
                    panic!("expected actual file match");
                };
                assert!(matched.target() == target);
                file.set_len(9).unwrap();
                let request = CacheFetchRequest::new(
                    "invalid unused URL",
                    plan,
                    &Deny,
                    &SystemResolver,
                    ReplacementPermission::Refuse,
                );
                assert!(matches!(
                    fetch_and_check_cache_once_on_worker(
                        &handle,
                        request,
                        &mut file,
                        || false,
                        &mut owner
                    ),
                    Err(CacheCheckError::Check(
                        pinvou_upgrade_byte_check::ByteCheckError::TooShort
                    ))
                ));
                assert_eq!(owner.begun, 2);
                drop(file);
                std::fs::remove_file(path).unwrap();
            })
            .await
            .unwrap();
        });
    }

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
