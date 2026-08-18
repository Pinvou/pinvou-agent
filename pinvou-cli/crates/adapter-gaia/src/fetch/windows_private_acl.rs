use std::ffi::c_void;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, SE_FILE_OBJECT, SetNamedSecurityInfoW,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, ACL_SIZE_INFORMATION, AclSizeInformation,
    AddAccessAllowedAceEx, CONTAINER_INHERIT_ACE, CreateWellKnownSid, DACL_SECURITY_INFORMATION,
    EqualSid, GetAclInformation, GetLengthSid, GetSecurityDescriptorControl, GetTokenInformation,
    INHERITED_ACE, InitializeAcl, OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
    SECURITY_MAX_SID_SIZE, TOKEN_QUERY, TOKEN_USER, TokenUser,
    UNPROTECTED_DACL_SECURITY_INFORMATION, WinBuiltinAdministratorsSid, WinLocalSystemSid,
};
use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub(super) fn ace_flags(directory: bool) -> u32 {
    if directory {
        CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE
    } else {
        0
    }
}

pub(super) fn apply_and_verify(path: &Path, directory: bool) -> Result<(), ()> {
    let wide = wide_path(path);
    let current = current_user_sid()?;
    let system = well_known_sid(WinLocalSystemSid)?;
    let administrators = well_known_sid(WinBuiltinAdministratorsSid)?;
    let sids = [
        current.as_psid(),
        system.as_psid(),
        administrators.as_psid(),
    ];
    let expected = [
        current.as_bytes().to_vec(),
        system.as_bytes().to_vec(),
        administrators.as_bytes().to_vec(),
    ];
    let acl = build_acl(&sids, ace_flags(directory))?;
    let previous = SecuritySnapshot::read(&wide)?;

    let result = unsafe {
        SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            current.as_psid(),
            std::ptr::null_mut(),
            acl.as_acl(),
            std::ptr::null(),
        )
    };
    if result != ERROR_SUCCESS {
        return Err(());
    }
    if verify(&wide, directory, &expected, current.as_psid()).is_err() {
        let _ = previous.restore(&wide);
        return Err(());
    }
    Ok(())
}

fn verify(wide: &[u16], directory: bool, expected: &[Vec<u8>; 3], owner: PSID) -> Result<(), ()> {
    let snapshot = SecuritySnapshot::read(wide)?;
    if snapshot.control & SE_DACL_PROTECTED == 0 || unsafe { EqualSid(snapshot.owner, owner) } == 0
    {
        return Err(());
    }
    let acl = copy_acl_bytes(snapshot.dacl)?;
    verify_acl_bytes(&acl, directory, expected)
}

fn copy_acl_bytes(acl: *mut ACL) -> Result<Vec<u8>, ()> {
    if acl.is_null() {
        return Err(());
    }
    let mut info = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    if unsafe {
        GetAclInformation(
            acl,
            (&mut info as *mut ACL_SIZE_INFORMATION).cast::<c_void>(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
        || info.AceCount != 3
        || info.AclBytesInUse < size_of::<ACL>() as u32
        || info.AclBytesInUse > u16::MAX as u32
    {
        return Err(());
    }
    let bytes = unsafe {
        std::slice::from_raw_parts(acl.cast::<u8>(), info.AclBytesInUse as usize).to_vec()
    };
    Ok(bytes)
}

fn verify_acl_bytes(acl: &[u8], directory: bool, expected: &[Vec<u8>; 3]) -> Result<(), ()> {
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
    if acl.len() < size_of::<ACL>()
        || read_u16(acl, 2)? as usize != acl.len()
        || read_u16(acl, 4)? != 3
    {
        return Err(());
    }
    let expected_flags = ace_flags(directory) as u8;
    let mut matched = [false; 3];
    let mut offset = size_of::<ACL>();
    for _ in 0..3 {
        let header_end = offset.checked_add(4).ok_or(())?;
        let header = acl.get(offset..header_end).ok_or(())?;
        let ace_size = u16::from_le_bytes([header[2], header[3]]) as usize;
        let ace_end = offset.checked_add(ace_size).ok_or(())?;
        let ace = acl.get(offset..ace_end).ok_or(())?;
        if header[0] != ACCESS_ALLOWED_ACE_TYPE
            || header[1] & INHERITED_ACE as u8 != 0
            || header[1] != expected_flags
            || ace.len() < 16
            || read_u32(ace, 4)? != FILE_ALL_ACCESS
        {
            return Err(());
        }
        let sid = ace.get(8..).ok_or(())?;
        let sub_authorities = *sid.get(1).ok_or(())? as usize;
        if sid.first() != Some(&1)
            || 8_usize
                .checked_add(sub_authorities.checked_mul(4).ok_or(())?)
                .ok_or(())?
                != sid.len()
        {
            return Err(());
        }
        let Some(position) = expected
            .iter()
            .position(|expected_sid| expected_sid.as_slice() == sid)
        else {
            return Err(());
        };
        if matched[position] {
            return Err(());
        }
        matched[position] = true;
        offset = ace_end;
    }
    if offset == acl.len() && matched.into_iter().all(|value| value) {
        Ok(())
    } else {
        Err(())
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ()> {
    let raw: [u8; 2] = bytes
        .get(offset..offset + 2)
        .ok_or(())?
        .try_into()
        .map_err(|_| ())?;
    Ok(u16::from_le_bytes(raw))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ()> {
    let raw: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or(())?
        .try_into()
        .map_err(|_| ())?;
    Ok(u32::from_le_bytes(raw))
}

struct SecuritySnapshot {
    descriptor: PSECURITY_DESCRIPTOR,
    owner: PSID,
    dacl: *mut ACL,
    control: u16,
}

impl SecuritySnapshot {
    fn read(wide: &[u16]) -> Result<Self, ()> {
        let mut owner = std::ptr::null_mut();
        let mut dacl = std::ptr::null_mut();
        let mut descriptor = std::ptr::null_mut();
        let result = unsafe {
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        if result != ERROR_SUCCESS || descriptor.is_null() || owner.is_null() || dacl.is_null() {
            if !descriptor.is_null() {
                unsafe { LocalFree(descriptor) };
            }
            return Err(());
        }
        let mut control = 0_u16;
        let mut revision = 0_u32;
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
            unsafe { LocalFree(descriptor) };
            return Err(());
        }
        Ok(Self {
            descriptor,
            owner,
            dacl,
            control,
        })
    }

    fn restore(&self, wide: &[u16]) -> Result<(), ()> {
        let protection = if self.control & SE_DACL_PROTECTED != 0 {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };
        let result = unsafe {
            SetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | protection,
                self.owner,
                std::ptr::null_mut(),
                self.dacl,
                std::ptr::null(),
            )
        };
        if result == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(())
        }
    }
}

impl Drop for SecuritySnapshot {
    fn drop(&mut self) {
        if !self.descriptor.is_null() {
            unsafe { LocalFree(self.descriptor) };
        }
    }
}

struct SidBuffer {
    words: Vec<usize>,
    byte_len: usize,
}

impl SidBuffer {
    fn with_bytes(bytes: usize) -> Self {
        Self {
            words: vec![0; bytes.div_ceil(size_of::<usize>())],
            byte_len: bytes,
        }
    }

    fn as_psid(&self) -> PSID {
        self.words.as_ptr() as PSID
    }

    fn as_mut_psid(&mut self) -> PSID {
        self.words.as_mut_ptr() as PSID
    }

    fn as_bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.words.as_ptr().cast::<u8>(), self.byte_len) }
    }
}

fn current_user_sid() -> Result<SidBuffer, ()> {
    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(());
    }
    let result = (|| {
        let mut needed = 0_u32;
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
        if needed == 0 {
            return Err(());
        }
        let mut token_data = SidBuffer::with_bytes(needed as usize);
        if unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                token_data.as_mut_psid(),
                needed,
                &mut needed,
            )
        } == 0
        {
            return Err(());
        }
        let token_user = unsafe { &*(token_data.as_psid() as *const TOKEN_USER) };
        copy_sid(token_user.User.Sid)
    })();
    unsafe { CloseHandle(token) };
    result
}

fn well_known_sid(kind: i32) -> Result<SidBuffer, ()> {
    let mut size = SECURITY_MAX_SID_SIZE;
    let mut sid = SidBuffer::with_bytes(size as usize);
    if unsafe { CreateWellKnownSid(kind, std::ptr::null_mut(), sid.as_mut_psid(), &mut size) } == 0
    {
        return Err(());
    }
    sid.byte_len = size as usize;
    Ok(sid)
}

fn copy_sid(source: PSID) -> Result<SidBuffer, ()> {
    let length = unsafe { GetLengthSid(source) };
    if length == 0 || length > SECURITY_MAX_SID_SIZE {
        return Err(());
    }
    let mut sid = SidBuffer::with_bytes(length as usize);
    unsafe {
        std::ptr::copy_nonoverlapping(
            source as *const u8,
            sid.as_mut_psid() as *mut u8,
            length as usize,
        )
    };
    Ok(sid)
}

struct AclBuffer {
    words: Vec<usize>,
}

impl AclBuffer {
    fn as_acl(&self) -> *mut ACL {
        self.words.as_ptr() as *mut ACL
    }
}

fn build_acl(sids: &[PSID; 3], flags: u32) -> Result<AclBuffer, ()> {
    let fixed_ace = size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>();
    let bytes = size_of::<ACL>()
        + sids
            .iter()
            .map(|sid| fixed_ace + unsafe { GetLengthSid(*sid) } as usize)
            .sum::<usize>();
    let acl = AclBuffer {
        words: vec![0; bytes.div_ceil(size_of::<usize>())],
    };
    if unsafe { InitializeAcl(acl.as_acl(), bytes as u32, ACL_REVISION) } == 0 {
        return Err(());
    }
    for sid in sids {
        if unsafe {
            AddAccessAllowedAceEx(acl.as_acl(), ACL_REVISION, flags, FILE_ALL_ACCESS, *sid)
        } == 0
        {
            return Err(());
        }
    }
    Ok(acl)
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::verify_acl_bytes;

    #[test]
    fn truncated_ace_is_rejected_without_reading_past_the_acl() {
        let mut acl = vec![0_u8; 12];
        let acl_len = acl.len() as u16;
        acl[2..4].copy_from_slice(&acl_len.to_le_bytes());
        acl[4..6].copy_from_slice(&1_u16.to_le_bytes());
        acl[8] = 0;
        acl[10..12].copy_from_slice(&u16::MAX.to_le_bytes());

        let expected = [Vec::new(), Vec::new(), Vec::new()];
        assert!(verify_acl_bytes(&acl, false, &expected).is_err());
    }
}
