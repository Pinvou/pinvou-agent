//! Native token custody for the fixed CLI creation owner. This module exposes
//! no process creation, arbitrary PID selector, IPC grant, or installer action.
//! The caller must still own the verified executable and its startup handles.
use super::{
    handle::Handle,
    installed::StartedServiceContext,
    peer::Peer,
    pipe::Pipe,
    process::{TokenFacts, token_facts},
    registry::Store,
};
use crate::{
    error::{Error, Result},
    handoff::Role,
};
use std::{marker::PhantomData, ptr, rc::Rc};
use windows_sys::Win32::{
    Foundation::{ERROR_NO_TOKEN, GetLastError, HANDLE, LUID, SetLastError},
    Security::{
        AdjustTokenPrivileges, DuplicateTokenEx, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW,
        RevertToSelf, SE_PRIVILEGE_ENABLED, SecurityImpersonation, TOKEN_ADJUST_PRIVILEGES,
        TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_PRIVILEGES, TOKEN_QUERY,
        TokenImpersonation, TokenPrimary,
    },
    System::Threading::{
        GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken, SetThreadToken,
    },
};

pub(super) fn same_requester(a: &TokenFacts, b: &TokenFacts) -> bool {
    a.user == b.user
        && a.integrity == b.integrity
        && a.elevated == b.elevated
        && a.administrator == b.administrator
        && a.interactive == b.interactive
        && a.network == b.network
        && a.app_container == b.app_container
}

/// Primary token duplicated only from the same authenticated live pipe peer.
/// No caller-provided process ID, username, token handle or identity enters.
pub(super) struct RequesterToken(Handle);
impl RequesterToken {
    #[allow(unsafe_code)]
    pub(super) fn capture(host: &StartedServiceContext, peer: &Peer, pipe: &Pipe) -> Result<Self> {
        if !matches!(peer.role, Role::Launcher | Role::CliRequester) {
            return Err(Error::PeerRefused);
        }
        peer.recheck(pipe)?;
        let actual = Store::new(&host.context.seed)
            .read(&host.context.seed)?
            .ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&actual)?;
        let mut raw = ptr::null_mut();
        if unsafe { OpenProcessToken(peer.process.raw(), TOKEN_QUERY | TOKEN_DUPLICATE, &mut raw) }
            == 0
        {
            return Err(Error::PeerRefused);
        }
        let original = Handle::owned(raw)?;
        let observed = token_facts(&original, None)?;
        if !same_requester(&observed, &peer.process.token)
            || observed.network
            || observed.app_container
            || observed.user.sid == "S-1-5-18"
            || observed.integrity < 0x2000
        {
            return Err(Error::PeerRefused);
        }
        let mut duplicate = ptr::null_mut();
        if unsafe {
            DuplicateTokenEx(
                original.raw(),
                TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY,
                ptr::null(),
                SecurityImpersonation,
                TokenPrimary,
                &mut duplicate,
            )
        } == 0
        {
            return Err(Error::PeerRefused);
        }
        let token = Handle::owned(duplicate)?;
        if !same_requester(&token_facts(&token, None)?, &observed) {
            return Err(Error::PeerRefused);
        }
        peer.recheck(pipe)?;
        host.authenticate_running(&actual)?;
        Ok(Self(token))
    }
    pub(super) fn raw(&self) -> HANDLE {
        self.0.raw()
    }
}

/// Thread-affine System impersonation token containing only the three approved
/// enabled privileges. Never enables them on the shared service process token.
/// A failed revert is fatal, as continuing under unintended impersonation is
/// unsafe. Existing service installations lacking the privileges fail closed.
pub(super) struct ScopedCreationPrivileges<'input> {
    _token: Handle,
    _input: PhantomData<&'input ()>,
    _not_send: PhantomData<Rc<()>>,
}
impl<'input> ScopedCreationPrivileges<'input> {
    #[allow(unsafe_code)]
    pub(super) fn enter(
        input: &'input super::cli_spawn::CliCreationInput,
        peer: &Peer,
        pipe: &Pipe,
    ) -> Result<Self> {
        input.recheck(peer, pipe)?;
        let host = input.host();
        let actual = Store::new(&host.context.seed)
            .read(&host.context.seed)?
            .ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&actual)?;
        let mut existing = ptr::null_mut();
        if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut existing) } != 0 {
            drop(Handle::owned(existing)?);
            return Err(Error::SecurityRefused);
        }
        if unsafe { GetLastError() } != ERROR_NO_TOKEN {
            return Err(Error::SecurityRefused);
        }
        let mut source = ptr::null_mut();
        if unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_DUPLICATE,
                &mut source,
            )
        } == 0
        {
            return Err(Error::Unavailable);
        }
        let source = Handle::owned(source)?;
        let service_sid = actual
            .service_identity
            .service_sid
            .as_deref()
            .ok_or(Error::ServiceIdentityRefused)?;
        let service = token_facts(&source, Some(service_sid))?;
        if service.user != host.context.process.token.user
            || service.user.sid != "S-1-5-18"
            || service.user.session_id != 0
            || !service.service_sid_enabled
            || service.app_container
            || service.network
        {
            return Err(Error::ServiceIdentityRefused);
        }
        let mut raw = ptr::null_mut();
        if unsafe {
            DuplicateTokenEx(
                source.raw(),
                TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES | TOKEN_IMPERSONATE,
                ptr::null(),
                SecurityImpersonation,
                TokenImpersonation,
                &mut raw,
            )
        } == 0
        {
            return Err(Error::Unavailable);
        }
        let token = Handle::owned(raw)?;
        if unsafe {
            AdjustTokenPrivileges(
                token.raw(),
                1,
                ptr::null(),
                0,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(Error::Unavailable);
        }
        #[repr(C)]
        struct Three {
            count: u32,
            entries: [LUID_AND_ATTRIBUTES; 3],
        }
        let mut entries = [LUID_AND_ATTRIBUTES::default(); 3];
        for (entry, name) in entries.iter_mut().zip([
            "SeChangeNotifyPrivilege",
            "SeAssignPrimaryTokenPrivilege",
            "SeIncreaseQuotaPrivilege",
        ]) {
            let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            let mut luid = LUID::default();
            if unsafe { LookupPrivilegeValueW(ptr::null(), name.as_ptr(), &mut luid) } == 0 {
                return Err(Error::Unavailable);
            }
            *entry = LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            };
        }
        let enabled = Three { count: 3, entries };
        unsafe { SetLastError(0) };
        if unsafe {
            AdjustTokenPrivileges(
                token.raw(),
                0,
                (&enabled as *const Three).cast::<TOKEN_PRIVILEGES>(),
                0,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        } == 0
            || unsafe { GetLastError() } != 0
        {
            return Err(Error::PrerequisiteUnavailable);
        }
        host.authenticate_running(&actual)?;
        input.recheck(peer, pipe)?;
        if unsafe { SetThreadToken(ptr::null(), token.raw()) } == 0 {
            return Err(Error::Unavailable);
        }
        Ok(Self {
            _token: token,
            _input: PhantomData,
            _not_send: PhantomData,
        })
    }
}
impl Drop for ScopedCreationPrivileges<'_> {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        if unsafe { RevertToSelf() } == 0 {
            std::process::abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::process::NativeUser;
    use super::*;
    use crate::identity::LogonId;

    fn requester() -> TokenFacts {
        TokenFacts {
            user: NativeUser {
                sid: "S-1-5-21-1-2-3-1000".into(),
                logon: LogonId { high: 1, low: 2 },
                session_id: 4,
            },
            integrity: 0x2000,
            app_container: false,
            elevated: false,
            administrator: false,
            interactive: true,
            network: false,
            service_sid_enabled: false,
        }
    }

    #[test]
    fn requester_token_must_keep_user_logon_session_and_security_attributes() {
        let expected = requester();
        assert!(same_requester(&expected, &requester()));
        for change in 0..9 {
            let mut actual = requester();
            match change {
                0 => actual.user.sid.push('1'),
                1 => actual.user.logon.low += 1,
                2 => actual.user.session_id += 1,
                3 => actual.integrity += 1,
                4 => actual.elevated = true,
                5 => actual.administrator = true,
                6 => actual.app_container = true,
                7 => actual.network = true,
                8 => actual.interactive = false,
                _ => unreachable!(),
            }
            assert!(!same_requester(&expected, &actual));
        }
    }
}
