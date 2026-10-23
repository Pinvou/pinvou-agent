//! Explicit elevated local package deployment. Byte custody is not remote trust.
//! Only the subsequently started, protected NSIS -> helper chain owns install intent.
use super::{
    image,
    layout::{self, Layout, ProtectedDirectory},
    process::RetainedProcess,
    registry::{BootstrapGuard, Store},
};
use crate::{
    error::{Error, Result},
    profile::{AppFile, InstallationManifest, Seed},
};
use pinvou_upgrade_custody::{self as custody, Directory};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

struct Member {
    expected: AppFile,
    identity: custody::Identity,
    file: File,
}
struct Package {
    path: PathBuf,
    root: Directory,
    directories: BTreeMap<String, Directory>,
    members: Vec<Member>,
    manifest: InstallationManifest,
    manifest_sha256: String,
}
fn ordinary(error: custody::Error) -> Error {
    layout::custody_error(error)
}
fn catalog(manifest: &InstallationManifest, installer: AppFile) -> Vec<AppFile> {
    let mut files = vec![
        installer,
        AppFile {
            relative_name: "pinvou-upgrade-helper.exe".into(),
            size: manifest.helper.size,
            sha256: manifest.helper.sha256.clone(),
        },
        AppFile {
            relative_name: "pinvou-upgrade-launcher.exe".into(),
            size: manifest.launcher.size,
            sha256: manifest.launcher.sha256.clone(),
        },
    ];
    files.extend(manifest.app_files.iter().map(|file| AppFile {
        relative_name: format!("app/{}", file.relative_name),
        size: file.size,
        sha256: file.sha256.clone(),
    }));
    files
}
fn observe(file: &mut File, name: &str) -> Result<AppFile> {
    let before = custody::identity(file).map_err(ordinary)?;
    let size = file.metadata().map_err(|_| Error::ImageRefused)?.len();
    file.seek(SeekFrom::Start(0))
        .map_err(|_| Error::ImageRefused)?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(|_| Error::ImageRefused)?;
        if n == 0 {
            break;
        }
        total = total.checked_add(n as u64).ok_or(Error::ImageRefused)?;
        if total > size {
            return Err(Error::ImageRefused);
        }
        hash.update(&buffer[..n]);
    }
    if total != size || custody::identity(file).map_err(ordinary)? != before {
        return Err(Error::ImageRefused);
    }
    Ok(AppFile {
        relative_name: name.into(),
        size,
        sha256: hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    })
}
impl Package {
    fn open(current: &RetainedProcess, seed: &Seed) -> Result<Self> {
        if current.image.file_name().and_then(|v| v.to_str()) != Some("pinvou-upgrade-helper.exe") {
            return Err(Error::InstallerContextRefused);
        }
        let path = current
            .image
            .parent()
            .ok_or(Error::InstallerContextRefused)?
            .to_owned();
        layout::validate_package_volume(&path)?;
        let root = custody::directory(&path).map_err(ordinary)?;
        root.recheck(&path).map_err(ordinary)?;
        let mut helper = root
            .file("pinvou-upgrade-helper.exe", false, false, false)
            .map_err(ordinary)?;
        let mut installer = root
            .file("installer.exe", false, false, false)
            .map_err(ordinary)?;
        let installer_reference = observe(&mut installer, "installer.exe")?;
        let installer_identity = custody::identity(&installer).map_err(ordinary)?;
        image::validate_package_machine(&mut installer, installer_reference.size, true)?;
        let inventory = crate::package_resource::inventory(&mut installer)?;
        let manifest = inventory.parse(seed)?;
        let manifest_sha256 = crate::json::sha256(&inventory.root);
        let files = catalog(&manifest, installer_reference);
        image::verify_bytes(&mut helper, manifest.helper.size, &manifest.helper.sha256)?;
        let helper_identity = custody::identity(&helper).map_err(ordinary)?;
        // The executing administrator selects this complete held package snapshot.
        // The filesystem helper is not claimed to be the running image's FileID.
        let mut package = Self {
            path,
            root,
            directories: BTreeMap::new(),
            members: Vec::new(),
            manifest,
            manifest_sha256,
        };
        for expected in files {
            let parts: Vec<_> = expected.relative_name.split('/').collect();
            let mut prefix = String::new();
            for component in &parts[..parts.len() - 1] {
                let parent = prefix.clone();
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(component);
                if !package.directories.contains_key(&prefix) {
                    let parent = package.directories.get(&parent).unwrap_or(&package.root);
                    let directory = parent
                        .open_dir(std::ffi::OsStr::new(component))
                        .map_err(ordinary)?;
                    package.directories.insert(prefix.clone(), directory);
                }
            }
            let mut file = match expected.relative_name.as_str() {
                "installer.exe" => installer.try_clone().map_err(|_| Error::Unavailable)?,
                "pinvou-upgrade-helper.exe" => {
                    helper.try_clone().map_err(|_| Error::Unavailable)?
                }
                _ => package
                    .directories
                    .get(&prefix)
                    .unwrap_or(&package.root)
                    .file(parts[parts.len() - 1], false, false, false)
                    .map_err(ordinary)?,
            };
            let identity = image::verify_bytes(&mut file, expected.size, &expected.sha256)?;
            if expected.relative_name == "installer.exe" && identity != installer_identity
                || expected.relative_name == "pinvou-upgrade-helper.exe"
                    && identity != helper_identity
            {
                return Err(Error::ImageRefused);
            }
            if matches!(
                expected.relative_name.as_str(),
                "pinvou-upgrade-helper.exe"
                    | "pinvou-upgrade-launcher.exe"
                    | "app/pinvou3-tauri.exe"
            ) {
                image::verify_package_role(
                    &mut file,
                    expected.size,
                    &expected.sha256,
                    seed,
                    false,
                )?;
            } else if expected.relative_name == "installer.exe" {
                image::verify_package_role(&mut file, expected.size, &expected.sha256, seed, true)?;
            }
            package.members.push(Member {
                expected,
                identity,
                file,
            });
        }
        package.recheck()?;
        Ok(package)
    }
    fn files(&self) -> Vec<AppFile> {
        self.members
            .iter()
            .map(|m| AppFile {
                relative_name: m.expected.relative_name.clone(),
                size: m.expected.size,
                sha256: m.expected.sha256.clone(),
            })
            .collect()
    }
    fn recheck(&self) -> Result<()> {
        self.root.recheck(&self.path).map_err(ordinary)?;
        for (name, directory) in &self.directories {
            directory
                .recheck(&self.path.join(name.replace('/', "\\")))
                .map_err(ordinary)?;
        }
        layout::validate_package_inventory(&self.root, &self.files())?;
        for member in &self.members {
            if custody::identity(&member.file).map_err(ordinary)? != member.identity {
                return Err(Error::ImageRefused);
            }
        }
        Ok(())
    }
    fn verify_all(&mut self) -> Result<()> {
        self.recheck()?;
        for member in &mut self.members {
            if image::verify_bytes(
                &mut member.file,
                member.expected.size,
                &member.expected.sha256,
            )? != member.identity
            {
                return Err(Error::ImageRefused);
            }
        }
        self.recheck()
    }
    fn check_registration(&self, record: &crate::registration::HostRegistration) -> Result<()> {
        let installer = self
            .members
            .iter()
            .find(|m| m.expected.relative_name == "installer.exe")
            .ok_or(Error::ImageRefused)?;
        if matches!(
            record.phase,
            crate::registration::Phase::Removing | crate::registration::Phase::RemovedTombstone
        ) || record.genesis_manifest_sha256 != self.manifest_sha256
            || record.source_set.nsis_sha256 != installer.expected.sha256
            || record.helper != self.manifest.helper
            || record.launcher != self.manifest.launcher
            || record.app != self.manifest.app
        {
            return Err(Error::ScopeConflict);
        }
        Ok(())
    }
}
fn copy_member(member: &mut Member, destination: &ProtectedDirectory) -> Result<()> {
    if image::verify_bytes(
        &mut member.file,
        member.expected.size,
        &member.expected.sha256,
    )? != member.identity
    {
        return Err(Error::ImageRefused);
    }
    let (directory, leaf) = member_parent(destination, &member.expected.relative_name, true)?;
    let parent = directory.as_ref().unwrap_or(destination);
    // Fresh source only. No overwrite, truncate, repair, delete or ACL conversion.
    let mut output = parent.fresh_file(&leaf, false)?;
    member
        .file
        .seek(SeekFrom::Start(0))
        .map_err(|_| Error::ImageRefused)?;
    copy_bytes(&mut member.file, &mut output, member.expected.size)?;
    output.sync_all().map_err(|_| Error::Unavailable)?;
    let identity = custody::identity(&output).map_err(ordinary)?;
    if custody::identity(&member.file).map_err(ordinary)? != member.identity {
        return Err(Error::ImageRefused);
    }
    drop(output);
    let mut actual = parent.file(&leaf)?;
    if image::verify_bytes(&mut actual, member.expected.size, &member.expected.sha256)? != identity
    {
        return Err(Error::ImageRefused);
    }
    Ok(())
}
fn copy_bytes(input: &mut impl Read, output: &mut impl Write, size: u64) -> Result<()> {
    let mut total = 0u64;
    let mut buffer = [0; 65536];
    loop {
        let n = input.read(&mut buffer).map_err(|_| Error::ImageRefused)?;
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
    if total != size {
        return Err(Error::ImageRefused);
    }
    Ok(())
}
fn member_parent(
    root: &ProtectedDirectory,
    name: &str,
    create: bool,
) -> Result<(Option<ProtectedDirectory>, String)> {
    if let Some(relative) = name.strip_prefix("app/") {
        let app = root.child("app", create, false, None)?;
        let (parent, leaf) = if create {
            app.relative_parent(relative)?
        } else {
            app.existing_relative_parent(relative)?
        };
        Ok((Some(parent.unwrap_or(app)), leaf))
    } else if matches!(
        name,
        "installer.exe" | "pinvou-upgrade-helper.exe" | "pinvou-upgrade-launcher.exe"
    ) {
        Ok((None, name.into()))
    } else {
        Err(Error::ImageRefused)
    }
}
struct Staged {
    root: ProtectedDirectory,
    members: Vec<Member>,
    parents: Vec<ProtectedDirectory>,
}
impl Staged {
    fn open(root: ProtectedDirectory, expected: &[AppFile]) -> Result<Self> {
        root.validate_package_inventory(expected)?;
        let mut members = Vec::new();
        let mut parents = Vec::new();
        for expected in expected {
            let (parent, leaf) = member_parent(&root, &expected.relative_name, false)?;
            let mut file = parent.as_ref().unwrap_or(&root).file(&leaf)?;
            let identity = image::verify_bytes(&mut file, expected.size, &expected.sha256)?;
            if let Some(parent) = parent {
                parents.push(parent);
            }
            members.push(Member {
                file,
                identity,
                expected: AppFile {
                    relative_name: expected.relative_name.clone(),
                    size: expected.size,
                    sha256: expected.sha256.clone(),
                },
            });
        }
        Ok(Self {
            root,
            members,
            parents,
        })
    }
    fn recheck(&mut self) -> Result<()> {
        self.root.recheck()?;
        for parent in &self.parents {
            parent.recheck()?;
        }
        for member in &mut self.members {
            if image::verify_bytes(
                &mut member.file,
                member.expected.size,
                &member.expected.sha256,
            )? != member.identity
            {
                return Err(Error::ImageRefused);
            }
        }
        self.root.validate_package_inventory(
            &self
                .members
                .iter()
                .map(|m| AppFile {
                    relative_name: m.expected.relative_name.clone(),
                    size: m.expected.size,
                    sha256: m.expected.sha256.clone(),
                })
                .collect::<Vec<_>>(),
        )
    }
}
pub(super) fn install_local() -> Result<u32> {
    let seed = Seed::embedded()?;
    // This is an explicit already-elevated administrator command, never a remote
    // update receipt or an instruction to create another elevation prompt.
    let current = RetainedProcess::current()?;
    let token = current.fresh_token(None)?;
    if !token.elevated
        || !token.administrator
        || token.integrity < 0x3000
        || token.app_container
        || token.network
        || token.user.sid == "S-1-5-18"
    {
        return Err(Error::InstallerContextRefused);
    }
    let mut package = Package::open(&current, &seed)?;
    // Complete input objects, inventory and bytes are checked before creating
    // any source directory. This local operation creates no Store/SCM intent.
    package.verify_all()?;
    let store = Store::new(&seed);
    let before = store.read(&seed)?;
    if let Some(record) = &before {
        package.check_registration(record)?;
    }
    let layout = Layout::discover(&seed)?;
    let mut staged = {
        let _guard = BootstrapGuard::acquire(&seed, false)?;
        let actual = store.read(&seed)?;
        if before.as_ref().map(|r| r.digest()).transpose()?
            != actual.as_ref().map(|r| r.digest()).transpose()?
        {
            return Err(Error::ScopeConflict);
        }
        if let Some(record) = &actual {
            package.check_registration(record)?;
        }
        package.verify_all()?;
        let root = match layout.source_root() {
            Ok(root) => root, // Existing root must already be complete and exact.
            Err(Error::SourceUnavailable) => {
                if actual.is_some() {
                    return Err(Error::SourceUnavailable);
                }
                let root = layout.create_source_root(&seed)?;
                for member in &mut package.members {
                    copy_member(member, &root)?;
                }
                root
            }
            Err(error) => return Err(error),
        };
        let mut staged = Staged::open(root, &package.files())?;
        if let Some(record) = &actual {
            let installer = staged
                .members
                .iter()
                .find(|m| m.expected.relative_name == "installer.exe")
                .ok_or(Error::ImageRefused)?;
            if installer.identity != record.original_installer_identity.source_nsis_file_identity {
                return Err(Error::ScopeConflict);
            }
        }
        staged.recheck()?;
        package.verify_all()?;
        staged
    }; // The original bootstrap mutex must be released before NSIS can use it.
    let installer_path = layout.source.join("installer.exe");
    // All sealed source members and parents remain held through loader reopening
    // and the complete natural installer execution. No shell/search-path command.
    let code = super::launch::run_local_installer(&current, &installer_path, || staged.recheck())?;
    // Once the original installer has returned, its status is authoritative.
    // No later observation replaces a nonzero original result or invents a
    // successful install. The held objects are released by normal RAII below.
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_copy_preserves_zero_bytes_and_multi_buffer_files() {
        for bytes in [Vec::new(), vec![17; 65537]] {
            let mut output = Vec::new();
            copy_bytes(&mut &bytes[..], &mut output, bytes.len() as u64).unwrap();
            assert_eq!(output, bytes);
        }
    }
    #[test]
    fn truncated_excess_and_io_failed_copies_never_succeed() {
        for (bytes, expected) in [(b"short".as_slice(), 6), (b"excess".as_slice(), 5)] {
            let mut output = Vec::new();
            assert!(copy_bytes(&mut &bytes[..], &mut output, expected).is_err());
            assert!(output.len() as u64 <= expected);
        }
        struct ReadFailure;
        impl Read for ReadFailure {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::Other.into())
            }
        }
        struct WriteFailure;
        impl Write for WriteFailure {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::Other.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert!(copy_bytes(&mut ReadFailure, &mut Vec::new(), 1).is_err());
        assert!(copy_bytes(&mut &b"a"[..], &mut WriteFailure, 1).is_err());
    }
}
