//! Upgrade V1 contract decoding and fixed-origin control POST adaptation.
//! Contract-valid transport bodies grant no signature/time/installation authority.

/// Actual single-envelope checks under independently supplied existing Root,
/// expected context/time/deny/high-water facts. Production is fixed here; this
/// facade establishes no protected bootstrap, persistent acceptance or rights.
pub(crate) fn verify_supplied_envelope_bytes<'a>(
    bytes: &[u8],
    root: &'a pinvou_upgrade_codec::RootPolicyShape,
    expected: &'a pinvou_upgrade_codec::Document,
    now: &'a pinvou_upgrade_codec::SuppliedProtocolTime,
    high_water: Option<&'a pinvou_upgrade_codec::Document>,
    denied_key_ids: &'a [String],
) -> Result<
    pinvou_upgrade_verifier::SignatureCheckedEnvelope<'a>,
    pinvou_upgrade_verifier::VerificationError,
> {
    let mut inputs = pinvou_upgrade_verifier::VerificationInputs::new(root, expected, now)
        .with_denied_key_ids(denied_key_ids);
    if let Some(record) = high_water {
        inputs = inputs.with_high_water(record);
    }
    pinvou_upgrade_verifier::verify_with_supplied_root(bytes, inputs)
}

/// One continuous Root successor using independently supplied full Root context,
/// safe time and the caller-owned complete active read set. No install/persistence.
pub(crate) fn verify_supplied_root_successor_bytes<'a>(
    bytes: &[u8],
    old_root: &'a pinvou_upgrade_codec::RootPolicyShape,
    expected_root: &pinvou_upgrade_codec::Document,
    now: &pinvou_upgrade_codec::SuppliedProtocolTime,
    active_metadata: &[pinvou_upgrade_verifier::ActiveMetadataInput<'_>],
) -> Result<
    pinvou_upgrade_verifier::SignatureCheckedRootSuccessor<'a>,
    pinvou_upgrade_verifier::VerificationError,
> {
    pinvou_upgrade_verifier::verify_root_successor(
        bytes,
        old_root,
        expected_root,
        now,
        active_metadata,
    )
}

/// Complete pure Root traversal. Only nonfinal expiry is allowed; no protected
/// install/persistence or active-snapshot completeness is established here.
pub(crate) fn verify_supplied_root_chain_bytes<'a>(
    successors: &[&[u8]],
    initial_root: &'a pinvou_upgrade_codec::RootPolicyShape,
    expected_root: &pinvou_upgrade_codec::Document,
    now: &pinvou_upgrade_codec::SuppliedProtocolTime,
    active_metadata: &[pinvou_upgrade_verifier::ActiveMetadataInput<'_>],
) -> Result<
    pinvou_upgrade_verifier::SignatureCheckedRootChain<'a>,
    pinvou_upgrade_verifier::VerificationError,
> {
    pinvou_upgrade_verifier::verify_root_chain(
        successors,
        initial_root,
        expected_root,
        now,
        active_metadata,
    )
}

/// Historical Production Package identity under caller-owned archived trust and
/// safe time. This proof grants no current qualification or execution rights.
pub(crate) fn verify_supplied_archived_package_bytes<'a>(
    bytes: &[u8],
    archived_root: &'a pinvou_upgrade_codec::RootPolicyShape,
    expected: &pinvou_upgrade_codec::Document,
    now: &pinvou_upgrade_codec::SuppliedProtocolTime,
) -> Result<
    pinvou_upgrade_verifier::ArchivedEnvelopeIdentity<'a>,
    pinvou_upgrade_verifier::VerificationError,
> {
    pinvou_upgrade_verifier::verify_archived_package(bytes, archived_root, expected, now)
}

/// Historical Release identity at each original object's own issued time.
/// Protected archive custody stays caller-owned; no current rights are granted.
pub(crate) fn verify_supplied_archived_release_bytes<'a>(
    bytes: &[u8],
    archived_root: &'a pinvou_upgrade_codec::RootPolicyShape,
    expected: &pinvou_upgrade_codec::Document,
) -> Result<
    pinvou_upgrade_verifier::ArchivedEnvelopeIdentity<'a>,
    pinvou_upgrade_verifier::VerificationError,
> {
    pinvou_upgrade_verifier::verify_archived_release(bytes, archived_root, expected)
}

/// Complete public chain under independently supplied existing trust, context,
/// safe time and denial facts. No persistent acceptance or current online rights.
pub(crate) fn verify_supplied_public_chain_bytes<'a>(
    timestamp_bytes: &[u8],
    snapshot_bytes: &[u8],
    member_bytes: &[&[u8]],
    inputs: pinvou_upgrade_verifier::PublicChainInputs<'a>,
) -> Result<
    pinvou_upgrade_verifier::SignatureCheckedPublicChain<'a>,
    pinvou_upgrade_verifier::PublicChainError,
> {
    pinvou_upgrade_verifier::verify_public_chain(
        timestamp_bytes,
        snapshot_bytes,
        member_bytes,
        inputs,
    )
}

/// Actual signed candidate pair and opening under independent caller context.
/// This neither selects a candidate nor grants persistent/current online rights.
pub(crate) fn verify_supplied_candidate_release_bytes<'a>(
    target_bytes: &[u8],
    release_bytes: &[u8],
    opening: &pinvou_upgrade_codec::Document,
    inputs: pinvou_upgrade_verifier::CandidateVerificationInputs<'a>,
) -> Result<
    pinvou_upgrade_verifier::SignatureCheckedCandidateRelease<'a>,
    pinvou_upgrade_verifier::CandidateVerificationError,
> {
    pinvou_upgrade_verifier::verify_candidate_release(target_bytes, release_bytes, opening, inputs)
}

/// Authenticate Package metadata and bind supplied Release Schema material.
/// Release authenticity and current execution/qualification remain external.
pub(crate) fn verify_supplied_release_package_bytes<'a>(
    supplied_release: &'a pinvou_upgrade_codec::SchemaCheckedEnvelope,
    package_bytes: &[u8],
    inputs: pinvou_upgrade_verifier::PackageBindingInputs<'a>,
) -> Result<
    pinvou_upgrade_verifier::SignatureCheckedPackageBinding<'a>,
    pinvou_upgrade_verifier::PackageBindingVerificationError,
> {
    pinvou_upgrade_verifier::verify_release_package(supplied_release, package_bytes, inputs)
}

/// Actual new Check/Refresh credentials correlated to the independent original
/// request; metadata acceptance and workflow writer CAS remain external.
pub(crate) fn verify_supplied_check_response_bytes<'a>(
    response: &[u8],
    operation: pinvou_upgrade_codec::CheckResponseOperation,
    inputs: pinvou_upgrade_verifier::CheckVerificationInputs<'a>,
) -> Result<
    pinvou_upgrade_verifier::SignatureCheckedCheckResponse<'a>,
    pinvou_upgrade_verifier::CheckVerificationError,
> {
    pinvou_upgrade_verifier::verify_check_response(response, operation, inputs)
}

/// Authenticate current Decision/Download inheritance; destination and active
/// protected owner qualification remain external before any network or action.
pub(crate) fn verify_supplied_download_response_bytes<'a>(
    response: &[u8],
    inputs: pinvou_upgrade_verifier::DownloadVerificationInputs<'a>,
) -> Result<
    pinvou_upgrade_verifier::SignatureCheckedDownloadResponse<'a>,
    pinvou_upgrade_verifier::DownloadVerificationError,
> {
    pinvou_upgrade_verifier::verify_download_info_response(response, inputs)
}

/// Signed historical/current Release renewal relationship; protected metadata
/// acceptance and complete chain qualification remain separate owners.
pub(crate) fn verify_supplied_release_renewal_bytes<'a>(
    previous: &[u8],
    next: &[u8],
    inputs: pinvou_upgrade_verifier::ReleaseRenewalInputs<'a>,
) -> Result<
    pinvou_upgrade_verifier::SignatureCheckedReleaseRenewal<'a>,
    pinvou_upgrade_verifier::ReleaseRenewalVerificationError,
> {
    pinvou_upgrade_verifier::verify_release_renewal(previous, next, inputs)
}

/// Exactly one fixed-origin POST; a dropped/error response can have committed
/// remotely. Owner-controlled retries and durable ACK/consume recovery remain separate.
pub(crate) async fn send_client_request_once(
    bytes: &[u8],
    operation: pinvou_upgrade_codec::ClientRequestOperation,
) -> Result<
    pinvou_upgrade_control_http::ReceivedResponse,
    pinvou_upgrade_control_http::ControlHttpError,
> {
    pinvou_upgrade_control_http::post_once(operation, bytes).await
}

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

/// Complete supplied client request contract; never authorization or a sent request.
pub(crate) fn check_supplied_client_request_bytes(
    bytes: &[u8],
    operation: pinvou_upgrade_codec::ClientRequestOperation,
) -> Result<(), pinvou_upgrade_codec::ClientRequestError> {
    decode_metadata(bytes)
        .map_err(pinvou_upgrade_codec::ClientRequestError::Json)?
        .assert_supplied_client_request_contract(operation)
}

/// Match complete supplied event request/receipt bytes; never local queue removal.
pub(crate) fn check_supplied_event_acknowledgements_bytes(
    request_bytes: &[u8],
    response_bytes: &[u8],
    operation: pinvou_upgrade_codec::ClientRequestOperation,
    http_status: u16,
) -> Result<(), pinvou_upgrade_codec::EventAcknowledgementError> {
    use pinvou_upgrade_codec::EventAcknowledgementError;
    let request = decode_metadata(request_bytes).map_err(EventAcknowledgementError::Json)?;
    let response = decode_metadata(response_bytes).map_err(EventAcknowledgementError::Json)?;
    request.assert_supplied_event_acknowledgements(&response, operation, http_status)
}

/// Complete supplied HTTP contract; unit success is structural for errors too.
pub(crate) fn check_supplied_client_http_response_bytes(
    bytes: &[u8],
    operation: pinvou_upgrade_codec::ClientResponseOperation,
    http_status: u16,
) -> Result<(), pinvou_upgrade_codec::ClientHttpResponseError> {
    decode_metadata(bytes)
        .map_err(pinvou_upgrade_codec::ClientHttpResponseError::Json)?
        .assert_supplied_client_http_response_contract(operation, http_status)
}

/// Supplied client HTTP error contract, never confirmed absence or retry rights.
pub(crate) fn check_supplied_client_error_response_bytes(
    bytes: &[u8],
    operation: pinvou_upgrade_codec::ClientResponseOperation,
    http_status: u16,
) -> Result<(), pinvou_upgrade_codec::ClientErrorResponseError> {
    decode_metadata(bytes)
        .map_err(pinvou_upgrade_codec::ClientErrorResponseError::Json)?
        .assert_supplied_client_error_response_contract(operation, http_status)
}

/// Fixed supplied client response contract; never authenticated transport or ACK.
pub(crate) fn check_supplied_client_response_bytes(
    bytes: &[u8],
    operation: pinvou_upgrade_codec::ClientResponseOperation,
) -> Result<(), pinvou_upgrade_codec::ClientResponseError> {
    decode_metadata(bytes)
        .map_err(pinvou_upgrade_codec::ClientResponseError::Json)?
        .assert_supplied_client_response_contract(operation)
}

/// Supplied channel response, never persisted settings or permission carry-over.
pub(crate) fn check_supplied_channel_change_response_bytes(
    bytes: &[u8],
) -> Result<(), pinvou_upgrade_codec::ChannelChangeResponseError> {
    decode_metadata(bytes)
        .map_err(pinvou_upgrade_codec::ChannelChangeResponseError::Json)?
        .assert_supplied_channel_change_response_contract()
}

/// Supplied helper-cleanup response contract, never protected-object deletion rights.
pub(crate) fn check_supplied_helper_cleanup_response_bytes(
    bytes: &[u8],
) -> Result<(), pinvou_upgrade_codec::HelperCleanupResponseError> {
    decode_metadata(bytes)
        .map_err(pinvou_upgrade_codec::HelperCleanupResponseError::Json)?
        .assert_supplied_helper_cleanup_response_contract()
}

/// Supplied receipt contract; never proof of event commitment or task completion.
pub(crate) fn check_supplied_receipt_response_bytes(
    bytes: &[u8],
    operation: pinvou_upgrade_codec::ReceiptResponseOperation,
) -> Result<(), pinvou_upgrade_codec::ReceiptResponseError> {
    decode_metadata(bytes)
        .map_err(pinvou_upgrade_codec::ReceiptResponseError::Json)?
        .assert_supplied_receipt_response_contract(operation)
}

/// Supplied task-create response contract; never authenticated task/action rights.
pub(crate) fn check_supplied_task_create_response_bytes(
    bytes: &[u8],
    operation: pinvou_upgrade_codec::TaskCreateResponseOperation,
) -> Result<(), pinvou_upgrade_codec::TaskCreateResponseError> {
    decode_metadata(bytes)
        .map_err(pinvou_upgrade_codec::TaskCreateResponseError::Json)?
        .assert_supplied_task_create_response_contract(operation)
}

/// Supplied transaction-control response contract, never current review or ACK.
pub(crate) fn check_supplied_transaction_control_response_bytes(
    bytes: &[u8],
    operation: pinvou_upgrade_codec::TransactionControlOperation,
) -> Result<(), pinvou_upgrade_codec::TransactionControlError> {
    decode_metadata(bytes)
        .map_err(pinvou_upgrade_codec::TransactionControlError::Json)?
        .assert_supplied_transaction_control_contract(operation)
}

/// Supplied response contract only; never an authorization or consumed ACK.
pub(crate) fn check_supplied_purpose_response_bytes(
    bytes: &[u8],
    operation: pinvou_upgrade_codec::PurposeResponseOperation,
) -> Result<(), pinvou_upgrade_codec::PurposeResponseError> {
    decode_metadata(bytes)
        .map_err(pinvou_upgrade_codec::PurposeResponseError::Json)?
        .assert_supplied_purpose_response_contract(operation)
}

/// Supplied download-info contract; does not approve or contact a file origin.
pub(crate) fn check_supplied_download_info_bytes(
    bytes: &[u8],
) -> Result<(), pinvou_upgrade_codec::DownloadInfoContractError> {
    decode_metadata(bytes)
        .map_err(pinvou_upgrade_codec::DownloadInfoContractError::Json)?
        .assert_supplied_download_info_contract()
}

/// Structural/supplied-semantic check/refresh API contract, never current trust.
pub(crate) fn check_supplied_check_response_bytes(
    bytes: &[u8],
    operation: pinvou_upgrade_codec::CheckResponseOperation,
) -> Result<(), pinvou_upgrade_codec::CheckResponseError> {
    decode_metadata(bytes)
        .map_err(pinvou_upgrade_codec::CheckResponseError::Json)?
        .assert_supplied_check_response_contract(operation)
}

pub(crate) fn check_supplied_public_graph_bytes(
    timestamp_bytes: &[u8],
    snapshot_bytes: &[u8],
    member_bytes: &[&[u8]],
    context_bytes: &[u8],
) -> Result<(), pinvou_upgrade_codec::PublicGraphError> {
    fn decode(
        bytes: &[u8],
    ) -> Result<pinvou_upgrade_codec::SchemaCheckedEnvelope, pinvou_upgrade_codec::PublicGraphError>
    {
        pinvou_upgrade_codec::decode_canonical_envelope(
            bytes,
            pinvou_upgrade_codec::InputKind::Metadata,
        )
        .map_err(pinvou_upgrade_codec::PublicGraphError::Envelope)?
        .check_full_schema()
        .map_err(pinvou_upgrade_codec::PublicGraphError::Schema)
    }
    let timestamp = decode(timestamp_bytes)?;
    let snapshot = decode(snapshot_bytes)?;
    if timestamp.envelope().signed().value()["role"] != "timestamp"
        || snapshot.envelope().signed().value()["role"] != "snapshot"
    {
        return Err(pinvou_upgrade_codec::PublicGraphError::InvalidRole);
    }
    let maximum = snapshot.envelope().signed().value()["entries"]
        .as_array()
        .ok_or(pinvou_upgrade_codec::PublicGraphError::InvalidFields)?
        .len();
    // Apply the same Snapshot-derived necessary count bound before allocating
    // decoded member factories; there is no independent aggregate byte cap.
    if member_bytes.len() > maximum {
        return Err(pinvou_upgrade_codec::PublicGraphError::MemberUnreferenced);
    }
    let members = member_bytes
        .iter()
        .map(|bytes| decode(bytes))
        .collect::<Result<Vec<_>, _>>()?;
    let context =
        decode_metadata(context_bytes).map_err(pinvou_upgrade_codec::PublicGraphError::Json)?;
    pinvou_upgrade_codec::assert_supplied_public_graph(&timestamp, &snapshot, &members, &context)
}

pub(crate) fn check_supplied_candidate_bytes(
    target_bytes: &[u8],
    release_bytes: &[u8],
    opening_bytes: &[u8],
) -> Result<(), pinvou_upgrade_codec::CandidateBindingError> {
    fn decode(
        bytes: &[u8],
    ) -> Result<
        pinvou_upgrade_codec::SchemaCheckedEnvelope,
        pinvou_upgrade_codec::CandidateBindingError,
    > {
        pinvou_upgrade_codec::decode_canonical_envelope(
            bytes,
            pinvou_upgrade_codec::InputKind::Metadata,
        )
        .map_err(pinvou_upgrade_codec::CandidateBindingError::Envelope)?
        .check_full_schema()
        .map_err(pinvou_upgrade_codec::CandidateBindingError::Schema)
    }
    let opening = decode_metadata(opening_bytes)
        .map_err(pinvou_upgrade_codec::CandidateBindingError::Json)?;
    pinvou_upgrade_codec::assert_supplied_candidate_binding(
        &decode(target_bytes)?,
        &decode(release_bytes)?,
        &opening,
    )
}

pub(crate) fn check_supplied_release_package_bytes(
    release_bytes: &[u8],
    package_bytes: &[u8],
    target_bytes: &[u8],
) -> Result<(), pinvou_upgrade_codec::ReleasePackageError> {
    fn decode(
        bytes: &[u8],
    ) -> Result<
        pinvou_upgrade_codec::SchemaCheckedEnvelope,
        pinvou_upgrade_codec::ReleasePackageError,
    > {
        pinvou_upgrade_codec::decode_canonical_envelope(
            bytes,
            pinvou_upgrade_codec::InputKind::Metadata,
        )
        .map_err(pinvou_upgrade_codec::ReleasePackageError::Envelope)?
        .check_full_schema()
        .map_err(pinvou_upgrade_codec::ReleasePackageError::Schema)
    }
    let target =
        decode_metadata(target_bytes).map_err(pinvou_upgrade_codec::ReleasePackageError::Json)?;
    pinvou_upgrade_codec::assert_supplied_release_package_binding(
        &decode(release_bytes)?,
        &decode(package_bytes)?,
        &target,
    )
}

pub(crate) fn check_supplied_release_renewal_bytes(
    previous_bytes: &[u8],
    next_bytes: &[u8],
) -> Result<(), pinvou_upgrade_codec::ReleaseRenewalError> {
    fn decode(
        bytes: &[u8],
    ) -> Result<
        pinvou_upgrade_codec::SchemaCheckedEnvelope,
        pinvou_upgrade_codec::ReleaseRenewalError,
    > {
        pinvou_upgrade_codec::decode_canonical_envelope(
            bytes,
            pinvou_upgrade_codec::InputKind::Metadata,
        )
        .map_err(pinvou_upgrade_codec::ReleaseRenewalError::Envelope)?
        .check_full_schema()
        .map_err(pinvou_upgrade_codec::ReleaseRenewalError::Schema)
    }
    pinvou_upgrade_codec::assert_supplied_release_renewal(
        &decode(previous_bytes)?,
        &decode(next_bytes)?,
    )
}

pub(crate) fn check_supplied_consume_binding_bytes(
    request_bytes: &[u8],
    authorization_bytes: &[u8],
) -> Result<(), pinvou_upgrade_codec::ConsumeBindingError> {
    let request =
        decode_metadata(request_bytes).map_err(pinvou_upgrade_codec::ConsumeBindingError::Json)?;
    let request = request
        .consume_request_shape()
        .map_err(pinvou_upgrade_codec::ConsumeBindingError::Request)?;
    let authorization = pinvou_upgrade_codec::decode_canonical_envelope(
        authorization_bytes,
        pinvou_upgrade_codec::InputKind::Credential,
    )
    .map_err(pinvou_upgrade_codec::ConsumeBindingError::Envelope)?
    .check_full_schema()
    .map_err(pinvou_upgrade_codec::ConsumeBindingError::Schema)?;
    pinvou_upgrade_codec::assert_supplied_consume_binding(&request, &authorization)
}

pub(crate) fn check_supplied_credential_semantics_bytes(
    original_bytes: &[u8],
    supplied_time: &[u8],
) -> Result<(), pinvou_upgrade_codec::CredentialSemanticsError> {
    let envelope = pinvou_upgrade_codec::decode_canonical_envelope(
        original_bytes,
        pinvou_upgrade_codec::InputKind::Credential,
    )
    .map_err(pinvou_upgrade_codec::CredentialSemanticsError::Envelope)?
    .check_full_schema()
    .map_err(pinvou_upgrade_codec::CredentialSemanticsError::Schema)?;
    let now = decode_metadata(supplied_time)
        .map_err(pinvou_upgrade_codec::CredentialSemanticsError::Json)?
        .protocol_time_shape()
        .map_err(pinvou_upgrade_codec::CredentialSemanticsError::Time)?;
    pinvou_upgrade_codec::assert_supplied_credential_semantics(&envelope, &now)
}

pub(crate) fn check_supplied_metadata_semantics_bytes(
    original_bytes: &[u8],
    supplied_time: &[u8],
    root_mode: pinvou_upgrade_codec::RootWindowMode,
) -> Result<(), pinvou_upgrade_codec::MetadataSemanticsError> {
    let envelope = pinvou_upgrade_codec::decode_canonical_envelope(
        original_bytes,
        pinvou_upgrade_codec::InputKind::Metadata,
    )
    .map_err(pinvou_upgrade_codec::MetadataSemanticsError::Envelope)?
    .check_full_schema()
    .map_err(pinvou_upgrade_codec::MetadataSemanticsError::Schema)?;
    let now = decode_metadata(supplied_time)
        .map_err(pinvou_upgrade_codec::MetadataSemanticsError::Json)?
        .protocol_time_shape()?;
    pinvou_upgrade_codec::assert_supplied_metadata_semantics(&envelope, &now, root_mode)
}

pub(crate) fn check_expected_envelope_context(
    original_bytes: &[u8],
    kind: pinvou_upgrade_codec::InputKind,
    independent_expected: &[u8],
) -> Result<(), pinvou_upgrade_codec::ContextBindingError> {
    let envelope = pinvou_upgrade_codec::decode_canonical_envelope(original_bytes, kind)
        .map_err(pinvou_upgrade_codec::ContextBindingError::Envelope)?
        .check_full_schema()
        .map_err(pinvou_upgrade_codec::ContextBindingError::Schema)?;
    let expected = decode_metadata(independent_expected)
        .map_err(pinvou_upgrade_codec::ContextBindingError::Json)?;
    pinvou_upgrade_codec::assert_supplied_expected_context(&envelope, &expected)
}

pub(crate) fn decode_schema_checked_envelope(
    bytes: &[u8],
    kind: pinvou_upgrade_codec::InputKind,
) -> Result<pinvou_upgrade_codec::SchemaCheckedEnvelope, pinvou_upgrade_codec::SchemaError> {
    pinvou_upgrade_codec::decode_canonical_envelope(bytes, kind)
        .map_err(|_| pinvou_upgrade_codec::SchemaError::InvalidEnvelope)?
        .check_full_schema()
}

pub(crate) fn check_supplied_high_water_bytes(
    original_envelope: &[u8],
    supplied_record: &[u8],
) -> Result<(), pinvou_upgrade_codec::HighWaterError> {
    // Encoding shape must be checked on these same original bytes.
    // The caller still supplies authenticated/protected acceptance separately.
    let envelope = pinvou_upgrade_codec::decode_canonical_envelope(
        original_envelope,
        pinvou_upgrade_codec::InputKind::Metadata,
    )
    .map_err(pinvou_upgrade_codec::HighWaterError::Envelope)?;
    let record =
        decode_metadata(supplied_record).map_err(pinvou_upgrade_codec::HighWaterError::Json)?;
    pinvou_upgrade_codec::assert_supplied_high_water(&envelope, &record)
}

pub(crate) fn decode_canonical_envelope_shape(
    bytes: &[u8],
    kind: pinvou_upgrade_codec::InputKind,
) -> Result<pinvou_upgrade_codec::CanonicalEnvelopeShape, pinvou_upgrade_codec::EnvelopeShapeError>
{
    pinvou_upgrade_codec::decode_canonical_envelope(bytes, kind)
}

pub(crate) fn declared_role_policy_bytes(
    root: &pinvou_upgrade_codec::RootPolicyShape,
    signed_body: &[u8],
) -> Result<Vec<u8>, pinvou_upgrade_codec::PolicyLookupError> {
    let context = decode_metadata(signed_body)
        .map_err(pinvou_upgrade_codec::PolicyLookupError::Json)?
        .role_context_shape()?;
    root.declared_policy_for(&context)?
        .canonical_bytes()
        .map_err(pinvou_upgrade_codec::PolicyLookupError::Json)
}

pub(crate) fn assert_rollout_opening_bytes(
    target: &[u8],
    release_hash: &[u8],
    opening: &[u8],
) -> Result<(), pinvou_upgrade_codec::OpeningBindingError> {
    let target =
        decode_metadata(target).map_err(pinvou_upgrade_codec::OpeningBindingError::Json)?;
    let hash =
        decode_metadata(release_hash).map_err(pinvou_upgrade_codec::OpeningBindingError::Json)?;
    let opening =
        decode_metadata(opening).map_err(pinvou_upgrade_codec::OpeningBindingError::Json)?;
    pinvou_upgrade_codec::assert_rollout_opening(&target, &hash, &opening)
}

pub(crate) fn match_metadata_reference_bytes(
    declared: &[u8],
    original_envelope: &[u8],
) -> Result<(), pinvou_upgrade_codec::MetadataReferenceError> {
    decode_metadata(declared)
        .map_err(pinvou_upgrade_codec::MetadataReferenceError::Json)?
        .metadata_reference_shape()?
        .assert_original_envelope(original_envelope)
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
    #[allow(dead_code)]
    mod download_fixture {
        include!("codec/tests/support/download_fixture.rs");
    }

    #[test]
    fn feature_facade_verifies_nineteen_original_and_current_download_signatures_and_refuses_denied_signers()
     {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let inventory: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/envelopes.json"
        )))
        .unwrap();
        let reference = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../specs/upgrade/v1/reference/canonical-json.mjs");
        let mut current = inventory["vectors"].as_array().unwrap().clone();
        download_fixture::replace_legacy_download(&mut current, &reference);
        for vector in &current {
            let root = decode_root_policy_shape(
                &serde_json::to_vec(
                    vector
                        .get("trustedRoot")
                        .unwrap_or(&inventory["trustedRoot"]),
                )
                .unwrap(),
            )
            .unwrap();
            let bytes = URL_SAFE_NO_PAD
                .decode(vector["envelopeBase64Url"].as_str().unwrap())
                .unwrap();
            let expected =
                decode_metadata(&serde_json::to_vec(&vector["expectedContext"]).unwrap()).unwrap();
            let now = decode_metadata(&serde_json::to_vec(&vector["now"]).unwrap())
                .unwrap()
                .protocol_time_shape()
                .unwrap();
            let checked = verify_supplied_envelope_bytes(&bytes, &root, &expected, &now, None, &[])
                .unwrap_or_else(|error| panic!("{}: {error}", vector["name"]));
            assert_eq!(checked.envelope().envelope().raw_bytes(), bytes);
            let denied: Vec<_> = checked.verified_signer_ids().iter().cloned().collect();
            assert!(matches!(
                verify_supplied_envelope_bytes(&bytes, &root, &expected, &now, None, &denied),
                Err(pinvou_upgrade_verifier::VerificationError::SigningKeyDenied)
            ));
        }
    }

    #[test]
    fn feature_facade_keeps_production_for_an_actually_signed_incremental_package() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let output = std::process::Command::new("node")
            .args([
                "--input-type=module",
                "-e",
                include_str!("verify/tests/envelope-oracle.mjs"),
            ])
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../specs/upgrade/v1/reference/canonical-json.mjs"),
            )
            .output()
            .expect("Node is required for the original full signature oracle");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cases: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
        let case = cases
            .iter()
            .find(|case| case["name"] == "same-signed-incremental-explicit-development")
            .unwrap();
        let bytes = URL_SAFE_NO_PAD
            .decode(case["bytes"].as_str().unwrap())
            .unwrap();
        let root =
            decode_root_policy_shape(&serde_json::to_vec(&case["trustedRoot"]).unwrap()).unwrap();
        let expected = decode_metadata(&serde_json::to_vec(&case["expected"]).unwrap()).unwrap();
        let now = decode_metadata(&serde_json::to_vec(&case["now"]).unwrap())
            .unwrap()
            .protocol_time_shape()
            .unwrap();
        assert!(
            pinvou_upgrade_verifier::verify_with_supplied_root(
                &bytes,
                pinvou_upgrade_verifier::VerificationInputs::new(&root, &expected, &now)
                    .with_package_mode(pinvou_upgrade_codec::PackageSemanticsMode::Development)
            )
            .is_ok()
        );
        assert!(matches!(
            verify_supplied_envelope_bytes(&bytes, &root, &expected, &now, None, &[]),
            Err(pinvou_upgrade_verifier::VerificationError::Metadata(
                pinvou_upgrade_codec::MetadataSemanticsError::IncrementalProduction
            ))
        ));
    }

    #[test]
    fn feature_facade_checks_both_root_policies_and_active_key_retention() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use pinvou_upgrade_verifier::ActiveMetadataInput;
        let output = std::process::Command::new("node")
            .args([
                "--input-type=module",
                "-e",
                include_str!("verify/tests/root-successor-oracle.mjs"),
            ])
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../specs/upgrade/v1/reference/canonical-json.mjs"),
            )
            .output()
            .expect("Node is required for the frozen Root successor oracle");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cases: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
        for name in [
            "distinct-old-and-next-two-of-three",
            "old-only",
            "next-only",
            "all-five-metadata-roles",
            "retired-active-key-refuses",
            "active-package-does-not-run-production-body-semantics",
        ] {
            let case = cases.iter().find(|c| c["name"] == name).unwrap();
            let old = decode_root_policy_shape(&serde_json::to_vec(&case["trustedRoot"]).unwrap())
                .unwrap();
            let expected =
                decode_metadata(&serde_json::to_vec(&case["expected"]).unwrap()).unwrap();
            let now = decode_metadata(&serde_json::to_vec(&case["now"]).unwrap())
                .unwrap()
                .protocol_time_shape()
                .unwrap();
            let bytes = URL_SAFE_NO_PAD
                .decode(case["bytes"].as_str().unwrap())
                .unwrap();
            let active: Vec<_> = case["activeEnvelopes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| {
                    (
                        URL_SAFE_NO_PAD
                            .decode(a["bytes"].as_str().unwrap())
                            .unwrap(),
                        decode_metadata(&serde_json::to_vec(&a["expected"]).unwrap()).unwrap(),
                    )
                })
                .collect();
            let inputs: Vec<_> = active
                .iter()
                .map(|(b, e)| ActiveMetadataInput::new(b, e))
                .collect();
            match verify_supplied_root_successor_bytes(&bytes, &old, &expected, &now, &inputs) {
                Ok(checked) => {
                    assert_eq!(case["accepted"], true, "unexpected acceptance {name}");
                    assert!(std::ptr::eq(checked.supplied_old_root(), &old));
                    assert_eq!(checked.envelope().envelope().raw_bytes(), bytes);
                }
                Err(error) => {
                    assert_eq!(
                        case["accepted"], false,
                        "unexpected refusal {name}: {error}"
                    );
                    assert_eq!(error.to_string(), case["rustCode"].as_str().unwrap());
                }
            }
        }
    }

    #[test]
    fn feature_facade_traverses_expired_intermediates_and_requires_fresh_final_root() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use pinvou_upgrade_verifier::ActiveMetadataInput;
        let output = std::process::Command::new("node")
            .args([
                "--input-type=module",
                "-e",
                include_str!("verify/tests/root-chain-oracle.mjs"),
            ])
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../specs/upgrade/v1/reference/canonical-json.mjs"),
            )
            .output()
            .expect("Node is required for the frozen complete Root-chain oracle");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cases: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
        for name in [
            "three-real-distinct-dual-threshold-steps",
            "expired-old-and-intermediates-fresh-final",
            "expired-final-refuses",
            "bad-middle-extra-does-not-return-prefix",
            "final-retirement-refuses",
            "4097-count-refuses-before-parse",
        ] {
            let case = cases.iter().find(|c| c["name"] == name).unwrap();
            let old = decode_root_policy_shape(&serde_json::to_vec(&case["trustedRoot"]).unwrap())
                .unwrap();
            let expected =
                decode_metadata(&serde_json::to_vec(&case["expected"]).unwrap()).unwrap();
            let now = decode_metadata(&serde_json::to_vec(&case["now"]).unwrap())
                .unwrap()
                .protocol_time_shape()
                .unwrap();
            let bytes: Vec<_> = case["successors"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| URL_SAFE_NO_PAD.decode(b.as_str().unwrap()).unwrap())
                .collect();
            let repeat = case["repeatSteps"].as_u64().unwrap() as usize;
            let successors: Vec<_> = (0..repeat)
                .flat_map(|_| bytes.iter().map(Vec::as_slice))
                .collect();
            let active: Vec<_> = case["activeEnvelopes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| {
                    (
                        URL_SAFE_NO_PAD
                            .decode(a["bytes"].as_str().unwrap())
                            .unwrap(),
                        decode_metadata(&serde_json::to_vec(&a["expected"]).unwrap()).unwrap(),
                    )
                })
                .collect();
            let inputs: Vec<_> = active
                .iter()
                .map(|(b, e)| ActiveMetadataInput::new(b, e))
                .collect();
            match verify_supplied_root_chain_bytes(&successors, &old, &expected, &now, &inputs) {
                Ok(checked) => {
                    assert_eq!(case["accepted"], true, "unexpected acceptance {name}");
                    assert!(std::ptr::eq(checked.supplied_initial_root(), &old));
                    assert_eq!(checked.checked_steps(), successors.len());
                    assert_eq!(
                        checked.final_envelope().envelope().raw_bytes(),
                        *successors.last().unwrap()
                    );
                }
                Err(error) => {
                    assert_eq!(
                        case["accepted"], false,
                        "unexpected refusal {name}: {error}"
                    );
                    assert_eq!(error.to_string(), case["rustCode"].as_str().unwrap());
                }
            }
        }
    }

    #[test]
    fn feature_facade_preserves_distinct_historical_identity_rules() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let output = std::process::Command::new("node")
            .args([
                "--input-type=module",
                "-e",
                include_str!("verify/tests/archive-identity-oracle.mjs"),
            ])
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../specs/upgrade/v1/reference/canonical-json.mjs"),
            )
            .output()
            .expect("Node is required for the frozen historical identity oracle");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cases: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
        for name in [
            "expired-root-still-identifies-package",
            "future-root-still-refuses-package",
            "archive-still-requires-production-full-package",
            "release-has-no-current-now-restriction",
            "root-issued-later-than-release-is-not-extra-policy",
            "release-issued-after-root-expiry-is-identity-only",
            "release-original-window-must-be-positive",
            "package-two-actual-signatures",
            "release-two-actual-signatures",
            "package-one-is-insufficient",
            "release-one-is-insufficient",
        ] {
            let case = cases.iter().find(|c| c["name"] == name).unwrap();
            let root =
                decode_root_policy_shape(&serde_json::to_vec(&case["archivedRoot"]).unwrap())
                    .unwrap();
            let expected =
                decode_metadata(&serde_json::to_vec(&case["expected"]).unwrap()).unwrap();
            let bytes = URL_SAFE_NO_PAD
                .decode(case["bytes"].as_str().unwrap())
                .unwrap();
            let result = match case["operation"].as_str().unwrap() {
                "package" => {
                    let now = decode_metadata(&serde_json::to_vec(&case["now"]).unwrap())
                        .unwrap()
                        .protocol_time_shape()
                        .unwrap();
                    verify_supplied_archived_package_bytes(&bytes, &root, &expected, &now)
                }
                "release" => verify_supplied_archived_release_bytes(&bytes, &root, &expected),
                other => panic!("unexpected archive operation {other}"),
            };
            match result {
                Ok(checked) => {
                    assert_eq!(
                        case["accepted"], true,
                        "unexpected identity acceptance {name}"
                    );
                    assert!(std::ptr::eq(checked.supplied_archived_root(), &root));
                    assert_eq!(checked.envelope().envelope().raw_bytes(), bytes);
                }
                Err(error) => {
                    assert_eq!(
                        case["accepted"], false,
                        "unexpected identity refusal {name}: {error}"
                    );
                    assert_eq!(error.to_string(), case["rustCode"].as_str().unwrap());
                }
            }
        }
    }

    #[test]
    fn feature_facade_authenticates_complete_public_chain_without_current_rights() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use pinvou_upgrade_verifier::PublicChainInputs;
        use std::collections::BTreeMap;
        let output = std::process::Command::new("node")
            .args([
                "--input-type=module",
                "-e",
                include_str!("verify/tests/public-chain-oracle.mjs"),
            ])
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../specs/upgrade/v1/reference/signatures.mjs"),
            )
            .output()
            .expect("Node is required for the frozen complete public-chain oracle");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cases: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
        for name in [
            "two-targets-two-packages-unordered-positive",
            "second-target-independent-policy-refuses",
            "second-target-complete-reference-scope-refuses",
            "ordinary-path-reachable-positive",
            "unreachable-candidate-leak",
            "target-key-denied-only-for-release-still-passes",
            "release-exact-role-deny-refuses",
            "bad-extra-after-valid-threshold-refuses",
            "production-incremental-package-refuses",
        ] {
            let case = cases.iter().find(|c| c["name"] == name).unwrap();
            let root = decode_root_policy_shape(&serde_json::to_vec(&case["trustedRoot"]).unwrap())
                .unwrap();
            let context = decode_metadata(&serde_json::to_vec(&case["context"]).unwrap()).unwrap();
            let now = decode_metadata(&serde_json::to_vec(&case["now"]).unwrap())
                .unwrap()
                .protocol_time_shape()
                .unwrap();
            let timestamp = STANDARD
                .decode(case["timestamp"].as_str().unwrap())
                .unwrap();
            let snapshot = STANDARD.decode(case["snapshot"].as_str().unwrap()).unwrap();
            let members: Vec<_> = case["members"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| STANDARD.decode(m.as_str().unwrap()).unwrap())
                .collect();
            let borrowed: Vec<_> = members.iter().map(Vec::as_slice).collect();
            let denied: Vec<String> = serde_json::from_value(case["deniedKeyIds"].clone()).unwrap();
            let role_denied: BTreeMap<String, Vec<String>> =
                serde_json::from_value(case["deniedKeyIdsByRole"].clone()).unwrap();
            let mut inputs =
                PublicChainInputs::new(&root, &context, &now).with_denied_key_ids(&denied);
            for (role, ids) in &role_denied {
                inputs = inputs.with_role_denied_key_ids(role, ids);
            }
            match verify_supplied_public_chain_bytes(&timestamp, &snapshot, &borrowed, inputs) {
                Ok(checked) => {
                    assert_eq!(
                        case["accepted"], true,
                        "unexpected public-chain acceptance {name}"
                    );
                    assert!(std::ptr::eq(checked.supplied_root(), &root));
                    assert_eq!(checked.members().len(), members.len());
                    for (envelope, raw) in checked.members().iter().zip(&members) {
                        assert_eq!(envelope.envelope().raw_bytes(), raw);
                    }
                }
                Err(error) => {
                    assert_eq!(
                        case["accepted"], false,
                        "unexpected public-chain refusal {name}: {error}"
                    );
                    let expected = match case["code"].as_str().unwrap() {
                        "ROLE_NOT_AUTHORIZED" => "ROLE_POLICY_NOT_DECLARED",
                        "REFERENCE_MISMATCH" => "METADATA_REFERENCE_MISMATCH",
                        code => code,
                    };
                    assert_eq!(
                        error.to_string(),
                        expected,
                        "wrong public-chain refusal {name}"
                    );
                }
            }
        }
    }

    #[test]
    fn feature_facade_authenticates_candidate_opening_without_selecting_it() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use pinvou_upgrade_verifier::CandidateVerificationInputs;
        let output = std::process::Command::new("node")
            .args([
                "--input-type=module",
                "-e",
                include_str!("verify/tests/candidate-oracle.mjs"),
            ])
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../specs/upgrade/v1/reference/signatures.mjs"),
            )
            .output()
            .expect("Node is required for the frozen complete candidate oracle");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cases: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
        for name in [
            "genuine-empty-catalog-candidate",
            "opening-restored-positive",
            "baseline-is-not-candidate",
            "null-opening-refuses",
            "independent-expectedReleaseId",
            "release-cannot-switch-component",
            "release-target-membership",
            "bad-extra-after-valid-target-threshold",
            "two-real-target-signatures",
            "invalid-release-signature",
        ] {
            let case = cases.iter().find(|c| c["name"] == name).unwrap();
            let root = decode_root_policy_shape(&serde_json::to_vec(&case["trustedRoot"]).unwrap())
                .unwrap();
            let context = decode_metadata(&serde_json::to_vec(&case["context"]).unwrap()).unwrap();
            let opening = decode_metadata(&serde_json::to_vec(&case["opening"]).unwrap()).unwrap();
            let now = decode_metadata(&serde_json::to_vec(&case["now"]).unwrap())
                .unwrap()
                .protocol_time_shape()
                .unwrap();
            let target = STANDARD
                .decode(case["targetBytes"].as_str().unwrap())
                .unwrap();
            let release = STANDARD
                .decode(case["releaseBytes"].as_str().unwrap())
                .unwrap();
            let denied: Vec<String> = serde_json::from_value(case["deniedKeyIds"].clone()).unwrap();
            let inputs = CandidateVerificationInputs::new(&root, &context, &now)
                .with_denied_key_ids(&denied);
            match verify_supplied_candidate_release_bytes(&target, &release, &opening, inputs) {
                Ok(checked) => {
                    assert_eq!(
                        case["accepted"], true,
                        "unexpected candidate acceptance {name}"
                    );
                    assert!(std::ptr::eq(checked.supplied_root(), &root));
                    assert_eq!(checked.target().envelope().raw_bytes(), target);
                    assert_eq!(checked.release().envelope().raw_bytes(), release);
                }
                Err(error) => {
                    assert_eq!(
                        case["accepted"], false,
                        "unexpected candidate refusal {name}: {error}"
                    );
                    let expected = match case["code"].as_str().unwrap() {
                        "OPENING_MISMATCH" => "ROLLOUT_OPENING_MISMATCH",
                        code => code,
                    };
                    assert_eq!(
                        error.to_string(),
                        expected,
                        "wrong candidate refusal {name}"
                    );
                }
            }
        }
    }

    #[test]
    fn feature_facade_authenticates_package_without_certifying_supplied_release() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use pinvou_upgrade_verifier::PackageBindingInputs;
        let output = std::process::Command::new("node")
            .args([
                "--input-type=module",
                "-e",
                include_str!("verify/tests/package-binding-oracle.mjs"),
            ])
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../specs/upgrade/v1/reference/signatures.mjs"),
            )
            .output()
            .expect("Node is required for the frozen Package binding oracle");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cases: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
        for name in [
            "original-package-binding",
            "release-mode-subset-positive",
            "expired-release-positive",
            "invalid-release-signature-positive",
            "foreign-release-component-positive",
            "bad-extra-after-valid-threshold",
            "two-actual-package-signatures",
            "unsigned-package-body-change",
            "original-reference-version",
            "production-incremental-refuses",
        ] {
            let case = cases.iter().find(|c| c["name"] == name).unwrap();
            let root = decode_root_policy_shape(&serde_json::to_vec(&case["trustedRoot"]).unwrap())
                .unwrap();
            let context = decode_metadata(&serde_json::to_vec(&case["context"]).unwrap()).unwrap();
            let now = decode_metadata(&serde_json::to_vec(&case["now"]).unwrap())
                .unwrap()
                .protocol_time_shape()
                .unwrap();
            let release_raw = STANDARD
                .decode(case["releaseBytes"].as_str().unwrap())
                .unwrap();
            let release = decode_schema_checked_envelope(
                &release_raw,
                pinvou_upgrade_codec::InputKind::Metadata,
            )
            .unwrap();
            let package = STANDARD
                .decode(case["packageBytes"].as_str().unwrap())
                .unwrap();
            let denied: Vec<String> = serde_json::from_value(case["deniedKeyIds"].clone()).unwrap();
            let inputs =
                PackageBindingInputs::new(&root, &context, &now).with_denied_key_ids(&denied);
            match verify_supplied_release_package_bytes(&release, &package, inputs) {
                Ok(checked) => {
                    assert_eq!(
                        case["accepted"], true,
                        "unexpected Package acceptance {name}"
                    );
                    assert!(std::ptr::eq(checked.supplied_root(), &root));
                    assert!(std::ptr::eq(checked.supplied_release(), &release));
                    assert_eq!(checked.checked_package().envelope().raw_bytes(), package);
                }
                Err(error) => {
                    assert_eq!(
                        case["accepted"], false,
                        "unexpected Package refusal {name}: {error}"
                    );
                    let expected = if name == "original-reference-version" {
                        "METADATA_REFERENCE_INVALID"
                    } else {
                        case["code"].as_str().unwrap()
                    };
                    assert_eq!(error.to_string(), expected, "wrong Package refusal {name}");
                }
            }
        }
    }

    #[test]
    fn feature_facade_authenticates_and_correlates_successful_check_responses() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use pinvou_upgrade_codec::CheckResponseOperation;
        use pinvou_upgrade_verifier::CheckVerificationInputs;
        let output = std::process::Command::new("node")
            .args([
                "--input-type=module",
                "-e",
                include_str!("verify/tests/check-response-oracle.mjs"),
            ])
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../specs/upgrade/v1/reference/signatures.mjs"),
            )
            .output()
            .expect("Node required for composed signed-response controls");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cases: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
        for name in [
            "signed-check-update",
            "signed-check-no-update",
            "signed-refresh",
            "expired-prior-refresh-decision",
            "unverified-old-refresh-signature",
            "whitespace-unordered-transport",
            "request-host-arch",
            "decision-requestNonce",
            "telemetryCredential-bad-extra-after-valid-threshold",
            "actual-dual-credential-thresholds",
            "telemetryCredential-at-expiry",
            "refreshed-workflow-mismatch",
        ] {
            let case = cases.iter().find(|c| c["name"] == name).unwrap();
            let root = decode_root_policy_shape(&serde_json::to_vec(&case["trustedRoot"]).unwrap())
                .unwrap();
            let now = decode_metadata(&serde_json::to_vec(&case["now"]).unwrap())
                .unwrap()
                .protocol_time_shape()
                .unwrap();
            let raw_request = STANDARD
                .decode(case["requestBytes"].as_str().unwrap())
                .unwrap();
            let request = decode_metadata(&raw_request).unwrap();
            let response = STANDARD
                .decode(case["responseBytes"].as_str().unwrap())
                .unwrap();
            let denied: Vec<String> = serde_json::from_value(case["deniedKeyIds"].clone()).unwrap();
            let operation = if case["operation"] == "check" {
                CheckResponseOperation::Check
            } else {
                CheckResponseOperation::Refresh
            };
            match verify_supplied_check_response_bytes(
                &response,
                operation,
                CheckVerificationInputs::new(&root, &request, &now).with_denied_key_ids(&denied),
            ) {
                Ok(proof) => {
                    assert_eq!(
                        case["accepted"], true,
                        "unexpected response acceptance {name}"
                    );
                    assert!(std::ptr::eq(proof.supplied_root(), &root));
                    assert!(std::ptr::eq(proof.supplied_request(), &request));
                    assert_eq!(proof.original_response_bytes(), response);
                    assert_eq!(proof.operation(), operation);
                    let transport = decode_metadata(&response).unwrap();
                    for (field, credential) in [
                        ("decision", proof.decision()),
                        ("telemetryCredential", proof.telemetry_credential()),
                    ] {
                        assert_eq!(
                            credential.envelope().raw_bytes(),
                            decode_metadata(
                                &serde_json::to_vec(&transport.value()[field]).unwrap()
                            )
                            .unwrap()
                            .canonical_bytes()
                            .unwrap()
                        );
                    }
                }
                Err(error) => {
                    assert_eq!(
                        case["accepted"], false,
                        "unexpected response refusal {name}: {error}"
                    );
                    assert_eq!(
                        error.to_string(),
                        case["code"].as_str().unwrap(),
                        "wrong response refusal {name}"
                    );
                }
            }
        }
    }

    #[test]
    fn feature_facade_authenticates_download_response_and_current_request_decision() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use pinvou_upgrade_verifier::DownloadVerificationInputs;
        let output = std::process::Command::new("node")
            .args([
                "--input-type=module",
                "-e",
                include_str!("verify/tests/download-response-oracle.mjs"),
            ])
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../specs/upgrade/v1/reference/signatures.mjs"),
            )
            .output()
            .expect("Node required for composed signed-download controls");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cases: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
        for name in [
            "signed-download-info",
            "transport-whitespace",
            "distinct-role-jti-and-key",
            "inherited-forward-path",
            "inherited-update",
            "requested-other-package",
            "both-actual-two-signer-thresholds",
            "credential-bad-extra-after-valid-threshold",
            "expired-request-decision",
            "new-download-at-expiry",
            "url-at-expiry",
            "url-last-valid-millisecond",
        ] {
            let case = cases.iter().find(|c| c["name"] == name).unwrap();
            let root = decode_root_policy_shape(&serde_json::to_vec(&case["trustedRoot"]).unwrap())
                .unwrap();
            let now = decode_metadata(&serde_json::to_vec(&case["now"]).unwrap())
                .unwrap()
                .protocol_time_shape()
                .unwrap();
            let raw_request = STANDARD
                .decode(case["requestBytes"].as_str().unwrap())
                .unwrap();
            let request = decode_metadata(&raw_request).unwrap();
            let response = STANDARD
                .decode(case["responseBytes"].as_str().unwrap())
                .unwrap();
            let denied: Vec<String> = serde_json::from_value(case["deniedKeyIds"].clone()).unwrap();
            match verify_supplied_download_response_bytes(
                &response,
                DownloadVerificationInputs::new(&root, &request, &now).with_denied_key_ids(&denied),
            ) {
                Ok(proof) => {
                    assert_eq!(
                        case["accepted"], true,
                        "unexpected download acceptance {name}"
                    );
                    assert!(std::ptr::eq(proof.supplied_root(), &root));
                    assert!(std::ptr::eq(proof.supplied_request(), &request));
                    assert_eq!(proof.original_response_bytes(), response);
                    let transport = decode_metadata(&response).unwrap();
                    let original =
                        decode_metadata(&serde_json::to_vec(&request.value()["decision"]).unwrap())
                            .unwrap()
                            .canonical_bytes()
                            .unwrap();
                    assert_eq!(proof.decision().envelope().raw_bytes(), original);
                    let original = decode_metadata(
                        &serde_json::to_vec(&transport.value()["credential"]).unwrap(),
                    )
                    .unwrap()
                    .canonical_bytes()
                    .unwrap();
                    assert_eq!(proof.download_credential().envelope().raw_bytes(), original);
                }
                Err(error) => {
                    assert_eq!(
                        case["accepted"], false,
                        "unexpected download refusal {name}: {error}"
                    );
                    assert_eq!(
                        error.to_string(),
                        case["code"].as_str().unwrap(),
                        "wrong download refusal {name}"
                    );
                }
            }
        }
    }

    #[test]
    fn feature_facade_preserves_authenticated_release_renewal_proof_levels() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use pinvou_upgrade_verifier::ReleaseRenewalInputs;
        let output = std::process::Command::new("node")
            .args([
                "--input-type=module",
                "-e",
                include_str!("verify/tests/release-renewal-oracle.mjs"),
            ])
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../specs/upgrade/v1/reference/signatures.mjs"),
            )
            .output()
            .expect("Node required for composed Release renewal controls");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cases: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
        for name in [
            "expired-history-distinct-current-signer",
            "future-historical-own-time-no-cross-order",
            "historical-key-denied-not-retroactive",
            "unchanged-current-root-and-release-key",
            "both-actual-independent-thresholds",
            "previous-bad-extra-after-valid-threshold",
            "immutable-notes",
            "independent-component",
            "revision-equal",
            "next-release-future",
            "next-noncanonical",
            "historical-original-window-invalid",
        ] {
            let case = cases.iter().find(|c| c["name"] == name).unwrap();
            let archived =
                decode_root_policy_shape(&serde_json::to_vec(&case["archivedRoot"]).unwrap())
                    .unwrap();
            let current =
                decode_root_policy_shape(&serde_json::to_vec(&case["currentRoot"]).unwrap())
                    .unwrap();
            let context = decode_metadata(&serde_json::to_vec(&case["context"]).unwrap()).unwrap();
            let now = decode_metadata(&serde_json::to_vec(&case["now"]).unwrap())
                .unwrap()
                .protocol_time_shape()
                .unwrap();
            let previous = STANDARD
                .decode(case["previousBytes"].as_str().unwrap())
                .unwrap();
            let next = STANDARD
                .decode(case["nextBytes"].as_str().unwrap())
                .unwrap();
            let denied: Vec<String> = serde_json::from_value(case["deniedKeyIds"].clone()).unwrap();
            match verify_supplied_release_renewal_bytes(
                &previous,
                &next,
                ReleaseRenewalInputs::new(&archived, &current, &context, &now)
                    .with_denied_key_ids(&denied),
            ) {
                Ok(proof) => {
                    assert_eq!(
                        case["accepted"], true,
                        "unexpected renewal acceptance {name}"
                    );
                    assert!(std::ptr::eq(
                        proof.previous().supplied_archived_root(),
                        &archived
                    ));
                    assert!(std::ptr::eq(proof.next().supplied_root(), &current));
                    assert_eq!(proof.previous().envelope().envelope().raw_bytes(), previous);
                    assert_eq!(proof.next().envelope().envelope().raw_bytes(), next);
                    assert_eq!(proof.next().supplied_time_millis(), now.as_millis());
                }
                Err(error) => {
                    assert_eq!(
                        case["accepted"], false,
                        "unexpected renewal refusal {name}: {error}"
                    );
                    assert_eq!(
                        error.to_string(),
                        case["code"].as_str().unwrap(),
                        "wrong renewal refusal {name}"
                    );
                }
            }
        }
    }

    #[test]
    fn feature_facade_matches_original_event_batches_without_queue_authority() {
        use pinvou_upgrade_codec::{
            ClientRequestOperation as Op, EventAcknowledgementError as AckError,
        };
        let output = std::process::Command::new("node")
            .args([
                "--input-type=module",
                "-e",
                include_str!("codec/tests/event-acknowledgement-fixtures.mjs"),
            ])
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../specs/upgrade/v1/reference/canonical-json.mjs"),
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let fixtures: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(fixtures.len(), 6);
        for case in fixtures {
            let op: Op = case["operation"].as_str().unwrap().parse().unwrap();
            let request = serde_json::to_vec(&case["request"]).unwrap();
            let response = serde_json::to_vec(&case["response"]).unwrap();
            assert_eq!(
                check_supplied_event_acknowledgements_bytes(&request, &response, op, 200),
                Ok(())
            );
            let mut changed = case["response"].clone();
            changed["acknowledgements"][1]["eventId"] = serde_json::json!("foreign-event");
            assert_eq!(
                check_supplied_event_acknowledgements_bytes(
                    &request,
                    &serde_json::to_vec(&changed).unwrap(),
                    op,
                    200
                ),
                Err(AckError::AcknowledgementMismatch)
            );
            let error = serde_json::to_vec(&serde_json::json!({"protocolVersion":1,
                "requestId":"request-error","code":"SERVICE_UNAVAILABLE","retryable":true}))
            .unwrap();
            assert_eq!(
                check_supplied_client_http_response_bytes(&error, op, 503),
                Ok(())
            );
            assert_eq!(
                check_supplied_event_acknowledgements_bytes(&request, &error, op, 503),
                Err(AckError::InvalidStatus)
            );
            for request_side in [true, false] {
                let original = if request_side { &request } else { &response };
                let duplicate = String::from_utf8(original.clone()).unwrap().replacen(
                    "{",
                    r#"{"protocolVersion":1,"#,
                    1,
                );
                let (input, receipt) = if request_side {
                    (duplicate.as_bytes(), response.as_slice())
                } else {
                    (request.as_slice(), duplicate.as_bytes())
                };
                assert_eq!(
                    check_supplied_event_acknowledgements_bytes(input, receipt, op, 200),
                    Err(AckError::Json(pinvou_upgrade_codec::Error::DuplicateKey))
                );
            }
            assert_eq!(
                check_supplied_event_acknowledgements_bytes(&request, &response, op, 200),
                Ok(())
            );
        }
    }

    #[test]
    fn feature_facade_checks_request_bytes_without_sending_or_authorizing_them() {
        use pinvou_upgrade_codec::{
            ClientRequestError as ContractError, ClientRequestOperation as Op, Error,
        };
        let mut status = serde_json::json!({"protocolVersion":1,"product":"pinvou","component":"app",
            "installationScopeId":"scope-1","purpose":"install","authorizationJti":"jti-1",
            "consumeKey":"consume-1","transactionId":"transaction-1",
            "consumeRequestDigest":"a".repeat(64),"recoverySecret":"A".repeat(43)});
        let check = |v: &serde_json::Value| {
            check_supplied_client_request_bytes(&serde_json::to_vec(v).unwrap(), Op::ConsumeStatus)
        };
        assert_eq!(check(&status), Ok(()));
        status["recoverySecret"] = serde_json::json!("A".repeat(42) + "B");
        assert_eq!(check(&status), Err(ContractError::InvalidContract));
        status["recoverySecret"] = serde_json::json!("A".repeat(43));
        assert_eq!(check(&status), Ok(()));
        let duplicate =
            serde_json::to_string(&status)
                .unwrap()
                .replacen("{", r#"{"protocolVersion":1,"#, 1);
        assert_eq!(
            check_supplied_client_request_bytes(duplicate.as_bytes(), Op::ConsumeStatus),
            Err(ContractError::Json(Error::DuplicateKey))
        );
    }

    #[test]
    fn feature_facade_dispatches_http_contracts_without_interpreting_business_success() {
        use pinvou_upgrade_codec::{
            ClientHttpResponseError as ContractError, ClientResponseOperation as Operation, Error,
        };
        let channel = serde_json::json!({"protocolVersion":1,"requestId":"request-1",
            "channel":"stable","channelRevision":1});
        let error = serde_json::json!({"protocolVersion":1,"requestId":"request-1",
            "code":"SERVICE_UNAVAILABLE","retryable":true});
        let check = |v: &serde_json::Value, status| {
            check_supplied_client_http_response_bytes(
                &serde_json::to_vec(v).unwrap(),
                Operation::ChannelChange,
                status,
            )
        };
        assert_eq!(check(&channel, 200), Ok(()));
        assert_eq!(check(&error, 503), Ok(()));
        assert_eq!(check(&channel, 204), Err(ContractError::InvalidStatus));
        assert!(matches!(
            check(&channel, 503),
            Err(ContractError::ErrorResponse(_))
        ));
        assert!(matches!(
            check(&error, 200),
            Err(ContractError::Operation(_))
        ));
        let duplicate = serde_json::to_string(&channel).unwrap().replacen(
            "{",
            r#"{"requestId":"duplicate","#,
            1,
        );
        assert_eq!(
            check_supplied_client_http_response_bytes(
                duplicate.as_bytes(),
                Operation::ChannelChange,
                200
            ),
            Err(ContractError::Json(Error::DuplicateKey))
        );
        assert_eq!(check(&channel, 200), Ok(()));
    }

    #[test]
    fn feature_facade_checks_error_membership_without_retry_or_outcome_authority() {
        use pinvou_upgrade_codec::{
            ClientErrorResponseError as ContractError, ClientResponseOperation as Operation,
        };
        let unactivated = serde_json::json!({"protocolVersion":1,"requestId":"request-1",
            "code":"SCOPE_UNACTIVATED","retryable":true,"checkAfterMs":86400000});
        let check = |v: &serde_json::Value, operation, status| {
            check_supplied_client_error_response_bytes(
                &serde_json::to_vec(v).unwrap(),
                operation,
                status,
            )
        };
        assert_eq!(check(&unactivated, Operation::Check, 409), Ok(()));
        assert_eq!(
            check(&unactivated, Operation::Refresh, 409),
            Err(ContractError::InvalidContract)
        );
        assert_eq!(
            check(&unactivated, Operation::Check, 503),
            Err(ContractError::InvalidContract)
        );
        assert_eq!(check(&unactivated, Operation::Check, 409), Ok(()));
        let unknown = serde_json::json!({"protocolVersion":1,"requestId":"request-1",
            "code":"UNKNOWN_CONSUME_OUTCOME","retryable":false});
        assert_eq!(check(&unknown, Operation::ConsumeStatus, 404), Ok(()));
        assert_eq!(
            check(&unknown, Operation::ConsumeInstall, 404),
            Err(ContractError::InvalidContract)
        );
    }

    #[test]
    fn feature_facade_routes_client_bytes_without_transport_or_trusted_ack() {
        use pinvou_upgrade_codec::{
            ChannelChangeResponseError, ClientResponseError as ContractError,
            ClientResponseOperation as Operation, Error,
        };
        let mut channel = serde_json::json!({"protocolVersion":1,"requestId":"request-1",
            "channel":"stable","channelRevision":1});
        let check = |value: &serde_json::Value, operation| {
            check_supplied_client_response_bytes(&serde_json::to_vec(value).unwrap(), operation)
        };
        assert_eq!(check(&channel, Operation::ChannelChange), Ok(()));
        channel["channelRevision"] = serde_json::json!(0);
        assert_eq!(
            check(&channel, Operation::ChannelChange),
            Err(ContractError::ChannelChange(
                ChannelChangeResponseError::InvalidContract
            ))
        );
        channel["channelRevision"] = serde_json::json!(1);
        let mut padded = serde_json::to_vec(&channel).unwrap();
        padded.resize(65537, b' ');
        assert_eq!(
            check_supplied_client_response_bytes(&padded, Operation::ChannelChange),
            Ok(())
        );
        let duplicate = serde_json::to_string(&channel).unwrap().replacen(
            "{",
            r#"{"requestId":"duplicate","#,
            1,
        );
        assert_eq!(
            check_supplied_client_response_bytes(duplicate.as_bytes(), Operation::ChannelChange),
            Err(ContractError::Json(Error::DuplicateKey))
        );
        let consumed = serde_json::json!({"protocolVersion":1,"requestId":"request-1",
            "state":"consumed","transactionId":"transaction-1"});
        assert_eq!(check(&consumed, Operation::ConsumeStatus), Ok(()));
        let receipt = serde_json::json!({"protocolVersion":1,"requestId":"request-1",
            "acknowledgements":[{"eventId":"event-1","disposition":"committed"}]});
        assert_eq!(check(&receipt, Operation::TelemetryEvents), Ok(()));
    }

    #[test]
    fn feature_facade_checks_channel_response_without_changing_settings_or_qualifications() {
        use pinvou_upgrade_codec::ChannelChangeResponseError as ContractError;
        let mut response = serde_json::json!({"protocolVersion":1,"requestId":"request-1",
            "channel":"stable","channelRevision":1});
        let check = |v: &serde_json::Value| {
            check_supplied_channel_change_response_bytes(&serde_json::to_vec(v).unwrap())
        };
        for channel in ["stable", "beta", "internal"] {
            response["channel"] = serde_json::json!(channel);
            assert_eq!(check(&response), Ok(()));
        }
        response["channelRevision"] = serde_json::json!(0);
        assert_eq!(check(&response), Err(ContractError::InvalidContract));
        response["channelRevision"] = serde_json::json!(1);
        assert_eq!(check(&response), Ok(()));
    }

    #[test]
    fn feature_facade_checks_cleanup_response_without_authorizing_protected_object_deletion() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use pinvou_upgrade_codec::HelperCleanupResponseError as ContractError;
        let vectors: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/envelopes.json"
        )))
        .unwrap();
        let v = vectors["vectors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == "helper-cleanup")
            .unwrap();
        let envelope: serde_json::Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(v["envelopeBase64Url"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        let mut response =
            serde_json::json!({"protocolVersion":1,"requestId":"request-1","credential":envelope});
        let check = |v: &serde_json::Value| {
            check_supplied_helper_cleanup_response_bytes(&serde_json::to_vec(v).unwrap())
        };
        assert_eq!(check(&response), Ok(()));
        response["credential"]["signed"]["role"] = serde_json::json!("download");
        assert_eq!(check(&response), Err(ContractError::InvalidContract));
        response["credential"]["signed"]["role"] = serde_json::json!("helper-cleanup");
        assert_eq!(check(&response), Ok(()));
    }

    #[test]
    fn feature_facade_checks_receipts_without_committing_events_or_releasing_protection() {
        use pinvou_upgrade_codec::{
            ReceiptResponseError as ContractError, ReceiptResponseOperation as Operation,
        };
        for operation in [
            Operation::TelemetryEvents,
            Operation::PreinstallEvents,
            Operation::InstallEvents,
            Operation::ActivateEvents,
            Operation::ReconciliationEvents,
            Operation::RecoveryReviewEvents,
            Operation::ReconciliationAction,
            Operation::RecoveryReviewAction,
        ] {
            let events = !matches!(
                operation,
                Operation::ReconciliationAction | Operation::RecoveryReviewAction
            );
            let mut response = serde_json::json!({"protocolVersion":1,"requestId":"request-1"});
            if events {
                response["acknowledgements"] = serde_json::json!([
                {"eventId":"event-1","disposition":"committed"},{"eventId":"event-1","disposition":"late-diagnostic"}]);
            } else {
                response["taskId"] = serde_json::json!("task-1");
                response["taskRevision"] = serde_json::json!(0);
                response["disposition"] = serde_json::json!("recorded");
            }
            let check = |v: &serde_json::Value| {
                check_supplied_receipt_response_bytes(&serde_json::to_vec(v).unwrap(), operation)
            };
            assert_eq!(check(&response), Ok(()));
            let baseline = response.clone();
            if events {
                response["acknowledgements"][0]["disposition"] = serde_json::json!("pending");
            } else {
                response["disposition"] = serde_json::json!("approved");
            }
            assert_eq!(check(&response), Err(ContractError::InvalidContract));
            assert_eq!(check(&baseline), Ok(()));
        }
    }

    #[test]
    fn feature_facade_binds_task_responses_without_creating_a_task_or_action_authority() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use pinvou_upgrade_codec::{
            TaskCreateResponseError as ContractError, TaskCreateResponseOperation as Operation,
        };
        let vectors: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/envelopes.json"
        )))
        .unwrap();
        let envelope = |name: &str| -> serde_json::Value {
            let v = vectors["vectors"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["name"] == name)
                .unwrap();
            serde_json::from_slice(
                &URL_SAFE_NO_PAD
                    .decode(v["envelopeBase64Url"].as_str().unwrap())
                    .unwrap(),
            )
            .unwrap()
        };
        for (operation, prefix) in [
            (Operation::Reconciliation, "reconciliation"),
            (Operation::RecoveryReview, "recovery-review"),
        ] {
            let action = envelope(&format!("{prefix}-action"));
            let event = envelope(&format!("{prefix}-event"));
            let mut response = serde_json::json!({"protocolVersion":1,"requestId":"request-1",
                "task":action["signed"]["task"],"actionCredential":action,"eventCredential":event});
            let check = |v: &serde_json::Value| {
                check_supplied_task_create_response_bytes(
                    &serde_json::to_vec(v).unwrap(),
                    operation,
                )
            };
            assert_eq!(check(&response), Ok(()));
            response["eventCredential"]["signed"]["task"]["revision"] = serde_json::json!(2);
            assert_eq!(check(&response), Err(ContractError::BindingMismatch));
            response["task"]["revision"] = serde_json::json!(2);
            response["actionCredential"]["signed"]["task"]["revision"] = serde_json::json!(2);
            assert_eq!(check(&response), Ok(()));
        }
    }

    #[test]
    fn feature_facade_checks_transaction_responses_without_creating_review_or_ack() {
        use pinvou_upgrade_codec::{
            TransactionControlError as ContractError, TransactionControlOperation as Operation,
        };
        for operation in [
            Operation::ConsumeStatus,
            Operation::CancelPreinstall,
            Operation::CancelInstall,
            Operation::CancelActivate,
            Operation::ReviewPreinstall,
            Operation::ReviewInstall,
            Operation::ReviewActivate,
        ] {
            let review = matches!(
                operation,
                Operation::ReviewPreinstall | Operation::ReviewInstall | Operation::ReviewActivate
            );
            let mut response = serde_json::json!({"protocolVersion":1,"requestId":"request-1"});
            if review {
                response["transactionId"] = serde_json::json!("transaction-1");
                response["transactionRevision"] = serde_json::json!(0);
                response["reviewedAt"] = serde_json::json!(0);
                response["verifiedObjectSha256"] = serde_json::json!("a".repeat(64));
                response["currentReadSet"] = serde_json::json!([
                    {"recordKind":"transaction","recordId":"record-1","revision":1,"sha256":"a".repeat(64)},
                    {"recordKind":"transaction","recordId":"record-1","revision":2,"sha256":"a".repeat(64)}]);
            } else if operation == Operation::ConsumeStatus {
                response["state"] = serde_json::json!("consumed");
                response["transactionId"] = serde_json::json!("transaction-1");
            } else {
                response["authorizationState"] = serde_json::json!("consumed");
                response["authorizationRevision"] = serde_json::json!(0);
            }
            let check = |value: &serde_json::Value| {
                check_supplied_transaction_control_response_bytes(
                    &serde_json::to_vec(value).unwrap(),
                    operation,
                )
            };
            assert_eq!(check(&response), Ok(()));
            let baseline = response.clone();
            if review {
                response["currentReadSet"][1] = response["currentReadSet"][0].clone();
            } else if operation == Operation::ConsumeStatus {
                response["transactionId"] = serde_json::Value::Null;
            } else {
                response["authorizationState"] = serde_json::json!("pending");
            }
            assert_eq!(check(&response), Err(ContractError::InvalidContract));
            assert_eq!(check(&baseline), Ok(()));
        }
    }

    #[test]
    fn feature_facade_checks_six_purpose_responses_without_creating_an_authorization_or_ack() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use pinvou_upgrade_codec::{PurposeResponseError, PurposeResponseOperation as Operation};
        let vectors: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/envelopes.json"
        )))
        .unwrap();
        let envelope = |role: &str| -> serde_json::Value {
            let v = vectors["vectors"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["name"] == role)
                .unwrap();
            serde_json::from_slice(
                &URL_SAFE_NO_PAD
                    .decode(v["envelopeBase64Url"].as_str().unwrap())
                    .unwrap(),
            )
            .unwrap()
        };
        for (validate, consume, purpose) in [
            (
                Operation::ValidatePreinstall,
                Operation::ConsumePreinstall,
                "preinstall",
            ),
            (
                Operation::ValidateInstall,
                Operation::ConsumeInstall,
                "install",
            ),
            (
                Operation::ValidateActivate,
                Operation::ConsumeActivate,
                "activate",
            ),
        ] {
            let authorization = envelope(&format!("authorization-{purpose}"));
            let response = serde_json::json!({"protocolVersion":1,"requestId":"request-1",
                "authorizationRevision":0,"authorization":authorization});
            let check = |value: &serde_json::Value, operation| {
                check_supplied_purpose_response_bytes(
                    &serde_json::to_vec(value).unwrap(),
                    operation,
                )
            };
            assert_eq!(check(&response, validate), Ok(()));
            assert_eq!(
                check(&response, consume),
                Err(PurposeResponseError::InvalidContract)
            );
            let credential = envelope(&format!("{purpose}-transaction-event"));
            let mut response = serde_json::json!({"protocolVersion":1,"requestId":"request-1",
                "authorizationState":"consumed","authorizationRevision":0,
                "transactionId":credential["signed"]["transactionId"],"transactionCredential":credential});
            assert_eq!(check(&response, consume), Ok(()));
            response["transactionId"] = serde_json::json!("other-transaction");
            assert_eq!(
                check(&response, consume),
                Err(PurposeResponseError::BindingMismatch)
            );
            response["transactionId"] =
                response["transactionCredential"]["signed"]["transactionId"].clone();
            assert_eq!(check(&response, consume), Ok(()));
        }
    }

    #[test]
    fn feature_facade_binds_download_info_without_approving_destination_or_fetch_rights() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use pinvou_upgrade_codec::DownloadInfoContractError;
        let reference = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../specs/upgrade/v1/reference/canonical-json.mjs");
        let v = download_fixture::current_download_for_url(&reference, Some("https://"));
        let credential: serde_json::Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(v["envelopeBase64Url"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        let mut response = serde_json::json!({"protocolVersion":1,"requestId":"request-1",
            "packageId":credential["signed"]["packageId"],"size":credential["signed"]["package"]["size"],
            "sha256":credential["signed"]["package"]["sha256"],"url":"https://",
            "expiresAt":credential["signed"]["exp"],"fileSourceClass":"primary","credential":credential});
        let check = |value: &serde_json::Value| {
            check_supplied_download_info_bytes(&serde_json::to_vec(value).unwrap())
        };
        assert_eq!(check(&response), Ok(())); // Opaque Schema URL data is not ApprovedDestination.
        response["packageId"] = serde_json::json!("other-package");
        assert_eq!(
            check(&response),
            Err(DownloadInfoContractError::BindingMismatch)
        );
        response["packageId"] = response["credential"]["signed"]["packageId"].clone();
        assert_eq!(check(&response), Ok(()));
        response["expiresAt"] = response["credential"]["signed"]["nbf"].clone();
        assert_eq!(
            check(&response),
            Err(DownloadInfoContractError::BindingMismatch)
        );
        response["expiresAt"] = response["credential"]["signed"]["exp"].clone();
        response["url"] = serde_json::json!("http://example.invalid/file");
        assert_eq!(
            check(&response),
            Err(DownloadInfoContractError::InvalidContract)
        );
    }

    #[test]
    fn feature_facade_checks_closed_check_refresh_contract_without_authentication() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use pinvou_upgrade_codec::{CheckResponseError, CheckResponseOperation};
        let vectors: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/envelopes.json"
        )))
        .unwrap();
        let envelope = |role: &str| -> serde_json::Value {
            let v = vectors["vectors"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["name"] == role)
                .unwrap();
            serde_json::from_slice(
                &URL_SAFE_NO_PAD
                    .decode(v["envelopeBase64Url"].as_str().unwrap())
                    .unwrap(),
            )
            .unwrap()
        };
        let mut response = serde_json::json!({"protocolVersion":1,"requestId":"request-1",
            "decision":envelope("decision"),"telemetryCredential":envelope("telemetry-event")});
        let check = |value: &serde_json::Value, operation| {
            check_supplied_check_response_bytes(&serde_json::to_vec(value).unwrap(), operation)
        };
        assert_eq!(check(&response, CheckResponseOperation::Check), Ok(()));
        response["telemetryCredential"]["signed"]["decisionId"] =
            serde_json::json!("other-decision");
        assert_eq!(
            check(&response, CheckResponseOperation::Check),
            Err(CheckResponseError::BindingMismatch)
        );
        response["telemetryCredential"]["signed"]["decisionId"] =
            response["decision"]["signed"]["decisionId"].clone();
        response["workflowRevision"] = serde_json::json!(0);
        response["writableSessionId"] =
            response["decision"]["signed"]["telemetrySessionId"].clone();
        assert_eq!(check(&response, CheckResponseOperation::Refresh), Ok(()));
        response["writableSessionId"] = serde_json::json!("other-session");
        assert_eq!(
            check(&response, CheckResponseOperation::Refresh),
            Err(CheckResponseError::BindingMismatch)
        );
        assert_eq!(
            check(&response, CheckResponseOperation::Check),
            Err(CheckResponseError::InvalidContract)
        );
    }

    #[test]
    fn feature_facade_checks_complete_public_graph_relationships_without_granting_trust() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let vectors: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/envelopes.json"
        )))
        .unwrap();
        let original = |role: &str| {
            let v = vectors["vectors"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["name"] == role)
                .unwrap();
            URL_SAFE_NO_PAD
                .decode(v["envelopeBase64Url"].as_str().unwrap())
                .unwrap()
        };
        let timestamp = original("timestamp");
        let snapshot = original("snapshot");
        let members = [original("target"), original("release"), original("package")];
        let borrowed: Vec<&[u8]> = members.iter().map(Vec::as_slice).collect();
        let context = br#"{"product":"pinvou","component":"app"}"#;
        assert!(
            check_supplied_public_graph_bytes(&timestamp, &snapshot, &borrowed, context).is_ok()
        );
        assert_eq!(
            check_supplied_public_graph_bytes(&timestamp, &snapshot, &borrowed[..2], context),
            Err(pinvou_upgrade_codec::PublicGraphError::MemberMissing),
        );
        assert!(matches!(
            check_supplied_public_graph_bytes(
                &timestamp,
                &snapshot,
                &borrowed,
                br#"{"product":"pinvou","component":"engine"}"#
            ),
            Err(pinvou_upgrade_codec::PublicGraphError::Context(_)),
        ));
        let mut extra = borrowed.clone();
        extra.push(members[2].as_slice());
        assert_eq!(
            check_supplied_public_graph_bytes(&timestamp, &snapshot, &extra, context),
            Err(pinvou_upgrade_codec::PublicGraphError::MemberUnreferenced),
        );
    }

    #[test]
    fn feature_facade_binds_candidate_raw_release_and_opening_without_selecting_it() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use sha2::{Digest, Sha256};
        let vectors: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/envelopes.json"
        )))
        .unwrap();
        let original = |role: &str| {
            let v = vectors["vectors"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["name"] == role)
                .unwrap();
            URL_SAFE_NO_PAD
                .decode(v["envelopeBase64Url"].as_str().unwrap())
                .unwrap()
        };
        let release = original("release");
        let mut target: serde_json::Value = serde_json::from_slice(&original("target")).unwrap();
        let hash: String = Sha256::digest(&release)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let opening = serde_json::json!({
            "targetKey": target["signed"]["scope"]["targetKey"],
            "deploymentId": "qa-deployment", "deploymentRevision": 1,
            "rolloutId": "qa-rollout", "rolloutRevision": 1,
            "releaseEnvelopeSha256": hash, "leafSalt": "0".repeat(64),
        });
        let opening_raw = serde_json::to_vec(&opening).unwrap();
        let commitment = decode_metadata(&opening_raw)
            .unwrap()
            .rollout_commitment()
            .unwrap();
        target["signed"]["rolloutSetCommitment"] = serde_json::json!(commitment.commitment_hash());
        target["signed"]["baselineRelease"]["envelopeSha256"] = serde_json::json!("0".repeat(64));
        let raw = decode_metadata(&serde_json::to_vec(&target).unwrap())
            .unwrap()
            .canonical_bytes()
            .unwrap();
        assert!(check_supplied_candidate_bytes(&raw, &release, &opening_raw).is_ok());
        target["signed"]["baselineRelease"]["envelopeSha256"] =
            opening["releaseEnvelopeSha256"].clone();
        let raw = decode_metadata(&serde_json::to_vec(&target).unwrap())
            .unwrap()
            .canonical_bytes()
            .unwrap();
        assert_eq!(
            check_supplied_candidate_bytes(&raw, &release, &opening_raw),
            Err(pinvou_upgrade_codec::CandidateBindingError::IsBaseline)
        );
        target["signed"]["baselineRelease"]["envelopeSha256"] = serde_json::json!("0".repeat(64));
        let raw = decode_metadata(&serde_json::to_vec(&target).unwrap())
            .unwrap()
            .canonical_bytes()
            .unwrap();
        assert!(check_supplied_candidate_bytes(&raw, &release, &opening_raw).is_ok());
        assert_eq!(
            check_supplied_candidate_bytes(&raw, &release, b"null"),
            Err(pinvou_upgrade_codec::CandidateBindingError::Opening(
                pinvou_upgrade_codec::OpeningBindingError::Mismatch,
            )),
        );
    }

    #[test]
    fn feature_facade_binds_release_package_after_matching_the_complete_reference() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let vectors: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/envelopes.json"
        )))
        .unwrap();
        let original = |role: &str| {
            let v = vectors["vectors"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["name"] == role)
                .unwrap();
            URL_SAFE_NO_PAD
                .decode(v["envelopeBase64Url"].as_str().unwrap())
                .unwrap()
        };
        let mut release: serde_json::Value = serde_json::from_slice(&original("release")).unwrap();
        let mut package: serde_json::Value = serde_json::from_slice(&original("package")).unwrap();
        let key = serde_json::to_vec(&package["signed"]["scope"]["targetKey"]).unwrap();
        let bind = |release: &mut serde_json::Value, package: &serde_json::Value| {
            let raw = decode_metadata(&serde_json::to_vec(package).unwrap())
                .unwrap()
                .canonical_bytes()
                .unwrap();
            let reference = pinvou_upgrade_codec::derive_metadata_reference(&raw).unwrap();
            release["signed"]["targets"][0]["packageManifest"] =
                serde_json::from_slice(reference.canonical_bytes()).unwrap();
            raw
        };
        let raw = bind(&mut release, &package);
        let rraw = decode_metadata(&serde_json::to_vec(&release).unwrap())
            .unwrap()
            .canonical_bytes()
            .unwrap();
        assert!(check_supplied_release_package_bytes(&rraw, &raw, &key).is_ok());
        package["signed"]["launcher"]["publisher"] = serde_json::json!("another-publisher");
        let raw = bind(&mut release, &package);
        let rraw = decode_metadata(&serde_json::to_vec(&release).unwrap())
            .unwrap()
            .canonical_bytes()
            .unwrap();
        assert_eq!(
            check_supplied_release_package_bytes(&rraw, &raw, &key),
            Err(pinvou_upgrade_codec::ReleasePackageError::BindingMismatch)
        );
        assert_eq!(
            check_supplied_release_package_bytes(&rraw, &raw, b"null"),
            Err(pinvou_upgrade_codec::ReleasePackageError::TargetInvalid)
        );
    }

    #[test]
    fn feature_facade_checks_release_renewal_business_without_publishing_or_time_authority() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let vectors: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/envelopes.json"
        )))
        .unwrap();
        let vector = vectors["vectors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == "release")
            .unwrap();
        let previous = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let mut next: serde_json::Value = serde_json::from_slice(&previous).unwrap();
        next["signed"]["revision"] =
            serde_json::json!(next["signed"]["revision"].as_u64().unwrap() + 2);
        next["signed"]["issuedAt"] = serde_json::json!(9007199254740991u64);
        next["signed"]["expiresAt"] = serde_json::json!(0);
        let bytes = decode_metadata(&serde_json::to_vec(&next).unwrap())
            .unwrap()
            .canonical_bytes()
            .unwrap();
        assert!(check_supplied_release_renewal_bytes(&previous, &bytes).is_ok());
        next["signed"]["appVersion"] = serde_json::json!("9.0.0");
        let bytes = decode_metadata(&serde_json::to_vec(&next).unwrap())
            .unwrap()
            .canonical_bytes()
            .unwrap();
        assert_eq!(
            check_supplied_release_renewal_bytes(&previous, &bytes),
            Err(pinvou_upgrade_codec::ReleaseRenewalError::BusinessChanged)
        );
        assert_eq!(
            check_supplied_release_renewal_bytes(&previous, &previous),
            Err(pinvou_upgrade_codec::ReleaseRenewalError::RevisionInvalid)
        );
    }

    #[test]
    fn feature_facade_matches_three_supplied_consume_bindings_and_refuses_stale_raw_hash() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use sha2::{Digest, Sha256};
        let vectors: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/envelopes.json"
        )))
        .unwrap();
        let base: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/digests.json"
        )))
        .unwrap();
        for purpose in ["preinstall", "install", "activate"] {
            let role = format!("authorization-{purpose}");
            let vector = vectors["vectors"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["name"] == role)
                .unwrap();
            let raw = URL_SAFE_NO_PAD
                .decode(vector["envelopeBase64Url"].as_str().unwrap())
                .unwrap();
            let mut envelope: serde_json::Value = serde_json::from_slice(&raw).unwrap();
            let signed = &envelope["signed"];
            let mut request = base["consumeRequest"].clone();
            for field in [
                "product",
                "component",
                "purpose",
                "authorizationJti",
                "transactionId",
                "clientFactsDigest",
                "preparation",
                "freezeEpoch",
                "backup",
                "staged",
                "preinstallSlot",
            ] {
                request[field] = signed[field].clone();
            }
            request["installationScopeId"] = signed["scope"]["installationScopeId"].clone();
            request["plans"] = signed["update"]["plans"].clone();
            let hash: String = Sha256::digest(&raw)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            request["authorizationEnvelopeSha256"] = serde_json::json!(hash);
            let request_raw = serde_json::to_vec(&request).unwrap();
            assert!(check_supplied_consume_binding_bytes(&request_raw, &raw).is_ok());
            envelope["signatures"][0]["signature"] =
                serde_json::json!(URL_SAFE_NO_PAD.encode([0; 64]));
            let changed = decode_metadata(&serde_json::to_vec(&envelope).unwrap())
                .unwrap()
                .canonical_bytes()
                .unwrap();
            assert_eq!(
                check_supplied_consume_binding_bytes(&request_raw, &changed),
                Err(pinvou_upgrade_codec::ConsumeBindingError::BindingMismatch)
            );
            request["headers"] = serde_json::json!(null);
            assert!(matches!(
                check_supplied_consume_binding_bytes(&serde_json::to_vec(&request).unwrap(), &raw),
                Err(pinvou_upgrade_codec::ConsumeBindingError::Request(_))
            ));
        }
    }

    #[test]
    fn feature_facade_checks_credential_semantics_without_authentication_or_consumption() {
        let vectors: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/envelopes.json"
        )))
        .unwrap();
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let vector = &vectors["vectors"][6];
        let raw = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let now = serde_json::to_vec(&vector["now"]).unwrap();
        assert!(check_supplied_credential_semantics_bytes(&raw, &now).is_ok());
        let expiry = serde_json::to_vec(&value["signed"]["exp"]).unwrap();
        assert_eq!(
            check_supplied_credential_semantics_bytes(&raw, &expiry),
            Err(pinvou_upgrade_codec::CredentialSemanticsError::WindowClosed)
        );
        value["signed"]["updateAvailable"] = serde_json::json!(false);
        let changed = decode_metadata(&serde_json::to_vec(&value).unwrap())
            .unwrap()
            .canonical_bytes()
            .unwrap();
        assert_eq!(
            check_supplied_credential_semantics_bytes(&changed, &now),
            Err(pinvou_upgrade_codec::CredentialSemanticsError::DecisionBinding)
        );
        assert!(matches!(
            check_supplied_credential_semantics_bytes(&raw, b"null"),
            Err(pinvou_upgrade_codec::CredentialSemanticsError::Time(_))
        ));
    }

    #[test]
    fn feature_facade_checks_metadata_windows_at_supplied_time_without_clock_authority() {
        let vectors: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/envelopes.json"
        )))
        .unwrap();
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let vector = &vectors["vectors"][0];
        let raw = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let now = serde_json::to_vec(&vector["now"]).unwrap();
        assert!(
            check_supplied_metadata_semantics_bytes(
                &raw,
                &now,
                pinvou_upgrade_codec::RootWindowMode::Current
            )
            .is_ok()
        );
        let expiry = serde_json::to_vec(&value["signed"]["expiresAt"]).unwrap();
        assert_eq!(
            check_supplied_metadata_semantics_bytes(
                &raw,
                &expiry,
                pinvou_upgrade_codec::RootWindowMode::Current
            ),
            Err(pinvou_upgrade_codec::MetadataSemanticsError::ClosedWindow)
        );
        assert!(
            check_supplied_metadata_semantics_bytes(
                &raw,
                &expiry,
                pinvou_upgrade_codec::RootWindowMode::AllowExpired
            )
            .is_ok()
        );
        assert_eq!(
            check_supplied_metadata_semantics_bytes(
                &raw,
                b"null",
                pinvou_upgrade_codec::RootWindowMode::Current
            ),
            Err(pinvou_upgrade_codec::MetadataSemanticsError::InvalidTime)
        );
    }

    #[test]
    fn feature_facade_binds_independent_expected_context_and_full_scope() {
        let vectors: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/envelopes.json"
        )))
        .unwrap();
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        for (index, kind) in [
            (0, pinvou_upgrade_codec::InputKind::Metadata),
            (6, pinvou_upgrade_codec::InputKind::Credential),
        ] {
            let vector = &vectors["vectors"][index];
            let raw = URL_SAFE_NO_PAD
                .decode(vector["envelopeBase64Url"].as_str().unwrap())
                .unwrap();
            let mut expected = vector["expectedContext"].clone();
            assert!(
                check_expected_envelope_context(
                    &raw,
                    kind,
                    &serde_json::to_vec(&expected).unwrap()
                )
                .is_ok()
            );
            expected["scope"]["unexpected"] = serde_json::json!(true);
            assert_eq!(
                check_expected_envelope_context(
                    &raw,
                    kind,
                    &serde_json::to_vec(&expected).unwrap()
                ),
                Err(pinvou_upgrade_codec::ContextBindingError::Mismatch)
            );
            expected = vector["expectedContext"].clone();
            expected["product"] = serde_json::json!("other");
            assert_eq!(
                check_expected_envelope_context(
                    &raw,
                    kind,
                    &serde_json::to_vec(&expected).unwrap()
                ),
                Err(pinvou_upgrade_codec::ContextBindingError::Mismatch)
            );
            expected.as_object_mut().unwrap().remove("component");
            assert_eq!(
                check_expected_envelope_context(
                    &raw,
                    kind,
                    &serde_json::to_vec(&expected).unwrap()
                ),
                Err(pinvou_upgrade_codec::ContextBindingError::InvalidExpected)
            );
        }
    }

    #[test]
    fn feature_facade_checks_complete_frozen_envelope_schema_without_signature_trust() {
        let vectors: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/envelopes.json"
        )))
        .unwrap();
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        for (index, kind) in [
            (0, pinvou_upgrade_codec::InputKind::Metadata),
            (6, pinvou_upgrade_codec::InputKind::Credential),
        ] {
            let raw = URL_SAFE_NO_PAD
                .decode(
                    vectors["vectors"][index]["envelopeBase64Url"]
                        .as_str()
                        .unwrap(),
                )
                .unwrap();
            let shape = decode_schema_checked_envelope(&raw, kind).unwrap();
            assert_eq!(shape.envelope().raw_bytes(), raw);
            let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
            value["signed"]["unexpected"] = serde_json::json!(true);
            let changed = decode_metadata(&serde_json::to_vec(&value).unwrap())
                .unwrap()
                .canonical_bytes()
                .unwrap();
            assert!(matches!(
                decode_schema_checked_envelope(&changed, kind),
                Err(pinvou_upgrade_codec::SchemaError::InvalidEnvelope)
            ));
            value["signed"]
                .as_object_mut()
                .unwrap()
                .remove("unexpected");
            value["signatures"][0]["signature"] =
                serde_json::json!(URL_SAFE_NO_PAD.encode([0; 64]));
            let changed = decode_metadata(&serde_json::to_vec(&value).unwrap())
                .unwrap()
                .canonical_bytes()
                .unwrap();
            assert!(decode_schema_checked_envelope(&changed, kind).is_ok());
        }
    }

    #[test]
    fn feature_facade_compares_supplied_high_water_with_original_full_envelope() {
        let vectors: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/envelopes.json"
        )))
        .unwrap();
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use sha2::{Digest, Sha256};
        let vector = vectors["vectors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == "release")
            .unwrap();
        let raw = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let envelope: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let signed = &envelope["signed"];
        let hash: String = Sha256::digest(&raw)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut record = serde_json::json!({"scope":vector["expectedContext"],
            "version":signed["revision"], "envelopeSha256":hash});
        assert!(
            check_supplied_high_water_bytes(&raw, &serde_json::to_vec(&record).unwrap()).is_ok()
        );
        record["envelopeSha256"] = serde_json::json!("0".repeat(64));
        assert_eq!(
            check_supplied_high_water_bytes(&raw, &serde_json::to_vec(&record).unwrap()),
            Err(pinvou_upgrade_codec::HighWaterError::Equivocation)
        );
        record["version"] = serde_json::json!(signed["revision"].as_u64().unwrap() + 1);
        assert_eq!(
            check_supplied_high_water_bytes(&raw, &serde_json::to_vec(&record).unwrap()),
            Err(pinvou_upgrade_codec::HighWaterError::Rollback)
        );
    }

    #[test]
    fn feature_facade_checks_original_envelope_encoding_and_role_profile() {
        let vectors: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/envelopes.json"
        )))
        .unwrap();
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        for (index, kind) in [
            (0, pinvou_upgrade_codec::InputKind::Metadata),
            (6, pinvou_upgrade_codec::InputKind::Credential),
        ] {
            let raw = URL_SAFE_NO_PAD
                .decode(
                    vectors["vectors"][index]["envelopeBase64Url"]
                        .as_str()
                        .unwrap(),
                )
                .unwrap();
            let shape = decode_canonical_envelope_shape(&raw, kind).unwrap();
            assert_eq!(shape.raw_bytes(), raw);
            assert!(!shape.signatures().is_empty());
            let opposite = if kind == pinvou_upgrade_codec::InputKind::Metadata {
                pinvou_upgrade_codec::InputKind::Credential
            } else {
                pinvou_upgrade_codec::InputKind::Metadata
            };
            assert!(matches!(
                decode_canonical_envelope_shape(&raw, opposite),
                Err(pinvou_upgrade_codec::EnvelopeShapeError::WrongProfile)
            ));
            let mut changed = raw.clone();
            changed.push(b' ');
            assert!(matches!(
                decode_canonical_envelope_shape(&changed, kind),
                Err(pinvou_upgrade_codec::EnvelopeShapeError::NotCanonical)
            ));
            let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
            value["signatures"][0]["signature"] = serde_json::json!("A".repeat(85) + "B");
            let canonical = decode_metadata(&serde_json::to_vec(&value).unwrap())
                .unwrap()
                .canonical_bytes()
                .unwrap();
            assert!(matches!(
                decode_canonical_envelope_shape(&canonical, kind),
                Err(pinvou_upgrade_codec::EnvelopeShapeError::InvalidSignature)
            ));
        }
    }

    #[test]
    fn feature_facade_looks_up_only_exact_declared_role_scope_without_product_fallback() {
        let vector: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/envelopes.json"
        )))
        .unwrap();
        let root =
            decode_root_policy_shape(&serde_json::to_vec(&vector["trustedRoot"]).unwrap()).unwrap();
        let mut context = serde_json::json!({"role":"target","product":"pinvou","component":"app","scope":{"channel":"stable","targetKey":"linux-x86_64-deb-perMachine"}});
        let selected =
            declared_role_policy_bytes(&root, &serde_json::to_vec(&context).unwrap()).unwrap();
        let selected: serde_json::Value = serde_json::from_slice(&selected).unwrap();
        assert_eq!(selected["role"], serde_json::json!("target"));
        assert_eq!(selected["threshold"], serde_json::json!(1));
        context["product"] = serde_json::json!("other-product");
        assert_eq!(
            declared_role_policy_bytes(&root, &serde_json::to_vec(&context).unwrap()),
            Err(pinvou_upgrade_codec::PolicyLookupError::ProductMismatch)
        );
        context["product"] = serde_json::json!("pinvou");
        context["scope"]["channel"] = serde_json::json!("beta");
        assert_eq!(
            declared_role_policy_bytes(&root, &serde_json::to_vec(&context).unwrap()),
            Err(pinvou_upgrade_codec::PolicyLookupError::NotDeclared)
        );
    }

    #[test]
    fn feature_facade_binds_supplied_target_hash_opening_and_rejects_null_or_wrong_hash() {
        let vector: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/upgrade/v1/vectors/crypto/digests.json"
        )))
        .unwrap();
        let opening = serde_json::to_vec(&vector["opening"]).unwrap();
        let hash = serde_json::to_vec(&vector["opening"]["releaseEnvelopeSha256"]).unwrap();
        let target = serde_json::json!({"scope":{"targetKey":vector["opening"]["targetKey"]},"rolloutSetCommitment":vector["expectedOpeningCommitment"]});
        let target = serde_json::to_vec(&target).unwrap();
        assert!(assert_rollout_opening_bytes(&target, &hash, &opening).is_ok());
        assert_eq!(
            assert_rollout_opening_bytes(&target, &hash, b"null"),
            Err(pinvou_upgrade_codec::OpeningBindingError::Mismatch)
        );
        let wrong = serde_json::to_vec(&serde_json::json!("0".repeat(64))).unwrap();
        assert_eq!(
            assert_rollout_opening_bytes(&target, &wrong, &opening),
            Err(pinvou_upgrade_codec::OpeningBindingError::Mismatch)
        );
    }

    #[test]
    fn feature_facade_matches_complete_metadata_reference_to_original_wire_bytes() {
        let bytes = br#"{"signed":{"role":"root","scope":{},"version":1,"expiresAt":0}}"#;
        let declared = pinvou_upgrade_codec::derive_metadata_reference(bytes).unwrap();
        assert!(match_metadata_reference_bytes(declared.canonical_bytes(), bytes).is_ok());
        let mut changed = bytes.to_vec();
        changed.push(b' ');
        assert_eq!(
            match_metadata_reference_bytes(declared.canonical_bytes(), &changed),
            Err(pinvou_upgrade_codec::MetadataReferenceError::Mismatch)
        );
        let mut reference: serde_json::Value =
            serde_json::from_slice(declared.canonical_bytes()).unwrap();
        reference["version"] = serde_json::json!(2);
        assert_eq!(
            match_metadata_reference_bytes(&serde_json::to_vec(&reference).unwrap(), bytes),
            Err(pinvou_upgrade_codec::MetadataReferenceError::Mismatch)
        );
    }

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

    #[tokio::test]
    async fn feature_facade_rejects_bad_control_requests_before_network() {
        assert!(matches!(
            send_client_request_once(
                b"{}",
                pinvou_upgrade_codec::ClientRequestOperation::ConsumeStatus
            )
            .await,
            Err(pinvou_upgrade_control_http::ControlHttpError::RequestContract(_))
        ));
        assert!(matches!(
            send_client_request_once(
                br#"{"protocolVersion":1,"protocolVersion":1}"#,
                pinvou_upgrade_codec::ClientRequestOperation::ConsumeStatus,
            )
            .await,
            Err(pinvou_upgrade_control_http::ControlHttpError::RequestJson(
                pinvou_upgrade_codec::Error::DuplicateKey
            ))
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
