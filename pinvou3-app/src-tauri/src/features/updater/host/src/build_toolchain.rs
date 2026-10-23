//! The enabled host profile requires the actual standard MSVC toolset floor.
use std::path::Path;
pub fn minimum_from_msvc_linker(path: &Path) -> Option<String> {
    if !path.file_name()?.to_str()?.eq_ignore_ascii_case("link.exe") {
        return None;
    }
    for parent in path.ancestors() {
        if parent
            .parent()?
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("MSVC"))
        {
            let text = parent.file_name()?.to_str()?;
            let parts: Vec<_> = text.split('.').collect();
            if parts.len() != 3
                || parts.iter().any(|part| {
                    part.is_empty()
                        || !part.bytes().all(|byte| byte.is_ascii_digit())
                        || part.parse::<u32>().is_err()
                })
            {
                return None;
            }
            return Some(format!("{text}.0"));
        }
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn minimum_is_the_actual_msvc_toolset_not_an_old_installed_runtime() {
        assert_eq!(
            minimum_from_msvc_linker(Path::new(
                "C:/VS/VC/Tools/MSVC/14.50.35717/bin/Hostx64/x64/link.exe"
            ))
            .as_deref(),
            Some("14.50.35717.0")
        );
        for path in [
            "C:/tools/link.exe",
            "C:/MSVC/14.50/bin/link.exe",
            "C:/MSVC/14.50.35717/bin/clang.exe",
        ] {
            assert!(minimum_from_msvc_linker(Path::new(path)).is_none());
        }
    }
}
