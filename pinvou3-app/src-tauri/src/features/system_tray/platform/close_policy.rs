/// Never consume a close unless a tray exists and hiding actually succeeds.
/// Auxiliary windows keep their normal close behavior.
pub(crate) fn close_to_tray<E>(
    label: &str,
    tray_available: bool,
    hide: impl FnOnce() -> Result<(), E>,
) -> Result<bool, E> {
    if label != "main" || !tray_available {
        return Ok(false);
    }
    hide().map(|()| true)
}

#[cfg(test)]
mod tests {
    use super::close_to_tray;

    #[test]
    fn main_close_is_consumed_only_after_hiding_the_window() {
        let mut hides = 0;
        assert_eq!(
            close_to_tray("main", true, || {
                hides += 1;
                Ok::<_, ()>(())
            }),
            Ok(true)
        );
        assert_eq!(hides, 1);
    }

    #[test]
    fn missing_tray_and_auxiliary_windows_close_normally() {
        for (label, available) in [("main", false), ("pet", true), ("reader-1", true)] {
            assert_eq!(
                close_to_tray::<()>(label, available, || panic!("must not hide")),
                Ok(false)
            );
        }
    }

    #[test]
    fn hide_failure_does_not_trap_the_user_in_an_invisible_process() {
        assert_eq!(
            close_to_tray("main", true, || Err("hide failed")),
            Err("hide failed")
        );
    }
}
