//! Cache-only embedded PE provider. All self-referential SDK objects have stable
//! Box addresses; native state is CLOSED before those allocations or File die.
use super::provider;
use crate::{ChainEvidence, Error, SignatureEvidence, Verification, cancelled};
use std::{fs::File, mem::size_of, os::windows::io::AsRawHandle, sync::atomic::AtomicBool};
use windows_sys::Win32::Security::WinTrust::*;

const MAX_SIGNATURES: u32 = 16;
pub(super) struct State<'a> {
    data: Box<WINTRUST_DATA>,
    _file_info: Box<WINTRUST_FILE_INFO>,
    settings: Box<WINTRUST_SIGNATURE_SETTINGS>,
    _path: Vec<u16>,
    _file: &'a File,
    action: Box<windows_sys::core::GUID>,
    started: bool,
    status: i32,
}
impl<'a> State<'a> {
    fn new(file: &'a File, path: Vec<u16>, index: u32, count: bool) -> Self {
        let mut file_info = Box::new(WINTRUST_FILE_INFO {
            cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: if path.is_empty() {
                std::ptr::null()
            } else {
                path.as_ptr()
            },
            hFile: file.as_raw_handle(),
            pgKnownSubject: std::ptr::null_mut(),
        });
        let mut settings = Box::new(WINTRUST_SIGNATURE_SETTINGS {
            cbStruct: size_of::<WINTRUST_SIGNATURE_SETTINGS>() as u32,
            dwIndex: index,
            dwFlags: if count {
                WSS_GET_SECONDARY_SIG_COUNT
            } else {
                WSS_VERIFY_SPECIFIC
            },
            ..Default::default()
        });
        let data = Box::new(WINTRUST_DATA {
            cbStruct: size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
            dwUnionChoice: WTD_CHOICE_FILE,
            Anonymous: WINTRUST_DATA_0 {
                pFile: &mut *file_info,
            },
            dwStateAction: WTD_STATEACTION_VERIFY,
            dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL
                | WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT
                | WTD_DISABLE_MD2_MD4,
            pSignatureSettings: &mut *settings,
            ..Default::default()
        });
        Self {
            data,
            _file_info: file_info,
            settings,
            _path: path,
            _file: file,
            action: Box::new(WINTRUST_ACTION_GENERIC_VERIFY_V2),
            started: false,
            status: 0,
        }
    }
    #[allow(unsafe_code)]
    fn verify(&mut self) {
        self.started = true;
        // SAFETY: exact SDK structures with stable Box pointees. The file stays
        // borrowed; optional path buffer is owned, UTF-16 terminated. Production
        // supplies NULL path and only the already-locked actual hFile.
        self.status = unsafe {
            WinVerifyTrust(
                (-1isize) as *mut _,
                &mut *self.action,
                (&mut *self.data as *mut WINTRUST_DATA).cast(),
            )
        };
    }
    #[allow(unsafe_code)]
    fn evidence(&self, index: u32) -> Result<SignatureEvidence, Error> {
        // A cryptographic refusal need not contain a constructed certificate
        // chain. Preserve the native failure instead of demanding success-only
        // evidence from (for example) TRUST_E_BAD_DIGEST or NOSIGNATURE.
        if self.status != 0 {
            return Ok(SignatureEvidence {
                index,
                native_status: format!("{:08x}", self.status as u32),
                verification: match self.status as u32 {
                    0x800b0100 => Verification::Unsigned,
                    0x80096010 | 0x80096004 => Verification::InvalidSignature,
                    0x800b010c | 0x800b0111 => Verification::Refused,
                    _ => Verification::Unknown,
                },
                provider_error: None,
                provider_final_error: None,
                trust_step_errors: Vec::new(),
                signer: None,
                timestamps: Vec::new(),
            });
        }
        let mut signer = None;
        let mut timestamps: Vec<ChainEvidence> = Vec::new();
        let mut evidence_complete = false;
        let mut provider_error = None;
        let mut provider_final_error = None;
        let mut trust_step_errors = Vec::new();
        if !self.data.hWVTStateData.is_null() {
            // SAFETY: helper pointers are Microsoft-owned and live until CLOSE.
            let data = unsafe { WTHelperProvDataFromStateData(self.data.hWVTStateData) };
            if !data.is_null() {
                let native = unsafe { provider::sized(data) }?;
                if native.csSigners > 1 {
                    return Err(Error::EvidenceUnavailable);
                }
                provider_error = Some(native.dwError);
                provider_final_error = Some(native.dwFinalError);
                if native.cdwTrustStepErrors > 128 {
                    return Err(Error::EvidenceUnavailable);
                }
                if native.cdwTrustStepErrors > 0 && !native.padwTrustStepErrors.is_null() {
                    trust_step_errors = unsafe {
                        std::slice::from_raw_parts(
                            native.padwTrustStepErrors,
                            native.cdwTrustStepErrors as usize,
                        )
                    }
                    .to_vec();
                }
                if native.csSigners == 1 {
                    let pointer = unsafe { WTHelperGetProvSignerFromChain(data, 0, 0, 0) };
                    if !pointer.is_null() {
                        let original = unsafe { provider::sized(pointer) }?;
                        if original.csCounterSigners > 16 {
                            return Err(Error::EvidenceUnavailable);
                        }
                        signer = Some(unsafe { provider::chain(pointer) }?);
                        for counter in 0..original.csCounterSigners {
                            let pointer =
                                unsafe { WTHelperGetProvSignerFromChain(data, 0, 1, counter) };
                            if unsafe { provider::sized(pointer) }?.csCounterSigners != 0 {
                                return Err(Error::EvidenceUnavailable);
                            }
                            timestamps.push(unsafe { provider::chain(pointer) }?);
                        }
                        evidence_complete = native.dwError == 0
                            && native.dwFinalError == 0
                            && !trust_step_errors.is_empty()
                            && trust_step_errors.iter().all(|e| *e == 0)
                            && signer
                                .as_ref()
                                .is_some_and(|s| s.signer_type == 0 && provider::complete(s))
                            && timestamps.iter().all(|s| {
                                s.signer_type == SGNR_TYPE_TIMESTAMP && provider::complete(s)
                            });
                    }
                }
            }
        }
        let verification = match self.status as u32 {
            0 if evidence_complete => Verification::Verified,
            0x800b0100 => Verification::Unsigned,
            0x80096010 | 0x80096004 => Verification::InvalidSignature,
            0x800b010c | 0x800b0111 => Verification::Refused,
            _ => Verification::Unknown,
        };
        Ok(SignatureEvidence {
            index,
            native_status: format!("{:08x}", self.status as u32),
            verification,
            provider_error,
            provider_final_error,
            trust_step_errors,
            signer,
            timestamps,
        })
    }
    #[allow(unsafe_code)]
    fn close(&mut self) -> Result<(), Error> {
        self.data.dwStateAction = WTD_STATEACTION_CLOSE;
        // SAFETY: same stable allocations and live file as VERIFY. A failed
        // explicit CLOSE is not reported as successful evidence; Drop retries.
        let status = unsafe {
            WinVerifyTrust(
                (-1isize) as *mut _,
                &mut *self.action,
                (&mut *self.data as *mut WINTRUST_DATA).cast(),
            )
        };
        if status != 0 {
            return Err(Error::EvidenceUnavailable);
        }
        self.started = false;
        Ok(())
    }
}
impl Drop for State<'_> {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        if self.started {
            self.data.dwStateAction = WTD_STATEACTION_CLOSE;
            // SAFETY: same original data/action/file-info and stable buffers are
            // all still live; CLOSE always precedes their Rust field drops.
            unsafe {
                WinVerifyTrust(
                    (-1isize) as *mut _,
                    &mut *self.action,
                    (&mut *self.data as *mut WINTRUST_DATA).cast(),
                )
            };
        }
    }
}
fn inspect_with_path(
    file: &File,
    cancel: &AtomicBool,
    path: Vec<u16>,
    mut after_verify: impl FnMut(),
) -> Result<Vec<SignatureEvidence>, Error> {
    cancelled(cancel)?;
    let mut initial = State::new(file, path.clone(), 0, true);
    initial.verify();
    after_verify();
    cancelled(cancel)?;
    if initial.settings.cSecondarySigs >= MAX_SIGNATURES {
        return Err(Error::EvidenceUnavailable);
    }
    let count = initial.settings.cSecondarySigs;
    initial.close()?;
    drop(initial);
    let mut evidence = Vec::with_capacity(count as usize + 1);
    // GET_SECONDARY_SIG_COUNT can auto-select any signature (and has no valid
    // verified-index on failure). It is enumeration only. Verify EVERY index
    // explicitly, including primary 0, rather than attributing default evidence
    // to the wrong signature on a dual-signed image.
    for index in 0..=count {
        cancelled(cancel)?;
        let mut state = State::new(file, path.clone(), index, false);
        state.verify();
        after_verify();
        cancelled(cancel)?;
        if state.status == 0 && state.settings.dwVerifiedSigIndex != index {
            return Err(Error::EvidenceUnavailable);
        }
        evidence.push(state.evidence(index)?);
        state.close()?;
    }
    Ok(evidence)
}
pub(super) fn inspect(file: &File, cancel: &AtomicBool) -> Result<Vec<SignatureEvidence>, Error> {
    inspect_with_path(file, cancel, Vec::new(), || {})
}

#[cfg(test)]
pub(super) fn supplied_path_for_test(
    file: &File,
    path: &std::path::Path,
) -> Result<Vec<SignatureEvidence>, Error> {
    use std::os::windows::ffi::OsStrExt;
    let mut path: Vec<u16> = path.as_os_str().encode_wide().collect();
    path.push(0);
    inspect_with_path(file, &AtomicBool::new(false), path, || {})
}

#[cfg(test)]
pub(super) fn cancel_after_actual_native_call(
    file: &File,
    cancel: &AtomicBool,
) -> Result<Vec<SignatureEvidence>, Error> {
    // The hook changes only cancellation AFTER genuine native completion. It
    // cannot replace a signature, provider result, file, clock or authority.
    inspect_with_path(file, cancel, Vec::new(), || {
        cancel.store(true, std::sync::atomic::Ordering::Release)
    })
}
