use crate::NodeError;
use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
use windows_sys::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, LocalFree},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            },
            GetLengthSid, GetTokenInformation, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
            TOKEN_GROUPS, TOKEN_QUERY, TokenGroups,
        },
        System::{
            SystemServices::SE_GROUP_LOGON_ID,
            Threading::{GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken},
        },
    },
    core::PWSTR,
};

pub struct SecurityDescriptor(PSECURITY_DESCRIPTOR);
impl SecurityDescriptor {
    pub fn for_current_logon() -> Result<Self, NodeError> {
        let sid = current_logon_sid()?;
        let text = sid_to_string(sid.as_ptr().cast_mut().cast())?;
        let wide = wide(&format!("D:P(A;;GA;;;{text})(A;;GA;;;SY)"));
        let mut value = std::ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                1,
                &mut value,
                std::ptr::null_mut(),
            )
        } == 0
        {
            Err(NodeError::Io(std::io::Error::last_os_error()))
        } else {
            Ok(Self(value))
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

pub fn peer_is_current_logon(pipe: HANDLE) -> Result<bool, NodeError> {
    use windows_sys::Win32::{Security::RevertToSelf, System::Pipes::ImpersonateNamedPipeClient};
    if unsafe { ImpersonateNamedPipeClient(pipe) } == 0 {
        return Err(NodeError::Io(std::io::Error::last_os_error()));
    }
    let result = (|| {
        let mut token = std::ptr::null_mut();
        if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) } == 0 {
            return Err(NodeError::Io(std::io::Error::last_os_error()));
        }
        let _guard = HandleGuard(token);
        Ok(token_logon_sid(token)? == current_logon_sid()?)
    })();
    if unsafe { RevertToSelf() } == 0 {
        return Err(NodeError::Io(std::io::Error::last_os_error()));
    }
    result
}
fn current_logon_sid() -> Result<Vec<u8>, NodeError> {
    let mut token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(NodeError::Io(std::io::Error::last_os_error()));
    }
    let _guard = HandleGuard(token);
    token_logon_sid(token)
}
fn token_logon_sid(token: HANDLE) -> Result<Vec<u8>, NodeError> {
    let mut required = 0;
    unsafe { GetTokenInformation(token, TokenGroups, std::ptr::null_mut(), 0, &mut required) };
    if required == 0 {
        return Err(NodeError::Io(std::io::Error::last_os_error()));
    }
    let word_count = (required as usize).div_ceil(std::mem::size_of::<usize>());
    let mut bytes = vec![0usize; word_count];
    if unsafe {
        GetTokenInformation(
            token,
            TokenGroups,
            bytes.as_mut_ptr().cast(),
            required,
            &mut required,
        )
    } == 0
    {
        return Err(NodeError::Io(std::io::Error::last_os_error()));
    }
    let groups = unsafe { &*bytes.as_ptr().cast::<TOKEN_GROUPS>() };
    let entries =
        unsafe { std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize) };
    let entry = entries
        .iter()
        .find(|entry| entry.Attributes & (SE_GROUP_LOGON_ID as u32) == SE_GROUP_LOGON_ID as u32)
        .ok_or(NodeError::InvalidMessage)?;
    let length = unsafe { GetLengthSid(entry.Sid) } as usize;
    if length == 0 {
        return Err(NodeError::InvalidMessage);
    }
    Ok(unsafe { std::slice::from_raw_parts(entry.Sid.cast::<u8>(), length) }.to_vec())
}
fn sid_to_string(sid: PSID) -> Result<String, NodeError> {
    let mut pointer: PWSTR = std::ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut pointer) } == 0 {
        return Err(NodeError::Io(std::io::Error::last_os_error()));
    }
    let mut length = 0;
    while unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    let result = String::from_utf16(unsafe { std::slice::from_raw_parts(pointer, length) })
        .map_err(|_| NodeError::InvalidMessage);
    unsafe { LocalFree(pointer.cast()) };
    result
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
