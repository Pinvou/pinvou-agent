//! Copies bounded public certificate/chain observations while WinTrust state is
//! live. Raw provider pointers never escape or enter the report/diagnostics.
use crate::{CertificateEvidence, ChainEvidence, Error, hash};
use std::mem::size_of;
use windows_sys::Win32::{
    Foundation::FILETIME,
    Security::{
        Cryptography::{CERT_CHAIN_CONTEXT, CERT_CHAIN_ELEMENT, CERT_REVOCATION_INFO},
        WinTrust::{CRYPT_PROVIDER_CERT, CRYPT_PROVIDER_SGNR, WTHelperGetProvCertFromChain},
    },
};

pub(super) fn file_time(value: FILETIME) -> String {
    (((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64).to_string()
}

// SAFETY contract for these helpers: callers pass only pointers returned by the
// Microsoft provider for the still-live WinTrust state. Provider memory belongs
// to that state and is never accepted from file bytes or a caller's raw pointer.
// Only SDK structures whose FIRST field is cbStruct/cbSize use this helper. Read
// that u32 before constructing a full Rust reference, so an older short native
// structure is rejected without first borrowing beyond its advertised extent.
#[allow(unsafe_code)]
pub(super) unsafe fn sized<'a, T>(pointer: *const T) -> Result<&'a T, Error> {
    if pointer.is_null() || !pointer.is_aligned() {
        return Err(Error::EvidenceUnavailable);
    }
    let advertised = unsafe { pointer.cast::<u32>().read_unaligned() };
    if advertised < size_of::<T>() as u32 {
        return Err(Error::EvidenceUnavailable);
    }
    // SAFETY: provider contract above, checked minimum size/alignment, and the
    // caller retains native state for the entire lifetime of this private ref.
    Ok(unsafe { &*pointer })
}
#[allow(unsafe_code)]
unsafe fn certificate(pointer: *const CRYPT_PROVIDER_CERT) -> Result<CertificateEvidence, Error> {
    let value = unsafe { sized(pointer) }?;
    if value.pCert.is_null() {
        return Err(Error::EvidenceUnavailable);
    }
    // SAFETY: provider-owned CERT_CONTEXT and child buffers, alive until CLOSE.
    let cert = unsafe { &*value.pCert };
    if cert.pCertInfo.is_null()
        || cert.pbCertEncoded.is_null()
        || cert.cbCertEncoded == 0
        || cert.cbCertEncoded > 1_048_576
    {
        return Err(Error::EvidenceUnavailable);
    }
    let info = unsafe { &*cert.pCertInfo };
    let bytes =
        unsafe { std::slice::from_raw_parts(cert.pbCertEncoded, cert.cbCertEncoded as usize) };
    let element = if value.pChainElement.is_null() {
        None
    } else {
        let e = unsafe { sized::<CERT_CHAIN_ELEMENT>(value.pChainElement) }?;
        Some(e)
    };
    let revocation_error = if let Some(e) = element {
        if e.pRevocationInfo.is_null() {
            None
        } else {
            let r = unsafe { sized::<CERT_REVOCATION_INFO>(e.pRevocationInfo) }?;
            Some(r.dwRevocationResult)
        }
    } else {
        None
    };
    Ok(CertificateEvidence {
        certificate_sha256: hash(bytes),
        encoded_bytes: cert.cbCertEncoded,
        not_before_file_time: file_time(info.NotBefore),
        not_after_file_time: file_time(info.NotAfter),
        provider_error: value.dwError,
        chain_error_status: element.map(|e| e.TrustStatus.dwErrorStatus),
        chain_info_status: element.map(|e| e.TrustStatus.dwInfoStatus),
        revocation_error,
        trusted_root: value.fTrustedRoot != 0,
    })
}
#[allow(unsafe_code)]
pub(super) unsafe fn chain(pointer: *const CRYPT_PROVIDER_SGNR) -> Result<ChainEvidence, Error> {
    let signer = unsafe { sized(pointer) }?;
    if signer.csCertChain == 0 || signer.csCertChain > 32 || signer.pasCertChain.is_null() {
        return Err(Error::EvidenceUnavailable);
    }
    let mut certificates = Vec::with_capacity(signer.csCertChain as usize);
    for i in 0..signer.csCertChain {
        // SAFETY: bounded actual signer/index, while its native state is live.
        // Ask the provider for each element rather than guessing array stride.
        let cert = unsafe { WTHelperGetProvCertFromChain(pointer.cast_mut(), i) };
        certificates.push(unsafe { certificate(cert) }?);
    }
    let context = if signer.pChainContext.is_null() {
        None
    } else {
        let value = unsafe { sized::<CERT_CHAIN_CONTEXT>(signer.pChainContext) }?;
        if value.cChain == 0 || value.cChain > 32 || value.rgpChain.is_null() {
            return Err(Error::EvidenceUnavailable);
        }
        Some(value)
    };
    Ok(ChainEvidence {
        signer_type: signer.dwSignerType,
        provider_error: signer.dwError,
        verification_as_of_file_time: file_time(signer.sftVerifyAsOf),
        chain_error_status: context.map(|c| c.TrustStatus.dwErrorStatus),
        chain_info_status: context.map(|c| c.TrustStatus.dwInfoStatus),
        certificates,
    })
}
pub(super) fn complete(value: &ChainEvidence) -> bool {
    value.provider_error == 0
        && value.chain_error_status == Some(0)
        && !value.certificates.is_empty()
        && value.certificates.last().is_some_and(|c| c.trusted_root)
        && value.certificates.iter().enumerate().all(|(i, c)| {
            c.provider_error == 0
                && c.chain_error_status == Some(0)
                && if i + 1 == value.certificates.len() {
                    c.trusted_root
                } else {
                    !c.trusted_root && c.revocation_error == Some(0)
                }
        })
}
