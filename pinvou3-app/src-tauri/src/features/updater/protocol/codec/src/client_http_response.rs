//! Fixed supplied client HTTP response entry; no transport or current authority.
use crate::{
    ClientErrorResponseError, ClientResponseError, ClientResponseOperation, Document, Error,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientHttpResponseError {
    Json(Error),
    Operation(ClientResponseError),
    ErrorResponse(ClientErrorResponseError),
    InvalidStatus,
}
impl std::fmt::Display for ClientHttpResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => error.fmt(f),
            Self::Operation(error) => error.fmt(f),
            Self::ErrorResponse(error) => error.fmt(f),
            Self::InvalidStatus => f.write_str("API_CONTRACT_INVALID"),
        }
    }
}
impl std::error::Error for ClientHttpResponseError {}
impl Document {
    /// Complete frozen supplied client HTTP response contract. Only declared
    /// 200 operation responses and 4xx/5xx error responses can pass; each branch
    /// delegates the existing full contract. Unit success is structural even
    /// for a declared error, never success/ACK/absence/current rights.
    pub fn assert_supplied_client_http_response_contract(
        &self,
        operation: ClientResponseOperation,
        http_status: u16,
    ) -> Result<(), ClientHttpResponseError> {
        match http_status {
            200 => self
                .assert_supplied_client_response_contract(operation)
                .map_err(ClientHttpResponseError::Operation),
            400..=599 => self
                .assert_supplied_client_error_response_contract(operation, http_status)
                .map_err(ClientHttpResponseError::ErrorResponse),
            _ => Err(ClientHttpResponseError::InvalidStatus),
        }
    }
}
