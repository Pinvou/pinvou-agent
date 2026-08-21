use std::{
    io::{Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use pinvou_protocol::{HelloClient, HelloServer, encode_frame, read_frame};

use crate::{ControllerError, ControllerPaths, LocalEndpoint};

#[derive(Clone, Debug)]
pub struct LocalNodeSpec {
    executable: PathBuf,
    endpoint: String,
    instance_id: String,
    cleanup_path: Option<PathBuf>,
    lock_file: PathBuf,
}

impl LocalNodeSpec {
    pub fn for_controller(
        paths: &ControllerPaths,
        executable: PathBuf,
        instance_id: impl Into<String>,
    ) -> Result<Self, ControllerError> {
        let instance_id = instance_id.into();
        if executable.as_os_str().is_empty() || instance_id.is_empty() {
            return Err(ControllerError::InvalidMessage);
        }
        let (endpoint, cleanup_path) = match paths.endpoint() {
            LocalEndpoint::WindowsPipe(controller) => (format!("{controller}-node"), None),
            LocalEndpoint::UnixSocket(_) => {
                let socket = paths.runtime_root().join("pinvou/node.sock");
                (socket.display().to_string(), Some(socket))
            }
        };
        Ok(Self {
            executable,
            endpoint,
            instance_id,
            cleanup_path,
            lock_file: paths.data_root().join("node.lock"),
        })
    }
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeProcessStatus {
    Stopped,
    Running,
    RestartScheduled,
    RestartExhausted,
    CleanupFailed,
}

pub trait SupervisedChild: Send {
    fn try_exit(&mut self) -> std::io::Result<Option<i32>>;
    fn stop(&mut self) -> std::io::Result<()>;
    fn wait(&mut self) -> std::io::Result<()>;
    fn diagnostic_pid(&self) -> u32;
}

pub trait LocalNodeLauncher: Send + Sync {
    fn launch(&self, spec: &LocalNodeSpec) -> Result<Box<dyn SupervisedChild>, ControllerError>;
}

pub trait LocalNodeProbe: Send + Sync {
    fn protocol_version(&self, spec: &LocalNodeSpec) -> Result<u16, ControllerError>;
}

pub struct LocalNodeSupervisor {
    spec: LocalNodeSpec,
    launcher: Box<dyn LocalNodeLauncher>,
    probe: Box<dyn LocalNodeProbe>,
    child: Option<ManagedChild>,
    status: NodeProcessStatus,
    restart_count: u32,
    max_restarts: u32,
    base_backoff: Duration,
    max_backoff: Duration,
    restart_at: Option<Instant>,
}

impl LocalNodeSupervisor {
    pub fn new(spec: LocalNodeSpec) -> Self {
        Self::with_dependencies(
            spec,
            Box::new(ProcessNodeLauncher),
            Box::new(ProcessNodeProbe::default()),
            5,
            Duration::from_millis(100),
            Duration::from_secs(5),
        )
    }

    pub fn with_dependencies(
        spec: LocalNodeSpec,
        launcher: Box<dyn LocalNodeLauncher>,
        probe: Box<dyn LocalNodeProbe>,
        max_restarts: u32,
        base_backoff: Duration,
        max_backoff: Duration,
    ) -> Self {
        Self {
            spec,
            launcher,
            probe,
            child: None,
            status: NodeProcessStatus::Stopped,
            restart_count: 0,
            max_restarts,
            base_backoff,
            max_backoff,
            restart_at: None,
        }
    }

    pub fn start(&mut self) -> Result<(), ControllerError> {
        if self.child.is_some() {
            return Ok(());
        }
        self.launch_and_validate(Instant::now())
    }

    pub fn poll(&mut self, now: Instant) -> Result<NodeProcessStatus, ControllerError> {
        if self.status == NodeProcessStatus::RestartScheduled {
            if self.restart_at.is_some_and(|deadline| now >= deadline) {
                self.launch_and_validate(now)?;
            }
            return Ok(self.status);
        }
        let Some(child) = self.child.as_mut() else {
            return Ok(self.status);
        };
        if child.try_exit()?.is_none() {
            return Ok(NodeProcessStatus::Running);
        }
        let child = self.child.take().expect("child was observed above");
        self.terminate_child(child)?;
        if self.restart_count >= self.max_restarts {
            self.status = NodeProcessStatus::RestartExhausted;
            return Err(ControllerError::NodeRestartExhausted);
        }
        let multiplier = 1_u32
            .checked_shl(self.restart_count.min(31))
            .unwrap_or(u32::MAX);
        let delay = self
            .base_backoff
            .saturating_mul(multiplier)
            .min(self.max_backoff);
        self.restart_count += 1;
        self.restart_at = Some(now + delay);
        self.status = NodeProcessStatus::RestartScheduled;
        Ok(self.status)
    }

    pub fn stop(&mut self) -> Result<(), ControllerError> {
        if let Some(child) = self.child.take() {
            self.terminate_child(child)?;
        }
        self.restart_at = None;
        self.restart_count = 0;
        self.status = NodeProcessStatus::Stopped;
        self.cleanup_endpoint()
    }

    pub const fn status(&self) -> NodeProcessStatus {
        self.status
    }
    pub fn diagnostic_pid(&self) -> Option<u32> {
        self.child.as_ref().map(|child| child.diagnostic_pid())
    }

    fn launch_and_validate(&mut self, now: Instant) -> Result<(), ControllerError> {
        let child = self.launcher.launch(&self.spec)?;
        match self.probe.protocol_version(&self.spec) {
            Ok(pinvou_protocol::IPC_VERSION) => {
                self.child = Some(ManagedChild::new(child));
                self.status = NodeProcessStatus::Running;
                self.restart_at = None;
                Ok(())
            }
            Ok(_) | Err(ControllerError::ProtocolMismatch) => {
                self.terminate_child(ManagedChild::new(child))?;
                self.status = NodeProcessStatus::Stopped;
                Err(ControllerError::ProtocolMismatch)
            }
            Err(error) => {
                self.terminate_child(ManagedChild::new(child))?;
                if self.restart_count >= self.max_restarts {
                    self.status = NodeProcessStatus::RestartExhausted;
                    return Err(ControllerError::NodeRestartExhausted);
                }
                let multiplier = 1_u32
                    .checked_shl(self.restart_count.min(31))
                    .unwrap_or(u32::MAX);
                let delay = self
                    .base_backoff
                    .saturating_mul(multiplier)
                    .min(self.max_backoff);
                self.restart_count += 1;
                self.restart_at = Some(now + delay);
                self.status = NodeProcessStatus::RestartScheduled;
                let _ = error;
                Ok(())
            }
        }
    }

    fn cleanup_endpoint(&self) -> Result<(), ControllerError> {
        if let Some(path) = &self.spec.cleanup_path
            && path.exists()
        {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    fn terminate_child(&mut self, mut child: ManagedChild) -> Result<(), ControllerError> {
        if let Err(error) = child.stop_tree() {
            self.child = Some(child);
            self.status = NodeProcessStatus::CleanupFailed;
            return Err(ControllerError::Io(error));
        }
        if let Err(error) = child.reap() {
            self.child = Some(child);
            self.status = NodeProcessStatus::CleanupFailed;
            return Err(ControllerError::Io(error));
        }
        if let Err(error) = self.cleanup_endpoint() {
            self.child = Some(child);
            self.status = NodeProcessStatus::CleanupFailed;
            return Err(error);
        }
        Ok(())
    }
}

struct ManagedChild {
    process: Box<dyn SupervisedChild>,
    tree_terminated: bool,
    reaped: bool,
}

impl ManagedChild {
    fn new(process: Box<dyn SupervisedChild>) -> Self {
        Self {
            process,
            tree_terminated: false,
            reaped: false,
        }
    }
    fn try_exit(&mut self) -> std::io::Result<Option<i32>> {
        let status = self.process.try_exit()?;
        if status.is_some() {
            self.reaped = true;
        }
        Ok(status)
    }
    fn stop_tree(&mut self) -> std::io::Result<()> {
        if !self.tree_terminated {
            self.process.stop()?;
            self.tree_terminated = true;
        }
        Ok(())
    }
    fn reap(&mut self) -> std::io::Result<()> {
        if !self.reaped {
            self.process.wait()?;
            self.reaped = true;
        }
        Ok(())
    }
    fn diagnostic_pid(&self) -> u32 {
        self.process.diagnostic_pid()
    }
}

impl Drop for LocalNodeSupervisor {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub struct ProcessNodeLauncher;
impl LocalNodeLauncher for ProcessNodeLauncher {
    fn launch(&self, spec: &LocalNodeSpec) -> Result<Box<dyn SupervisedChild>, ControllerError> {
        let mut command = Command::new(&spec.executable);
        command
            .arg("--endpoint")
            .arg(&spec.endpoint)
            .arg("--instance-id")
            .arg(&spec.instance_id)
            .arg("--lock-file")
            .arg(&spec.lock_file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_child_group(&mut command);
        let child = command.spawn()?;
        Ok(Box::new(ProcessChild::new(child)?))
    }
}

struct ProcessChild {
    child: Child,
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(target_os = "linux")]
    process_group: i32,
}
unsafe impl Send for ProcessChild {}
impl ProcessChild {
    fn new(mut child: Child) -> std::io::Result<Self> {
        #[cfg(windows)]
        {
            let job = match create_kill_on_close_job(&child) {
                Ok(job) => job,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
            };
            if let Err(error) = resume_primary_thread(child.id()) {
                unsafe { windows_sys::Win32::Foundation::CloseHandle(job) };
                let _ = child.wait();
                return Err(error);
            }
            return Ok(Self { child, job });
        }
        #[cfg(not(windows))]
        {
            #[cfg(target_os = "linux")]
            return Ok(Self {
                process_group: child.id() as i32,
                child,
            });
            #[cfg(not(target_os = "linux"))]
            Ok(Self { child })
        }
    }
}
impl SupervisedChild for ProcessChild {
    fn try_exit(&mut self) -> std::io::Result<Option<i32>> {
        Ok(self
            .child
            .try_wait()?
            .map(|status| status.code().unwrap_or(1)))
    }
    fn stop(&mut self) -> std::io::Result<()> {
        #[cfg(any(windows, target_os = "linux"))]
        return stop_child_tree(self);
        #[cfg(not(any(windows, target_os = "linux")))]
        match self.child.try_wait()? {
            Some(_) => Ok(()),
            None => stop_child_tree(self),
        }
    }
    fn wait(&mut self) -> std::io::Result<()> {
        self.child.wait().map(|_| ())
    }
    fn diagnostic_pid(&self) -> u32 {
        self.child.id()
    }
}

#[cfg(windows)]
impl Drop for ProcessChild {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.job) };
    }
}

#[cfg(windows)]
fn configure_child_group(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(
        windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP
            | windows_sys::Win32::System::Threading::CREATE_SUSPENDED,
    );
}
#[cfg(target_os = "linux")]
fn configure_child_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    let expected_parent = unsafe { libc::getpid() };
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // Close the race where the controller dies between fork and prctl.
            if libc::getppid() != expected_parent {
                return Err(std::io::Error::other(
                    "controller exited during node launch",
                ));
            }
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // T8 runtime descendants must inherit an equivalent parent-death/tree contract.
            Ok(())
        });
    }
}
#[cfg(not(any(windows, target_os = "linux")))]
fn configure_child_group(_: &mut Command) {}

#[cfg(windows)]
fn create_kill_on_close_job(
    child: &Child,
) -> std::io::Result<windows_sys::Win32::Foundation::HANDLE> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let configured = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            std::mem::size_of_val(&info) as u32,
        )
    };
    let assigned = configured != 0
        && unsafe { AssignProcessToJobObject(job, child.as_raw_handle().cast()) } != 0;
    if !assigned {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(job) };
        return Err(std::io::Error::last_os_error());
    }
    Ok(job)
}

#[cfg(windows)]
fn resume_primary_thread(process_id: u32) -> std::io::Result<()> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
    };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
    let mut found = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while found && entry.th32OwnerProcessID != process_id {
        found = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    if !found {
        return Err(std::io::Error::other(
            "suspended node primary thread not found",
        ));
    }
    let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
    if thread.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    let resumed = unsafe { ResumeThread(thread) };
    unsafe { CloseHandle(thread) };
    if resumed == u32::MAX {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn stop_child_tree(process: &mut ProcessChild) -> std::io::Result<()> {
    if unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(process.job, 1) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
#[cfg(target_os = "linux")]
fn stop_child_tree(process: &mut ProcessChild) -> std::io::Result<()> {
    let group = -process.process_group;
    if unsafe { libc::kill(group, libc::SIGTERM) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            let _ = process.child.try_wait()?;
            return Ok(());
        }
        return Err(error);
    }
    for _ in 0..20 {
        let _ = process.child.try_wait()?;
        if !process_group_exists(process.process_group)? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if unsafe { libc::kill(group, libc::SIGKILL) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error);
        }
    }
    for _ in 0..100 {
        let _ = process.child.try_wait()?;
        if !process_group_exists(process.process_group)? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "local node process group survived SIGKILL",
    ))
}

#[cfg(target_os = "linux")]
fn process_group_exists(process_group: i32) -> std::io::Result<bool> {
    if unsafe { libc::kill(-process_group, 0) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(error),
    }
}
#[cfg(not(any(windows, target_os = "linux")))]
fn stop_child_tree(process: &mut ProcessChild) -> std::io::Result<()> {
    process.child.kill()
}

pub struct ProcessNodeProbe {
    attempts: usize,
    retry_delay: Duration,
}
impl Default for ProcessNodeProbe {
    fn default() -> Self {
        Self {
            attempts: 40,
            retry_delay: Duration::from_millis(25),
        }
    }
}
impl LocalNodeProbe for ProcessNodeProbe {
    fn protocol_version(&self, spec: &LocalNodeSpec) -> Result<u16, ControllerError> {
        let mut last = None;
        for _ in 0..self.attempts {
            match connect_endpoint(spec.endpoint()) {
                Ok(mut stream) => {
                    let hello =
                        HelloClient::new(serde_json::json!({"client":"local-node-supervisor"}))
                            .map_err(|_| ControllerError::InvalidMessage)?;
                    stream.write_all(
                        &encode_frame(&hello).map_err(|_| ControllerError::InvalidMessage)?,
                    )?;
                    let answer: HelloServer =
                        read_frame(&mut stream).map_err(|_| ControllerError::ProtocolMismatch)?;
                    if answer.instance_id() != spec.instance_id() {
                        return Err(ControllerError::ProtocolMismatch);
                    }
                    return Ok(answer.protocol_version());
                }
                Err(error) => last = Some(error),
            }
            std::thread::sleep(self.retry_delay);
        }
        Err(ControllerError::Io(last.unwrap_or_else(|| {
            std::io::Error::other("node endpoint unavailable")
        })))
    }
}

pub(crate) trait ReadWrite: Read + Write + Send {}
impl<T: Read + Write + Send> ReadWrite for T {}

#[cfg(windows)]
pub(crate) fn connect_endpoint(endpoint: &str) -> std::io::Result<Box<dyn ReadWrite>> {
    Ok(Box::new(
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(endpoint)?,
    ))
}
#[cfg(target_os = "linux")]
pub(crate) fn connect_endpoint(endpoint: &str) -> std::io::Result<Box<dyn ReadWrite>> {
    Ok(Box::new(std::os::unix::net::UnixStream::connect(
        std::path::Path::new(endpoint),
    )?))
}
#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) fn connect_endpoint(_: &str) -> std::io::Result<Box<dyn ReadWrite>> {
    Err(std::io::Error::other("unsupported local IPC platform"))
}

#[cfg(all(test, target_os = "linux"))]
mod linux_process_tree_tests {
    use super::*;

    #[test]
    fn abnormal_controller_exit_kills_the_node_leader_via_parent_death_contract() {
        const HELPER: &str = "PINVOU_PDEATH_TEST_HELPER";
        const PID_FILE: &str = "PINVOU_PDEATH_TEST_PID_FILE";
        if std::env::var_os(HELPER).is_some() {
            let pid_file = std::env::var_os(PID_FILE).unwrap();
            let mut command = Command::new("sh");
            command.arg("-c").arg(format!(
                "echo $$ > '{}'; while :; do sleep 1; done",
                PathBuf::from(pid_file).display()
            ));
            configure_child_group(&mut command);
            let child = command.spawn().unwrap();
            let _process = ProcessChild::new(child).unwrap();
            for _ in 0..100 {
                if PathBuf::from(std::env::var_os(PID_FILE).unwrap()).exists() {
                    std::process::exit(0);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            std::process::exit(1);
        }

        let root = std::env::temp_dir().join(format!(
            "pinvou-parent-death-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let pid_file = root.join("node.pid");
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "local_node_supervisor::linux_process_tree_tests::abnormal_controller_exit_kills_the_node_leader_via_parent_death_contract",
                "--nocapture",
            ])
            .env(HELPER, "1")
            .env(PID_FILE, &pid_file)
            .status()
            .unwrap();
        assert!(status.success());
        let node_pid: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        for _ in 0..200 {
            if !process_is_running(node_pid) {
                std::fs::remove_dir_all(root).unwrap();
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        unsafe { libc::kill(node_pid, libc::SIGKILL) };
        panic!("node survived controller parent death");
    }

    fn process_is_running(pid: i32) -> bool {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        stat.rsplit_once(") ")
            .and_then(|(_, tail)| tail.chars().next())
            .is_some_and(|state| state != 'Z')
    }

    #[test]
    fn exited_leader_still_cleans_a_term_ignoring_grandchild_group() {
        let root = std::env::temp_dir().join(format!(
            "pinvou-process-group-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let pid_file = root.join("grandchild.pid");
        let script = format!(
            "sh -c 'trap \"\" TERM; echo $$ > \"{}\"; while :; do sleep 1; done' & exit 17",
            pid_file.display()
        );
        let mut command = Command::new("sh");
        command.arg("-c").arg(script);
        configure_child_group(&mut command);
        let child = command.spawn().unwrap();
        let mut process = ProcessChild::new(child).unwrap();
        for _ in 0..100 {
            if pid_file.exists() && process.try_exit().unwrap().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let grandchild: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(unsafe { libc::kill(grandchild, 0) }, 0);
        process.stop().unwrap();
        process.wait().unwrap();
        for _ in 0..100 {
            if unsafe { libc::kill(grandchild, 0) } != 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(unsafe { libc::kill(grandchild, 0) }, -1);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(all(test, windows))]
mod windows_process_tree_tests {
    use super::*;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_OBJECT_0},
        System::Threading::{OpenProcess, WaitForSingleObject},
    };

    #[test]
    fn job_termination_cleans_a_real_grandchild_even_after_the_leader_exits() {
        let root = std::env::temp_dir().join(format!(
            "pinvou-job-tree-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let pid_file = root.join("grandchild.pid");
        let script = format!(
            "$p=Start-Process powershell.exe -PassThru -ArgumentList '-NoProfile','-Command','while($true){{Start-Sleep -Milliseconds 100}}'; Set-Content -LiteralPath '{}' -Value $p.Id; [Environment]::Exit(17)",
            pid_file.display()
        );
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-Command", &script]);
        configure_child_group(&mut command);
        let child = command.spawn().unwrap();
        let mut process = ProcessChild::new(child).unwrap();
        for _ in 0..200 {
            if pid_file.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let grandchild: u32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let mut leader_exited = false;
        for _ in 0..500 {
            if process.try_exit().unwrap().is_some() {
                leader_exited = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(leader_exited);
        const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
        let handle = unsafe { OpenProcess(SYNCHRONIZE_ACCESS, 0, grandchild) };
        assert!(!handle.is_null());
        process.stop().unwrap();
        assert_eq!(unsafe { WaitForSingleObject(handle, 5_000) }, WAIT_OBJECT_0);
        unsafe { CloseHandle(handle) };
        std::fs::remove_dir_all(root).unwrap();
    }
}
