use codex_app_server_capture::clock::{HostMonotonicClock, MonotonicClock};

#[test]
fn host_clock_returns_non_decreasing_nanoseconds() {
    let clock = HostMonotonicClock::new().expect("host monotonic clock must be available");
    let first = clock.now_ns().unwrap();
    let second = clock.now_ns().unwrap();

    assert!(second >= first);
}
