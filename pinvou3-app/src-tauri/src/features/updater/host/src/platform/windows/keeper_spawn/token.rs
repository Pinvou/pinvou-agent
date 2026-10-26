//! Same-primary restricted keeper token, obtained only from the current actual
//! System/service process. Restricting BU+IU deliberately excludes serviceSID:
//! installed code/state trees grant that SID full access. This is not a sandbox.
use super::*;
use crate::platform::windows::process::{NativeUser, token_facts};
use std::mem::{offset_of, size_of};
use windows_sys::Win32::{
    Foundation::{ERROR_INSUFFICIENT_BUFFER, LUID, LocalFree},
    Security::{
        Authorization::ConvertStringSidToSidW, CreateRestrictedToken, GetTokenInformation,
        LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, SID_AND_ATTRIBUTES,
        TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_GROUPS, TOKEN_INFORMATION_CLASS,
        TOKEN_PRIVILEGES, TOKEN_QUERY, TOKEN_STATISTICS, TokenGroups, TokenPrimary,
        TokenPrivileges, TokenRestrictedSids, TokenStatistics,
    },
    System::{
        SystemServices::{SE_GROUP_ENABLED, SE_GROUP_USE_FOR_DENY_ONLY},
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

const USERS: &str = "S-1-5-32-545";
const INTERACTIVE: &str = "S-1-5-4";
const ADMINISTRATORS: &str = "S-1-5-32-544";

/// Fixed current-process query token, checked against the actual current SCM
/// service. This does not read private state or mint an assignable source token.
#[allow(unsafe_code)]
pub(super) fn verify_current_restricted(service_sid: &str) -> Result<()> {
    let seed = crate::profile::Seed::embedded()?;
    let layout = crate::platform::windows::layout::Layout::discover(&seed)?;
    let public = Store::new(&seed).read_public(&seed)?;
    if public.service_identity.service_sid.as_deref() != Some(service_sid) {
        return Err(Error::ServiceIdentityRefused);
    }
    let service = crate::platform::windows::scm::live_process(&seed, &layout)?;
    if service.token.user.sid != "S-1-5-18"
        || service.token.user.session_id != 0
        || !service.token.service_sid_enabled
        || service.token.app_container
        || service.token.network
    {
        return Err(Error::ServiceIdentityRefused);
    }
    let mut raw = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
        return Err(Error::SecurityRefused);
    }
    let verifier = RestrictedKeeperToken {
        token: Handle::owned(raw)?,
        source_user: service.token.user,
        service_sid: service_sid.to_owned(),
    };
    verifier.verify(&verifier.token)
}

/// Query an existing retained keeper primary token. Recovery neither constructs
/// an assignable token nor enables launch privileges. The source identity is the
/// authenticated current System SCM service; the old service need not be alive.
#[allow(unsafe_code)]
pub(super) fn verify_recovered_process(
    host: &StartedServiceContext,
    service_sid: &str,
    process: &Handle,
) -> Result<()> {
    let record = current(host)?;
    if record.service_identity.service_sid.as_deref() != Some(service_sid)
        || host.context.process.token.user.sid != "S-1-5-18"
        || host.context.process.token.user.session_id != 0
        || !host.context.process.token.service_sid_enabled
        || host.context.process.token.network
        || host.context.process.token.app_container
    {
        return Err(Error::ServiceIdentityRefused);
    }
    let mut raw = ptr::null_mut();
    if unsafe { OpenProcessToken(process.raw(), TOKEN_QUERY, &mut raw) } == 0 {
        return Err(Error::SecurityRefused);
    }
    let verifier = RestrictedKeeperToken {
        token: Handle::owned(raw)?,
        source_user: host.context.process.token.user.clone(),
        service_sid: service_sid.to_owned(),
    };
    verifier.verify(&verifier.token)
}

pub(super) struct RestrictedKeeperToken {
    token: Handle,
    source_user: NativeUser,
    pub(super) service_sid: String,
}
struct LocalSid(*mut std::ffi::c_void);
impl Drop for LocalSid {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
impl LocalSid {
    #[allow(unsafe_code)]
    fn new(text: &str) -> Result<Self> {
        let text: Vec<_> = text.encode_utf16().chain(Some(0)).collect();
        let mut sid = ptr::null_mut();
        if unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut sid) } == 0 || sid.is_null() {
            return Err(Error::SecurityRefused);
        }
        Ok(Self(sid))
    }
}
struct TokenBuffer {
    words: Vec<usize>,
    length: usize,
}
impl TokenBuffer {
    #[allow(unsafe_code)]
    fn read(token: &Handle, class: TOKEN_INFORMATION_CLASS) -> Result<Self> {
        let mut required = 0;
        if unsafe { GetTokenInformation(token.raw(), class, ptr::null_mut(), 0, &mut required) }
            != 0
            || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER
            || required == 0
            || required > 65_536
        {
            return Err(Error::SecurityRefused);
        }
        let mut words = vec![0usize; (required as usize).div_ceil(size_of::<usize>())];
        let capacity = words.len() * size_of::<usize>();
        if unsafe {
            GetTokenInformation(
                token.raw(),
                class,
                words.as_mut_ptr().cast(),
                capacity as u32,
                &mut required,
            )
        } == 0
            || required as usize > capacity
        {
            return Err(Error::SecurityRefused);
        }
        Ok(Self {
            words,
            length: required as usize,
        })
    }
    #[allow(unsafe_code)]
    fn typed<T>(&self) -> Result<&T> {
        if self.length < size_of::<T>() {
            return Err(Error::SecurityRefused);
        }
        Ok(unsafe { &*self.words.as_ptr().cast::<T>() })
    }
    #[allow(unsafe_code)]
    fn sid(&self, sid: *mut std::ffi::c_void) -> Result<String> {
        let start = self.words.as_ptr() as usize;
        let end = start
            .checked_add(self.length)
            .ok_or(Error::SecurityRefused)?;
        let address = sid as usize;
        if address < start || address.checked_add(8).is_none_or(|n| n > end) {
            return Err(Error::SecurityRefused);
        }
        let count = unsafe { *sid.cast::<u8>().add(1) } as usize;
        if count > 15 || address.checked_add(8 + 4 * count).is_none_or(|n| n > end) {
            return Err(Error::SecurityRefused);
        }
        security::sid_string(sid)
    }
    fn array_extent<T>(&self, offset: usize, count: u32) -> Result<()> {
        if count > 4096
            || (count as usize)
                .checked_mul(size_of::<T>())
                .and_then(|n| offset.checked_add(n))
                .is_none_or(|n| n > self.length)
        {
            return Err(Error::SecurityRefused);
        }
        Ok(())
    }
}
fn same_luid(a: LUID, b: LUID) -> bool {
    a.LowPart == b.LowPart && a.HighPart == b.HighPart
}
#[allow(unsafe_code)]
fn change_notify() -> Result<LUID> {
    let name: Vec<_> = "SeChangeNotifyPrivilege\0".encode_utf16().collect();
    let mut value = LUID::default();
    if unsafe { LookupPrivilegeValueW(ptr::null(), name.as_ptr(), &mut value) } == 0 {
        return Err(Error::Unavailable);
    }
    Ok(value)
}
#[allow(unsafe_code)]
fn privileges_except_notify(source: &Handle) -> Result<Vec<LUID_AND_ATTRIBUTES>> {
    let buffer = TokenBuffer::read(source, TokenPrivileges)?;
    let privileges = buffer.typed::<TOKEN_PRIVILEGES>()?;
    buffer.array_extent::<LUID_AND_ATTRIBUTES>(
        offset_of!(TOKEN_PRIVILEGES, Privileges),
        privileges.PrivilegeCount,
    )?;
    let notify = change_notify()?;
    let mut delete = Vec::new();
    for i in 0..privileges.PrivilegeCount as usize {
        let privilege = unsafe { *privileges.Privileges.as_ptr().add(i) };
        if !same_luid(privilege.Luid, notify) {
            delete.push(LUID_AND_ATTRIBUTES {
                Luid: privilege.Luid,
                Attributes: 0,
            });
        }
    }
    Ok(delete)
}
impl RestrictedKeeperToken {
    #[allow(unsafe_code)]
    pub(super) fn from_service(host: &StartedServiceContext, service_sid: &str) -> Result<Self> {
        let mut raw = ptr::null_mut();
        if unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY,
                &mut raw,
            )
        } == 0
        {
            return Err(Error::SecurityRefused);
        }
        let source = Handle::owned(raw)?;
        let facts = token_facts(&source, Some(service_sid))?;
        if facts.user != host.context.process.token.user
            || facts.user.sid != "S-1-5-18"
            || facts.user.session_id != 0
            || !facts.service_sid_enabled
            || facts.network
            || facts.app_container
        {
            return Err(Error::ServiceIdentityRefused);
        }
        if TokenBuffer::read(&source, TokenStatistics)?
            .typed::<TOKEN_STATISTICS>()?
            .TokenType
            != TokenPrimary
        {
            return Err(Error::SecurityRefused);
        }
        let administrators = LocalSid::new(ADMINISTRATORS)?;
        let users = LocalSid::new(USERS)?;
        let interactive = LocalSid::new(INTERACTIVE)?;
        let disabled = [SID_AND_ATTRIBUTES {
            Sid: administrators.0,
            Attributes: 0,
        }];
        let restricting = [
            SID_AND_ATTRIBUTES {
                Sid: users.0,
                Attributes: 0,
            },
            SID_AND_ATTRIBUTES {
                Sid: interactive.0,
                Attributes: 0,
            },
        ];
        // Explicit deletion prevents a future source privilege from merely
        // remaining disabled but re-enableable in the child. No privilege is added.
        let delete = privileges_except_notify(&source)?;
        let mut token = ptr::null_mut();
        if unsafe {
            CreateRestrictedToken(
                source.raw(),
                0,
                disabled.len() as u32,
                disabled.as_ptr(),
                delete.len() as u32,
                delete.as_ptr(),
                restricting.len() as u32,
                restricting.as_ptr(),
                &mut token,
            )
        } == 0
        {
            return Err(Error::SecurityRefused);
        }
        let value = Self {
            token: Handle::owned(token)?,
            source_user: facts.user,
            service_sid: service_sid.to_owned(),
        };
        value.verify(&value.token)?;
        Ok(value)
    }
    pub(super) fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.token.raw()
    }
    #[allow(unsafe_code)]
    pub(super) fn verify_process(&self, process: &Handle) -> Result<()> {
        let mut token = ptr::null_mut();
        if unsafe { OpenProcessToken(process.raw(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(Error::SecurityRefused);
        }
        self.verify(&Handle::owned(token)?)
    }
    #[allow(unsafe_code)]
    fn verify(&self, token: &Handle) -> Result<()> {
        let facts = token_facts(token, Some(&self.service_sid))?;
        if facts.user != self.source_user
            || facts.administrator
            || !facts.service_sid_enabled
            || facts.app_container
            || facts.network
        {
            return Err(Error::SecurityRefused);
        }
        if TokenBuffer::read(token, TokenStatistics)?
            .typed::<TOKEN_STATISTICS>()?
            .TokenType
            != TokenPrimary
        {
            return Err(Error::SecurityRefused);
        }
        let groups_buffer = TokenBuffer::read(token, TokenGroups)?;
        let groups = groups_buffer.typed::<TOKEN_GROUPS>()?;
        groups_buffer.array_extent::<SID_AND_ATTRIBUTES>(
            offset_of!(TOKEN_GROUPS, Groups),
            groups.GroupCount,
        )?;
        let mut administrators_deny_only = false;
        for i in 0..groups.GroupCount as usize {
            let group = unsafe { &*groups.Groups.as_ptr().add(i) };
            if groups_buffer.sid(group.Sid)? == ADMINISTRATORS {
                administrators_deny_only = group.Attributes & SE_GROUP_USE_FOR_DENY_ONLY as u32
                    != 0
                    && group.Attributes & SE_GROUP_ENABLED as u32 == 0;
            }
        }
        if !administrators_deny_only {
            return Err(Error::SecurityRefused);
        }
        let restricted_buffer = TokenBuffer::read(token, TokenRestrictedSids)?;
        let groups = restricted_buffer.typed::<TOKEN_GROUPS>()?;
        restricted_buffer.array_extent::<SID_AND_ATTRIBUTES>(
            offset_of!(TOKEN_GROUPS, Groups),
            groups.GroupCount,
        )?;
        if groups.GroupCount != 2 {
            return Err(Error::SecurityRefused);
        }
        let mut restricted = Vec::new();
        for i in 0..2 {
            restricted
                .push(restricted_buffer.sid(unsafe { (*groups.Groups.as_ptr().add(i)).Sid })?);
        }
        restricted.sort();
        if restricted
            .iter()
            .map(String::as_str)
            .ne([USERS, INTERACTIVE])
        {
            return Err(Error::SecurityRefused);
        }
        let privileges_buffer = TokenBuffer::read(token, TokenPrivileges)?;
        let privileges = privileges_buffer.typed::<TOKEN_PRIVILEGES>()?;
        privileges_buffer.array_extent::<LUID_AND_ATTRIBUTES>(
            offset_of!(TOKEN_PRIVILEGES, Privileges),
            privileges.PrivilegeCount,
        )?;
        if privileges.PrivilegeCount != 1
            || !same_luid(privileges.Privileges[0].Luid, change_notify()?)
            || privileges.Privileges[0].Attributes & SE_PRIVILEGE_ENABLED == 0
        {
            return Err(Error::SecurityRefused);
        }
        Ok(())
    }
}
