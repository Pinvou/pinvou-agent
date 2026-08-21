use std::{
    collections::VecDeque,
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use pinvou_controller::{
    ControllerError, ControllerPaths, ControllerSession, HostPlatform, LocalIpcListener,
    LocalNodeClient, LocalNodeLauncher, LocalNodeProbe, LocalNodeSpec, LocalNodeSupervisor,
    NodeProcessStatus, SupervisedChild,
};

#[derive(Clone)]
struct FakeLauncher {
    exits: Arc<Mutex<VecDeque<Option<i32>>>>,
    launches: Arc<Mutex<usize>>,
}
struct FakeChild {
    exit: Option<i32>,
    stopped: bool,
}
struct StopFailingChild;
impl SupervisedChild for StopFailingChild {
    fn try_exit(&mut self) -> io::Result<Option<i32>> {
        Ok(None)
    }
    fn stop(&mut self) -> io::Result<()> {
        Err(io::Error::other("stop failed"))
    }
    fn wait(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn diagnostic_pid(&self) -> u32 {
        9898
    }
}
struct StopFailingLauncher;
impl LocalNodeLauncher for StopFailingLauncher {
    fn launch(&self, _: &LocalNodeSpec) -> Result<Box<dyn SupervisedChild>, ControllerError> {
        Ok(Box::new(StopFailingChild))
    }
}
#[derive(Default)]
struct LifecycleCounts {
    stops: usize,
    waits: usize,
}
struct CountingChild(Arc<Mutex<LifecycleCounts>>);
impl SupervisedChild for CountingChild {
    fn try_exit(&mut self) -> io::Result<Option<i32>> {
        Ok(None)
    }
    fn stop(&mut self) -> io::Result<()> {
        self.0.lock().unwrap().stops += 1;
        Ok(())
    }
    fn wait(&mut self) -> io::Result<()> {
        self.0.lock().unwrap().waits += 1;
        Ok(())
    }
    fn diagnostic_pid(&self) -> u32 {
        7070
    }
}
struct CountingLauncher(Arc<Mutex<LifecycleCounts>>);
impl LocalNodeLauncher for CountingLauncher {
    fn launch(&self, _: &LocalNodeSpec) -> Result<Box<dyn SupervisedChild>, ControllerError> {
        Ok(Box::new(CountingChild(Arc::clone(&self.0))))
    }
}
impl SupervisedChild for FakeChild {
    fn try_exit(&mut self) -> io::Result<Option<i32>> {
        Ok(self.exit.take())
    }
    fn stop(&mut self) -> io::Result<()> {
        self.stopped = true;
        Ok(())
    }
    fn wait(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn diagnostic_pid(&self) -> u32 {
        4242
    }
}
impl LocalNodeLauncher for FakeLauncher {
    fn launch(&self, _: &LocalNodeSpec) -> Result<Box<dyn SupervisedChild>, ControllerError> {
        *self.launches.lock().unwrap() += 1;
        Ok(Box::new(FakeChild {
            exit: self.exits.lock().unwrap().pop_front().flatten(),
            stopped: false,
        }))
    }
}
struct FakeProbe(u16);
impl LocalNodeProbe for FakeProbe {
    fn protocol_version(&self, _: &LocalNodeSpec) -> Result<u16, ControllerError> {
        Ok(self.0)
    }
}
struct UnavailableProbe;
impl LocalNodeProbe for UnavailableProbe {
    fn protocol_version(&self, _: &LocalNodeSpec) -> Result<u16, ControllerError> {
        Err(ControllerError::Io(io::Error::other("not ready")))
    }
}

fn spec() -> LocalNodeSpec {
    let root = std::env::temp_dir().join(format!(
        "pinvou-node-spec-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let paths = ControllerPaths::from_roots(
        HostPlatform::Linux,
        root.join("data"),
        root.join("runtime"),
        "unused",
    )
    .unwrap();
    LocalNodeSpec::for_controller(&paths, PathBuf::from("pinvou-node"), "instance-1").unwrap()
}

#[test]
fn local_node_spec_places_runtime_state_in_the_private_data_root() {
    let root = std::env::temp_dir().join(format!(
        "pinvou-node-state-spec-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let paths = ControllerPaths::from_roots(
        HostPlatform::Linux,
        root.join("data"),
        root.join("runtime"),
        "unused",
    )
    .unwrap();
    let spec =
        LocalNodeSpec::for_controller(&paths, PathBuf::from("pinvou-node"), "instance-1").unwrap();

    assert_eq!(
        spec.state_file(),
        &root.join("data").join("runtime-selection.json")
    );
}

#[test]
fn crashes_restart_with_bounded_exponential_backoff() {
    let launches = Arc::new(Mutex::new(0));
    let launcher = FakeLauncher {
        exits: Arc::new(Mutex::new(VecDeque::from([Some(5), None]))),
        launches: launches.clone(),
    };
    let now = Instant::now();
    let mut supervisor = LocalNodeSupervisor::with_dependencies(
        spec(),
        Box::new(launcher),
        Box::new(FakeProbe(1)),
        3,
        Duration::from_millis(10),
        Duration::from_millis(40),
    );
    supervisor.start().unwrap();
    assert_eq!(
        supervisor.poll(now).unwrap(),
        NodeProcessStatus::RestartScheduled
    );
    assert_eq!(
        supervisor.poll(now + Duration::from_millis(9)).unwrap(),
        NodeProcessStatus::RestartScheduled
    );
    assert_eq!(
        supervisor.poll(now + Duration::from_millis(10)).unwrap(),
        NodeProcessStatus::Running
    );
    assert_eq!(*launches.lock().unwrap(), 2);
}

#[test]
fn protocol_mismatch_stops_child_and_is_never_reported_running() {
    let launcher = FakeLauncher {
        exits: Arc::new(Mutex::new(VecDeque::from([None]))),
        launches: Arc::new(Mutex::new(0)),
    };
    let mut supervisor = LocalNodeSupervisor::with_dependencies(
        spec(),
        Box::new(launcher),
        Box::new(FakeProbe(2)),
        3,
        Duration::from_millis(10),
        Duration::from_millis(40),
    );
    assert!(matches!(
        supervisor.start(),
        Err(ControllerError::ProtocolMismatch)
    ));
    assert_eq!(supervisor.status(), NodeProcessStatus::Stopped);
}

#[test]
fn explicit_stop_uses_child_handle_and_cleans_endpoint_not_pid_liveness() {
    let spec = spec();
    let endpoint = PathBuf::from(spec.endpoint());
    std::fs::create_dir_all(endpoint.parent().unwrap()).unwrap();
    std::fs::write(&endpoint, b"stale endpoint").unwrap();
    let launcher = FakeLauncher {
        exits: Arc::new(Mutex::new(VecDeque::from([None]))),
        launches: Arc::new(Mutex::new(0)),
    };
    let mut supervisor = LocalNodeSupervisor::with_dependencies(
        spec,
        Box::new(launcher),
        Box::new(FakeProbe(1)),
        3,
        Duration::from_millis(10),
        Duration::from_millis(40),
    );
    supervisor.start().unwrap();
    assert_eq!(supervisor.diagnostic_pid(), Some(4242));
    supervisor.stop().unwrap();
    assert_eq!(supervisor.status(), NodeProcessStatus::Stopped);
    assert!(!endpoint.exists());
    let _ = std::fs::remove_dir_all(endpoint.parent().unwrap().parent().unwrap());
}

#[test]
fn transient_readiness_failure_enters_bounded_restart_instead_of_permanent_rejection() {
    let launcher = FakeLauncher {
        exits: Arc::new(Mutex::new(VecDeque::from([None]))),
        launches: Arc::new(Mutex::new(0)),
    };
    let mut supervisor = LocalNodeSupervisor::with_dependencies(
        spec(),
        Box::new(launcher),
        Box::new(UnavailableProbe),
        1,
        Duration::from_millis(10),
        Duration::from_millis(10),
    );
    supervisor.start().unwrap();
    assert_eq!(supervisor.status(), NodeProcessStatus::RestartScheduled);
}

#[test]
fn failed_stop_retains_the_supervision_handle_and_never_claims_stopped() {
    let mut supervisor = LocalNodeSupervisor::with_dependencies(
        spec(),
        Box::new(StopFailingLauncher),
        Box::new(FakeProbe(1)),
        1,
        Duration::from_millis(10),
        Duration::from_millis(10),
    );
    supervisor.start().unwrap();
    assert!(matches!(supervisor.stop(), Err(ControllerError::Io(_))));
    assert_eq!(supervisor.status(), NodeProcessStatus::CleanupFailed);
    assert_eq!(supervisor.diagnostic_pid(), Some(9898));
}

#[test]
fn endpoint_cleanup_failure_never_reterminates_or_reaps_the_old_process_tree() {
    let spec = spec();
    let endpoint = PathBuf::from(spec.endpoint());
    std::fs::create_dir_all(&endpoint).unwrap();
    let counts = Arc::new(Mutex::new(LifecycleCounts::default()));
    let mut supervisor = LocalNodeSupervisor::with_dependencies(
        spec,
        Box::new(CountingLauncher(Arc::clone(&counts))),
        Box::new(FakeProbe(pinvou_protocol::IPC_VERSION)),
        1,
        Duration::from_millis(1),
        Duration::from_millis(1),
    );
    supervisor.start().unwrap();
    assert!(supervisor.stop().is_err());
    assert!(supervisor.stop().is_err());
    let counts = counts.lock().unwrap();
    assert_eq!(counts.stops, 1);
    assert_eq!(counts.waits, 1);
    drop(counts);
    std::fs::remove_dir_all(endpoint.parent().unwrap().parent().unwrap()).unwrap();
}

#[test]
fn local_node_client_performs_real_instance_challenge_and_health_ipc() {
    let root = std::env::temp_dir().join(format!(
        "pinvou-local-node-client-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let platform = HostPlatform::current().unwrap();
    let paths = ControllerPaths::from_roots(
        platform,
        root.join("data"),
        root.join("runtime"),
        "node-client-contract",
    )
    .unwrap();
    let endpoint = paths.endpoint().display();
    let mut listener = LocalIpcListener::bind(paths.endpoint()).unwrap();
    let client = std::thread::spawn(move || {
        let mut client = LocalNodeClient::connect(&endpoint, "client-instance").unwrap();
        client.health().unwrap();
    });
    listener
        .serve_one(&ControllerSession::new("client-instance").unwrap())
        .unwrap();
    client.join().unwrap();
    drop(listener);
    let _ = std::fs::remove_dir_all(root);
}
