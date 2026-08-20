use std::{
    io::Write,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    ControllerError, ControllerPaths, ControllerSession, HostPlatform, InstanceLock,
    LocalIpcListener, RollingLog,
};

#[derive(Clone, Copy, Debug)]
pub struct DetachedLaunch {
    platform: HostPlatform,
}

impl DetachedLaunch {
    /// Builds the stage-1 launcher primitive. Wiring it into the product CLI belongs to T9.
    pub const fn for_platform(platform: HostPlatform) -> Self {
        Self { platform }
    }
    pub const fn detached_process(self) -> bool {
        matches!(self.platform, HostPlatform::Windows)
    }
    pub const fn new_process_group(self) -> bool {
        matches!(self.platform, HostPlatform::Windows)
    }
    pub const fn creates_session(self) -> bool {
        matches!(self.platform, HostPlatform::Linux)
    }
    pub const fn registers_login_autostart(self) -> bool {
        false
    }

    pub fn spawn(self, executable: &std::path::Path, args: &[&str]) -> Result<(), ControllerError> {
        let mut command = Command::new(executable);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match self.platform {
            HostPlatform::Windows => configure_windows_detach(&mut command),
            HostPlatform::Linux => configure_unix_detach(&mut command),
        }
        command.spawn()?;
        Ok(())
    }
}

pub fn run_from_env() -> Result<(), ControllerError> {
    let mut args = std::env::args_os().skip(1);
    match (args.next(), args.next()) {
        (Some(flag), None) if flag == "--check-config" => {
            ControllerPaths::discover()?;
            println!("controller configuration ok");
            Ok(())
        }
        (None, None) => run_controller(),
        _ => Err(ControllerError::Usage),
    }
}

fn run_controller() -> Result<(), ControllerError> {
    let paths = ControllerPaths::discover()?;
    paths.prepare_data_root()?;
    let _lock = InstanceLock::acquire(paths.lock_file())?;
    let log = Arc::new(Mutex::new(RollingLog::open(paths.log_file().to_owned())?));
    let instance_id = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ControllerError::InvalidMessage)?
            .as_nanos()
    );
    let session = ControllerSession::new(instance_id)?;
    let mut listener = LocalIpcListener::bind(paths.endpoint())?;
    {
        let mut writer = log.lock().map_err(|_| ControllerError::InvalidMessage)?;
        writeln!(writer, "controller started")?;
        writer.flush()?;
    }
    loop {
        // Per-client errors are handled in its worker; accept errors terminate instead of spinning.
        listener.serve_one_logged(&session, Arc::clone(&log))?;
    }
}

#[cfg(windows)]
fn configure_windows_detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
}
#[cfg(not(windows))]
fn configure_windows_detach(_: &mut Command) {}

#[cfg(target_os = "linux")]
fn configure_unix_detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}
#[cfg(not(target_os = "linux"))]
fn configure_unix_detach(_: &mut Command) {}
