//! Fixed control-origin transport and actual Windows entropy. Received bytes
//! remain DATA; only the dedicated time verifier and native owner can use them.
use crate::error::{Error, Result};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use pinvou_upgrade_control_http::{Operation, RootGetResult, get_root_once, post_once};
use std::ptr;
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
};

/// No endpoint, headers, proxy, retry, supplied entropy or wall-clock options.
/// Dropping an HTTP future cancels that attempt; it does not assert that the
/// server did not commit it. The owner retains its original nonce/send bracket.
pub(super) struct TimeTransport;

impl TimeTransport {
    pub(super) fn new() -> Self {
        Self
    }

    /// Exactly one real BCrypt call, encoded as the wire's canonical 32-byte
    /// base64url nonce. The older IPC hex nonce is deliberately not reused.
    #[allow(unsafe_code)]
    pub(super) fn fresh_nonce(&self) -> Result<String> {
        let mut bytes = [0u8; 32];
        let status = unsafe {
            BCryptGenRandom(
                ptr::null_mut(),
                bytes.as_mut_ptr(),
                bytes.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status < 0 {
            return Err(Error::Unavailable);
        }
        Ok(encode_nonce(bytes))
    }

    /// Original Root distribution DATA, including a nonauthoritative Missing.
    /// No latest selector or new trust anchor is inferred by this adapter.
    pub(super) async fn get_root(&self, product: &str, version: u64) -> Result<RootGetResult> {
        get_root_once(product, version)
            .await
            .map_err(|_| Error::SourceUnavailable)
    }

    /// Actual fixed POST /v1/upgrade/time with the existing closed request,
    /// 64-KiB canonical response contract and original TLS/idle/connect limits.
    /// No cumulative download deadline is added. Rejection is never a clock.
    pub(super) async fn post_time(&self, canonical_request: &[u8]) -> Result<Vec<u8>> {
        let response = post_once(Operation::Time, canonical_request)
            .await
            .map_err(|_| Error::SourceUnavailable)?;
        if response.status() != 200 {
            return Err(Error::SourceUnavailable);
        }
        Ok(response.bytes().to_vec())
    }
}

fn encode_nonce(bytes: [u8; 32]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonce_encoding_preserves_all_32_bytes_with_canonical_tail() {
        let mut original = [0u8; 32];
        for (index, byte) in original.iter_mut().enumerate() {
            *byte = index as u8;
        }
        let encoded = encode_nonce(original);
        assert_eq!(encoded, "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8");
        assert_eq!(encoded.len(), 43);
        assert_eq!(URL_SAFE_NO_PAD.decode(&encoded).unwrap(), original);
        assert_eq!(
            URL_SAFE_NO_PAD.encode(URL_SAFE_NO_PAD.decode(&encoded).unwrap()),
            encoded
        );
        assert!(
            encoded
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
    }

    #[test]
    fn windows_entropy_nonce_has_the_actual_wire_length_and_round_trip() {
        // A real raw Windows RNG test, not an installed-SCM/clock claim.
        let transport = TimeTransport::new();
        let first = transport.fresh_nonce().unwrap();
        let second = transport.fresh_nonce().unwrap();
        assert_ne!(first, second);
        for nonce in [first, second] {
            assert_eq!(nonce.len(), 43);
            let raw = URL_SAFE_NO_PAD.decode(&nonce).unwrap();
            assert_eq!(raw.len(), 32);
            assert_eq!(URL_SAFE_NO_PAD.encode(raw), nonce);
        }
    }
}
