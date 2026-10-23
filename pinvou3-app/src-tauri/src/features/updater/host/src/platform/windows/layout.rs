//! Fixed native KnownFolder namespace and at-creation protected children.
//! No caller path or ordinary Directory is a host constructor.
use super::security::inspect_file;
use crate::{
    error::{Error, Result},
    profile::{AppFile, Seed},
};
use pinvou_upgrade_custody::{self as custody, CreationSecurity, Directory};
use std::{
    ffi::OsStr,
    fs::File,
    path::{Path, PathBuf},
    ptr,
};
use windows_sys::{
    Win32::{
        Globalization::{CSTR_EQUAL, CompareStringOrdinal},
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_ProgramData, FOLDERID_ProgramFilesX64, SHGetKnownFolderPath},
    },
    core::GUID,
};

pub(super) fn custody_error(e: custody::Error) -> Error {
    match e {
        custody::Error::NotFound => Error::SourceUnavailable,
        custody::Error::Busy => Error::Busy,
        custody::Error::AlreadyExists => Error::ScopeConflict,
        custody::Error::UnsafeObject => Error::SecurityRefused,
        custody::Error::Unsupported => Error::Unsupported,
        _ => Error::Unavailable,
    }
}
pub(super) fn validate_package_volume(path: &Path) -> Result<()> {
    verify_local_volume(path)
}
#[allow(unsafe_code)]
fn known_folder(id: &GUID) -> Result<PathBuf> {
    let mut pointer = ptr::null_mut();
    if unsafe { SHGetKnownFolderPath(id, 0, ptr::null_mut(), &mut pointer) } < 0
        || pointer.is_null()
    {
        return Err(Error::Unavailable);
    }
    struct Memory(*mut u16);
    impl Drop for Memory {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            unsafe {
                CoTaskMemFree(self.0.cast());
            }
        }
    }
    let _memory = Memory(pointer);
    let mut len = 0;
    while len < 32768 && unsafe { *pointer.add(len) } != 0 {
        len += 1;
    }
    if len == 0 || len == 32768 {
        return Err(Error::Unavailable);
    }
    let text = String::from_utf16(unsafe { std::slice::from_raw_parts(pointer, len) })
        .map_err(|_| Error::Unavailable)?;
    let path = PathBuf::from(text);
    if !path.is_absolute() {
        return Err(Error::SecurityRefused);
    }
    Ok(path)
}

#[allow(unsafe_code)]
fn verify_local_volume(path: &Path) -> Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumeInformationByHandleW};
    use windows_sys::Win32::System::{
        SystemServices::FILE_PERSISTENT_ACLS, WindowsProgramming::DRIVE_FIXED,
    };
    let drive = path.ancestors().last().ok_or(Error::SecurityRefused)?;
    let drive_text = drive.to_str().ok_or(Error::SecurityRefused)?;
    if drive_text.len() != 3
        || drive_text.as_bytes()[1] != b':'
        || !drive_text.as_bytes()[0].is_ascii_alphabetic()
    {
        return Err(Error::SecurityRefused);
    }
    let wide: Vec<u16> = drive_text.encode_utf16().chain(Some(0)).collect();
    if unsafe { GetDriveTypeW(wide.as_ptr()) } != DRIVE_FIXED {
        return Err(Error::SecurityRefused);
    }
    let directory = custody::directory(drive).map_err(custody_error)?;
    let file = directory
        .retained_directories()
        .last()
        .ok_or(Error::SecurityRefused)?;
    let mut fs = [0u16; 32];
    let mut flags = 0u32;
    if unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut flags,
            fs.as_mut_ptr(),
            fs.len() as u32,
        )
    } == 0
    {
        return Err(Error::Unavailable);
    }
    let length = fs
        .iter()
        .position(|u| *u == 0)
        .ok_or(Error::SecurityRefused)?;
    // First concrete host layout supports local NTFS only. No drive mapping,
    // remote namespace, unknown filesystem or missing persistent ACL fallback.
    if fs[..length] != [b'N' as u16, b'T' as u16, b'F' as u16, b'S' as u16]
        || flags & FILE_PERSISTENT_ACLS == 0
    {
        return Err(Error::Unsupported);
    }
    Ok(())
}

pub(super) struct Layout {
    pub(super) program_files: PathBuf,
    pub(super) program_data: PathBuf,
    pub(super) source: PathBuf,
    pub(super) code: PathBuf,
    pub(super) app: PathBuf,
    pub(super) state: PathBuf,
}
impl Layout {
    pub(super) fn discover(seed: &Seed) -> Result<Self> {
        let program_files = known_folder(&FOLDERID_ProgramFilesX64)?;
        let program_data = known_folder(&FOLDERID_ProgramData)?;
        Ok(Self {
            source: program_files
                .join("Pinvou")
                .join(seed.bootstrap_source_leaf())
                .join(&seed.profile_id)
                .join(&seed.build_id),
            code: program_files
                .join("Pinvou")
                .join(seed.updater_leaf())
                .join(&seed.profile_id),
            app: program_files
                .join("Pinvou")
                .join(seed.app_leaf())
                .join(&seed.profile_id),
            state: program_data
                .join("Pinvou")
                .join(seed.updater_leaf())
                .join(&seed.profile_id),
            program_files,
            program_data,
        })
    }
    pub(super) fn helper_path(&self, seed: &Seed) -> PathBuf {
        self.code
            .join("helpers")
            .join(&seed.build_id)
            .join("pinvou-upgrade-helper.exe")
    }
    pub(super) fn launcher_path(&self) -> PathBuf {
        self.code.join("pinvou-upgrade-launcher.exe")
    }
    pub(super) fn app_path(&self) -> PathBuf {
        self.app.join("pinvou3-tauri.exe")
    }
    pub(super) fn create_source_root(&self, seed: &Seed) -> Result<ProtectedDirectory> {
        let root = ProtectedDirectory::known_root(&self.program_files)?;
        let parent = root
            .child("Pinvou", true, false, None)?
            .child(seed.bootstrap_source_leaf(), true, false, None)?
            .child(&seed.profile_id, true, false, None)?;
        parent.recheck()?;
        let sd = creation_security(false, None)?;
        // FILE_CREATE only: never open or adopt a leaf that raced creation.
        let directory = parent
            .directory
            .create_dir_with_security(OsStr::new(&seed.build_id), &sd)
            .map_err(custody_error)?;
        let value = ProtectedDirectory {
            directory,
            path: self.source.clone(),
            owned_from: parent.owned_from,
            service_sid: None,
        };
        value.recheck()?;
        Ok(value)
    }
    pub(super) fn source_root(&self) -> Result<ProtectedDirectory> {
        ProtectedDirectory::open_existing(&self.source, None)
    }
    /// Byte staging is separate from installed targets and preparation locks.
    /// The leaf is generated by the private adapter, never supplied over IPC.
    pub(super) fn create_download_stage(
        &self,
        seed: &Seed,
        leaf: &str,
    ) -> Result<ProtectedDirectory> {
        if !leaf.starts_with("bytes-") || !crate::profile::relative_name(leaf) || leaf.contains('/')
        {
            return Err(Error::SecurityRefused);
        }
        // A sibling namespace, not a new entry inside the installed code/state
        // inventories. Normal T31 verify/remove never adopts or scavenges it.
        let namespace = match seed.profile_kind.as_str() {
            "production" => "UpdaterDownloadStaging",
            "lab" => "UpdaterDownloadStagingLab",
            _ => return Err(Error::SecurityRefused),
        };
        let parent = ProtectedDirectory::known_root(&self.program_files)?
            .child("Pinvou", true, false, None)?
            .child(namespace, true, true, None)?
            .child(&seed.profile_id, true, false, None)?;
        parent.recheck()?;
        let security = creation_security(true, None)?;
        let directory = parent
            .directory
            .create_dir_with_security(OsStr::new(leaf), &security)
            .map_err(custody_error)?;
        let value = ProtectedDirectory {
            directory,
            path: parent.path.join(leaf),
            owned_from: parent.owned_from,
            service_sid: None,
        };
        value.recheck()?;
        Ok(value)
    }
    /// Must only be called after original native installation intent committed.
    pub(super) fn create_targets(&self, seed: &Seed, service_sid: Option<&str>) -> Result<Targets> {
        let files = ProtectedDirectory::known_root(&self.program_files)?;
        let data = ProtectedDirectory::known_root(&self.program_data)?;
        let files = files.child("Pinvou", true, false, service_sid)?;
        let data = data.child("Pinvou", true, true, service_sid)?;
        let code = files
            .child(seed.updater_leaf(), true, false, service_sid)?
            .child(&seed.profile_id, true, false, service_sid)?;
        let app = files
            .child(seed.app_leaf(), true, false, service_sid)?
            .child(&seed.profile_id, true, false, service_sid)?;
        let state = data
            .child(seed.updater_leaf(), true, true, service_sid)?
            .child(&seed.profile_id, true, true, service_sid)?;
        Ok(Targets { code, app, state })
    }
    pub(super) fn existing_targets(&self, service_sid: Option<&str>) -> Result<ExistingTargets> {
        fn optional(path: &Path, sid: Option<&str>) -> Result<Option<ProtectedDirectory>> {
            match ProtectedDirectory::open_existing(path, sid) {
                Ok(root) => Ok(Some(root)),
                Err(Error::SourceUnavailable) => Ok(None),
                Err(error) => Err(error),
            }
        }
        Ok(ExistingTargets {
            code: optional(&self.code, service_sid)?,
            app: optional(&self.app, service_sid)?,
            state: optional(&self.state, service_sid)?,
        })
    }
    pub(super) fn any_target_exists(&self) -> Result<bool> {
        for path in [&self.code, &self.app, &self.state] {
            match custody::directory(path) {
                Ok(_) => return Ok(true),
                Err(custody::Error::NotFound) => {}
                Err(error) => return Err(custody_error(error)),
            }
        }
        Ok(false)
    }
    pub(super) fn open_targets_optional(&self, service_sid: Option<&str>) -> Result<Targets> {
        Ok(Targets {
            code: ProtectedDirectory::open_existing(&self.code, service_sid)?,
            app: ProtectedDirectory::open_existing(&self.app, service_sid)?,
            state: ProtectedDirectory::open_existing(&self.state, service_sid)?,
        })
    }
    pub(super) fn client_roots(
        &self,
        service_sid: &str,
    ) -> Result<(ProtectedDirectory, ProtectedDirectory)> {
        Ok((
            ProtectedDirectory::open_existing(&self.code, Some(service_sid))?,
            ProtectedDirectory::open_existing(&self.app, Some(service_sid))?,
        ))
    }
    pub(super) fn open_targets(&self, service_sid: &str) -> Result<Targets> {
        self.open_targets_with_poll(service_sid, &mut || false)
    }
    pub(super) fn open_targets_with_poll(
        &self,
        service_sid: &str,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<Targets> {
        Ok(Targets {
            code: ProtectedDirectory::open_existing_with_poll(
                &self.code,
                Some(service_sid),
                cancelled,
            )?,
            app: ProtectedDirectory::open_existing_with_poll(
                &self.app,
                Some(service_sid),
                cancelled,
            )?,
            state: ProtectedDirectory::open_existing_with_poll(
                &self.state,
                Some(service_sid),
                cancelled,
            )?,
        })
    }
}
pub(super) struct ExistingTargets {
    pub(super) code: Option<ProtectedDirectory>,
    pub(super) app: Option<ProtectedDirectory>,
    pub(super) state: Option<ProtectedDirectory>,
}
fn code_inventory(seed: &Seed, manifest: &crate::profile::InstallationManifest) -> Vec<AppFile> {
    vec![
        AppFile {
            relative_name: format!("helpers/{}/pinvou-upgrade-helper.exe", seed.build_id),
            size: manifest.helper.size,
            sha256: manifest.helper.sha256.clone(),
        },
        AppFile {
            relative_name: "pinvou-upgrade-launcher.exe".into(),
            size: manifest.launcher.size,
            sha256: manifest.launcher.sha256.clone(),
        },
    ]
}
impl ExistingTargets {
    pub(super) fn validate_known_contents(
        &self,
        seed: &Seed,
        manifest: &crate::profile::InstallationManifest,
    ) -> Result<()> {
        if let Some(code) = &self.code {
            code.validate_inventory_mode(&code_inventory(seed, manifest), false)?;
        }
        if let Some(app) = &self.app {
            app.validate_inventory_mode(&manifest.app_files, false)?;
        }
        if let Some(state) = &self.state {
            state.validate_inventory(&[])?;
        }
        Ok(())
    }
    pub(super) fn remove_known(
        self,
        seed: &Seed,
        manifest: &crate::profile::InstallationManifest,
    ) -> Result<()> {
        self.validate_known_contents(seed, manifest)?;
        // Remove fixed executable entry points first. Classic disposition
        // refuses a running image/read lease; no later launch via these names
        // is possible after a successful unlink. Never remove arbitrary files.
        if let Some(app) = &self.app {
            app.remove_member("pinvou3-tauri.exe")?;
        }
        if let Some(code) = &self.code {
            code.remove_member("pinvou-upgrade-launcher.exe")?;
        }
        if let Some(app) = self.app {
            app.remove_known_tree(&manifest.app_files)?;
        }
        if let Some(code) = self.code {
            code.remove_known_tree(&code_inventory(seed, manifest))?;
        }
        if let Some(state) = self.state {
            state.remove_known_tree(&[])?;
        }
        Ok(())
    }
}
pub(super) struct Targets {
    pub(super) code: ProtectedDirectory,
    pub(super) app: ProtectedDirectory,
    pub(super) state: ProtectedDirectory,
}
impl Targets {
    /// This bootstrap profile owns exactly one helper build, one launcher and
    /// the declared app tree. It has no persistent external owner/state files.
    /// Extra versions/namespaces are preserved by refusal, never scavenged.
    pub(super) fn validate_known_contents(
        &self,
        seed: &Seed,
        manifest: &crate::profile::InstallationManifest,
        complete: bool,
    ) -> Result<()> {
        self.validate_known_contents_with_poll(seed, manifest, complete, &mut || false)
    }
    pub(super) fn validate_known_contents_with_poll(
        &self,
        seed: &Seed,
        manifest: &crate::profile::InstallationManifest,
        complete: bool,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<()> {
        crate::download_stage::poll(cancelled)?;
        let code = code_inventory(seed, manifest);
        self.code
            .validate_inventory_mode_with_poll(&code, complete, cancelled)?;
        self.app
            .validate_inventory_mode_with_poll(&manifest.app_files, complete, cancelled)?;
        self.state
            .validate_inventory_mode_with_poll(&[], true, cancelled)?;
        Ok(())
    }
}

/// Private proof holds each actual ancestry object. No public/from-JSON ctor.
pub(super) struct ProtectedDirectory {
    directory: Directory,
    path: PathBuf,
    owned_from: usize,
    service_sid: Option<String>,
}
impl ProtectedDirectory {
    fn known_root(path: &Path) -> Result<Self> {
        verify_local_volume(path)?;
        let directory = custody::directory(path).map_err(custody_error)?;
        let value = Self {
            directory,
            path: path.to_owned(),
            owned_from: usize::MAX,
            service_sid: None,
        };
        value.recheck()?;
        Ok(value)
    }
    fn open_existing(path: &Path, service_sid: Option<&str>) -> Result<Self> {
        Self::open_existing_with_poll(path, service_sid, &mut || false)
    }
    fn open_existing_with_poll(
        path: &Path,
        service_sid: Option<&str>,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<Self> {
        crate::download_stage::poll(cancelled)?;
        verify_local_volume(path)?;
        crate::download_stage::poll(cancelled)?;
        let directory = custody::directory(path).map_err(custody_error)?;
        crate::download_stage::poll(cancelled)?;
        let count = directory.retained_directories().len();
        // Every Pinvou-owned component beginning after KnownFolder must itself
        // be protected, not only the final leaf. OS drive/KnownFolder remain
        // actual inspected ancestors under their native policy.
        let components: Vec<_> = path.components().collect();
        let mut pinvou = None;
        for (index, component) in components.iter().enumerate() {
            crate::download_stage::poll(cancelled)?;
            if component.as_os_str() == OsStr::new("Pinvou") {
                pinvou = Some(index);
                break;
            }
        }
        let pinvou = pinvou.ok_or(Error::SecurityRefused)?;
        // Directory custody collapses Prefix/Root into its first drive handle.
        let owned_from = pinvou.checked_sub(1).ok_or(Error::SecurityRefused)?;
        if owned_from >= count {
            return Err(Error::SecurityRefused);
        }
        let value = Self {
            directory,
            path: path.to_owned(),
            owned_from,
            service_sid: service_sid.map(str::to_owned),
        };
        value.recheck_with_poll(cancelled)?;
        Ok(value)
    }
    pub(super) fn recheck(&self) -> Result<()> {
        self.recheck_with_poll(&mut || false)
    }
    fn recheck_with_poll(&self, cancelled: &mut dyn FnMut() -> bool) -> Result<()> {
        crate::download_stage::poll(cancelled)?;
        self.directory.recheck(&self.path).map_err(custody_error)?;
        crate::download_stage::poll(cancelled)?;
        for (index, file) in self.directory.retained_directories().enumerate() {
            crate::download_stage::poll(cancelled)?;
            inspect_file(file, index >= self.owned_from, self.service_sid.as_deref())?;
            crate::download_stage::poll(cancelled)?;
        }
        Ok(())
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub(super) fn child(
        &self,
        name: &str,
        create: bool,
        private: bool,
        service_sid: Option<&str>,
    ) -> Result<Self> {
        self.child_with_poll(name, create, private, service_sid, &mut || false)
    }
    pub(super) fn child_with_poll(
        &self,
        name: &str,
        create: bool,
        private: bool,
        service_sid: Option<&str>,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<Self> {
        crate::download_stage::poll(cancelled)?;
        if name.contains('/') || !crate::profile::relative_name(name) {
            return Err(Error::SecurityRefused);
        }
        self.recheck_with_poll(cancelled)?;
        let directory = match self.directory.open_dir(OsStr::new(name)) {
            Ok(child) => child,
            Err(custody::Error::NotFound) if create => {
                let sd = creation_security(private, service_sid)?;
                self.directory
                    .create_dir_with_security(OsStr::new(name), &sd)
                    .map_err(custody_error)?
            }
            Err(e) => return Err(custody_error(e)),
        };
        crate::download_stage::poll(cancelled)?;
        let value = Self {
            owned_from: self
                .owned_from
                .min(self.directory.retained_directories().len()),
            directory,
            path: self.path.join(name),
            service_sid: service_sid
                .map(str::to_owned)
                .or_else(|| self.service_sid.clone()),
        };
        value.recheck_with_poll(cancelled)?;
        Ok(value)
    }
    pub(super) fn file(&self, name: &str) -> Result<File> {
        if name.contains('/') || !crate::profile::relative_name(name) {
            return Err(Error::SecurityRefused);
        }
        self.recheck()?;
        let file = self
            .directory
            .file(name, false, false, false)
            .map_err(custody_error)?;
        custody::identity(&file).map_err(custody_error)?;
        inspect_file(&file, true, self.service_sid.as_deref())?;
        Ok(file)
    }
    pub(super) fn remove_file_exact(&self, name: &str, expected: &custody::Identity) -> Result<()> {
        if name.contains('/') || !crate::profile::relative_name(name) {
            return Err(Error::SecurityRefused);
        }
        self.recheck()?;
        self.directory
            .remove_file_exact(OsStr::new(name), expected)
            .map_err(custody_error)?;
        self.recheck()
    }
    fn remove_member(&self, name: &str) -> Result<()> {
        let (parent, leaf) = match self.descend_parent(name, false) {
            Ok(value) => value,
            Err(Error::SourceUnavailable) => return Ok(()),
            Err(error) => return Err(error),
        };
        let parent = parent.as_ref().unwrap_or(self);
        let file = match parent.file(&leaf) {
            Ok(file) => file,
            Err(Error::SourceUnavailable) => return Ok(()),
            Err(error) => return Err(error),
        };
        let identity = custody::identity(&file).map_err(custody_error)?;
        drop(file);
        parent.remove_file_exact(&leaf, &identity)
    }
    fn remove_known_tree(self, files: &[AppFile]) -> Result<()> {
        let expected = catalog_tree(files)?;
        self.validate_inventory_mode(files, false)?;
        fn remove(parent: &ProtectedDirectory, nodes: &[CatalogNode]) -> Result<()> {
            for node in nodes {
                if !node.file {
                    match parent.child(&node.name, false, false, parent.service_sid.as_deref()) {
                        Ok(child) => {
                            remove(&child, &node.children)?;
                            child.remove_empty_self()?;
                        }
                        Err(Error::SourceUnavailable) => {}
                        Err(error) => return Err(error),
                    }
                } else {
                    parent.remove_member(&node.name)?;
                }
            }
            Ok(())
        }
        remove(&self, &expected)?;
        self.remove_empty_self()
    }
    /// Consume only this known owned child after its manifest tree was removed.
    /// Retain the actual parent while releasing OUR child handle before DELETE.
    pub(super) fn remove_empty_self(self) -> Result<()> {
        self.recheck()?;
        let identity = self.directory.directory_identity().map_err(custody_error)?;
        let leaf = self
            .path
            .file_name()
            .ok_or(Error::SecurityRefused)?
            .to_owned();
        let parent = Self::open_existing(
            self.path.parent().ok_or(Error::SecurityRefused)?,
            self.service_sid.as_deref(),
        )?;
        drop(self);
        parent.recheck()?;
        parent
            .directory
            .remove_empty_dir_exact(&leaf, &identity)
            .map_err(custody_error)?;
        parent.recheck()
    }
    pub(super) fn fresh_file(&self, name: &str, private: bool) -> Result<File> {
        if name.contains('/') || !crate::profile::relative_name(name) {
            return Err(Error::SecurityRefused);
        }
        self.recheck()?;
        let sd = creation_security(private, self.service_sid.as_deref())?;
        let file = self
            .directory
            .create_file_with_security(OsStr::new(name), &sd)
            .map_err(custody_error)?;
        inspect_file(&file, true, self.service_sid.as_deref())?;
        Ok(file)
    }
    pub(super) fn relative_file(&self, name: &str) -> Result<File> {
        let (directory, leaf) = self.descend_parent(name, false)?;
        directory.as_ref().unwrap_or(self).file(&leaf)
    }
    pub(super) fn existing_relative_parent(&self, name: &str) -> Result<(Option<Self>, String)> {
        self.descend_parent(name, false)
    }
    pub(super) fn relative_parent(&self, name: &str) -> Result<(Option<Self>, String)> {
        self.descend_parent(name, true)
    }
    fn descend_parent(&self, name: &str, create: bool) -> Result<(Option<Self>, String)> {
        if !crate::profile::relative_name(name) {
            return Err(Error::SecurityRefused);
        }
        let mut parts = name.split('/').peekable();
        let mut directory: Option<Self> = None;
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                return Ok((directory, part.to_owned()));
            }
            let next = directory.as_ref().unwrap_or(self).child(
                part,
                create,
                false,
                self.service_sid.as_deref(),
            )?;
            // Retain ONE complete ancestry rather than a triangular collection
            // of every prefix's cloned handles (resource quota counts handles).
            directory = Some(next);
        }
        Err(Error::SecurityRefused)
    }
}
fn creation_security(private: bool, service_sid: Option<&str>) -> Result<CreationSecurity> {
    if service_sid.is_some_and(|sid| {
        !sid.starts_with("S-1-5-80-")
            || !sid
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'S' || b == b'-')
    }) {
        return Err(Error::SecurityRefused);
    }
    let sid_ace = service_sid
        .map(|sid| format!("(A;OICI;FA;;;{sid})"))
        .unwrap_or_default();
    let users = if private { "" } else { "(A;OICI;GRGX;;;BU)" };
    CreationSecurity::from_sddl(&format!(
        "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA){sid_ace}{users}"
    ))
    .map_err(custody_error)
}

#[allow(unsafe_code)]
pub(super) fn ordinal_equal(left: &str, right: &str) -> Result<bool> {
    let a: Vec<u16> = left.encode_utf16().collect();
    let b: Vec<u16> = right.encode_utf16().collect();
    let result =
        unsafe { CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1) };
    if result == 0 {
        Err(Error::Unavailable)
    } else {
        Ok(result == CSTR_EQUAL)
    }
}

struct CatalogNode {
    name: String,
    file: bool,
    children: Vec<CatalogNode>,
}
fn catalog_tree(files: &[AppFile]) -> Result<Vec<CatalogNode>> {
    catalog_tree_with(files, crate::profile::relative_name)
}
fn catalog_tree_with(files: &[AppFile], valid: fn(&str) -> bool) -> Result<Vec<CatalogNode>> {
    catalog_tree_with_poll(files, valid, &mut || false)
}
fn catalog_tree_with_poll(
    files: &[AppFile],
    valid: fn(&str) -> bool,
    cancelled: &mut dyn FnMut() -> bool,
) -> Result<Vec<CatalogNode>> {
    fn add(
        nodes: &mut Vec<CatalogNode>,
        parts: &[&str],
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<()> {
        crate::download_stage::poll(cancelled)?;
        let mut found = None;
        for (i, node) in nodes.iter().enumerate() {
            crate::download_stage::poll(cancelled)?;
            if ordinal_equal(&node.name, parts[0])? {
                found = Some(i);
                break;
            }
        }
        if let Some(i) = found {
            let node = &mut nodes[i];
            if node.name != parts[0] || node.file || parts.len() == 1 {
                return Err(Error::ImageRefused);
            }
            add(&mut node.children, &parts[1..], cancelled)
        } else {
            let mut node = CatalogNode {
                name: parts[0].to_owned(),
                file: parts.len() == 1,
                children: Vec::new(),
            };
            if parts.len() > 1 {
                add(&mut node.children, &parts[1..], cancelled)?;
            }
            nodes.push(node);
            Ok(())
        }
    }
    let mut tree = Vec::new();
    crate::download_stage::poll(cancelled)?;
    for file in files {
        crate::download_stage::poll(cancelled)?;
        if !valid(&file.relative_name) {
            return Err(Error::ImageRefused);
        }
        add(
            &mut tree,
            &file.relative_name.split('/').collect::<Vec<_>>(),
            cancelled,
        )?;
    }
    crate::download_stage::poll(cancelled)?;
    Ok(tree)
}
/// Tree checks every ancestor, including nonadjacent file/dir collisions.
pub(super) fn validate_native_catalog(files: &[AppFile]) -> Result<()> {
    catalog_tree(files).map(|_| ())
}

/// Match one actual cursor page at a time against the bounded expected tree.
/// Returns ordinary validation, never a protected-directory/native role token.
fn match_directory(directory: &Directory, expected: &[CatalogNode]) -> Result<()> {
    match_directory_contents(directory, expected, true)
}
fn match_directory_contents(
    directory: &Directory,
    expected: &[CatalogNode],
    complete: bool,
) -> Result<()> {
    match_directory_contents_with_poll(directory, expected, complete, &mut || false)
}
fn match_directory_contents_with_poll(
    directory: &Directory,
    expected: &[CatalogNode],
    complete: bool,
    cancelled: &mut dyn FnMut() -> bool,
) -> Result<()> {
    crate::download_stage::poll(cancelled)?;
    let mut seen = vec![false; expected.len()];
    // Custody's callback error is only a stop signal. Preserve typed host
    // cancellation separately so it cannot become an I/O/security refusal.
    let mut stopped = false;
    let result = directory.visit_entries(|entry| {
        callback_poll(cancelled, &mut stopped)?;
        let mut found = None;
        for (index, node) in expected.iter().enumerate() {
            callback_poll(cancelled, &mut stopped)?;
            if ordinal_equal(&entry.name, &node.name).map_err(|_| custody::Error::IoUnavailable)? {
                found = Some(index);
                break;
            }
        }
        let index = found.ok_or(custody::Error::UnsafeObject)?;
        let node = &expected[index];
        if seen[index] || entry.name != node.name || entry.directory == node.file {
            return Err(custody::Error::UnsafeObject);
        }
        if node.file {
            callback_poll(cancelled, &mut stopped)?;
            let file = directory.file(&entry.name, false, false, false)?;
            custody::identity(&file)?; // Actual single-link ordinary file.
            callback_poll(cancelled, &mut stopped)?;
        }
        seen[index] = true;
        callback_poll(cancelled, &mut stopped)
    });
    finish_directory_visit(result, stopped, cancelled)?;
    if complete {
        for present in seen {
            crate::download_stage::poll(cancelled)?;
            if !present {
                return Err(Error::ImageRefused);
            }
        }
    }
    crate::download_stage::poll(cancelled)
}
fn callback_poll(
    cancelled: &mut dyn FnMut() -> bool,
    stopped: &mut bool,
) -> std::result::Result<(), custody::Error> {
    *stopped |= cancelled();
    if *stopped {
        Err(custody::Error::IoUnavailable)
    } else {
        Ok(())
    }
}
fn finish_directory_visit(
    result: std::result::Result<(), custody::Error>,
    stopped: bool,
    cancelled: &mut dyn FnMut() -> bool,
) -> Result<()> {
    if stopped {
        return Err(Error::StagingCanceled);
    }
    crate::download_stage::poll(cancelled)?;
    result.map_err(custody_error)
}
impl ProtectedDirectory {
    pub(super) fn validate_inventory(&self, files: &[AppFile]) -> Result<()> {
        self.validate_inventory_mode(files, true)
    }
    pub(super) fn validate_partial_inventory(&self, files: &[AppFile]) -> Result<()> {
        self.validate_inventory_mode(files, false)
    }
    fn validate_inventory_mode(&self, files: &[AppFile], complete: bool) -> Result<()> {
        self.validate_inventory_mode_with_poll(files, complete, &mut || false)
    }
    fn validate_inventory_mode_with_poll(
        &self,
        files: &[AppFile],
        complete: bool,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<()> {
        let tree = catalog_tree_with_poll(files, crate::profile::relative_name, cancelled)?;
        fn walk(
            root: &ProtectedDirectory,
            path: &str,
            nodes: &[CatalogNode],
            complete: bool,
            cancelled: &mut dyn FnMut() -> bool,
        ) -> Result<()> {
            crate::download_stage::poll(cancelled)?;
            // Close this actual directory before descending. Retaining every
            // prefix's cloned ancestry recursively would grow handles by n².
            {
                let current = if path.is_empty() {
                    None
                } else {
                    let mut directory: Option<ProtectedDirectory> = None;
                    for part in path.split('/') {
                        crate::download_stage::poll(cancelled)?;
                        let next = match directory.as_ref().unwrap_or(root).child_with_poll(
                            part,
                            false,
                            false,
                            root.service_sid.as_deref(),
                            cancelled,
                        ) {
                            Ok(next) => next,
                            Err(Error::SourceUnavailable) if !complete => return Ok(()),
                            Err(error) => return Err(error),
                        };
                        directory = Some(next);
                    }
                    directory
                };
                let current = current.as_ref().unwrap_or(root);
                current.recheck_with_poll(cancelled)?;
                match_directory_contents_with_poll(&current.directory, nodes, complete, cancelled)?;
            }
            for node in nodes {
                crate::download_stage::poll(cancelled)?;
                if node.file {
                    continue;
                }
                let child = if path.is_empty() {
                    node.name.clone()
                } else {
                    format!("{path}/{}", node.name)
                };
                walk(root, &child, &node.children, complete, cancelled)?;
            }
            crate::download_stage::poll(cancelled)
        }
        self.recheck_with_poll(cancelled)?;
        walk(self, "", &tree, complete, cancelled)?;
        self.recheck_with_poll(cancelled)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn files(names: &[&str]) -> Vec<AppFile> {
        names
            .iter()
            .map(|n| AppFile {
                relative_name: (*n).to_owned(),
                size: 0,
                sha256: "0".repeat(64),
            })
            .collect()
    }
    #[test]
    fn catalog_cancellation_interrupts_actual_ancestor_matching() {
        let files: Vec<_> = (0..128)
            .map(|index| AppFile {
                relative_name: format!("group/item-{index}/payload"),
                size: 0,
                sha256: "0".repeat(64),
            })
            .collect();
        assert!(matches!(
            catalog_tree_with_poll(&files, crate::profile::relative_name, &mut || true),
            Err(Error::StagingCanceled)
        ));
        let mut polls = 0;
        assert!(matches!(
            catalog_tree_with_poll(&files, crate::profile::relative_name, &mut || {
                polls += 1;
                polls == 75
            }),
            Err(Error::StagingCanceled)
        ));
        assert_eq!(polls, 75);
        let tree = catalog_tree(&files).unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].children.len(), 128);
    }
    #[test]
    fn custody_callback_cancellation_is_not_an_io_or_security_refusal() {
        let mut stopped = false;
        assert!(callback_poll(&mut || false, &mut stopped).is_ok());
        let sentinel = callback_poll(&mut || true, &mut stopped);
        assert!(sentinel.is_err());
        // The callback may no longer report true on a subsequent invocation;
        // its latched cancellation still owns the result after custody returns.
        assert_eq!(
            finish_directory_visit(sentinel, stopped, &mut || false),
            Err(Error::StagingCanceled)
        );
        assert_eq!(
            finish_directory_visit(Err(custody::Error::UnsafeObject), false, &mut || false),
            Err(Error::SecurityRefused)
        );
        assert_eq!(
            finish_directory_visit(Ok(()), false, &mut || true),
            Err(Error::StagingCanceled)
        );
        assert!(finish_directory_visit(Ok(()), false, &mut || false).is_ok());
    }
    #[test]
    fn actual_ordinal_tree_rejects_every_ancestor_alias() {
        for names in [
            &["a", "a-", "a/file"][..],
            &["a/file", "A/other"][..],
            &["a/file", "a/FILE"][..],
            &["a/file", "a"][..],
        ] {
            assert!(validate_native_catalog(&files(names)).is_err());
        }
        validate_native_catalog(&files(&[
            "a/file",
            "a/other",
            "日本語/a",
            "日本語/b",
            "ordinary$/file",
        ]))
        .unwrap();
        assert!(ordinal_equal("a", "A").unwrap());
    }
    #[test]
    fn known_folder_discovery_does_not_create_a_host() {
        let files = known_folder(&FOLDERID_ProgramFilesX64).unwrap();
        let data = known_folder(&FOLDERID_ProgramData).unwrap();
        assert!(files.is_absolute() && data.is_absolute());
        verify_local_volume(&files).unwrap();
        verify_local_volume(&data).unwrap();
        // Discovery does not create or qualify any Pinvou directory.
    }
    struct OrdinaryTree {
        path: PathBuf,
        parent: PathBuf,
    }
    impl OrdinaryTree {
        fn new() -> Self {
            let parent = std::env::temp_dir().canonicalize().unwrap();
            let path = parent.join(format!(
                "pinvou-t31-ordinary-inventory-{}",
                super::super::pipe::nonce().unwrap()
            ));
            std::fs::create_dir(&path).unwrap();
            let actual = path.canonicalize().unwrap();
            assert_eq!(actual.parent(), Some(parent.as_path()));
            Self {
                path: actual,
                parent,
            }
        }
    }
    impl Drop for OrdinaryTree {
        fn drop(&mut self) {
            // Only this fresh explicitly named child of the captured temp parent.
            // Refuse cleanup if its root was redirected outside that exact parent.
            if self
                .path
                .canonicalize()
                .ok()
                .as_ref()
                .is_some_and(|p| p == &self.path && p.parent() == Some(self.parent.as_path()))
            {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }
    }
    #[test]
    fn actual_inventory_rejects_extra_file_empty_directory_type_missing_and_hardlink() {
        let tree = OrdinaryTree::new();
        let root = tree.path.join("app");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("a")).unwrap();
        for name in ["a/file", "a/other", "日本語.txt", "zero"] {
            std::fs::write(root.join(name), b"").unwrap();
        }
        let expected = catalog_tree(&files(&["a/file", "a/other", "日本語.txt", "zero"])).unwrap();
        {
            let directory = custody::directory(&root).unwrap();
            match_directory(&directory, &expected).unwrap();
            let child = directory.open_dir(OsStr::new("a")).unwrap();
            match_directory(
                &child,
                &expected.iter().find(|n| n.name == "a").unwrap().children,
            )
            .unwrap();
            std::fs::write(root.join("extra"), b"").unwrap();
            assert!(match_directory(&directory, &expected).is_err());
            std::fs::remove_file(root.join("extra")).unwrap();
            match_directory(&directory, &expected).unwrap();
            std::fs::create_dir(root.join("empty-extra")).unwrap();
            assert!(match_directory(&directory, &expected).is_err());
            std::fs::remove_dir(root.join("empty-extra")).unwrap();
            match_directory(&directory, &expected).unwrap();
            std::fs::remove_file(root.join("zero")).unwrap();
            assert!(match_directory(&directory, &expected).is_err());
            std::fs::create_dir(root.join("zero")).unwrap();
            assert!(match_directory(&directory, &expected).is_err());
            std::fs::remove_dir(root.join("zero")).unwrap();
            std::fs::write(root.join("zero"), b"").unwrap();
            match_directory(&directory, &expected).unwrap();
            let external = tree.path.join("external-link");
            std::fs::hard_link(root.join("zero"), &external).unwrap();
            assert!(match_directory(&directory, &expected).is_err());
            std::fs::remove_file(external).unwrap();
            match_directory(&directory, &expected).unwrap();
        }
        // Ordinary enum/hash inputs never construct ProtectedDirectory or a
        // SourceInstallerContext. Each failure restores actual positive state.
    }
    #[test]
    fn actual_inventory_refuses_native_junction_and_case_mismatch() {
        let tree = OrdinaryTree::new();
        let root = tree.path.join("app");
        let outside = tree.path.join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("file"), b"unchanged").unwrap();
        let link = root.join("a");
        let output = std::process::Command::new(r"C:\Windows\System32\cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(
                link.as_os_str()
                    .to_str()
                    .unwrap()
                    .trim_start_matches(r"\\?\"),
            )
            .arg(
                outside
                    .as_os_str()
                    .to_str()
                    .unwrap()
                    .trim_start_matches(r"\\?\"),
            )
            .output()
            .unwrap();
        assert!(output.status.success());
        struct Junction(PathBuf);
        impl Drop for Junction {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir(&self.0);
            }
        }
        let junction = Junction(link.clone());
        let expected = catalog_tree(&files(&["a/file"])).unwrap();
        let directory = custody::directory(&root).unwrap();
        assert!(match_directory(&directory, &expected).is_err());
        assert_eq!(std::fs::read(outside.join("file")).unwrap(), b"unchanged");
        drop(junction);
        std::fs::create_dir(&link).unwrap();
        std::fs::write(link.join("file"), b"").unwrap();
        match_directory(&directory, &expected).unwrap();
        let child = directory.open_dir(OsStr::new("a")).unwrap();
        let children = &expected[0].children;
        match_directory(&child, children).unwrap();
        std::fs::rename(link.join("file"), link.join("FILE")).unwrap();
        assert!(match_directory(&child, children).is_err());
        std::fs::rename(link.join("FILE"), link.join("file")).unwrap();
        match_directory(&child, children).unwrap();
    }
    #[test]
    fn actual_exact_removal_respects_reader_leases_identity_and_link_count() {
        let tree = OrdinaryTree::new();
        let path = tree.path.join("member");
        std::fs::write(&path, b"old").unwrap();
        let directory = custody::directory(&tree.path).unwrap();
        let retained = directory.file("member", false, false, false).unwrap();
        let original = custody::identity(&retained).unwrap();
        assert!(matches!(
            directory.remove_file_exact(OsStr::new("member"), &original),
            Err(custody::Error::Busy)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        drop(retained);
        std::fs::write(tree.path.join("other"), b"other").unwrap();
        let other = directory.file("other", false, false, false).unwrap();
        let wrong = custody::identity(&other).unwrap();
        assert!(matches!(
            directory.remove_file_exact(OsStr::new("member"), &wrong),
            Err(custody::Error::UnsafeObject)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        let link = tree.path.join("second-link");
        std::fs::hard_link(&path, &link).unwrap();
        assert!(matches!(
            directory.remove_file_exact(OsStr::new("member"), &original),
            Err(custody::Error::UnsafeObject)
        ));
        std::fs::remove_file(&link).unwrap();
        directory
            .remove_file_exact(OsStr::new("member"), &original)
            .unwrap();
        assert!(!path.exists());
        std::fs::write(&path, b"replacement").unwrap();
        assert!(
            directory
                .remove_file_exact(OsStr::new("member"), &original)
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    }
    #[test]
    fn actual_empty_directory_removal_requires_closed_handles_exact_identity_and_emptiness() {
        let tree = OrdinaryTree::new();
        let path = tree.path.join("child");
        std::fs::create_dir(&path).unwrap();
        let root = custody::directory(&tree.path).unwrap();
        let child = root.open_dir(OsStr::new("child")).unwrap();
        let original = child.directory_identity().unwrap();
        assert!(matches!(
            root.remove_empty_dir_exact(OsStr::new("child"), &original),
            Err(custody::Error::Busy)
        ));
        drop(child);
        std::fs::write(path.join("unknown"), b"keep").unwrap();
        assert!(
            root.remove_empty_dir_exact(OsStr::new("child"), &original)
                .is_err()
        );
        assert_eq!(std::fs::read(path.join("unknown")).unwrap(), b"keep");
        std::fs::remove_file(path.join("unknown")).unwrap();
        let wrong = custody::directory(&tree.path)
            .unwrap()
            .directory_identity()
            .unwrap();
        assert!(matches!(
            root.remove_empty_dir_exact(OsStr::new("child"), &wrong),
            Err(custody::Error::UnsafeObject)
        ));
        root.remove_empty_dir_exact(OsStr::new("child"), &original)
            .unwrap();
        assert!(!path.exists());
        std::fs::create_dir(&path).unwrap();
        assert!(
            root.remove_empty_dir_exact(OsStr::new("child"), &original)
                .is_err()
        );
        assert!(path.is_dir());
        std::fs::write(tree.path.join("file"), b"keep").unwrap();
        assert!(
            root.remove_empty_dir_exact(OsStr::new("file"), &original)
                .is_err()
        );
        assert_eq!(std::fs::read(tree.path.join("file")).unwrap(), b"keep");
    }
    #[test]
    fn actual_native_cursor_is_paged_and_independent_for_duplicate_readers() {
        use std::sync::Arc;
        let tree = OrdinaryTree::new();
        let mut names = Vec::new();
        for i in 0..256 {
            let name = format!("{i:04}-{}", "x".repeat(200));
            std::fs::write(tree.path.join(&name), b"").unwrap();
            names.push(name);
        }
        let directory = Arc::new(custody::directory(&tree.path).unwrap());
        let one = directory.clone();
        let two = directory.clone();
        let read = |directory: Arc<Directory>| {
            let mut names = Vec::new();
            directory
                .visit_entries(|entry| {
                    assert!(!entry.directory);
                    names.push(entry.name);
                    Ok(())
                })
                .unwrap();
            names.sort();
            names
        };
        let a = std::thread::spawn(move || read(one));
        let b = std::thread::spawn(move || read(two));
        let first = a.join();
        let second = b.join();
        let first = first.unwrap();
        let second = second.unwrap();
        names.sort();
        assert_eq!(first, names);
        assert_eq!(second, names);
        let mut calls = 0;
        assert_eq!(
            directory.visit_entries(|_| {
                calls += 1;
                Err(pinvou_upgrade_custody::Error::InvalidInput)
            }),
            Err(pinvou_upgrade_custody::Error::InvalidInput)
        );
        assert_eq!(calls, 1);
        let mut after = Vec::new();
        directory
            .visit_entries(|entry| {
                after.push(entry.name);
                Ok(())
            })
            .unwrap();
        after.sort();
        assert_eq!(after, names);
        // >64KiB returned directory records and concurrent independent cursors.
    }
}

fn package_name(value: &str) -> bool {
    matches!(
        value,
        "installer.exe" | "pinvou-upgrade-helper.exe" | "pinvou-upgrade-launcher.exe"
    ) || value
        .strip_prefix("app/")
        .is_some_and(crate::profile::relative_name)
}
/// Ordinary input or protected output namespace only. No capability is minted.
pub(super) fn validate_package_inventory(root: &Directory, files: &[AppFile]) -> Result<()> {
    fn walk(root: &Directory, nodes: &[CatalogNode]) -> Result<()> {
        match_directory(root, nodes)?;
        for node in nodes.iter().filter(|n| !n.file) {
            let child = root
                .open_dir(OsStr::new(&node.name))
                .map_err(custody_error)?;
            walk(&child, &node.children)?;
        }
        Ok(())
    }
    let tree = catalog_tree_with(files, package_name)?;
    walk(root, &tree)
}
impl ProtectedDirectory {
    pub(super) fn validate_package_inventory(&self, files: &[AppFile]) -> Result<()> {
        self.recheck()?;
        validate_package_inventory(&self.directory, files)?;
        self.recheck()
    }
}
#[cfg(test)]
mod package_catalog_tests {
    use super::*;
    #[test]
    fn app_prefix_preserves_original_name_budget_and_closes_other_source_paths() {
        let maximum = format!("app/{}", "x".repeat(1024));
        assert!(package_name(&maximum));
        assert!(!package_name(&format!("app/{}", "x".repeat(1025))));
        for bad in [
            "other.exe",
            "app/../outside",
            "APP/file",
            "app/C:/file",
            "app/",
            "installer.exe/child",
        ] {
            assert!(!package_name(bad));
        }
    }
}
