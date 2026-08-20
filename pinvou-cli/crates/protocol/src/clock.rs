use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct HostMonotonicTimestamp(u64);

impl HostMonotonicTimestamp {
    pub const fn as_nanos(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ClockError {
    #[error("host monotonic clock is unsupported on this platform")]
    UnsupportedPlatform,
    #[error("host monotonic clock API failed")]
    PlatformFailure,
    #[error("host monotonic timestamp overflowed")]
    Overflow,
}

pub struct HostMonotonicClock;

impl HostMonotonicClock {
    pub fn now() -> Result<HostMonotonicTimestamp, ClockError> {
        platform_now()
    }
}

#[cfg(target_os = "windows")]
fn platform_now() -> Result<HostMonotonicTimestamp, ClockError> {
    use windows_sys::Win32::System::Performance::{
        QueryPerformanceCounter, QueryPerformanceFrequency,
    };

    let mut counter = 0_i64;
    let mut frequency = 0_i64;
    // SAFETY: both APIs only write an i64 to the valid pointers supplied here.
    if unsafe { QueryPerformanceFrequency(&mut frequency) } == 0
        || unsafe { QueryPerformanceCounter(&mut counter) } == 0
        || counter < 0
        || frequency <= 0
    {
        return Err(ClockError::PlatformFailure);
    }
    let nanos = (counter as u128)
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_div(frequency as u128))
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(ClockError::Overflow)?;
    Ok(HostMonotonicTimestamp(nanos))
}

#[cfg(target_os = "linux")]
fn platform_now() -> Result<HostMonotonicTimestamp, ClockError> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: CLOCK_MONOTONIC is a valid Linux clock id and `time` is writable.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0
        || time.tv_sec < 0
        || !(0..1_000_000_000).contains(&time.tv_nsec)
    {
        return Err(ClockError::PlatformFailure);
    }
    let nanos = (time.tv_sec as u128)
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(time.tv_nsec as u128))
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(ClockError::Overflow)?;
    Ok(HostMonotonicTimestamp(nanos))
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn platform_now() -> Result<HostMonotonicTimestamp, ClockError> {
    Err(ClockError::UnsupportedPlatform)
}
