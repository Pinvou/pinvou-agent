use std::{ffi::OsStr, os::windows::ffi::OsStrExt, path::Path};

use windows_sys::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, LocalFree},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            },
            DACL_SECURITY_INFORMATION, GetLengthSid, GetTokenInformation, PSECURITY_DESCRIPTOR,
            PSID, SECURITY_ATTRIBUTES, SetFileSecurityW, TOKEN_GROUPS, TOKEN_QUERY, TOKEN_USER,
            TokenGroups, TokenUser,
        },
        System::{
            SystemServices::SE_GROUP_LOGON_ID,
            Threading::{GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken},
        },
    },
    core::PWSTR,
};

use crate::ControllerError;

pub struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl SecurityDescriptor {
    pub fn for_current_logon() -> Result<Self, ControllerError> {
        let sid = current_logon_sid()?;
        let sid_text = sid_to_string(sid.as_ptr().cast_mut().cast())?;
        let sddl = format!("D:P(A;;GA;;;{sid_text})(A;;GA;;;SY)");
        let wide = wide(&sddl);
        let mut descriptor = std::ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            Err(ControllerError::Io(std::io::Error::last_os_error()))
        } else {
            Ok(Self(descriptor))
        }
    }

    fn for_current_user() -> Result<Self, ControllerError> {
        let sid = current_user_sid()?;
        let sid_text = sid_to_string(sid.as_ptr().cast_mut().cast())?;
        let sddl = format!("D:P(A;;GA;;;{sid_text})(A;;GA;;;SY)");
        let wide = wide(&sddl);
        let mut descriptor = std::ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            Err(ControllerError::Io(std::io::Error::last_os_error()))
        } else {
            Ok(Self(descriptor))
        }
    }

    pub fn attributes(&mut self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0.cast(),
            bInheritHandle: 0,
        }
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0.cast()) };
    }
}

pub fn current_logon_sid_string() -> Result<String, ControllerError> {
    let sid = current_logon_sid()?;
    sid_to_string(sid.as_ptr().cast_mut().cast())
}

pub fn peer_is_current_logon(pipe: HANDLE) -> Result<bool, ControllerError> {
    use windows_sys::Win32::{Security::RevertToSelf, System::Pipes::ImpersonateNamedPipeClient};
    if unsafe { ImpersonateNamedPipeClient(pipe) } == 0 {
        return Err(ControllerError::Io(std::io::Error::last_os_error()));
    }
    let result = (|| {
        let mut token = std::ptr::null_mut();
        if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) } == 0 {
            return Err(ControllerError::Io(std::io::Error::last_os_error()));
        }
        let _guard = HandleGuard(token);
        Ok(token_logon_sid(token)? == current_logon_sid()?)
    })();
    if unsafe { RevertToSelf() } == 0 {
        return Err(ControllerError::Io(std::io::Error::last_os_error()));
    }
    result
}

pub fn apply_current_logon_dacl(path: &Path) -> Result<(), ControllerError> {
    let descriptor = SecurityDescriptor::for_current_user()?;
    let path = wide(path.as_os_str());
    let info = DACL_SECURITY_INFORMATION;
    let ok = unsafe { SetFileSecurityW(path.as_ptr(), info, descriptor.0) };
    if ok == 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(5) {
            Ok(())
        } else {
            Err(ControllerError::Io(error))
        }
    } else {
        Ok(())
    }
}

fn current_logon_sid() -> Result<Vec<u8>, ControllerError> {
    let mut token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(ControllerError::Io(std::io::Error::last_os_error()));
    }
    let _guard = HandleGuard(token);
    token_logon_sid(token)
}

fn current_user_sid() -> Result<Vec<u8>, ControllerError> {
    let mut token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(ControllerError::Io(std::io::Error::last_os_error()));
    }
    let _guard = HandleGuard(token);
    let mut required = 0;
    unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut required) };
    if required == 0 {
        return Err(ControllerError::Io(std::io::Error::last_os_error()));
    }
    let word_count = (required as usize).div_ceil(std::mem::size_of::<usize>());
    let mut storage = vec![0_usize; word_count];
    if unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            storage.as_mut_ptr().cast(),
            required,
            &mut required,
        )
    } == 0
    {
        return Err(ControllerError::Io(std::io::Error::last_os_error()));
    }
    let user = unsafe { &*(storage.as_ptr().cast::<TOKEN_USER>()) };
    let length = unsafe { GetLengthSid(user.User.Sid) } as usize;
    if length == 0 {
        return Err(ControllerError::InvalidMessage);
    }
    Ok(unsafe { std::slice::from_raw_parts(user.User.Sid.cast::<u8>(), length) }.to_vec())
}

fn token_logon_sid(token: HANDLE) -> Result<Vec<u8>, ControllerError> {
    let mut required = 0;
    unsafe { GetTokenInformation(token, TokenGroups, std::ptr::null_mut(), 0, &mut required) };
    if required == 0 {
        return Err(ControllerError::Io(std::io::Error::last_os_error()));
    }
    // TOKEN_GROUPS contains pointer-aligned entries. A Vec<u8> does not
    // guarantee enough alignment for casting its allocation to TOKEN_GROUPS.
    let word_count = (required as usize).div_ceil(std::mem::size_of::<usize>());
    let mut storage = vec![0_usize; word_count];
    if unsafe {
        GetTokenInformation(
            token,
            TokenGroups,
            storage.as_mut_ptr().cast(),
            required,
            &mut required,
        )
    } == 0
    {
        return Err(ControllerError::Io(std::io::Error::last_os_error()));
    }
    let groups = unsafe { &*(storage.as_ptr().cast::<TOKEN_GROUPS>()) };
    let entries =
        unsafe { std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize) };
    let entry = entries
        .iter()
        .find(|entry| entry.Attributes & (SE_GROUP_LOGON_ID as u32) == SE_GROUP_LOGON_ID as u32)
        .ok_or(ControllerError::InvalidMessage)?;
    let length = unsafe { GetLengthSid(entry.Sid) } as usize;
    if length == 0 {
        return Err(ControllerError::InvalidMessage);
    }
    Ok(unsafe { std::slice::from_raw_parts(entry.Sid.cast::<u8>(), length) }.to_vec())
}

fn sid_to_string(sid: PSID) -> Result<String, ControllerError> {
    let mut pointer: PWSTR = std::ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut pointer) } == 0 {
        return Err(ControllerError::Io(std::io::Error::last_os_error()));
    }
    let mut length = 0;
    while unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    let value = String::from_utf16(unsafe { std::slice::from_raw_parts(pointer, length) })
        .map_err(|_| ControllerError::InvalidMessage);
    unsafe { LocalFree(pointer.cast()) };
    value
}

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

struct HandleGuard(HANDLE);
impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}
