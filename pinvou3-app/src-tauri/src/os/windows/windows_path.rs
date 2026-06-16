use std::path::PathBuf;

pub fn user_home_dir() -> PathBuf {
    if let Ok(home) = std::env::var("USERPROFILE") {
        if !home.trim().is_empty() {
            return PathBuf::from(home);
        }
    }
    if let (Ok(drive), Ok(path)) = (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        let home = format!("{drive}{path}");
        if !home.trim().is_empty() {
            return PathBuf::from(home);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return platform_compat_path(&home);
        }
    }
    std::env::temp_dir()
}

pub fn platform_compat_path(value: &str) -> PathBuf {
    let trimmed = value.trim();
    let normalized = trimmed.replace('\\', "/");
    if normalized == "/tmp" || normalized.starts_with("/tmp/") {
        let rest = normalized
            .trim_start_matches("/tmp")
            .trim_start_matches('/');
        return if rest.is_empty() {
            std::env::temp_dir()
        } else {
            std::env::temp_dir().join(rest.replace('/', "\\"))
        };
    }

    PathBuf::from(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_tmp_path_maps_to_temp_dir() {
        assert_eq!(
            platform_compat_path("/tmp/pinvou3-test-override"),
            std::env::temp_dir().join("pinvou3-test-override")
        );
    }
}
