use anyhow::{Result, bail};

/// A same-boot monotonic source. Values are nanoseconds since an unspecified
/// host monotonic epoch and must never be interpreted as wall-clock time.
pub trait MonotonicClock: Send + Sync {
    fn now_ns(&self) -> Result<u64>;
}

#[derive(Debug)]
pub struct HostMonotonicClock {
    #[cfg(windows)]
    frequency: u64,
}

impl HostMonotonicClock {
    pub fn new() -> Result<Self> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Performance::QueryPerformanceFrequency;

            let mut frequency = 0_i64;
            // SAFETY: `frequency` is a valid writable pointer for the duration of the call.
            if unsafe { QueryPerformanceFrequency(&mut frequency) } == 0 || frequency <= 0 {
                bail!("QueryPerformanceFrequency failed");
            }
            Ok(Self {
                frequency: frequency as u64,
            })
        }
        #[cfg(target_os = "linux")]
        {
            Ok(Self {})
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            bail!("host monotonic clock is supported only on Windows and Linux")
        }
    }
}

impl MonotonicClock for HostMonotonicClock {
    fn now_ns(&self) -> Result<u64> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Performance::QueryPerformanceCounter;

            let mut counter = 0_i64;
            // SAFETY: `counter` is a valid writable pointer for the duration of the call.
            if unsafe { QueryPerformanceCounter(&mut counter) } == 0 || counter < 0 {
                bail!("QueryPerformanceCounter failed");
            }
            let nanos = (counter as u128)
                .saturating_mul(1_000_000_000)
                .checked_div(self.frequency as u128)
                .expect("QPC frequency was validated as positive");
            Ok(u64::try_from(nanos).unwrap_or(u64::MAX))
        }
        #[cfg(target_os = "linux")]
        {
            let mut value = Timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // SAFETY: `value` is a valid writable timespec and CLOCK_MONOTONIC is a valid id.
            if unsafe { clock_gettime(CLOCK_MONOTONIC, &mut value) } != 0
                || value.tv_sec < 0
                || value.tv_nsec < 0
            {
                bail!("clock_gettime(CLOCK_MONOTONIC) failed");
            }
            let nanos = (value.tv_sec as u128)
                .saturating_mul(1_000_000_000)
                .saturating_add(value.tv_nsec as u128);
            Ok(u64::try_from(nanos).unwrap_or(u64::MAX))
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            bail!("host monotonic clock is supported only on Windows and Linux")
        }
    }
}

#[cfg(target_os = "linux")]
const CLOCK_MONOTONIC: std::ffi::c_int = 1;

#[cfg(target_os = "linux")]
#[repr(C)]
struct Timespec {
    tv_sec: std::ffi::c_long,
    tv_nsec: std::ffi::c_long,
}

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn clock_gettime(clock_id: std::ffi::c_int, value: *mut Timespec) -> std::ffi::c_int;
}
