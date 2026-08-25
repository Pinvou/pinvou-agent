use std::{
    io::Write,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    ControllerError, ControllerPaths, ControllerSession, HostPlatform, InstanceLock,
    LocalIpcListener, LocalNodeSpec, LocalNodeSupervisor, RollingLog, error::io_context,
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
        #[cfg(debug_assertions)]
        (Some(flag), None) if flag == "--once-for-test" => {
            run_controller_once_with_local_node_for_test()
        }
        (None, None) => run_controller(),
        _ => Err(ControllerError::Usage),
    }
}

fn run_controller() -> Result<(), ControllerError> {
    let paths = ControllerPaths::discover()?;
    paths
        .prepare_data_root()
        .map_err(controller_context("prepare controller data root"))?;
    let _lock = InstanceLock::acquire(paths.lock_file())
        .map_err(controller_context("acquire controller lock"))?;
    let log = Arc::new(Mutex::new(
        RollingLog::open(paths.log_file().to_owned()).map_err(io_context("open controller log"))?,
    ));
    let instance_id = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ControllerError::InvalidMessage)?
            .as_nanos()
    );
    let node_instance_id = format!("node-{instance_id}");
    let node_spec = LocalNodeSpec::for_controller(
        &paths,
        sibling_node_executable()?,
        node_instance_id.clone(),
    )?;
    let node_endpoint = node_spec.endpoint().to_owned();
    let mut node_supervisor = LocalNodeSupervisor::new(node_spec);
    node_supervisor.start().map_err(|error| match error {
        ControllerError::Io(source) => ControllerError::IoContext {
            context: "start local node",
            source,
        },
        other => other,
    })?;
    let node_supervisor = Arc::new(Mutex::new(node_supervisor));
    let workspace = std::env::current_dir()?;
    let session = ControllerSession::with_local_node_and_storage(
        instance_id,
        node_endpoint,
        node_instance_id,
        paths.data_root(),
        workspace,
    )?;
    let mut listener = LocalIpcListener::bind(paths.endpoint())
        .map_err(controller_context("bind controller IPC"))?;
    {
        let mut writer = log.lock().map_err(|_| ControllerError::InvalidMessage)?;
        writeln!(writer, "controller started")?;
        writer.flush()?;
    }
    let monitor = Arc::clone(&node_supervisor);
    let monitor_log = Arc::clone(&log);
    std::thread::Builder::new()
        .name("pinvou-local-node-supervisor".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(100));
                let result = monitor
                    .lock()
                    .map_err(|_| ControllerError::InvalidMessage)
                    .and_then(|mut supervisor| supervisor.poll(Instant::now()));
                if let Err(error) = result {
                    if let Ok(mut writer) = monitor_log.lock() {
                        let _ = writeln!(writer, "local node supervision failed: {error}");
                        let _ = writer.flush();
                    }
                    break;
                }
            }
        })?;
    loop {
        // Per-client errors are handled in its worker; accept errors terminate instead of spinning.
        listener.serve_one_logged(&session, Arc::clone(&log))?;
    }
}

#[cfg(debug_assertions)]
pub fn run_controller_once_for_test(
    paths: ControllerPaths,
    session: ControllerSession,
    ready: std::sync::mpsc::Sender<()>,
) -> Result<(), ControllerError> {
    paths
        .prepare_data_root()
        .map_err(controller_context("prepare controller data root"))?;
    let mut listener = LocalIpcListener::bind(paths.endpoint())
        .map_err(controller_context("bind controller IPC"))?;
    let _ = ready.send(());
    listener.serve_one_blocking(&session)?;
    Ok(())
}

#[cfg(debug_assertions)]
fn run_controller_once_with_local_node_for_test() -> Result<(), ControllerError> {
    let paths = ControllerPaths::discover()?;
    paths
        .prepare_data_root()
        .map_err(controller_context("prepare controller data root"))?;
    let _lock = InstanceLock::acquire(paths.lock_file())
        .map_err(controller_context("acquire controller lock"))?;
    let instance_id = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ControllerError::InvalidMessage)?
            .as_nanos()
    );
    let node_instance_id = format!("node-{instance_id}");
    let node_spec = LocalNodeSpec::for_controller(
        &paths,
        sibling_node_executable()?,
        node_instance_id.clone(),
    )?;
    let node_endpoint = node_spec.endpoint().to_owned();
    let mut node_supervisor = LocalNodeSupervisor::new(node_spec);
    node_supervisor.start().map_err(|error| match error {
        ControllerError::Io(source) => ControllerError::IoContext {
            context: "start local node",
            source,
        },
        other => other,
    })?;
    let workspace = std::env::current_dir()?;
    let session = ControllerSession::with_local_node_and_storage(
        instance_id,
        node_endpoint,
        node_instance_id,
        paths.data_root(),
        workspace,
    )?;
    let mut listener = LocalIpcListener::bind(paths.endpoint())
        .map_err(controller_context("bind controller IPC"))?;
    let result = listener.serve_one_blocking(&session);
    let stop = node_supervisor.stop();
    result?;
    stop
}

fn controller_context(context: &'static str) -> impl FnOnce(ControllerError) -> ControllerError {
    move |error| match error {
        ControllerError::Io(source) => ControllerError::IoContext { context, source },
        other => other,
    }
}

fn sibling_node_executable() -> Result<std::path::PathBuf, ControllerError> {
    let current = std::env::current_exe()?.canonicalize()?;
    let directory = current.parent().ok_or(ControllerError::PathUnavailable)?;
    #[cfg(windows)]
    let candidate = directory.join("pinvou-node.exe");
    #[cfg(not(windows))]
    let candidate = directory.join("pinvou-node");
    let candidate = candidate.canonicalize()?;
    if !candidate.is_absolute() || !candidate.metadata()?.is_file() {
        return Err(ControllerError::PathUnavailable);
    }
    Ok(candidate)
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
