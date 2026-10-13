//! Feature-specific OS adaptation; unavailable implementations fail explicitly.
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub(super) use windows::{create, duplicate_read};

#[cfg(not(target_os = "windows"))]
pub(super) fn create(_: &std::path::Path) -> Result<std::fs::File, crate::CacheObjectError> {
    Err(crate::CacheObjectError::Unsupported)
}
#[cfg(not(target_os = "windows"))]
pub(super) fn duplicate_read(_: &std::fs::File) -> Result<std::fs::File, crate::CacheObjectError> {
    Err(crate::CacheObjectError::Unsupported)
}
#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    #[test]
    fn unavailable_platform_does_not_create_or_truncate_a_path() {
        assert!(matches!(
            crate::create_exclusive_cache(std::path::Path::new("unused")),
            Err(crate::CacheObjectError::Unsupported)
        ));
    }
}
