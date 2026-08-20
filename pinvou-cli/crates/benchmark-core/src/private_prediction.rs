use std::fmt;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::security::{validate_component, validate_safe_text};
use crate::{BenchmarkError, PredictionHandle, Result, TaskOutcome};

const MAGIC: &[u8; 4] = b"PVP2";
const SCHEMA_VERSION: u8 = 2;
const INTEGRITY_BYTES: usize = 32;
const MAX_PRIVATE_PREDICTION_BYTES: usize = 1024 * 1024;
const MAX_ENVELOPE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrivatePredictionContentType {
    Utf8TextV1,
    CanonicalJsonV1,
}

impl PrivatePredictionContentType {
    pub(crate) fn type_tag(self) -> &'static str {
        match self {
            Self::Utf8TextV1 => "utf8-text/v1",
            Self::CanonicalJsonV1 => "canonical-json/v1",
        }
    }

    fn tag(self) -> u8 {
        match self {
            Self::Utf8TextV1 => 1,
            Self::CanonicalJsonV1 => 2,
        }
    }

    fn from_tag(tag: u8) -> Result<Self> {
        match tag {
            1 => Ok(Self::Utf8TextV1),
            2 => Ok(Self::CanonicalJsonV1),
            _ => Err(corrupt()),
        }
    }
}

pub struct PrivatePredictionPayload {
    content_type: PrivatePredictionContentType,
    bytes: Zeroizing<Vec<u8>>,
}

impl PrivatePredictionPayload {
    pub fn utf8(value: impl Into<String>) -> Result<Self> {
        Self::new(
            PrivatePredictionContentType::Utf8TextV1,
            value.into().into_bytes(),
        )
    }

    pub fn canonical_json(bytes: Vec<u8>) -> Result<Self> {
        Self::new(PrivatePredictionContentType::CanonicalJsonV1, bytes)
    }

    fn new(content_type: PrivatePredictionContentType, bytes: Vec<u8>) -> Result<Self> {
        Self::new_zeroizing(content_type, Zeroizing::new(bytes))
    }

    fn new_zeroizing(
        content_type: PrivatePredictionContentType,
        bytes: Zeroizing<Vec<u8>>,
    ) -> Result<Self> {
        if bytes.len() > MAX_PRIVATE_PREDICTION_BYTES {
            return Err(BenchmarkError::coded("private_prediction_too_large"));
        }
        if content_type == PrivatePredictionContentType::Utf8TextV1
            && std::str::from_utf8(&bytes).is_err()
        {
            return Err(BenchmarkError::coded("private_prediction_invalid_utf8"));
        }
        if content_type == PrivatePredictionContentType::CanonicalJsonV1 {
            let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
            <serde::de::IgnoredAny as serde::Deserialize>::deserialize(&mut deserializer)
                .map_err(|_| BenchmarkError::coded("private_prediction_invalid_json"))?;
            if deserializer.end().is_err() || !compact_json_bytes(&bytes) {
                return Err(BenchmarkError::coded("private_prediction_invalid_json"));
            }
        }
        Ok(Self {
            content_type,
            bytes,
        })
    }

    pub fn content_type(&self) -> PrivatePredictionContentType {
        self.content_type
    }

    pub fn expose_to_scorer(&self) -> &[u8] {
        &self.bytes
    }
}

fn compact_json_bytes(bytes: &[u8]) -> bool {
    let mut quoted = false;
    let mut escaped = false;
    for &byte in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
        } else if byte.is_ascii_whitespace() {
            return false;
        }
    }
    !quoted && !escaped
}

impl fmt::Debug for PrivatePredictionPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PrivatePredictionPayload([redacted])")
    }
}

pub(crate) struct PrivatePredictionStore {
    run_id: String,
    predictions_dir: PathBuf,
}

impl PrivatePredictionStore {
    pub(crate) fn create(run_dir: &Path, run_id: &str) -> Result<Self> {
        validate_component(run_id)?;
        if !run_dir.is_absolute() || !run_dir.is_dir() {
            return Err(BenchmarkError::coded("unsafe_private_store_path"));
        }
        let run_dir = run_dir
            .canonicalize()
            .map_err(|_| BenchmarkError::coded("unsafe_private_store_path"))?;
        if run_dir.file_name().and_then(|value| value.to_str()) != Some(run_id) {
            return Err(BenchmarkError::coded("unsafe_private_store_path"));
        }
        let private_dir = run_dir.join("private");
        ensure_private_directory(&private_dir)?;
        let predictions_dir = private_dir.join("predictions");
        ensure_private_directory(&predictions_dir)?;
        Ok(Self {
            run_id: run_id.to_owned(),
            predictions_dir,
        })
    }

    pub(crate) fn put(
        &self,
        task_id: &str,
        prediction_type: &str,
        payload: PrivatePredictionPayload,
    ) -> Result<PredictionHandle> {
        validate_component(task_id)?;
        validate_safe_text(prediction_type)?;
        validate_private_directory(&self.predictions_dir)?;
        let handle = PredictionHandle::new(random_hex::<32>());
        let binding = binding_bytes(
            &self.run_id,
            task_id,
            prediction_type,
            handle.expose_to_adapter(),
        )?;
        let protected = protect(payload.expose_to_scorer(), &binding)?;
        let integrity = integrity_digest(payload.content_type(), &binding, &protected);
        let envelope = encode_envelope(payload.content_type(), &binding, &integrity, &protected)?;
        self.publish_blob(&handle, &envelope)?;
        Ok(handle)
    }

    pub(crate) fn scorer_view(&self) -> ScorerView {
        ScorerView {
            run_id: self.run_id.clone(),
            predictions_dir: self.predictions_dir.clone(),
        }
    }

    fn publish_blob(&self, handle: &PredictionHandle, bytes: &[u8]) -> Result<()> {
        let suffix = random_hex::<8>();
        let temporary =
            self.predictions_dir
                .join(format!(".{}.{}.tmp", handle.expose_to_adapter(), suffix));
        let destination = self.blob_path(handle);
        let result = (|| {
            let mut file = open_private_new_file(&temporary)?;
            file.write_all(bytes)
                .map_err(|_| BenchmarkError::coded("private_prediction_write_failed"))?;
            file.flush()
                .map_err(|_| BenchmarkError::coded("private_prediction_write_failed"))?;
            file.sync_all()
                .map_err(|_| BenchmarkError::coded("private_prediction_write_failed"))?;
            validate_private_file(&temporary)?;
            fs::hard_link(&temporary, &destination)
                .map_err(|_| BenchmarkError::coded("private_prediction_write_failed"))?;
            validate_private_file(&destination)?;
            fs::remove_file(&temporary)
                .map_err(|_| BenchmarkError::coded("private_prediction_write_failed"))?;
            sync_parent_directory(&self.predictions_dir)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn blob_path(&self, handle: &PredictionHandle) -> PathBuf {
        self.predictions_dir
            .join(format!("{}.blob", handle.expose_to_adapter()))
    }
}

impl fmt::Debug for PrivatePredictionStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PrivatePredictionStore([redacted])")
    }
}

#[derive(Clone)]
pub struct ScorerView {
    run_id: String,
    predictions_dir: PathBuf,
}

impl ScorerView {
    pub fn resolve(&self, outcome: &TaskOutcome) -> Result<PrivatePredictionPayload> {
        let prediction = outcome
            .prediction()
            .ok_or_else(|| BenchmarkError::coded("private_prediction_unavailable"))?;
        self.resolve_bound(
            outcome.task_id(),
            prediction.type_tag(),
            prediction.payload_handle(),
        )
    }

    fn resolve_bound(
        &self,
        task_id: &str,
        prediction_type: &str,
        handle: &PredictionHandle,
    ) -> Result<PrivatePredictionPayload> {
        validate_component(task_id)?;
        validate_safe_text(prediction_type)?;
        validate_handle(handle.expose_to_adapter())?;
        validate_private_directory(&self.predictions_dir)?;
        let path = self
            .predictions_dir
            .join(format!("{}.blob", handle.expose_to_adapter()));
        if !path.exists() {
            return Err(BenchmarkError::coded("private_prediction_not_found"));
        }
        validate_private_file(&path)?;
        let mut file = File::open(&path)
            .map_err(|_| BenchmarkError::coded("private_prediction_read_failed"))?;
        let size = file
            .metadata()
            .map_err(|_| BenchmarkError::coded("private_prediction_read_failed"))?
            .len();
        if size > MAX_ENVELOPE_BYTES {
            return Err(corrupt());
        }
        let mut envelope = Vec::with_capacity(size as usize);
        file.read_to_end(&mut envelope)
            .map_err(|_| BenchmarkError::coded("private_prediction_read_failed"))?;
        let expected_binding = binding_bytes(
            &self.run_id,
            task_id,
            prediction_type,
            handle.expose_to_adapter(),
        )?;
        let (content_type, binding, integrity, protected) = decode_envelope(&envelope)?;
        if binding != expected_binding {
            return Err(corrupt());
        }
        if integrity != integrity_digest(content_type, &expected_binding, protected).as_slice() {
            return Err(corrupt());
        }
        let plaintext = unprotect(protected, &expected_binding)?;
        PrivatePredictionPayload::new_zeroizing(content_type, plaintext).map_err(|_| corrupt())
    }
}

impl fmt::Debug for ScorerView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ScorerView([redacted])")
    }
}

fn binding_bytes(
    run_id: &str,
    task_id: &str,
    prediction_type: &str,
    handle: &str,
) -> Result<Vec<u8>> {
    validate_component(run_id)?;
    validate_component(task_id)?;
    validate_safe_text(prediction_type)?;
    validate_handle(handle)?;
    let mut binding = b"pinvou-private-prediction/v1".to_vec();
    for value in [run_id, task_id, prediction_type, handle] {
        let length = u16::try_from(value.len()).map_err(|_| corrupt())?;
        binding.extend_from_slice(&length.to_le_bytes());
        binding.extend_from_slice(value.as_bytes());
    }
    Ok(binding)
}

fn encode_envelope(
    content_type: PrivatePredictionContentType,
    binding: &[u8],
    integrity: &[u8; INTEGRITY_BYTES],
    protected: &[u8],
) -> Result<Vec<u8>> {
    let binding_len = u16::try_from(binding.len()).map_err(|_| corrupt())?;
    let protected_len = u32::try_from(protected.len()).map_err(|_| corrupt())?;
    let capacity = (12 + INTEGRITY_BYTES)
        .checked_add(binding.len())
        .and_then(|value| value.checked_add(protected.len()))
        .ok_or_else(corrupt)?;
    if capacity as u64 > MAX_ENVELOPE_BYTES {
        return Err(corrupt());
    }
    let mut result = Vec::with_capacity(capacity);
    result.extend_from_slice(MAGIC);
    result.push(SCHEMA_VERSION);
    result.push(content_type.tag());
    result.extend_from_slice(&binding_len.to_le_bytes());
    result.extend_from_slice(&protected_len.to_le_bytes());
    result.extend_from_slice(integrity);
    result.extend_from_slice(binding);
    result.extend_from_slice(protected);
    Ok(result)
}

type DecodedEnvelope<'a> = (PrivatePredictionContentType, &'a [u8], &'a [u8], &'a [u8]);

fn decode_envelope(bytes: &[u8]) -> Result<DecodedEnvelope<'_>> {
    let header_len = 12 + INTEGRITY_BYTES;
    if bytes.len() < header_len || &bytes[..4] != MAGIC || bytes[4] != SCHEMA_VERSION {
        return Err(corrupt());
    }
    let content_type = PrivatePredictionContentType::from_tag(bytes[5])?;
    let binding_len = usize::from(u16::from_le_bytes([bytes[6], bytes[7]]));
    let protected_len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let binding_end = header_len.checked_add(binding_len).ok_or_else(corrupt)?;
    let protected_end = binding_end.checked_add(protected_len).ok_or_else(corrupt)?;
    if protected_end != bytes.len() {
        return Err(corrupt());
    }
    Ok((
        content_type,
        &bytes[header_len..binding_end],
        &bytes[12..header_len],
        &bytes[binding_end..protected_end],
    ))
}

fn integrity_digest(
    content_type: PrivatePredictionContentType,
    binding: &[u8],
    protected: &[u8],
) -> [u8; INTEGRITY_BYTES] {
    let mut hasher = Sha256::new();
    hasher.update(b"pinvou-private-prediction-integrity/v2");
    hasher.update([content_type.tag()]);
    hasher.update((binding.len() as u64).to_le_bytes());
    hasher.update(binding);
    hasher.update((protected.len() as u64).to_le_bytes());
    hasher.update(protected);
    hasher.finalize().into()
}

fn dpapi_entropy(binding: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"pinvou-private-prediction-dpapi-entropy/v1");
    hasher.update((binding.len() as u64).to_le_bytes());
    hasher.update(binding);
    hasher.finalize().into()
}

fn random_hex<const N: usize>() -> String {
    let bytes: [u8; N] = rand::random();
    let mut value = String::with_capacity(N * 2);
    for byte in bytes {
        let _ = write!(&mut value, "{byte:02x}");
    }
    value
}

fn validate_handle(handle: &str) -> Result<()> {
    if handle.len() != 64
        || !handle
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(BenchmarkError::coded("invalid_private_prediction_handle"));
    }
    Ok(())
}

fn corrupt() -> BenchmarkError {
    BenchmarkError::coded("private_prediction_corrupt")
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    if path.exists() {
        return validate_private_directory(path);
    }
    create_private_directory(path)?;
    validate_private_directory(path)
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder
        .create(path)
        .map_err(|_| BenchmarkError::coded("private_prediction_write_failed"))
}

#[cfg(windows)]
fn create_private_directory(path: &Path) -> Result<()> {
    fs::create_dir(path).map_err(|_| BenchmarkError::coded("private_prediction_write_failed"))?;
    if crate::apply_windows_private_acl(path, true).is_err() {
        let _ = fs::remove_dir(path);
        return Err(BenchmarkError::coded("private_permissions_unsupported"));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn create_private_directory(path: &Path) -> Result<()> {
    fs::create_dir(path).map_err(|_| BenchmarkError::coded("private_prediction_write_failed"))
}

fn validate_private_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| BenchmarkError::coded("private_prediction_read_failed"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(BenchmarkError::coded("unsafe_private_store_path"));
    }
    validate_platform_directory(path, &metadata)
}

fn validate_private_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| BenchmarkError::coded("private_prediction_read_failed"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(BenchmarkError::coded("private_prediction_corrupt"));
    }
    validate_platform_file(path, &metadata)
}

#[cfg(unix)]
fn validate_platform_directory(_path: &Path, metadata: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o777 != 0o700 {
        return Err(BenchmarkError::coded("private_permissions_unsupported"));
    }
    Ok(())
}

#[cfg(windows)]
fn validate_platform_directory(path: &Path, _metadata: &fs::Metadata) -> Result<()> {
    crate::apply_windows_private_acl(path, true)
        .map_err(|_| BenchmarkError::coded("private_permissions_unsupported"))
}

#[cfg(not(any(unix, windows)))]
fn validate_platform_directory(_path: &Path, _metadata: &fs::Metadata) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn validate_platform_file(_path: &Path, metadata: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o777 != 0o600 {
        return Err(BenchmarkError::coded("private_permissions_unsupported"));
    }
    Ok(())
}

#[cfg(windows)]
fn validate_platform_file(path: &Path, _metadata: &fs::Metadata) -> Result<()> {
    crate::apply_windows_private_acl(path, false)
        .map_err(|_| BenchmarkError::coded("private_permissions_unsupported"))
}

#[cfg(not(any(unix, windows)))]
fn validate_platform_file(_path: &Path, _metadata: &fs::Metadata) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn open_private_new_file(path: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| BenchmarkError::coded("private_prediction_write_failed"))
}

#[cfg(windows)]
fn open_private_new_file(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| BenchmarkError::coded("private_prediction_write_failed"))?;
    if crate::apply_windows_private_acl(path, false).is_err() {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(BenchmarkError::coded("private_permissions_unsupported"));
    }
    Ok(file)
}

#[cfg(not(any(unix, windows)))]
fn open_private_new_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| BenchmarkError::coded("private_prediction_write_failed"))
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| BenchmarkError::coded("private_prediction_write_failed"))
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(windows)]
fn protect(plaintext: &[u8], binding: &[u8]) -> Result<Vec<u8>> {
    use std::ptr::null;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(plaintext.len())
            .map_err(|_| BenchmarkError::coded("private_prediction_too_large"))?,
        pbData: plaintext.as_ptr() as *mut u8,
    };
    let entropy_bytes = dpapi_entropy(binding);
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: entropy_bytes.len() as u32,
        pbData: entropy_bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let succeeded = unsafe {
        CryptProtectData(
            &input,
            null(),
            &entropy,
            null(),
            null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if succeeded == 0 || output.pbData.is_null() {
        return Err(BenchmarkError::coded("private_protection_unavailable"));
    }
    let protected =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(protected)
}

#[cfg(windows)]
fn unprotect(protected: &[u8], binding: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(protected.len()).map_err(|_| corrupt())?,
        pbData: protected.as_ptr() as *mut u8,
    };
    let entropy_bytes = dpapi_entropy(binding);
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: entropy_bytes.len() as u32,
        pbData: entropy_bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let succeeded = unsafe {
        CryptUnprotectData(
            &input,
            null_mut(),
            &entropy,
            null(),
            null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if succeeded == 0 || output.pbData.is_null() {
        return Err(corrupt());
    }
    let plaintext = Zeroizing::new(unsafe {
        std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
    });
    unsafe {
        std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
        LocalFree(output.pbData.cast());
    }
    Ok(plaintext)
}

#[cfg(not(windows))]
fn protect(plaintext: &[u8], _binding: &[u8]) -> Result<Vec<u8>> {
    Ok(plaintext.to_vec())
}

#[cfg(not(windows))]
fn unprotect(protected: &[u8], _binding: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    Ok(Zeroizing::new(protected.to_vec()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

    struct TestRun(PathBuf);

    impl TestRun {
        fn new(run_id: &str) -> Self {
            let unique = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!(
                    "pinvou-private-prediction-{}-{unique}",
                    std::process::id()
                ))
                .join(run_id);
            fs::create_dir_all(&path).expect("create test run");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestRun {
        fn drop(&mut self) {
            let root = self.0.parent().expect("test root");
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn payload_is_bounded_and_debug_redacted() {
        let payload = PrivatePredictionPayload::utf8("answer-sentinel").expect("bounded payload");
        fn assert_zeroizing(_: &Zeroizing<Vec<u8>>) {}
        assert_zeroizing(&payload.bytes);
        assert_eq!(
            format!("{payload:?}"),
            "PrivatePredictionPayload([redacted])"
        );
        assert_eq!(
            payload.content_type(),
            PrivatePredictionContentType::Utf8TextV1
        );
        assert_eq!(payload.expose_to_scorer(), b"answer-sentinel");

        let error = PrivatePredictionPayload::utf8("x".repeat(1_048_577))
            .expect_err("oversized payload must fail");
        assert_eq!(error.code(), "private_prediction_too_large");
    }

    #[test]
    fn envelope_integrity_binds_protected_payload_and_metadata() {
        let binding = b"run-task-type-handle";
        let digest = integrity_digest(
            PrivatePredictionContentType::Utf8TextV1,
            binding,
            b"answer-sentinel",
        );
        assert_ne!(
            digest,
            integrity_digest(
                PrivatePredictionContentType::Utf8TextV1,
                binding,
                b"answer-sentineL",
            )
        );
        assert_ne!(
            digest,
            integrity_digest(
                PrivatePredictionContentType::Utf8TextV1,
                b"other-binding",
                b"answer-sentinel",
            )
        );
    }

    #[test]
    fn canonical_json_v1_requires_valid_compact_json() {
        let payload = PrivatePredictionPayload::canonical_json(br#"{"answer":1}"#.to_vec())
            .expect("compact canonical json");
        assert_eq!(
            payload.content_type(),
            PrivatePredictionContentType::CanonicalJsonV1
        );
        for invalid in [br#"{ "answer": 1 }"#.as_slice(), b"not-json".as_slice()] {
            let error = PrivatePredictionPayload::canonical_json(invalid.to_vec())
                .expect_err("non-canonical json");
            assert_eq!(error.code(), "private_prediction_invalid_json");
        }
    }

    #[test]
    fn dpapi_entropy_is_a_domain_separated_binding_digest() {
        let entropy = dpapi_entropy(b"binding");
        assert_eq!(entropy.len(), 32);
        assert_ne!(entropy.as_slice(), b"binding");
        assert_ne!(entropy, dpapi_entropy(b"binding-2"));
    }

    #[test]
    fn integrity_corruption_and_truncation_fail_closed() {
        let run = TestRun::new("run-integrity");
        let store = PrivatePredictionStore::create(run.path(), "run-integrity").unwrap();
        let handle = store
            .put(
                "task-1",
                "gaia-final/v1",
                PrivatePredictionPayload::utf8("answer-sentinel").unwrap(),
            )
            .unwrap();
        let path = store.blob_path(&handle);
        let original = fs::read(&path).unwrap();

        let mut corrupted_digest = original.clone();
        corrupted_digest[12] ^= 0x5a;
        fs::write(&path, corrupted_digest).unwrap();
        let corrupted = store
            .scorer_view()
            .resolve_bound("task-1", "gaia-final/v1", &handle)
            .unwrap_err();
        assert_eq!(corrupted.code(), "private_prediction_corrupt");

        fs::write(&path, &original[..original.len() - 1]).unwrap();
        let truncated = store
            .scorer_view()
            .resolve_bound("task-1", "gaia-final/v1", &handle)
            .unwrap_err();
        assert_eq!(truncated.code(), "private_prediction_corrupt");
    }

    #[test]
    fn store_round_trips_with_run_task_type_and_handle_binding() {
        let run = TestRun::new("run-1");
        let store = PrivatePredictionStore::create(run.path(), "run-1").expect("store");
        let handle = store
            .put(
                "task-1",
                "gaia-final/v1",
                PrivatePredictionPayload::utf8("answer-sentinel").unwrap(),
            )
            .expect("put");
        let raw_handle = handle.expose_to_adapter();
        assert_eq!(raw_handle.len(), 64);
        assert!(
            raw_handle
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
        assert_eq!(format!("{store:?}"), "PrivatePredictionStore([redacted])");
        drop(store);

        let reopened = PrivatePredictionStore::create(run.path(), "run-1").expect("reopen store");
        let view = reopened.scorer_view();
        let output = view
            .resolve_bound("task-1", "gaia-final/v1", &handle)
            .expect("resolve");
        assert_eq!(output.expose_to_scorer(), b"answer-sentinel");
        assert_eq!(format!("{view:?}"), "ScorerView([redacted])");

        let wrong_task = view
            .resolve_bound("task-2", "gaia-final/v1", &handle)
            .expect_err("task binding");
        assert_eq!(wrong_task.code(), "private_prediction_corrupt");
        assert!(!wrong_task.to_string().contains("answer-sentinel"));
        assert!(!wrong_task.to_string().contains(raw_handle));
    }

    #[test]
    fn public_handle_cannot_cross_run_boundaries() {
        let first = TestRun::new("run-a");
        let first_store = PrivatePredictionStore::create(first.path(), "run-a").unwrap();
        let handle = first_store
            .put(
                "task-1",
                "gaia-final/v1",
                PrivatePredictionPayload::utf8("answer-sentinel").unwrap(),
            )
            .unwrap();
        let second = TestRun::new("run-b");
        let second_store = PrivatePredictionStore::create(second.path(), "run-b").unwrap();

        let missing = second_store
            .scorer_view()
            .resolve_bound("task-1", "gaia-final/v1", &handle)
            .expect_err("handle alone is not a capability");
        assert_eq!(missing.code(), "private_prediction_not_found");

        fs::copy(
            first_store.blob_path(&handle),
            second_store.blob_path(&handle),
        )
        .expect("simulate cross-run blob swap");
        let swapped = second_store
            .scorer_view()
            .resolve_bound("task-1", "gaia-final/v1", &handle)
            .expect_err("blob is bound to its originating run");
        assert_eq!(swapped.code(), "private_prediction_corrupt");
    }

    #[cfg(windows)]
    #[test]
    fn windows_blob_is_dpapi_protected_and_corruption_fails_closed() {
        let run = TestRun::new("run-win");
        let store = PrivatePredictionStore::create(run.path(), "run-win").unwrap();
        let handle = store
            .put(
                "task-1",
                "gaia-final/v1",
                PrivatePredictionPayload::utf8("answer-sentinel").unwrap(),
            )
            .unwrap();
        let path = store.blob_path(&handle);
        let mut blob = fs::read(&path).unwrap();
        assert!(
            !blob
                .windows(b"answer-sentinel".len())
                .any(|part| part == b"answer-sentinel")
        );
        let last = blob.last_mut().expect("protected body");
        *last ^= 0x5a;
        fs::write(path, blob).unwrap();

        let error = store
            .scorer_view()
            .resolve_bound("task-1", "gaia-final/v1", &handle)
            .expect_err("corruption");
        assert_eq!(error.code(), "private_prediction_corrupt");
    }

    #[cfg(windows)]
    #[test]
    fn windows_dpapi_rejects_a_wrong_entropy_binding() {
        let protected = protect(b"answer-sentinel", b"binding-a").expect("DPAPI protect");
        let error = unprotect(&protected, b"binding-b").expect_err("wrong entropy");
        assert_eq!(error.code(), "private_prediction_corrupt");
    }

    #[cfg(unix)]
    #[test]
    fn unix_store_creates_and_revalidates_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let run = TestRun::new("run-unix");
        let store = PrivatePredictionStore::create(run.path(), "run-unix").unwrap();
        let handle = store
            .put(
                "task-1",
                "gaia-final/v1",
                PrivatePredictionPayload::utf8("answer-sentinel").unwrap(),
            )
            .unwrap();
        assert_eq!(
            fs::metadata(&store.predictions_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let blob_path = store.blob_path(&handle);
        assert_eq!(
            fs::metadata(&blob_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::set_permissions(&blob_path, fs::Permissions::from_mode(0o644)).unwrap();
        let error = store
            .scorer_view()
            .resolve_bound("task-1", "gaia-final/v1", &handle)
            .expect_err("unsafe permissions");
        assert_eq!(error.code(), "private_permissions_unsupported");
    }
}
