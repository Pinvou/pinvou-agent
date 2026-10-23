//! Fixed installed Windows Explorer provenance, not package/publisher approval.
//! WRP images have legitimate WinSxS hardlinks. This sealed OS-only constructor
//! does not weaken the ordinary custody/package single-link requirements.
use super::{layout::ordinal_equal, security::inspect_file};
use crate::error::{Error, Result};
use std::{
    fs::{File, OpenOptions},
    mem::size_of,
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
    ptr,
};
use windows_sys::Win32::{
    Security::{Cryptography::Catalog::*, WinTrust::*},
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, GetFileInformationByHandle,
    },
    System::ApplicationInstallationAndServicing::SfcIsFileProtected,
    System::Com::CoTaskMemFree,
    UI::Shell::{FOLDERID_Windows, SHGetKnownFolderPath},
};

#[derive(PartialEq, Eq)]
struct Identity {
    volume: u32,
    high: u32,
    low: u32,
    created_high: u32,
    created_low: u32,
    links: u32,
}
fn wide(value: &Path) -> Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    let value: Vec<_> = value.as_os_str().encode_wide().collect();
    if value.contains(&0) || value.len() >= 32768 {
        return Err(Error::OptionalLaunchUnavailable);
    }
    Ok(value.into_iter().chain(Some(0)).collect())
}
#[allow(unsafe_code)]
fn identity(file: &File) -> Result<Identity> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
        || info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
        || info.nNumberOfLinks == 0
    {
        return Err(Error::OptionalLaunchUnavailable);
    }
    Ok(Identity {
        volume: info.dwVolumeSerialNumber,
        high: info.nFileIndexHigh,
        low: info.nFileIndexLow,
        created_high: info.ftCreationTime.dwHighDateTime,
        created_low: info.ftCreationTime.dwLowDateTime,
        links: info.nNumberOfLinks,
    })
}
#[allow(unsafe_code)]
fn windows_root() -> Result<PathBuf> {
    let mut raw = ptr::null_mut();
    if unsafe { SHGetKnownFolderPath(&FOLDERID_Windows, 0, ptr::null_mut(), &mut raw) } < 0
        || raw.is_null()
    {
        return Err(Error::OptionalLaunchUnavailable);
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
    let _memory = Memory(raw);
    let mut length = 0;
    while length < 32768 && unsafe { *raw.add(length) } != 0 {
        length += 1;
    }
    if length == 0 || length == 32768 {
        return Err(Error::OptionalLaunchUnavailable);
    }
    let path = PathBuf::from(
        String::from_utf16(unsafe { std::slice::from_raw_parts(raw, length) })
            .map_err(|_| Error::OptionalLaunchUnavailable)?,
    );
    if !path.is_absolute() {
        return Err(Error::OptionalLaunchUnavailable);
    }
    Ok(path)
}
/// Only the installed OS catalog namespace has CryptSvc as a native writer.
/// This SID is resolved from the fixed local virtual service account; no caller
/// identity or policy string can add a principal to host/source file security.
#[allow(unsafe_code)]
fn cryptsvc_sid() -> Result<String> {
    use windows_sys::Win32::Security::LookupAccountNameW;
    let account: Vec<u16> = "NT SERVICE\\CryptSvc"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut size = 0;
    let mut domain_size = 0;
    let mut kind = 0;
    unsafe {
        LookupAccountNameW(
            ptr::null(),
            account.as_ptr(),
            ptr::null_mut(),
            &mut size,
            ptr::null_mut(),
            &mut domain_size,
            &mut kind,
        );
    }
    if !(8..=68).contains(&size) || domain_size > 256 {
        return Err(Error::OptionalLaunchUnavailable);
    }
    let mut sid = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    let mut domain = vec![0u16; domain_size as usize];
    if unsafe {
        LookupAccountNameW(
            ptr::null(),
            account.as_ptr(),
            sid.as_mut_ptr().cast(),
            &mut size,
            domain.as_mut_ptr(),
            &mut domain_size,
            &mut kind,
        )
    } == 0
        || size as usize > sid.len() * size_of::<usize>()
    {
        return Err(Error::OptionalLaunchUnavailable);
    }
    let value = super::security::sid_string(sid.as_mut_ptr().cast())?;
    if !value.starts_with("S-1-5-80-") {
        return Err(Error::OptionalLaunchUnavailable);
    }
    Ok(value)
}
struct OsFile {
    file: File,
    parents: pinvou_upgrade_custody::Directory,
    identity: Identity,
    catalog_policy: Option<(usize, String)>,
}
impl OsFile {
    fn open(path: &Path, catalog_root: Option<&Path>) -> Result<Self> {
        let catalog_policy = match catalog_root {
            Some(root) => {
                if !under_windows(path, root)? {
                    return Err(Error::OptionalLaunchUnavailable);
                }
                Some((
                    root.components()
                        .count()
                        .checked_sub(2)
                        .ok_or(Error::OptionalLaunchUnavailable)?,
                    cryptsvc_sid()?,
                ))
            }
            None => None,
        };
        let parents = pinvou_upgrade_custody::directory(
            path.parent().ok_or(Error::OptionalLaunchUnavailable)?,
        )
        .map_err(|_| Error::OptionalLaunchUnavailable)?;
        for (index, parent) in parents.retained_directories().enumerate() {
            inspect_file(
                parent,
                false,
                catalog_policy
                    .as_ref()
                    .and_then(|(from, sid)| (index >= *from).then_some(sid.as_str())),
            )?;
        }
        // This is the one fixed OS-only file bridge. Retained real ancestry
        // denies replacement while CreateFile reopens the exact native path.
        // The leaf itself is opened no-follow and denies every writer/deleter.
        let file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|_| Error::OptionalLaunchUnavailable)?;
        inspect_file(
            &file,
            false,
            catalog_policy.as_ref().map(|(_, sid)| sid.as_str()),
        )?;
        let id = identity(&file)?;
        Ok(Self {
            file,
            parents,
            identity: id,
            catalog_policy,
        })
    }
    fn recheck(&self) -> Result<()> {
        for (index, parent) in self.parents.retained_directories().enumerate() {
            inspect_file(
                parent,
                false,
                self.catalog_policy
                    .as_ref()
                    .and_then(|(from, sid)| (index >= *from).then_some(sid.as_str())),
            )?;
        }
        inspect_file(
            &self.file,
            false,
            self.catalog_policy.as_ref().map(|(_, sid)| sid.as_str()),
        )?;
        if identity(&self.file)? != self.identity {
            return Err(Error::OptionalLaunchUnavailable);
        }
        Ok(())
    }
}
struct Admin(isize);
impl Drop for Admin {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe {
            CryptCATAdminReleaseContext(self.0, 0);
        }
    }
}
struct Catalog<'a> {
    admin: &'a Admin,
    handle: isize,
}
impl Drop for Catalog<'_> {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe {
            CryptCATAdminReleaseCatalogContext(self.admin.0, self.handle, 0);
        }
    }
}
struct Verification<'a> {
    data: Box<WINTRUST_DATA>,
    _info: Box<WINTRUST_CATALOG_INFO>,
    _member: &'a File,
    _catalog: &'a OsFile,
    _admin: &'a Admin,
    action: windows_sys::core::GUID,
}
impl Drop for Verification<'_> {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        self.data.dwStateAction = WTD_STATEACTION_CLOSE;
        unsafe {
            WinVerifyTrust(
                (-1isize) as *mut _,
                &mut self.action,
                (&mut *self.data as *mut WINTRUST_DATA).cast(),
            );
        }
    }
}
fn under_windows(path: &Path, windows: &Path) -> Result<bool> {
    let mut actual = path.components();
    for expected in windows.components() {
        let Some(found) = actual.next() else {
            return Ok(false);
        };
        if !ordinal_equal(
            found
                .as_os_str()
                .to_str()
                .ok_or(Error::OptionalLaunchUnavailable)?,
            expected
                .as_os_str()
                .to_str()
                .ok_or(Error::OptionalLaunchUnavailable)?,
        )? {
            return Ok(false);
        }
    }
    Ok(actual.next().is_some()
        && !path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        }))
}
#[allow(unsafe_code)]
fn catalog_member(member: &File, path: &Path, windows: &Path) -> Result<()> {
    let algorithm: [u16; 7] = [83, 72, 65, 50, 53, 54, 0];
    let mut raw = 0;
    if unsafe {
        CryptCATAdminAcquireContext2(&mut raw, ptr::null(), algorithm.as_ptr(), ptr::null(), 0)
    } == 0
        || raw == 0
    {
        return Err(Error::OptionalLaunchUnavailable);
    }
    let admin = Admin(raw);
    let mut length = 0u32;
    unsafe {
        CryptCATAdminCalcHashFromFileHandle2(
            admin.0,
            member.as_raw_handle(),
            &mut length,
            ptr::null_mut(),
            0,
        );
    }
    if length != 32 {
        return Err(Error::OptionalLaunchUnavailable);
    }
    let mut hash = vec![0u8; length as usize];
    if unsafe {
        CryptCATAdminCalcHashFromFileHandle2(
            admin.0,
            member.as_raw_handle(),
            &mut length,
            hash.as_mut_ptr(),
            0,
        )
    } == 0
        || length != 32
    {
        return Err(Error::OptionalLaunchUnavailable);
    }
    let tag: Vec<u16> = hash
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let member_path = wide(path)?;
    // At most sixteen actual catalog contexts/bounded SDK filenames. Retain
    // every returned context until enumeration is finished, then release each
    // before its original CATAdmin owner. No catalog install/download fallback.
    let mut catalogs = Vec::<Catalog<'_>>::new();
    let mut previous = 0isize;
    for index in 0..16 {
        let handle = unsafe {
            CryptCATAdminEnumCatalogFromHash(
                admin.0,
                hash.as_ptr(),
                length,
                0,
                if index == 0 {
                    ptr::null_mut()
                } else {
                    &mut previous
                },
            )
        };
        if handle == 0 {
            return Err(Error::OptionalLaunchUnavailable);
        }
        previous = handle;
        catalogs.push(Catalog {
            admin: &admin,
            handle,
        });
        let mut catalog_info = CATALOG_INFO {
            cbStruct: size_of::<CATALOG_INFO>() as u32,
            ..Default::default()
        };
        if unsafe { CryptCATCatalogInfoFromContext(handle, &mut catalog_info, 0) } == 0 {
            return Err(Error::OptionalLaunchUnavailable);
        }
        let count = catalog_info
            .wszCatalogFile
            .iter()
            .position(|unit| *unit == 0)
            .ok_or(Error::OptionalLaunchUnavailable)?;
        let catalog_path = PathBuf::from(
            String::from_utf16(&catalog_info.wszCatalogFile[..count])
                .map_err(|_| Error::OptionalLaunchUnavailable)?,
        );
        if !under_windows(&catalog_path, windows)? {
            continue;
        }
        let catalog_file = OsFile::open(
            &catalog_path,
            Some(&windows.join("System32").join("CatRoot")),
        )?;
        let catalog_wide = wide(&catalog_path)?;
        let mut info = Box::new(WINTRUST_CATALOG_INFO {
            cbStruct: size_of::<WINTRUST_CATALOG_INFO>() as u32,
            pcwszCatalogFilePath: catalog_wide.as_ptr(),
            pcwszMemberTag: tag.as_ptr(),
            pcwszMemberFilePath: member_path.as_ptr(),
            hMemberFile: member.as_raw_handle(),
            pbCalculatedFileHash: hash.as_mut_ptr(),
            cbCalculatedFileHash: length,
            hCatAdmin: admin.0,
            ..Default::default()
        });
        let data = Box::new(WINTRUST_DATA {
            cbStruct: size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_NONE,
            dwUnionChoice: WTD_CHOICE_CATALOG,
            Anonymous: WINTRUST_DATA_0 {
                pCatalog: &mut *info,
            },
            dwStateAction: WTD_STATEACTION_VERIFY,
            dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_DISABLE_MD2_MD4,
            ..Default::default()
        });
        let mut verification = Verification {
            data,
            _info: info,
            _member: member,
            _catalog: &catalog_file,
            _admin: &admin,
            action: WINTRUST_ACTION_GENERIC_VERIFY_V2,
        };
        let status = unsafe {
            WinVerifyTrust(
                (-1isize) as *mut _,
                &mut verification.action,
                (&mut *verification.data as *mut WINTRUST_DATA).cast(),
            )
        };
        drop(verification);
        catalog_file.recheck()?;
        if status == 0 {
            return Ok(());
        }
    }
    Err(Error::OptionalLaunchUnavailable)
}
pub(super) struct Explorer {
    path: PathBuf,
    image: OsFile,
}
impl Explorer {
    #[allow(unsafe_code)]
    pub(super) fn open() -> Result<Self> {
        Self::checked(&windows_root()?.join("explorer.exe"))
    }
    #[allow(unsafe_code)]
    fn checked(path: &Path) -> Result<Self> {
        let windows = windows_root()?;
        let expected = windows.join("explorer.exe");
        if !ordinal_equal(
            path.to_str().ok_or(Error::OptionalLaunchUnavailable)?,
            expected.to_str().ok_or(Error::OptionalLaunchUnavailable)?,
        )? {
            return Err(Error::OptionalLaunchUnavailable);
        }
        let path = expected;
        let image = OsFile::open(&path, None)?;
        if unsafe { SfcIsFileProtected(ptr::null_mut(), wide(&path)?.as_ptr()) } == 0 {
            return Err(Error::OptionalLaunchUnavailable);
        }
        catalog_member(&image.file, &path, &windows)?;
        image.recheck()?;
        Ok(Self { path, image })
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub(super) fn recheck(&self) -> Result<()> {
        self.image.recheck()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "explicit actual Windows catalog/WRP positive; no complete certificate cache assumption"]
    #[allow(unsafe_code)]
    fn actual_fixed_explorer_catalog_positive_and_changed_non_os_bytes_refused() {
        let windows = windows_root().unwrap();
        let path = windows.join("explorer.exe");
        let image =
            OsFile::open(&path, None).expect("actual protected Explorer image and ancestors");
        assert_ne!(
            unsafe { SfcIsFileProtected(ptr::null_mut(), wide(&path).unwrap().as_ptr()) },
            0,
            "actual WRP membership"
        );
        catalog_member(&image.file, &path, &windows)
            .expect("actual protected matching Windows catalog");
        drop(image);
        let explorer = Explorer::open().unwrap();
        explorer.recheck().unwrap();
        let root = std::env::temp_dir().join(format!(
            "pinvou-os-member-{}",
            super::super::pipe::nonce().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        struct Temp(PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(self.0.join("ordinary.exe"));
                let _ = std::fs::remove_dir(&self.0);
            }
        }
        let _temp = Temp(root.clone());
        let copy = root.join("ordinary.exe");
        std::fs::copy(explorer.path(), &copy).unwrap();
        assert!(!under_windows(&copy, &windows).unwrap());
        assert!(matches!(
            Explorer::checked(&copy),
            Err(Error::OptionalLaunchUnavailable)
        ));
        assert_eq!(
            unsafe { SfcIsFileProtected(ptr::null_mut(), wide(&copy).unwrap().as_ptr()) },
            0
        );
        let file = File::open(&copy).unwrap();
        catalog_member(&file, &copy, &windows)
            .expect("same ordinary copy has genuine catalog member bytes");
        drop(file);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&copy)
            .unwrap();
        use std::io::{Read, Seek, SeekFrom, Write};
        file.seek(SeekFrom::Start(100)).unwrap();
        let mut original = [0];
        file.read_exact(&mut original).unwrap();
        file.seek(SeekFrom::Start(100)).unwrap();
        file.write_all(&[original[0] ^ 1]).unwrap();
        file.sync_all().unwrap();
        drop(file);
        let file = File::open(&copy).unwrap();
        assert!(catalog_member(&file, &copy, &windows).is_err());
        drop(file);
        let mut file = OpenOptions::new().write(true).open(&copy).unwrap();
        file.seek(SeekFrom::Start(100)).unwrap();
        file.write_all(&original).unwrap();
        file.sync_all().unwrap();
        drop(file);
        let file = File::open(&copy).unwrap();
        catalog_member(&file, &copy, &windows)
            .expect("exact restored bytes recover the same catalog member positive");
        drop(file);
        explorer.recheck().unwrap();
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
        fn handles() -> u32 {
            let mut value = 0;
            assert_ne!(
                unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut value) },
                0
            );
            value
        }
        for _ in 0..3 {
            drop(Explorer::open().unwrap());
        }
        let before = handles();
        for _ in 0..20 {
            drop(Explorer::open().unwrap());
        }
        assert!(
            handles() <= before,
            "catalog/WinTrust native handles retained after owners dropped"
        );
    }
}
