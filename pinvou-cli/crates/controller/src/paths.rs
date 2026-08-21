use std::path::{Path, PathBuf};

use crate::ControllerError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostPlatform {
    Windows,
    Linux,
}

impl HostPlatform {
    pub fn current() -> Result<Self, ControllerError> {
        #[cfg(windows)]
        return Ok(Self::Windows);
        #[cfg(target_os = "linux")]
        return Ok(Self::Linux);
        #[allow(unreachable_code)]
        Err(ControllerError::UnsupportedPlatform)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalEndpoint {
    WindowsPipe(String),
    UnixSocket(PathBuf),
}

impl LocalEndpoint {
    pub fn display(&self) -> String {
        match self {
            Self::WindowsPipe(name) => name.clone(),
            Self::UnixSocket(path) => path.display().to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ControllerPaths {
    data_root: PathBuf,
    runtime_root: PathBuf,
    endpoint: LocalEndpoint,
    lock_file: PathBuf,
    log_file: PathBuf,
}

impl ControllerPaths {
    pub fn discover() -> Result<Self, ControllerError> {
        let platform = HostPlatform::current()?;
        match platform {
            HostPlatform::Windows => {
                let local = std::env::var_os("LOCALAPPDATA")
                    .map(PathBuf::from)
                    .ok_or(ControllerError::PathUnavailable)?;
                Self::from_roots(
                    platform,
                    local.join("pinvou"),
                    local.join("pinvou"),
                    &session_scope()?,
                )
            }
            HostPlatform::Linux => {
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .ok_or(ControllerError::PathUnavailable)?;
                let data = std::env::var_os("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".local/share"))
                    .join("pinvou");
                let runtime = std::env::var_os("XDG_RUNTIME_DIR")
                    .map(PathBuf::from)
                    .ok_or(ControllerError::PathUnavailable)?;
                Self::from_roots(platform, data, runtime, "unused")
            }
        }
    }

    pub fn from_roots(
        platform: HostPlatform,
        data_root: PathBuf,
        runtime_root: PathBuf,
        session_scope: &str,
    ) -> Result<Self, ControllerError> {
        if data_root.as_os_str().is_empty()
            || runtime_root.as_os_str().is_empty()
            || path_contains_legacy_root(&data_root)
            || path_contains_legacy_root(&runtime_root)
        {
            return Err(ControllerError::PathUnavailable);
        }
        let endpoint = match platform {
            HostPlatform::Windows => {
                if session_scope.is_empty() {
                    return Err(ControllerError::PathUnavailable);
                }
                LocalEndpoint::WindowsPipe(format!(
                    r"\\.\pipe\pinvou-controller-{:016x}",
                    stable_hash(session_scope.as_bytes())
                ))
            }
            HostPlatform::Linux => {
                LocalEndpoint::UnixSocket(runtime_root.join("pinvou/controller.sock"))
            }
        };
        Ok(Self {
            lock_file: data_root.join("controller.lock"),
            log_file: data_root.join("logs/controller.log"),
            data_root,
            runtime_root,
            endpoint,
        })
    }

    pub fn data_root(&self) -> &Path {
        &self.data_root
    }
    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }
    pub fn endpoint(&self) -> &LocalEndpoint {
        &self.endpoint
    }
    pub fn lock_file(&self) -> &Path {
        &self.lock_file
    }
    pub fn log_file(&self) -> &Path {
        &self.log_file
    }

    pub fn prepare_data_root(&self) -> Result<(), ControllerError> {
        #[cfg(target_os = "linux")]
        prepare_linux_private_directory(&self.data_root)?;
        #[cfg(not(target_os = "linux"))]
        std::fs::create_dir_all(&self.data_root)?;
        #[cfg(windows)]
        crate::windows_security::apply_current_logon_dacl(&self.data_root)?;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn prepare_linux_private_directory(path: &Path) -> Result<(), ControllerError> {
    use std::os::unix::fs::PermissionsExt;

    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(ControllerError::PathUnavailable);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path)?;
        }
        Err(error) => return Err(error.into()),
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ControllerError::PathUnavailable);
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn path_contains_legacy_root(path: &Path) -> bool {
    path.components().any(|part| part.as_os_str() == ".pinvou3")
}

fn stable_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

#[cfg(windows)]
fn session_scope() -> Result<String, ControllerError> {
    #[cfg(debug_assertions)]
    if let Some(scope) = std::env::var_os("PINVOU_CONTROLLER_SESSION_SCOPE_FOR_TEST") {
        let scope = scope
            .into_string()
            .map_err(|_| ControllerError::PathUnavailable)?;
        if !scope.is_empty() {
            return Ok(scope);
        }
    }
    crate::windows_security::current_logon_sid_string()
}

#[cfg(not(windows))]
fn session_scope() -> Result<String, ControllerError> {
    Err(ControllerError::UnsupportedPlatform)
}
