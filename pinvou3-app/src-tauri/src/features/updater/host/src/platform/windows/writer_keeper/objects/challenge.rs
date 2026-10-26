//! Owned, one-use keeper challenge events and fixed-prefix orphan retirement.
//! Only Actor's actual authenticated current-SCM path takes a received event.
//! The actor is the sole closer, but a timed-out producer may already create
//! its NEXT challenge while an older dispatch is still running. Native names
//! bind authenticated service epoch and monotonically allocated request serial;
//! cleanup may retire only strictly older orders, never an equal/later request.
//! The service never uses DUPLICATE_CLOSE_SOURCE for these target handles.
use super::*;
use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::SetEvent};

const NATIVE_EVENT_PREFIX: &str = "\\BaseNamedObjects\\PinvouKeeperEvent-";

/// An actually validated and continuously owned current kernel event. No Clone,
/// Serialize, Deserialize, arbitrary owned-handle constructor or raw setter.
pub(in crate::platform::windows::writer_keeper) struct ValidatedChallenge {
    handle: Handle,
    nonce: String,
    host_epoch: u64,
    serial: u64,
}

/// Service-process ordinal allocated before any native creation/duplication.
/// No wrap, reuse, wall clock, timeout or caller-provided counter. A fresh
/// service incarnation is ordered by its protected host epoch instead.
pub(super) fn next_serial() -> Result<u64> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |prior| {
        prior.checked_add(1).filter(|next| *next <= json::SAFE_MAX)
    })
    .map(|prior| prior + 1)
    .map_err(|_| Error::HandoffOutcomeUnknown)
}
fn valid_order(host_epoch: u64, serial: u64) -> bool {
    host_epoch > 0 && host_epoch <= json::SAFE_MAX && serial > 0 && serial <= json::SAFE_MAX
}
pub(super) fn event_name(host_epoch: u64, serial: u64, nonce: &str) -> Result<String> {
    if !valid_order(host_epoch, serial) || !json::digest(nonce) {
        return Err(Error::SecurityRefused);
    }
    Ok(format!(
        "Global\\PinvouKeeperEvent-{host_epoch}-{serial}-{nonce}"
    ))
}
pub(super) fn inspect_event(raw: HANDLE, host_epoch: u64, serial: u64, nonce: &str) -> Result<()> {
    flags(raw, false)?;
    granted(raw, EVENT_SIGNAL_QUERY)?;
    if object_string(raw, ObjectTypeInformation)? != "Event"
        || object_string(raw, OBJECT_NAME_INFORMATION)?
            != native_global_name(&event_name(host_epoch, serial, nonce)?)?
    {
        return Err(Error::SecurityRefused);
    }
    Ok(())
}
/// Fixed EventBasicInformation only; this is not a caller-selected native query.
/// A valid challenge is a manual-reset event whose state is exactly 0 or 1.
#[allow(unsafe_code)]
fn event_state(raw: HANDLE) -> Result<i32> {
    #[repr(C)]
    #[derive(Default)]
    struct EventBasic {
        event_type: i32,
        state: i32,
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQueryEvent(
            event: HANDLE,
            class: i32,
            output: *mut c_void,
            length: u32,
            returned: *mut u32,
        ) -> i32;
    }
    let mut state = EventBasic::default();
    let mut length = 0;
    if unsafe {
        NtQueryEvent(
            raw,
            0,
            (&mut state as *mut EventBasic).cast(),
            size_of::<EventBasic>() as u32,
            &mut length,
        )
    } < 0
        || length != size_of::<EventBasic>() as u32
        || state.event_type != 0
        || !matches!(state.state, 0 | 1)
    {
        return Err(Error::SecurityRefused);
    }
    Ok(state.state)
}
fn fresh(raw: HANDLE, host_epoch: u64, serial: u64, nonce: &str) -> Result<()> {
    inspect_event(raw, host_epoch, serial, nonce)?;
    if event_state(raw)? != 0 {
        return Err(Error::SecurityRefused);
    }
    Ok(())
}
/// Authenticate actual type/name/exact access/flags/manual fresh state BEFORE
/// adopting a received address. On failure the raw address is never closed.
/// The authenticated single-thread Actor calls this before any fallible action.
pub(in crate::platform::windows::writer_keeper) fn take_challenge(
    raw: HANDLE,
    host_epoch: u64,
    serial: u64,
    nonce: &str,
) -> Result<ValidatedChallenge> {
    fresh(raw, host_epoch, serial, nonce)?;
    // No await/IO/other closer can replace this value between native validation
    // and adoption. The service cannot close remote source handles. From here
    // Handle owns the same object across all Actor return/unwind paths.
    Ok(ValidatedChallenge {
        handle: Handle::owned(raw)?,
        nonce: nonce.to_owned(),
        host_epoch,
        serial,
    })
}
/// Signal through a borrow of that SAME owned event. No second adoption or
/// CloseHandle occurs here; the enclosing token always closes exactly once.
#[allow(unsafe_code)]
pub(in crate::platform::windows::writer_keeper) fn signal_challenge(
    current: &ValidatedChallenge,
) -> Result<()> {
    fresh(
        current.handle.raw(),
        current.host_epoch,
        current.serial,
        &current.nonce,
    )?;
    if unsafe { SetEvent(current.handle.raw()) } == 0 {
        return Err(Error::HandoffOutcomeUnknown);
    }
    Ok(())
}

fn challenge_identity(name: &str) -> Result<Option<(u64, u64, &str)>> {
    let Some(body) = name.strip_prefix(NATIVE_EVENT_PREFIX) else {
        return Ok(None);
    };
    let mut parts = body.split('-');
    let epoch = parts.next().ok_or(Error::HandoffOutcomeUnknown)?;
    let serial = parts.next().ok_or(Error::HandoffOutcomeUnknown)?;
    let nonce = parts.next().ok_or(Error::HandoffOutcomeUnknown)?;
    let parsed_epoch = epoch
        .parse::<u64>()
        .map_err(|_| Error::HandoffOutcomeUnknown)?;
    let parsed_serial = serial
        .parse::<u64>()
        .map_err(|_| Error::HandoffOutcomeUnknown)?;
    if parts.next().is_some()
        || !valid_order(parsed_epoch, parsed_serial)
        || parsed_epoch.to_string() != epoch
        || parsed_serial.to_string() != serial
        || !json::digest(nonce)
    {
        return Err(Error::HandoffOutcomeUnknown);
    }
    Ok(Some((parsed_epoch, parsed_serial, nonce)))
}
fn inspect_orphan(raw: HANDLE, host_epoch: u64, serial: u64, nonce: &str) -> Result<()> {
    inspect_event(raw, host_epoch, serial, nonce)?;
    event_state(raw).map(|_| ())
}

/// Close only a fully inspected old fixed-purpose event address from the native
/// snapshot. A new, continuously held duplicate pins the exact original object;
/// raw-address equality alone is never used as a closing proof.
#[allow(unsafe_code)]
fn close_orphan(
    raw: HANDLE,
    host_epoch: u64,
    serial: u64,
    nonce: &str,
    current: &ValidatedChallenge,
) -> Result<()> {
    inspect_orphan(raw, host_epoch, serial, nonce)?;
    let pinned = duplicate_local(raw, EVENT_SIGNAL_QUERY, false)?;
    inspect_orphan(pinned.raw(), host_epoch, serial, nonce)?;
    if unsafe { CompareObjectHandles(pinned.raw(), raw) } == 0
        || unsafe { CompareObjectHandles(pinned.raw(), current.handle.raw()) } != 0
    {
        return Err(Error::HandoffOutcomeUnknown);
    }
    inspect_orphan(raw, host_epoch, serial, nonce)?;
    if unsafe { CompareObjectHandles(pinned.raw(), raw) } == 0 || unsafe { CloseHandle(raw) } == 0 {
        return Err(Error::HandoffOutcomeUnknown);
    }
    // Only our newly created pin drops here. It cannot double-own the original
    // numeric value. No other current token, IO handle or Job was adopted.
    Ok(())
}

/// Retire predelivery and prevalidation orphan events only after the Actor has
/// authenticated the current service and strongly owns its current challenge.
/// A later service request may already be in flight after an older timeout.
/// Native epoch/serial ordering excludes it; the actor's single thread proves
/// strictly older dispatch owners have already dropped before this dispatch.
#[allow(unsafe_code)]
pub(in crate::platform::windows::writer_keeper) fn cleanup_orphan_challenges(
    current: &ValidatedChallenge,
) -> Result<()> {
    fresh(
        current.handle.raw(),
        current.host_epoch,
        current.serial,
        &current.nonce,
    )?;
    let mut old = Vec::new();
    for entry in native_handles()? {
        if entry.raw == current.handle.raw() {
            continue;
        }
        if object_string(entry.raw, ObjectTypeInformation)? != "Event" {
            continue;
        }
        // Exclude every alias of the actual current object, not just its number.
        if unsafe { CompareObjectHandles(entry.raw, current.handle.raw()) } != 0 {
            continue;
        }
        let Some(name) = object_text(entry.raw, OBJECT_NAME_INFORMATION)? else {
            continue; // unnamed stop/overlapped IO events are never candidates
        };
        let Some((host_epoch, serial, nonce)) = challenge_identity(&name)? else {
            continue;
        };
        // Equal/different objects have no proven orphan ordering either.
        // A later request may be a legitimate successor created while THIS
        // dispatch was paused beyond its producer's transport timeout.
        if (host_epoch, serial) >= (current.host_epoch, current.serial) {
            continue;
        }
        if entry.granted_access != EVENT_SIGNAL_QUERY || entry.attributes != 0 {
            return Err(Error::HandoffOutcomeUnknown);
        }
        inspect_orphan(entry.raw, host_epoch, serial, nonce)?;
        old.push((entry.raw, host_epoch, serial, nonce.to_owned()));
    }
    // Native names continuously bind the order; the service only adds target
    // handles, never closes/reuses them. The actor is the sole closer and any
    // older owned token was dropped before this single-thread dispatch began.
    for (raw, host_epoch, serial, nonce) in old {
        close_orphan(raw, host_epoch, serial, &nonce, current)?;
    }
    fresh(
        current.handle.raw(),
        current.host_epoch,
        current.serial,
        &current.nonce,
    )
}

#[cfg(test)]
mod tests {
    //! Ordinary current-user owned Global Event primitives only. No System
    //! descriptor, restricted service token, SCM, keeper launch, grants or E2E.
    use super::*;
    use windows_sys::Win32::{
        Foundation::{ERROR_INVALID_HANDLE, GetHandleInformation, GetLastError},
        System::Threading::{CREATE_EVENT_INITIAL_SET, CreateEventExW},
    };
    // The production actor/controller is single-instance and serialized. Keep
    // these own-process ordinary fixtures from becoming each other's orphans.
    static OWNED_EVENTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[allow(unsafe_code)]
    fn ordinary_event(nonce: &str, flags: u32, rights: u32) -> Handle {
        ordinary_named_event(&event_name(1, 100, nonce).unwrap(), flags, rights)
    }
    #[allow(unsafe_code)]
    fn ordinary_named_event(name: &str, flags: u32, rights: u32) -> Handle {
        let name: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
        // The fixture's REAL creation ownership can always be dropped. It is
        // separate from adopting an untrusted received number in production.
        let raw = unsafe { CreateEventExW(ptr::null(), name.as_ptr(), flags, rights) };
        let code = unsafe { GetLastError() };
        let owned = Handle::owned(raw).unwrap();
        assert_ne!(code, ERROR_ALREADY_EXISTS);
        owned
    }
    fn take_fixture(owned: Handle, nonce: &str) -> ValidatedChallenge {
        let current = take_challenge(owned.raw(), 1, 100, nonce).unwrap();
        // Transfer the fixture's actual CreateEvent handle, never double-own it.
        std::mem::forget(owned);
        current
    }
    #[allow(unsafe_code)]
    fn assert_closed(raw: HANDLE) {
        let mut flags = 0;
        assert_eq!(unsafe { GetHandleInformation(raw, &mut flags) }, 0);
        assert_eq!(unsafe { GetLastError() }, ERROR_INVALID_HANDLE);
        // A REAL next native census, not merely the Rust wrapper count.
        assert!(
            !native_handles()
                .unwrap()
                .iter()
                .any(|entry| entry.raw == raw)
        );
    }
    fn fail_after_take(owned: Handle, nonce: &str) -> Result<()> {
        let _current = take_fixture(owned, nonce);
        Err(Error::Referenced)
    }
    #[test]
    fn actual_owned_event_drops_on_postvalidation_failure_and_signal_borrows() {
        let _serial = OWNED_EVENTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let nonce = super::super::super::super::pipe::nonce().unwrap();
        let owned = ordinary_event(&nonce, CREATE_EVENT_MANUAL_RESET, EVENT_SIGNAL_QUERY);
        let raw = owned.raw();
        assert_eq!(fail_after_take(owned, &nonce), Err(Error::Referenced));
        assert_closed(raw);

        let nonce = super::super::super::super::pipe::nonce().unwrap();
        let owned = ordinary_event(&nonce, CREATE_EVENT_MANUAL_RESET, EVENT_SIGNAL_QUERY);
        let raw = owned.raw();
        let current = take_fixture(owned, &nonce);
        signal_challenge(&current).unwrap();
        assert_eq!(event_state(raw).unwrap(), 1);
        // Borrowed signal left the token's real handle live until THIS drop.
        granted(raw, EVENT_SIGNAL_QUERY).unwrap();
        drop(current);
        assert_closed(raw);
    }
    #[test]
    fn actual_predelivery_orphans_are_closed_but_current_and_unnamed_io_survive() {
        let _serial = OWNED_EVENTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut lost = Vec::new();
        for state in [0, CREATE_EVENT_INITIAL_SET] {
            let nonce = super::super::super::super::pipe::nonce().unwrap();
            let owned = ordinary_named_event(
                &event_name(1, 1 + u64::from(state != 0), &nonce).unwrap(),
                CREATE_EVENT_MANUAL_RESET | state,
                EVENT_SIGNAL_QUERY,
            );
            lost.push(owned.raw());
            // Model only the actual successful duplicate with no delivered
            // request: kernel-owned target exists, no Rust wrapper received it.
            std::mem::forget(owned);
        }
        let io = super::super::super::super::pipe::Event::new().unwrap();
        let nonce = super::super::super::super::pipe::nonce().unwrap();
        let current = take_fixture(
            ordinary_event(&nonce, CREATE_EVENT_MANUAL_RESET, EVENT_SIGNAL_QUERY),
            &nonce,
        );
        let current_alias =
            duplicate_local(current.handle.raw(), EVENT_SIGNAL_QUERY, false).unwrap();
        let other_nonce = super::super::super::super::pipe::nonce().unwrap();
        let other = ordinary_named_event(
            &format!("Global\\PinvouKeeperContrast-{other_nonce}"),
            CREATE_EVENT_MANUAL_RESET,
            EVENT_SIGNAL_QUERY,
        );
        let mut later = Vec::new();
        for (epoch, serial) in [(1, 100), (1, 101), (2, 1)] {
            let nonce = super::super::super::super::pipe::nonce().unwrap();
            later.push(ordinary_named_event(
                &event_name(epoch, serial, &nonce).unwrap(),
                CREATE_EVENT_MANUAL_RESET,
                EVENT_SIGNAL_QUERY,
            ));
        }
        cleanup_orphan_challenges(&current).unwrap();
        let snapshot = native_handles().unwrap();
        for owned in &later {
            assert!(snapshot.iter().any(|entry| entry.raw == owned.raw()));
            assert_eq!(event_state(owned.raw()).unwrap(), 0);
        }
        for raw in lost {
            assert!(!snapshot.iter().any(|entry| entry.raw == raw));
        }
        assert!(
            snapshot
                .iter()
                .any(|entry| entry.raw == current.handle.raw())
        );
        assert!(
            snapshot
                .iter()
                .any(|entry| entry.raw == current_alias.raw())
        );
        assert!(snapshot.iter().any(|entry| entry.raw == other.raw()));
        assert!(snapshot.iter().any(|entry| entry.raw == io.raw()));
        assert_eq!(event_state(current.handle.raw()).unwrap(), 0);
        io.set().unwrap();
        assert!(io.is_set().unwrap());
        assert_eq!(event_state(other.raw()).unwrap(), 0);
        cleanup_orphan_challenges(&current).unwrap(); // actual repeat census
    }
    #[test]
    fn native_validation_failure_never_adopts_or_closes_the_received_raw() {
        let _serial = OWNED_EVENTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (mode, rights) in [
            (CREATE_EVENT_MANUAL_RESET | CREATE_EVENT_INITIAL_SET, 3),
            (0, 3),
            (CREATE_EVENT_MANUAL_RESET, 0x0010_0003),
        ] {
            let nonce = super::super::super::super::pipe::nonce().unwrap();
            let owned = ordinary_event(&nonce, mode, rights);
            assert!(take_challenge(owned.raw(), 1, 100, &nonce).is_err());
            // Still owned by its real creation fixture, NOT by take_challenge.
            granted(owned.raw(), rights).unwrap();
        }
    }
}
