#[cfg(not(any(target_os = "windows", target_os = "linux")))]
use pinvou_protocol::ClockError;
use pinvou_protocol::HostMonotonicClock;

#[test]
fn host_clock_is_monotonic_or_explicitly_unsupported() {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        let first = HostMonotonicClock::now().unwrap();
        let second = HostMonotonicClock::now().unwrap();
        assert!(second >= first);
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    assert_eq!(
        HostMonotonicClock::now(),
        Err(ClockError::UnsupportedPlatform)
    );
}

#[test]
fn host_timestamp_is_a_plain_same_boot_measurement() {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        let timestamp = HostMonotonicClock::now().unwrap();
        assert!(timestamp.as_nanos() > 0);
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    assert_eq!(
        HostMonotonicClock::now(),
        Err(ClockError::UnsupportedPlatform)
    );
}
