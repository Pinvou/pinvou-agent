//! Checked counter units and detached numeric observations only.
//! No system API, service identity, UTC, continuity or execution authority lives here.
use crate::error::{Error, Result};

const TICKS_PER_MILLISECOND: u64 = 10_000;

/// Bounds in the native counter's 100-nanosecond units. Floor/ceil preserve an
/// enclosing millisecond interval, including at exact and sub-ms boundaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CounterWindow {
    earliest_100ns: u64,
    latest_100ns: u64,
}
impl CounterWindow {
    fn new(earliest: u64, latest: u64) -> Result<Self> {
        if earliest > latest {
            return Err(Error::Unavailable);
        }
        let window = Self {
            earliest_100ns: earliest,
            latest_100ns: latest,
        };
        window.floor_millis()?;
        window.ceil_millis()?;
        Ok(window)
    }
    pub(crate) fn earliest_100ns(self) -> u64 {
        self.earliest_100ns
    }
    pub(crate) fn latest_100ns(self) -> u64 {
        self.latest_100ns
    }
    pub(crate) fn floor_millis(self) -> Result<u64> {
        let value = self.earliest_100ns / TICKS_PER_MILLISECOND;
        if value > crate::json::SAFE_MAX {
            return Err(Error::Unavailable);
        }
        Ok(value)
    }
    pub(crate) fn ceil_millis(self) -> Result<u64> {
        // Do not add 9,999 to the raw u64: that could overflow near its limit.
        let quotient = self.latest_100ns / TICKS_PER_MILLISECOND;
        let value = quotient
            .checked_add(u64::from(self.latest_100ns % TICKS_PER_MILLISECOND != 0))
            .filter(|value| *value <= crate::json::SAFE_MAX)
            .ok_or(Error::Unavailable)?;
        Ok(value)
    }
    fn elapsed_since(self, previous: Self) -> Result<Self> {
        // Overlapping/unordered observations cannot prove a nonnegative lower
        // bound. Never erase the problem with saturation or a guessed zero.
        let earliest = self
            .earliest_100ns
            .checked_sub(previous.latest_100ns)
            .ok_or(Error::Unavailable)?;
        let latest = self
            .latest_100ns
            .checked_sub(previous.earliest_100ns)
            .ok_or(Error::Unavailable)?;
        Self::new(earliest, latest)
    }
}

/// Detached DATA; even a native reading in an ordinary process is not an SCM
/// source capability. Each pair was read sequentially, not atomically together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CounterObservation {
    suspend_inclusive: CounterWindow,
    awake: CounterWindow,
}
impl CounterObservation {
    /// Numeric validation cannot authenticate where supplied numbers came from.
    /// The Windows adapter alone supplies actual reads to the sealed SCM sample.
    pub(crate) fn from_bracket(
        interrupt_before: u64,
        awake_before: u64,
        awake_after: u64,
        interrupt_after: u64,
    ) -> Result<Self> {
        let suspend_inclusive = CounterWindow::new(interrupt_before, interrupt_after)?;
        let awake = CounterWindow::new(awake_before, awake_after)?;
        // Both are boot-relative interrupt counters; unbiased excludes sleep.
        // Their sequential calls do not justify comparing two exact instants.
        if awake_after > interrupt_after {
            return Err(Error::Unavailable);
        }
        Ok(Self {
            suspend_inclusive,
            awake,
        })
    }
    pub(crate) fn suspend_inclusive(self) -> CounterWindow {
        self.suspend_inclusive
    }
    pub(crate) fn awake(self) -> CounterWindow {
        self.awake
    }
    /// Arithmetic DATA only. A source owner must separately prove the same live
    /// incarnation before comparing raw readings from different observations.
    pub(crate) fn elapsed_since(self, previous: Self) -> Result<Self> {
        Ok(Self {
            suspend_inclusive: self
                .suspend_inclusive
                .elapsed_since(previous.suspend_inclusive)?,
            awake: self.awake.elapsed_since(previous.awake)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_encloses_submillisecond_values_without_raw_add_overflow() {
        for (ticks, floor, ceil) in [
            (0, 0, 0),
            (1, 0, 1),
            (9_999, 0, 1),
            (10_000, 1, 1),
            (10_001, 1, 2),
            (u64::MAX, u64::MAX / 10_000, u64::MAX / 10_000 + 1),
        ] {
            let window = CounterWindow::new(ticks, ticks).unwrap();
            assert_eq!(window.floor_millis().unwrap(), floor);
            assert_eq!(window.ceil_millis().unwrap(), ceil);
        }
        let window = CounterWindow::new(9_999, 10_001).unwrap();
        assert_eq!(
            (
                window.floor_millis().unwrap(),
                window.ceil_millis().unwrap()
            ),
            (0, 2)
        );
    }

    #[test]
    fn unordered_and_inconsistent_counter_windows_fail_without_saturation() {
        for args in [(20, 1, 2, 19), (20, 2, 1, 21), (20, 1, 22, 21)] {
            assert_eq!(
                CounterObservation::from_bracket(args.0, args.1, args.2, args.3),
                Err(Error::Unavailable)
            );
        }
        let before = CounterObservation::from_bracket(100, 50, 52, 104).unwrap();
        let overlap = CounterObservation::from_bracket(103, 51, 54, 110).unwrap();
        assert_eq!(overlap.elapsed_since(before), Err(Error::Unavailable));
        let rollback = CounterObservation::from_bracket(90, 40, 42, 94).unwrap();
        assert_eq!(rollback.elapsed_since(before), Err(Error::Unavailable));
    }

    #[test]
    fn elapsed_bounds_keep_suspend_and_awake_distinct() {
        let before = CounterObservation::from_bracket(100_000, 50_000, 50_020, 100_040).unwrap();
        let after = CounterObservation::from_bracket(400_000, 60_000, 60_020, 400_040).unwrap();
        let elapsed = after.elapsed_since(before).unwrap();
        assert_eq!(
            (
                elapsed.suspend_inclusive().earliest_100ns(),
                elapsed.suspend_inclusive().latest_100ns()
            ),
            (299_960, 300_040)
        );
        assert_eq!(
            (
                elapsed.awake().earliest_100ns(),
                elapsed.awake().latest_100ns()
            ),
            (9_980, 10_020)
        );
        assert_eq!(
            (
                elapsed.suspend_inclusive().floor_millis().unwrap(),
                elapsed.suspend_inclusive().ceil_millis().unwrap()
            ),
            (29, 31)
        );
        assert_eq!(
            (
                elapsed.awake().floor_millis().unwrap(),
                elapsed.awake().ceil_millis().unwrap()
            ),
            (0, 2)
        );
    }

    #[test]
    fn native_counter_data_requires_supported_adapter_not_an_scm_fixture() {
        match crate::platform::native_supported() {
            Err(Error::Unsupported) => assert_eq!(
                crate::platform::read_monotonic_counters(),
                Err(Error::Unsupported)
            ),
            Ok(()) => {
                // A normal Windows test process can inspect real API DATA.
                // It does NOT construct StartedServiceContext or a sealed sample.
                let first = crate::platform::read_monotonic_counters().unwrap();
                let second = crate::platform::read_monotonic_counters().unwrap();
                let elapsed = second.elapsed_since(first).unwrap();
                assert!(
                    elapsed.suspend_inclusive().earliest_100ns()
                        <= elapsed.suspend_inclusive().latest_100ns()
                );
                assert!(elapsed.awake().earliest_100ns() <= elapsed.awake().latest_100ns());
            }
            Err(error) => panic!("native adapter unavailable: {error}"),
        }
    }
}
