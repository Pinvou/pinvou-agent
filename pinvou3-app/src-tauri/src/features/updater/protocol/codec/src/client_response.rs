//! Fixed client response routing only; no transport, current state or execution rights.
use crate::{
    ChannelChangeResponseError, CheckResponseError, CheckResponseOperation, Document,
    DownloadInfoContractError, Error, HelperCleanupResponseError, PurposeResponseError,
    PurposeResponseOperation, ReceiptResponseError, ReceiptResponseOperation,
    TransactionControlError, TransactionControlOperation,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientResponseOperation {
    Check,
    Refresh,
    DownloadInfo,
    ValidatePreinstall,
    ConsumePreinstall,
    CancelPreinstall,
    ExecutionReviewPreinstall,
    ValidateInstall,
    ConsumeInstall,
    CancelInstall,
    ExecutionReviewInstall,
    ValidateActivate,
    ConsumeActivate,
    CancelActivate,
    ExecutionReviewActivate,
    ConsumeStatus,
    HelperCleanup,
    ChannelChange,
    TelemetryEvents,
    PreinstallEvents,
    InstallEvents,
    ActivateEvents,
    ReconciliationEvents,
    RecoveryReviewEvents,
    ReconciliationAction,
    RecoveryReviewAction,
}
impl ClientResponseOperation {
    pub const ALL: [Self; 26] = [
        Self::Check,
        Self::Refresh,
        Self::DownloadInfo,
        Self::ValidatePreinstall,
        Self::ConsumePreinstall,
        Self::CancelPreinstall,
        Self::ExecutionReviewPreinstall,
        Self::ValidateInstall,
        Self::ConsumeInstall,
        Self::CancelInstall,
        Self::ExecutionReviewInstall,
        Self::ValidateActivate,
        Self::ConsumeActivate,
        Self::CancelActivate,
        Self::ExecutionReviewActivate,
        Self::ConsumeStatus,
        Self::HelperCleanup,
        Self::ChannelChange,
        Self::TelemetryEvents,
        Self::PreinstallEvents,
        Self::InstallEvents,
        Self::ActivateEvents,
        Self::ReconciliationEvents,
        Self::RecoveryReviewEvents,
        Self::ReconciliationAction,
        Self::RecoveryReviewAction,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Refresh => "refresh",
            Self::DownloadInfo => "download-info",
            Self::ValidatePreinstall => "validate-preinstall",
            Self::ConsumePreinstall => "consume-preinstall",
            Self::CancelPreinstall => "cancel-preinstall",
            Self::ExecutionReviewPreinstall => "execution-review-preinstall",
            Self::ValidateInstall => "validate-install",
            Self::ConsumeInstall => "consume-install",
            Self::CancelInstall => "cancel-install",
            Self::ExecutionReviewInstall => "execution-review-install",
            Self::ValidateActivate => "validate-activate",
            Self::ConsumeActivate => "consume-activate",
            Self::CancelActivate => "cancel-activate",
            Self::ExecutionReviewActivate => "execution-review-activate",
            Self::ConsumeStatus => "consume-status",
            Self::HelperCleanup => "helper-cleanup",
            Self::ChannelChange => "channel-change",
            Self::TelemetryEvents => "events-telemetry-event",
            Self::PreinstallEvents => "events-preinstall-transaction-event",
            Self::InstallEvents => "events-install-transaction-event",
            Self::ActivateEvents => "events-activate-transaction-event",
            Self::ReconciliationEvents => "events-reconciliation-event",
            Self::RecoveryReviewEvents => "events-recovery-review-event",
            Self::ReconciliationAction => "reconciliation-action",
            Self::RecoveryReviewAction => "recovery-review-action",
        }
    }
}
impl std::str::FromStr for ClientResponseOperation {
    type Err = ClientResponseError;
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "check" => Ok(Self::Check),
            "refresh" => Ok(Self::Refresh),
            "download-info" => Ok(Self::DownloadInfo),
            "validate-preinstall" => Ok(Self::ValidatePreinstall),
            "consume-preinstall" => Ok(Self::ConsumePreinstall),
            "cancel-preinstall" => Ok(Self::CancelPreinstall),
            "execution-review-preinstall" => Ok(Self::ExecutionReviewPreinstall),
            "validate-install" => Ok(Self::ValidateInstall),
            "consume-install" => Ok(Self::ConsumeInstall),
            "cancel-install" => Ok(Self::CancelInstall),
            "execution-review-install" => Ok(Self::ExecutionReviewInstall),
            "validate-activate" => Ok(Self::ValidateActivate),
            "consume-activate" => Ok(Self::ConsumeActivate),
            "cancel-activate" => Ok(Self::CancelActivate),
            "execution-review-activate" => Ok(Self::ExecutionReviewActivate),
            "consume-status" => Ok(Self::ConsumeStatus),
            "helper-cleanup" => Ok(Self::HelperCleanup),
            "channel-change" => Ok(Self::ChannelChange),
            "events-telemetry-event" => Ok(Self::TelemetryEvents),
            "events-preinstall-transaction-event" => Ok(Self::PreinstallEvents),
            "events-install-transaction-event" => Ok(Self::InstallEvents),
            "events-activate-transaction-event" => Ok(Self::ActivateEvents),
            "events-reconciliation-event" => Ok(Self::ReconciliationEvents),
            "events-recovery-review-event" => Ok(Self::RecoveryReviewEvents),
            "reconciliation-action" => Ok(Self::ReconciliationAction),
            "recovery-review-action" => Ok(Self::RecoveryReviewAction),
            _ => Err(ClientResponseError::UnknownOperation),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientResponseError {
    UnknownOperation,
    Json(Error),
    Check(CheckResponseError),
    Download(DownloadInfoContractError),
    Purpose(PurposeResponseError),
    Transaction(TransactionControlError),
    Receipt(ReceiptResponseError),
    HelperCleanup(HelperCleanupResponseError),
    ChannelChange(ChannelChangeResponseError),
}
impl std::fmt::Display for ClientResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownOperation => f.write_str("API_OPERATION_UNKNOWN"),
            Self::Json(e) => e.fmt(f),
            Self::Check(e) => e.fmt(f),
            Self::Download(e) => e.fmt(f),
            Self::Purpose(e) => e.fmt(f),
            Self::Transaction(e) => e.fmt(f),
            Self::Receipt(e) => e.fmt(f),
            Self::HelperCleanup(e) => e.fmt(f),
            Self::ChannelChange(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for ClientResponseError {}
impl Document {
    /// Delegates every fixed non-management response to its existing complete
    /// supplied contract. It adds no acceptance/authority/current-time rules.
    /// Caller operation must correlate with the real outstanding request in a
    /// future transport/owner layer; unit success is never trusted ACK or rights.
    pub fn assert_supplied_client_response_contract(
        &self,
        operation: ClientResponseOperation,
    ) -> Result<(), ClientResponseError> {
        match operation {
            ClientResponseOperation::Check => self
                .assert_supplied_check_response_contract(CheckResponseOperation::Check)
                .map_err(ClientResponseError::Check),
            ClientResponseOperation::Refresh => self
                .assert_supplied_check_response_contract(CheckResponseOperation::Refresh)
                .map_err(ClientResponseError::Check),
            ClientResponseOperation::DownloadInfo => self
                .assert_supplied_download_info_contract()
                .map_err(ClientResponseError::Download),
            ClientResponseOperation::ValidatePreinstall => self
                .assert_supplied_purpose_response_contract(
                    PurposeResponseOperation::ValidatePreinstall,
                )
                .map_err(ClientResponseError::Purpose),
            ClientResponseOperation::ConsumePreinstall => self
                .assert_supplied_purpose_response_contract(
                    PurposeResponseOperation::ConsumePreinstall,
                )
                .map_err(ClientResponseError::Purpose),
            ClientResponseOperation::CancelPreinstall => self
                .assert_supplied_transaction_control_contract(
                    TransactionControlOperation::CancelPreinstall,
                )
                .map_err(ClientResponseError::Transaction),
            ClientResponseOperation::ExecutionReviewPreinstall => self
                .assert_supplied_transaction_control_contract(
                    TransactionControlOperation::ReviewPreinstall,
                )
                .map_err(ClientResponseError::Transaction),
            ClientResponseOperation::ValidateInstall => self
                .assert_supplied_purpose_response_contract(
                    PurposeResponseOperation::ValidateInstall,
                )
                .map_err(ClientResponseError::Purpose),
            ClientResponseOperation::ConsumeInstall => self
                .assert_supplied_purpose_response_contract(PurposeResponseOperation::ConsumeInstall)
                .map_err(ClientResponseError::Purpose),
            ClientResponseOperation::CancelInstall => self
                .assert_supplied_transaction_control_contract(
                    TransactionControlOperation::CancelInstall,
                )
                .map_err(ClientResponseError::Transaction),
            ClientResponseOperation::ExecutionReviewInstall => self
                .assert_supplied_transaction_control_contract(
                    TransactionControlOperation::ReviewInstall,
                )
                .map_err(ClientResponseError::Transaction),
            ClientResponseOperation::ValidateActivate => self
                .assert_supplied_purpose_response_contract(
                    PurposeResponseOperation::ValidateActivate,
                )
                .map_err(ClientResponseError::Purpose),
            ClientResponseOperation::ConsumeActivate => self
                .assert_supplied_purpose_response_contract(
                    PurposeResponseOperation::ConsumeActivate,
                )
                .map_err(ClientResponseError::Purpose),
            ClientResponseOperation::CancelActivate => self
                .assert_supplied_transaction_control_contract(
                    TransactionControlOperation::CancelActivate,
                )
                .map_err(ClientResponseError::Transaction),
            ClientResponseOperation::ExecutionReviewActivate => self
                .assert_supplied_transaction_control_contract(
                    TransactionControlOperation::ReviewActivate,
                )
                .map_err(ClientResponseError::Transaction),
            ClientResponseOperation::ConsumeStatus => self
                .assert_supplied_transaction_control_contract(
                    TransactionControlOperation::ConsumeStatus,
                )
                .map_err(ClientResponseError::Transaction),
            ClientResponseOperation::HelperCleanup => self
                .assert_supplied_helper_cleanup_response_contract()
                .map_err(ClientResponseError::HelperCleanup),
            ClientResponseOperation::ChannelChange => self
                .assert_supplied_channel_change_response_contract()
                .map_err(ClientResponseError::ChannelChange),
            ClientResponseOperation::TelemetryEvents => self
                .assert_supplied_receipt_response_contract(
                    ReceiptResponseOperation::TelemetryEvents,
                )
                .map_err(ClientResponseError::Receipt),
            ClientResponseOperation::PreinstallEvents => self
                .assert_supplied_receipt_response_contract(
                    ReceiptResponseOperation::PreinstallEvents,
                )
                .map_err(ClientResponseError::Receipt),
            ClientResponseOperation::InstallEvents => self
                .assert_supplied_receipt_response_contract(ReceiptResponseOperation::InstallEvents)
                .map_err(ClientResponseError::Receipt),
            ClientResponseOperation::ActivateEvents => self
                .assert_supplied_receipt_response_contract(ReceiptResponseOperation::ActivateEvents)
                .map_err(ClientResponseError::Receipt),
            ClientResponseOperation::ReconciliationEvents => self
                .assert_supplied_receipt_response_contract(
                    ReceiptResponseOperation::ReconciliationEvents,
                )
                .map_err(ClientResponseError::Receipt),
            ClientResponseOperation::RecoveryReviewEvents => self
                .assert_supplied_receipt_response_contract(
                    ReceiptResponseOperation::RecoveryReviewEvents,
                )
                .map_err(ClientResponseError::Receipt),
            ClientResponseOperation::ReconciliationAction => self
                .assert_supplied_receipt_response_contract(
                    ReceiptResponseOperation::ReconciliationAction,
                )
                .map_err(ClientResponseError::Receipt),
            ClientResponseOperation::RecoveryReviewAction => self
                .assert_supplied_receipt_response_contract(
                    ReceiptResponseOperation::RecoveryReviewAction,
                )
                .map_err(ClientResponseError::Receipt),
        }
    }
}
