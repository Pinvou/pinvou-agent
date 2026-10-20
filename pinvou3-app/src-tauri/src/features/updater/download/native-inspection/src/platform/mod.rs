use crate::Error;
#[cfg(target_os = "windows")]
pub(crate) fn require_supported() -> Result<(), Error> {
    Ok(())
}
#[cfg(not(target_os = "windows"))]
pub(crate) fn require_supported() -> Result<(), Error> {
    Err(Error::Unsupported)
}

#[cfg(all(test, target_os = "windows"))]
mod windows_tests;
#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use crate::*;
    #[test]
    fn unavailable_platform_never_touches_paths() {
        let limits = Limits {
            file_bytes: 1,
            expanded_bytes: 1,
            entries: 1,
            metadata_bytes: 1,
            path_state_bytes: 1,
            ratio: 1,
        };
        assert!(matches!(
            inspect(
                Path::new("unused"),
                "",
                0,
                Path::new("unused"),
                "",
                &limits,
                &AtomicBool::new(false)
            ),
            Err(Error::Unsupported)
        ));
    }
}
