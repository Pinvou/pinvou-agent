use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn user_home_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
}

pub fn platform_compat_path(value: &str) -> PathBuf {
    PathBuf::from(value)
}

pub fn validate_upload_location(canon: &Path) -> Result<(), String> {
    let home_raw = user_home_dir();
    let home = platform_compat_path(
        &std::fs::canonicalize(&home_raw)
            .unwrap_or_else(|_| home_raw.clone())
            .to_string_lossy(),
    );
    if !canon.starts_with(&home) {
        return Err(format!("path {} not under $HOME", canon.display()));
    }
    Ok(())
}

pub fn path_component_eq(component: &OsStr, expected: &str) -> bool {
    component == OsStr::new(expected)
}

pub fn python_command() -> String {
    if which_in_path("python3") {
        return "python3".to_string();
    }
    if which_in_path("python") {
        return "python".to_string();
    }
    "python3".to_string()
}

pub fn bundled_onnxruntime_dylib_path() -> Option<PathBuf> {
    None
}

pub fn connector_cli_command(cli_bin: &str, program: &str) -> Command {
    Command::new(connector_cli_program(cli_bin, program))
}

fn connector_cli_program(cli_bin: &str, program: &str) -> OsString {
    if program == cli_bin {
        if let Some(bin_dir) = crate::platform::paths::bundle_connector_bin_dir() {
            let bundled = bin_dir.join(cli_bin);
            if bundled.is_file() {
                return bundled.into_os_string();
            }
        }
    }
    if program == cli_bin {
        let mut candidates = Vec::new();
        if let Ok(prefix) = std::env::var("NPM_CONFIG_PREFIX") {
            candidates.push(Path::new(&prefix).join("bin").join(program));
        }
        if let Ok(home) = std::env::var("HOME") {
            let home = Path::new(&home);
            candidates.push(home.join(".npm-global").join("bin").join(program));
            candidates.push(home.join(".local").join("bin").join(program));
        }
        for p in candidates {
            if p.is_file() {
                return p.into_os_string();
            }
        }
    }
    program.into()
}

pub fn apply_user_npm_prefix(cmd: &mut Command) {
    if std::env::var_os("NPM_CONFIG_PREFIX").is_some()
        || std::env::var_os("npm_config_prefix").is_some()
    {
        return;
    }

    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let prefix = Path::new(&home).join(".npm-global");
    let bin = prefix.join("bin");
    let _ = std::fs::create_dir_all(&bin);
    cmd.env("NPM_CONFIG_PREFIX", &prefix)
        .env("npm_config_prefix", &prefix);
    prepend_connector_path_entries(cmd, [bin]);
}

pub fn kill_pid_tree(pid: u32) {
    let _ = connector_cli_command("", "kill")
        .args(["-9", &pid.to_string()])
        .output();
}

fn prepend_connector_path_entries(cmd: &mut Command, dirs: impl IntoIterator<Item = PathBuf>) {
    let mut paths: Vec<PathBuf> = dirs
        .into_iter()
        .filter(|p| !p.as_os_str().is_empty())
        .collect();
    if let Some(current) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&current));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        cmd.env("PATH", joined);
    }
}

fn which_in_path(cmd: &str) -> bool {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            if dir.join(cmd).is_file() {
                return true;
            }
        }
    }
    false
}

pub fn pdf_tool_path(command: &str) -> PathBuf {
    PathBuf::from(command)
}

pub fn pandoc_tool_path() -> PathBuf {
    PathBuf::from("pandoc")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_location_rejects_outside_home() {
        assert!(validate_upload_location(Path::new("/etc/passwd")).is_err());
    }
}
