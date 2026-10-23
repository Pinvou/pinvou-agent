//! Actual complete bytes on retained protected objects; explicit legacy signer policy.
//! Observation/report alone never constructs a source/service/launch capability.
use super::{
    layout::{ProtectedDirectory, custody_error},
    security::inspect_file,
};
use crate::{
    error::{Error, Result},
    inventory::{FIRST_RESOURCE_ID, InventoryBytes, MAX_SHARDS},
    json,
    profile::{ImageCertificatePolicy, ImageReference, InstallationManifest, RootManifest, Seed},
};
use pinvou_upgrade_custody as custody;
use pinvou_upgrade_native_evidence::{self as native, ExpectedFile, Observation, Verification};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    ptr,
    sync::atomic::AtomicBool,
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_RESOURCE_DATA_NOT_FOUND, ERROR_RESOURCE_LANG_NOT_FOUND,
        ERROR_RESOURCE_NAME_NOT_FOUND, ERROR_RESOURCE_TYPE_NOT_FOUND, FreeLibrary, GetLastError,
        HMODULE,
    },
    System::LibraryLoader::{
        EnumResourceLanguagesW, FindResourceExW, LOAD_LIBRARY_AS_DATAFILE_EXCLUSIVE,
        LOAD_LIBRARY_AS_IMAGE_RESOURCE, LoadLibraryExW, LoadResource, LockResource, SizeofResource,
    },
};

pub(super) struct SealedImage {
    pub(super) file: File,
    _observation: Option<Observation>,
    pub(super) identity: custody::Identity,
    pub(super) size: u64,
    pub(super) sha256: String,
}
impl SealedImage {
    pub(super) fn open(
        root: &ProtectedDirectory,
        name: &str,
        reference: &ImageReference,
        seed: &Seed,
    ) -> Result<Self> {
        Self::open_role(root, name, reference, seed, false)
    }
    fn open_role(
        root: &ProtectedDirectory,
        name: &str,
        reference: &ImageReference,
        seed: &Seed,
        nsis: bool,
    ) -> Result<Self> {
        let mut file = root.relative_file(name)?;
        inspect_file(&file, true, None)?;
        let identity = custody::identity(&file).map_err(custody_error)?;
        reference.validate(seed)?;
        if verify_bytes(&mut file, reference.size, &reference.sha256)? != identity {
            return Err(Error::ImageRefused);
        }
        let observation = match seed.certificate_policy()? {
            ImageCertificatePolicy::LegacySelectedSigner(approved) => {
                let expected = ExpectedFile::new(reference.size, &reference.sha256)
                    .map_err(|_| Error::ImageRefused)?;
                let observation = native::observe_file(&file, &expected, &AtomicBool::new(false))
                    .map_err(|_| Error::ImageRefused)?;
                let valid = observation.report().signatures.iter().any(|signature| {
                    signature.verification == Verification::Verified
                        && signature.signer.as_ref().is_some_and(|chain| {
                            chain.provider_error == 0
                                && chain.certificates.first().is_some_and(|leaf| {
                                    leaf.provider_error == 0 && leaf.certificate_sha256 == approved
                                })
                        })
                });
                if !valid {
                    return Err(Error::ImageRefused);
                }
                Some(observation)
            }
            // No certificate observation is fabricated or needed for byte custody.
            // Source authority must be supplied by the separate native entry owner.
            ImageCertificatePolicy::Optional => None,
        };
        if custody::identity(&file).map_err(custody_error)? != identity {
            return Err(Error::ImageRefused);
        }
        require_machine(&mut file, reference.size, nsis)?;
        root.recheck()?;
        Ok(Self {
            file,
            _observation: observation,
            identity,
            size: reference.size,
            sha256: reference.sha256.clone(),
        })
    }
    /// Initial SOURCE outer/helper has no self-referential manifest pin. Its
    /// complete observed bytes are sealed under the actual protected source
    /// and independent selected signer policy, then bound into native intent.
    /// This private method is never exposed for arbitrary files or IPC inputs.
    pub(super) fn open_original(
        root: &ProtectedDirectory,
        name: &str,
        seed: &Seed,
    ) -> Result<Self> {
        if seed.certificate_policy()? == ImageCertificatePolicy::Optional {
            return Err(Error::InitialTrustUnavailable);
        }
        Self::open_observed_original(root, name, seed)
    }
    /// Byte observation for the native local administrator entry only. This
    /// returned image is NOT remote update or installation authorization.
    pub(super) fn observe_local_original(
        root: &ProtectedDirectory,
        name: &str,
        seed: &Seed,
    ) -> Result<Self> {
        if seed.certificate_policy()? != ImageCertificatePolicy::Optional
            || !matches!(name, "installer.exe" | "pinvou-upgrade-helper.exe")
        {
            return Err(Error::InstallerContextRefused);
        }
        Self::open_observed_original(root, name, seed)
    }
    /// Reopen outer bytes against an independently read original protected
    /// registration. Never choose a fresh expected hash for an existing intent.
    pub(super) fn open_registered_installer(
        root: &ProtectedDirectory,
        record: &crate::registration::HostRegistration,
        seed: &Seed,
    ) -> Result<Self> {
        record.validate(seed)?;
        let file = root.relative_file("installer.exe")?;
        let identity = custody::identity(&file).map_err(custody_error)?;
        let size = file.metadata().map_err(|_| Error::Unavailable)?.len();
        let value = Self::open_role(
            root,
            "installer.exe",
            &ImageReference {
                build_id: seed.build_id.clone(),
                version: seed.app_version.clone(),
                size,
                sha256: record.source_set.nsis_sha256.clone(),
                layout_id: seed.layout_id.clone(),
            },
            seed,
            true,
        )?;
        if value.identity != identity {
            return Err(Error::ImageRefused);
        }
        Ok(value)
    }
    fn open_observed_original(root: &ProtectedDirectory, name: &str, seed: &Seed) -> Result<Self> {
        let mut file = root.relative_file(name)?;
        let identity = custody::identity(&file).map_err(custody_error)?;
        let size = file.metadata().map_err(|_| Error::Unavailable)?.len();
        if size == 0 || size > json::SAFE_MAX {
            return Err(Error::ImageRefused);
        }
        let mut hasher = Sha256::new();
        let mut total = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            let n = file.read(&mut buffer).map_err(|_| Error::Unavailable)?;
            if n == 0 {
                break;
            }
            total = total.checked_add(n as u64).ok_or(Error::ImageRefused)?;
            if total > size {
                return Err(Error::ImageRefused);
            }
            hasher.update(&buffer[..n]);
        }
        if total != size || custody::identity(&file).map_err(custody_error)? != identity {
            return Err(Error::ImageRefused);
        }
        let sha256: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        drop(file);
        let value = Self::open_role(
            root,
            name,
            &ImageReference {
                build_id: seed.build_id.clone(),
                version: seed.app_version.clone(),
                size,
                sha256,
                layout_id: seed.layout_id.clone(),
            },
            seed,
            name == "installer.exe",
        )?;
        if value.identity != identity {
            return Err(Error::ImageRefused);
        }
        Ok(value)
    }
    pub(super) fn matches(&self, reference: &ImageReference) -> bool {
        self.size == reference.size && self.sha256 == reference.sha256
    }
}
/// This profile's app/helper/launcher must be actual x64 PE, not merely a
/// self-reported target string. The independently signed NSIS stub may be x86.
fn require_machine(file: &mut File, size: u64, nsis: bool) -> Result<()> {
    let mut dos = [0u8; 64];
    file.seek(SeekFrom::Start(0))
        .map_err(|_| Error::Unavailable)?;
    file.read_exact(&mut dos).map_err(|_| Error::ImageRefused)?;
    if &dos[..2] != b"MZ" {
        return Err(Error::ImageRefused);
    }
    let offset =
        u32::from_le_bytes(dos[60..64].try_into().map_err(|_| Error::ImageRefused)?) as u64;
    if offset < 64 || offset.checked_add(6).is_none_or(|n| n > size) {
        return Err(Error::ImageRefused);
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| Error::Unavailable)?;
    let mut pe = [0u8; 6];
    file.read_exact(&mut pe).map_err(|_| Error::ImageRefused)?;
    let machine = u16::from_le_bytes([pe[4], pe[5]]);
    if &pe[..4] != b"PE\0\0" || machine != 0x8664 && !(nsis && machine == 0x014c) {
        return Err(Error::ImageRefused);
    }
    Ok(())
}

/// Size/hash identity verification also supports ordinary zero-byte resources.
pub(super) fn verify_bytes(file: &mut File, size: u64, hash: &str) -> Result<custody::Identity> {
    let before = custody::identity(file).map_err(custody_error)?;
    if file.metadata().map_err(|_| Error::Unavailable)?.len() != size || !json::digest(hash) {
        return Err(Error::ImageRefused);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| Error::Unavailable)?;
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(|_| Error::Unavailable)?;
        if n == 0 {
            break;
        }
        total = total.checked_add(n as u64).ok_or(Error::ImageRefused)?;
        if total > size {
            return Err(Error::ImageRefused);
        }
        digest.update(&buffer[..n]);
    }
    let actual: String = digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if total != size || actual != hash || custody::identity(file).map_err(custody_error)? != before
    {
        return Err(Error::ImageRefused);
    }
    Ok(before)
}
/// Intent must be committed by the only caller before invoking this writer.
/// Existing objects are verified exactly, never truncated or repaired by ACL.
pub(super) fn copy_exact(
    source: &ProtectedDirectory,
    source_name: &str,
    target: &ProtectedDirectory,
    target_name: &str,
    size: u64,
    hash: &str,
) -> Result<()> {
    let mut input = source.relative_file(source_name)?;
    let source_identity = verify_bytes(&mut input, size, hash)?;
    let (directory, leaf) = target.relative_parent(target_name)?;
    let parent = directory.as_ref().unwrap_or(target);
    match parent.file(&leaf) {
        Ok(mut existing) => {
            verify_bytes(&mut existing, size, hash)?;
            return Ok(());
        }
        Err(Error::SourceUnavailable) => {}
        Err(other) => return Err(other),
    }
    input
        .seek(SeekFrom::Start(0))
        .map_err(|_| Error::Unavailable)?;
    let mut output = parent.fresh_file(&leaf, false)?;
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    loop {
        let n = input.read(&mut buffer).map_err(|_| Error::Unavailable)?;
        if n == 0 {
            break;
        }
        total = total.checked_add(n as u64).ok_or(Error::ImageRefused)?;
        if total > size {
            return Err(Error::ImageRefused);
        }
        output
            .write_all(&buffer[..n])
            .map_err(|_| Error::Unavailable)?;
    }
    output.sync_all().map_err(|_| Error::Unavailable)?;
    let written = custody::identity(&output).map_err(custody_error)?;
    if total != size || custody::identity(&input).map_err(custody_error)? != source_identity {
        return Err(Error::ImageRefused);
    }
    // No writer clone survives this point. Reopen under the actual retained
    // parent, with share-read only, then compare identity AND the full bytes.
    drop(output);
    let mut reopened = parent.file(&leaf)?;
    if verify_bytes(&mut reopened, size, hash)? != written {
        return Err(Error::ImageRefused);
    }
    source.recheck()?;
    parent.recheck()?;
    Ok(())
}

struct Module(HMODULE);
impl Drop for Module {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe {
            FreeLibrary(self.0);
        }
    }
}
/// Read resources from the already sealed exact NSIS file. LoadLibraryEx only
/// maps a data/image resource view, never executes its entry point. The retained
/// write/delete-denying file and protected ancestry prevent path replacement.
#[allow(unsafe_code)]
pub(super) fn manifest_from_installer(
    file: &SealedImage,
    path: &std::path::Path,
    seed: &Seed,
) -> Result<(InstallationManifest, InventoryBytes)> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let module = Module(unsafe {
        LoadLibraryExW(
            wide.as_ptr(),
            ptr::null_mut(),
            LOAD_LIBRARY_AS_DATAFILE_EXCLUSIVE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
        )
    });
    if module.0.is_null() {
        return Err(Error::ImageRefused);
    }
    let result = read_inventory_resources(&module, seed)?;
    if custody::identity(&file.file).map_err(custody_error)? != file.identity {
        return Err(Error::ImageRefused);
    }
    Ok(result)
}
fn read_inventory_resources(
    module: &Module,
    seed: &Seed,
) -> Result<(InstallationManifest, InventoryBytes)> {
    let bytes = resource(module, crate::profile::MANIFEST_RESOURCE_ID, false)?
        .ok_or(Error::ImageRefused)?;
    let root = RootManifest::parse(&bytes, seed)?;
    let mut shards = Vec::with_capacity(root.app_inventory.shards.len());
    for index in 0..MAX_SHARDS {
        let value = resource(module, FIRST_RESOURCE_ID + index as u16, true)?;
        match (index < root.app_inventory.shards.len(), value) {
            (true, Some(bytes)) => shards.push(bytes),
            (false, None) => {}
            _ => return Err(Error::ImageRefused),
        }
    }
    let input = InventoryBytes {
        root: bytes,
        shards,
    };
    let manifest = input.parse(seed)?;
    super::layout::validate_native_catalog(&manifest.app_files)?;
    Ok((manifest, input))
}
#[allow(unsafe_code)]
fn resource(module: &Module, id: u16, absent_allowed: bool) -> Result<Option<Vec<u8>>> {
    let mut languages = Vec::<u16>::new();
    unsafe extern "system" fn language(
        _module: HMODULE,
        _kind: *const u16,
        _name: *const u16,
        lang: u16,
        param: isize,
    ) -> i32 {
        // Synchronous SDK callback into this exact caller-owned, bounded Vec.
        let values = unsafe { &mut *(param as *mut Vec<u16>) };
        if values.len() >= 2 {
            return 0;
        }
        values.push(lang);
        1
    }
    let name = id as usize as *const u16;
    let kind = 10usize as *const u16;
    let found = unsafe {
        EnumResourceLanguagesW(
            module.0,
            kind,
            name,
            Some(language),
            (&mut languages as *mut Vec<u16>) as isize,
        )
    };
    let error = unsafe { GetLastError() };
    if found == 0
        && languages.is_empty()
        && absent_allowed
        && matches!(
            error,
            ERROR_RESOURCE_DATA_NOT_FOUND
                | ERROR_RESOURCE_TYPE_NOT_FOUND
                | ERROR_RESOURCE_NAME_NOT_FOUND
                | ERROR_RESOURCE_LANG_NOT_FOUND
        )
    {
        return Ok(None);
    }
    if found == 0 || languages != [0] {
        return Err(Error::ImageRefused);
    }
    let resource = unsafe { FindResourceExW(module.0, kind, name, 0) };
    if resource.is_null() {
        return Err(Error::ImageRefused);
    }
    let size = unsafe { SizeofResource(module.0, resource) } as usize;
    if size == 0 || size > json::RECORD_MAX {
        return Err(Error::ImageRefused);
    }
    let memory = unsafe { LoadResource(module.0, resource) };
    if memory.is_null() {
        return Err(Error::ImageRefused);
    }
    let pointer = unsafe { LockResource(memory) };
    if pointer.is_null() {
        return Err(Error::ImageRefused);
    }
    // SDK resource extent was bounded before allocation. The module and sealed
    // original installer/file lease remain alive for the complete copy.
    Ok(Some(
        unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), size) }.to_vec(),
    ))
}

#[cfg(test)]
mod inventory_resource_tests {
    use super::*;
    use crate::profile::AppFile;
    use std::{
        mem,
        path::{Path, PathBuf},
    };
    use windows_sys::Win32::System::LibraryLoader::{
        BeginUpdateResourceW, EndUpdateResourceW, UpdateResourceW,
    };
    fn wide(path: &Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }
    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    #[allow(unsafe_code)]
    fn update(path: &Path, id: u16, language: u16, bytes: Option<&[u8]>) {
        struct Pending(*mut std::ffi::c_void);
        impl Drop for Pending {
            #[allow(unsafe_code)]
            fn drop(&mut self) {
                if !self.0.is_null() {
                    unsafe {
                        EndUpdateResourceW(self.0, 1);
                    }
                }
            }
        }
        let mut pending = Pending(unsafe { BeginUpdateResourceW(wide(path).as_ptr(), 0) });
        assert!(!pending.0.is_null());
        let (pointer, length) = bytes.map_or((ptr::null(), 0), |bytes| {
            (bytes.as_ptr().cast(), bytes.len() as u32)
        });
        assert_ne!(
            unsafe {
                UpdateResourceW(
                    pending.0,
                    10usize as *const u16,
                    id as usize as *const u16,
                    language,
                    pointer,
                    length,
                )
            },
            0
        );
        let handle = mem::replace(&mut pending.0, ptr::null_mut());
        assert_ne!(unsafe { EndUpdateResourceW(handle, 0) }, 0);
    }
    #[allow(unsafe_code)]
    fn read(path: &Path, seed: &Seed) -> Result<usize> {
        let module = Module(unsafe {
            LoadLibraryExW(
                wide(path).as_ptr(),
                ptr::null_mut(),
                LOAD_LIBRARY_AS_DATAFILE_EXCLUSIVE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
            )
        });
        if module.0.is_null() {
            return Err(Error::ImageRefused);
        }
        read_inventory_resources(&module, seed).map(|(manifest, _)| manifest.app_files.len())
    }
    #[test]
    fn actual_data_only_pe_resources_refuse_missing_extra_language_and_changed_shards_then_restore()
    {
        let app = AppFile {
            relative_name: "pinvou3-tauri.exe".into(),
            size: 1,
            sha256: "2".repeat(64),
        };
        let (seed, input) = crate::inventory::tests::fixture(vec![
            vec![app],
            vec![AppFile {
                relative_name: "z".into(),
                size: 0,
                sha256: "3".repeat(64),
            }],
        ]);
        let path = std::env::temp_dir().join(format!(
            "pinvou-inventory-pe-{}.exe",
            super::super::pipe::nonce().unwrap()
        ));
        let _temp = Temp(path.clone());
        std::fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
        // An ordinary unsigned COPY of this test PE. Only the private data
        // reader is exercised; no SealedImage/SourceInstallerContext is made,
        // no PE is executed, and this is not NSIS CRC/signature qualification.
        update(
            &path,
            crate::profile::MANIFEST_RESOURCE_ID,
            0,
            Some(&input.root),
        );
        for (index, bytes) in input.shards.iter().enumerate() {
            update(&path, FIRST_RESOURCE_ID + index as u16, 0, Some(bytes));
        }
        assert_eq!(read(&path, &seed).unwrap(), 2);
        update(&path, FIRST_RESOURCE_ID, 0, None);
        assert!(read(&path, &seed).is_err());
        update(&path, FIRST_RESOURCE_ID, 0, Some(&input.shards[0]));
        assert_eq!(read(&path, &seed).unwrap(), 2);
        update(&path, FIRST_RESOURCE_ID + 7, 0, Some(&input.shards[0]));
        assert!(read(&path, &seed).is_err());
        update(&path, FIRST_RESOURCE_ID + 7, 0, None);
        assert_eq!(read(&path, &seed).unwrap(), 2);
        update(&path, FIRST_RESOURCE_ID, 1033, Some(&input.shards[0]));
        assert!(read(&path, &seed).is_err());
        update(&path, FIRST_RESOURCE_ID, 1033, None);
        assert_eq!(read(&path, &seed).unwrap(), 2);
        let mut wrong = input.shards[0].clone();
        wrong[0] ^= 1;
        update(&path, FIRST_RESOURCE_ID, 0, Some(&wrong));
        assert!(read(&path, &seed).is_err());
        update(&path, FIRST_RESOURCE_ID, 0, Some(&input.shards[0]));
        assert_eq!(read(&path, &seed).unwrap(), 2);
        update(
            &path,
            FIRST_RESOURCE_ID,
            0,
            Some(&vec![b'x'; json::RECORD_MAX + 1]),
        );
        assert!(read(&path, &seed).is_err());
        update(&path, FIRST_RESOURCE_ID, 0, Some(&input.shards[0]));
        assert_eq!(read(&path, &seed).unwrap(), 2);
    }
    #[test]
    fn cross_shard_native_ordinal_alias_and_ancestor_checks() {
        use crate::inventory::tests::{app as app_row, fixture, resource as row};

        // Data-only native catalog checks, not an opened/installed host.
        for groups in [
            vec![vec![row("A")], vec![row("a"), app_row()]],
            vec![vec![row("a"), row("a-")], vec![row("a/file"), app_row()]],
        ] {
            let (seed, input) = fixture(groups);
            let parsed = input.parse(&seed).unwrap();
            assert!(super::super::layout::validate_native_catalog(&parsed.app_files).is_err());
        }
        let (seed, input) = fixture(vec![
            vec![app_row(), row("\u{e000}")],
            vec![row("\u{10000}")],
        ]);
        let parsed = input.parse(&seed).unwrap();
        super::super::layout::validate_native_catalog(&parsed.app_files).unwrap();
    }
}
