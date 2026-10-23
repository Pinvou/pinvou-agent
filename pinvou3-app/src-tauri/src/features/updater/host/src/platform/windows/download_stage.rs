//! Retained ordinary input -> private protected bytes, never an execution image.
use super::{
    image,
    layout::{Layout, ProtectedDirectory, custody_error},
};
use crate::download_stage::DownloadedPackageSource;
use crate::{
    download_stage::{
        NativeRole, StagingLimits, bind_package, poll, require_available_space, unique_member,
    },
    error::{Error, Result},
    profile::{AppFile, Seed},
};
use pinvou_upgrade_archive::{ArchiveError, inspect_file, materialize_member};
use pinvou_upgrade_byte_check::ByteCheckError;
use pinvou_upgrade_codec::{Document, InputKind, derive_metadata_reference};
use pinvou_upgrade_custody::{self as custody, Identity};
use pinvou_upgrade_package_check::{
    NativeMaterializeError, PackageCheckError, PackageInspection, inspect_full_pack,
    select_native_member,
};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

const PACKAGE: &str = "full-pack.zip";
const NATIVE: &str = "native.exe";

pub(crate) struct ProtectedDownloadBytes {
    directory: ProtectedDirectory,
    package: File,
    native: File,
    package_id: Identity,
    native_id: Identity,
    package_size: u64,
    package_sha256: String,
    native_size: u64,
    native_sha256: String,
    // Exact signed inputs stay attached to these bytes; they are not serializable
    // current-trust records and no code can use this type as SealedImage.
    _package_envelope: Vec<u8>,
    _expected_context: Vec<u8>,
    _root_claims_sha256: String,
    _member_name: String,
    _native_claim: Vec<u8>,
    reference: crate::ObjectReference,
}
impl ProtectedDownloadBytes {
    pub(crate) fn seal_reference(&self) -> Result<&crate::ObjectReference> {
        let generation = self.reference.generation();
        let binding = self.reference.binding();
        if generation.package_identity() != &self.package_id
            || generation.native_identity() != &self.native_id
            || binding.full_pack_size() != self.package_size
            || binding.full_pack_sha256() != self.package_sha256
            || binding.native_size() != self.native_size
            || binding.native_sha256() != self.native_sha256
        {
            return Err(Error::ImageRefused);
        }
        Ok(&self.reference)
    }
    pub(crate) fn size(&self) -> u64 {
        self.native_size
    }
    pub(crate) fn sha256(&self) -> &str {
        &self.native_sha256
    }
    pub(crate) fn read_at(&mut self, offset: u64, output: &mut [u8]) -> Result<usize> {
        self.directory.recheck()?;
        if custody::identity(&self.native).map_err(custody_error)? != self.native_id {
            return Err(Error::ImageRefused);
        }
        if offset > self.native_size {
            return Err(Error::ImageRefused);
        }
        let count = output
            .len()
            .min(65_536)
            .min((self.native_size - offset).min(usize::MAX as u64) as usize);
        self.native
            .seek(SeekFrom::Start(offset))
            .map_err(|_| Error::Unavailable)?;
        self.native
            .read_exact(&mut output[..count])
            .map_err(|_| Error::ImageRefused)?;
        Ok(count)
    }
    pub(crate) fn recheck(&mut self, cancelled: &mut dyn FnMut() -> bool) -> Result<()> {
        poll(cancelled)?;
        self.directory.recheck()?;
        self.directory.validate_inventory(&[
            AppFile {
                relative_name: PACKAGE.into(),
                size: self.package_size,
                sha256: self.package_sha256.clone(),
            },
            AppFile {
                relative_name: NATIVE.into(),
                size: self.native_size,
                sha256: self.native_sha256.clone(),
            },
        ])?;
        for (name, held, identity, size, hash) in [
            (
                PACKAGE,
                &mut self.package,
                &self.package_id,
                self.package_size,
                self.package_sha256.as_str(),
            ),
            (
                NATIVE,
                &mut self.native,
                &self.native_id,
                self.native_size,
                self.native_sha256.as_str(),
            ),
        ] {
            if image::verify_bytes_with_poll(held, size, hash, cancelled)? != *identity {
                return Err(Error::ImageRefused);
            }
            let mut reopened = self.directory.file(name)?;
            if image::verify_bytes_with_poll(&mut reopened, size, hash, cancelled)? != *identity {
                return Err(Error::ImageRefused);
            }
        }
        self.directory.recheck()?;
        poll(cancelled)
    }
}

fn document(bytes: &[u8]) -> Result<Document> {
    Document::parse(bytes, InputKind::Metadata).map_err(|_| Error::ImageRefused)
}
fn archive_error(error: ArchiveError) -> Error {
    match error {
        ArchiveError::Cancelled => Error::StagingCanceled,
        _ => Error::ImageRefused,
    }
}
fn package_error(error: PackageCheckError) -> Error {
    match error {
        PackageCheckError::Archive(error) => archive_error(error),
        _ => Error::ImageRefused,
    }
}
fn member_error(error: NativeMaterializeError) -> Error {
    match error {
        NativeMaterializeError::Archive(error) => archive_error(error),
        NativeMaterializeError::Package(error) => package_error(error),
        _ => Error::ImageRefused,
    }
}
fn byte_error(error: ByteCheckError) -> Error {
    match error {
        ByteCheckError::Cancelled => Error::StagingCanceled,
        _ => Error::ImageRefused,
    }
}
fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn claim_size(value: &serde_json::Value) -> Result<u64> {
    value
        .as_f64()
        .filter(|number| {
            number.is_finite()
                && number.fract() == 0.0
                && (1.0..=crate::json::SAFE_MAX as f64).contains(number)
        })
        .map(|number| number as u64)
        .ok_or(Error::ImageRefused)
}
fn fresh_leaf() -> Result<String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    // Unpredictability is not authority. FILE_CREATE rejects even a colliding
    // name; an existing/partial directory is never opened or adopted.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| Error::Unavailable)?
        .as_nanos();
    Ok(format!(
        "bytes-{}-{stamp}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn require_space(path: &Path, bytes: u64, reserve: u64) -> Result<()> {
    require_available_space(free_space(path)?, bytes, reserve)
}
#[allow(unsafe_code)]
fn free_space(path: &Path) -> Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0u64;
    // A capacity observation, not reservation or a trusted preparation budget.
    if unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(Error::Unavailable);
    }
    Ok(available)
}

pub(crate) fn stage_download_bytes(
    source: &DownloadedPackageSource<'_>,
    role: NativeRole,
    limits: &StagingLimits,
    cancelled: &mut dyn FnMut() -> bool,
) -> Result<ProtectedDownloadBytes> {
    poll(cancelled)?;
    let seed = Seed::embedded()?;
    // All metadata signature/host bindings precede protected object creation.
    let bound = bind_package(source, &seed, cancelled)?;
    let package = &bound.document;
    poll(cancelled)?;
    let signed = &package.value()["signed"];
    let claim = &signed[role.field()];
    let declared_native = document(&serde_json::to_vec(claim).map_err(|_| Error::ImageRefused)?)?
        .native_identity_shape()
        .map_err(|_| Error::ImageRefused)?;
    let expected_format = match role {
        NativeRole::FinalInstaller => "nsis",
        NativeRole::ExecutionHelper => "helper",
        NativeRole::Launcher => "launcher",
    };
    if claim["format"] != expected_format {
        return Err(Error::ImageRefused);
    }
    let native_size = claim_size(&claim["size"])?;
    let native_hash = claim["sha256"]
        .as_str()
        .ok_or(Error::ImageRefused)?
        .to_owned();
    let full = &signed["fullPackage"];
    let artifact = pinvou_upgrade_transfer::Artifact::new(
        full["packageId"].as_str().ok_or(Error::ImageRefused)?,
        claim_size(&full["size"])?,
        full["sha256"].as_str().ok_or(Error::ImageRefused)?,
    )
    .map_err(|_| Error::ImageRefused)?;
    let expected = &bound.expected;
    let version = document(signed["appVersion"].to_string().as_bytes())?
        .app_version_shape()
        .map_err(|_| Error::ImageRefused)?;
    let time = document(source.historical_time.to_string().as_bytes())?
        .protocol_time_shape()
        .map_err(|_| Error::ImageRefused)?;
    let reference = derive_metadata_reference(source.envelope).map_err(|_| Error::ImageRefused)?;
    let input = PackageInspection {
        artifact,
        manifest_bytes: source.envelope,
        reference: &reference,
        expected_context: expected,
        expected_version: &version,
        supplied_time: &time,
        limits: limits.archive,
    };
    let mut reader = source.file.try_clone().map_err(|_| Error::Unavailable)?;
    // Retained ordinary caches may have their original two-link publication.
    // That does not make them protected: only the independent copied hash does.
    let source_id = custody::read_identity(&reader).map_err(custody_error)?;
    // Full original source verification before any output is created. The copy
    // and protected readback below still recheck every byte against signed hash.
    let inventory =
        inspect_full_pack(&mut reader, input, &mut *cancelled).map_err(package_error)?;
    // The archive owner already computed these actual digests. Match the signed
    // hex claim without adding a second hex decoder or caller member locator.
    let digest = inventory
        .members()
        .iter()
        .find(|member| member.size() == native_size && hex(&member.sha256()) == native_hash)
        .ok_or(Error::ImageRefused)?
        .sha256();
    let member_name = unique_member(
        inventory
            .members()
            .iter()
            .map(|member| (member.name(), member.size(), member.sha256())),
        native_size,
        digest,
    )?;
    let layout = Layout::discover(&seed)?;
    let needed = artifact
        .size()
        .checked_add(native_size)
        .ok_or(Error::PrerequisiteUnavailable)?;
    require_space(&layout.program_files, needed, limits.reserve_bytes)?;
    poll(cancelled)?;
    let directory = layout.create_download_stage(&seed, &fresh_leaf()?)?;
    require_space(directory.path(), needed, limits.reserve_bytes)?;
    let mut writer = directory.fresh_file(PACKAGE, true)?;
    let written_id = custody::identity(&writer).map_err(custody_error)?;
    pinvou_upgrade_byte_check::copy_and_check(&mut reader, &mut writer, artifact, &mut *cancelled)
        .map_err(byte_error)?;
    if custody::read_identity(&reader).map_err(custody_error)? != source_id
        || custody::identity(&writer).map_err(custody_error)? != written_id
    {
        return Err(Error::ImageRefused);
    }
    drop(writer);
    let mut protected_package = directory.file(PACKAGE)?;
    let package_hash = hex(&artifact.sha256());
    if image::verify_bytes_with_poll(
        &mut protected_package,
        artifact.size(),
        &package_hash,
        cancelled,
    )? != written_id
    {
        return Err(Error::ImageRefused);
    }
    let input = PackageInspection {
        artifact,
        manifest_bytes: source.envelope,
        reference: &reference,
        expected_context: expected,
        expected_version: &version,
        supplied_time: &time,
        limits: limits.archive,
    };
    // Rebind full protected ZIP and exact role; no cached inventory is promoted.
    let selected = select_native_member(
        &mut protected_package,
        &input,
        role.member_kind(),
        &member_name,
        &mut *cancelled,
    )
    .map_err(member_error)?;
    if selected.size() != native_size
        || hex(selected.sha256()) != native_hash
        || selected.identity().canonical_bytes() != declared_native.canonical_bytes()
    {
        return Err(Error::ImageRefused);
    }
    require_space(directory.path(), native_size, limits.reserve_bytes)?;
    poll(cancelled)?;
    let mut native_writer = directory.fresh_file(NATIVE, true)?;
    let native_id = custody::identity(&native_writer).map_err(custody_error)?;
    materialize_member(
        &mut protected_package,
        &mut native_writer,
        artifact,
        selected.expected_member().map_err(member_error)?,
        limits.archive,
        &mut *cancelled,
    )
    .map_err(archive_error)?;
    if custody::identity(&native_writer).map_err(custody_error)? != native_id {
        return Err(Error::ImageRefused);
    }
    drop(native_writer);
    let mut native = directory.file(NATIVE)?;
    if image::verify_bytes_with_poll(&mut native, native_size, &native_hash, cancelled)?
        != native_id
    {
        return Err(Error::ImageRefused);
    }
    image::verify_package_role_with_poll(
        &mut native,
        native_size,
        &native_hash,
        &seed,
        role == NativeRole::FinalInstaller,
        cancelled,
    )?;
    // Full protected package gets one final complete rescan after extraction.
    inspect_file(
        &mut protected_package,
        artifact,
        limits.archive,
        &mut *cancelled,
    )
    .map_err(archive_error)?;
    require_space(directory.path(), 0, limits.reserve_bytes)?;
    let mut result = ProtectedDownloadBytes {
        directory,
        package: protected_package,
        native,
        package_id: written_id,
        native_id,
        package_size: artifact.size(),
        package_sha256: package_hash,
        native_size,
        native_sha256: native_hash,
        _package_envelope: source.envelope.to_vec(),
        _expected_context: expected
            .canonical_bytes()
            .map_err(|_| Error::ImageRefused)?,
        _root_claims_sha256: source.root_claims_pin.to_owned(),
        _member_name: member_name,
        _native_claim: declared_native.canonical_bytes().to_vec(),
        reference: crate::execution_object::reference_from_seal(
            &seed, &bound, role, written_id, native_id,
        )?,
    };
    result.recheck(cancelled)?;
    Ok(result)
}
