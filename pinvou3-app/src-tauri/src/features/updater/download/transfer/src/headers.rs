use crate::{MAX_HEADER_BYTES, MAX_HEADER_FIELDS, MAX_HEADER_VALUE_BYTES, TransferError, trim_ows};

/// Raw RFC 9110 strong validator. Backslashes are opaque bytes, not escapes.
/// It authenticates no entity; source continuity must be proven independently.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct StrongEtag<'a>(&'a [u8]);
impl<'a> StrongEtag<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, TransferError> {
        if bytes.len() > MAX_HEADER_VALUE_BYTES {
            return Err(TransferError::HeadersAboveLimit);
        }
        let bytes = trim_ows(bytes);
        if bytes.len() < 2
            || bytes[0] != b'"'
            || bytes[bytes.len() - 1] != b'"'
            || !bytes[1..bytes.len() - 1]
                .iter()
                .all(|byte| *byte == 0x21 || (0x23..=0x7e).contains(byte) || *byte >= 0x80)
        {
            return Err(TransferError::InvalidEtag);
        }
        Ok(Self(bytes))
    }
    pub fn as_bytes(self) -> &'a [u8] {
        self.0
    }
}

/// Accept original field occurrences, before folding duplicates. The future
/// HTTP adapter bounds parsing before allocation and validates transfer framing;
/// this local profile also bounds borrowed fields before interpreting them.
#[derive(Clone, Copy)]
pub struct ResponseHeaders<'a> {
    pub(crate) etag: Option<&'a [u8]>,
    pub(crate) content_range: Option<&'a [u8]>,
    pub(crate) content_length: Option<&'a [u8]>,
    pub(crate) content_encoding: Option<&'a [u8]>,
}
impl<'a> ResponseHeaders<'a> {
    pub fn from_fields(
        fields: impl IntoIterator<Item = (&'a str, &'a [u8])>,
    ) -> Result<Self, TransferError> {
        let mut headers = Self {
            etag: None,
            content_range: None,
            content_length: None,
            content_encoding: None,
        };
        let mut total = 0_usize;
        for (index, (name, value)) in fields.into_iter().enumerate() {
            if index >= MAX_HEADER_FIELDS
                || name.len() > MAX_HEADER_VALUE_BYTES
                || value.len() > MAX_HEADER_VALUE_BYTES
            {
                return Err(TransferError::HeadersAboveLimit);
            }
            total = total
                .checked_add(name.len())
                .and_then(|total| total.checked_add(value.len()))
                .filter(|total| *total <= MAX_HEADER_BYTES)
                .ok_or(TransferError::HeadersAboveLimit)?;
            let destination = if name.eq_ignore_ascii_case("etag") {
                Some(&mut headers.etag)
            } else if name.eq_ignore_ascii_case("content-range") {
                Some(&mut headers.content_range)
            } else if name.eq_ignore_ascii_case("content-length") {
                Some(&mut headers.content_length)
            } else if name.eq_ignore_ascii_case("content-encoding") {
                Some(&mut headers.content_encoding)
            } else {
                None
            };
            if let Some(destination) = destination {
                if destination.is_some() {
                    return Err(TransferError::DuplicateHeader);
                }
                *destination = Some(trim_ows(value));
            }
        }
        Ok(headers)
    }
}
