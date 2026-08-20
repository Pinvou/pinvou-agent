//! Linux 同 UID Host Supervisor。
//!
//! daemon 只接受 systemd socket activation 的 AF_UNIX listener，只映射内嵌 descriptor
//! 到固定 user unit。没有任意 PID、unit、命令、shell 或 property 输入。

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pinvou_host_supervisor_protocol::{
    validate_instance_generation, CgroupObservation, HostWorkObservation, ManagedHostWork,
    MemoryPressure, ObservedWorkState, PressureLine, ProtocolError, SupervisorAction,
    SupervisorOutcome, SupervisorReceipt, SupervisorRequest, MAX_DETAIL_BYTES, MAX_REQUEST_BYTES,
    MAX_RESPONSE_BYTES, PROTOCOL_VERSION,
};
use serde::{Deserialize, Serialize};
use wait_timeout::ChildExt;

const DESCRIPTOR_JSON: &str = include_str!("../../descriptor/pinvou-app-v1.json");
const ASR_DESCRIPTOR_JSON: &str = include_str!("../../descriptor/pinvou-asr-v1.json");
const APP_UNIT: &str = "pinvou3-app.service";
const ASR_UNIT: &str = "pinvou-qwen3-asr.service";
const SOCKET_UNIT: &str = "pinvou3-supervisor.socket";
const SUPERVISOR_UNIT: &str = "pinvou3-supervisor.service";
const SOCKET_RELATIVE_PATH: &str = "pinvou-supervisor/control.sock";
const SYSTEMCTL_TIMEOUT: Duration = Duration::from_secs(12);
const IO_TIMEOUT: Duration = Duration::from_secs(8);
const CLIENT_CONNECT_RETRIES: usize = 30;
const CLIENT_CONNECT_RETRY_DELAY: Duration = Duration::from_millis(50);
const MAX_ACTIVE_CLIENTS: usize = 16;
const MAX_OBSERVATION_JOURNAL_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CONTROL_JOURNAL_BYTES: u64 = 4 * 1024 * 1024;
// Capacity exhaustion rejects a new directive id. Existing ids, especially Pending ids, are
// never evicted because forgetting one could replay a destructive side effect after restart.
const MAX_STORED_REQUESTS: usize = 65_536;
const MONITOR_INTERVAL: Duration = Duration::from_secs(2);
const MONITOR_HEARTBEAT: Duration = Duration::from_secs(30);
static COMPACTION_SEQUENCE: AtomicUsize = AtomicUsize::new(1);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmbeddedDescriptor {
    schema_version: u16,
    descriptor_id: String,
    descriptor_revision: String,
    unit: String,
    executable: String,
    allowed_actions: Vec<String>,
    resource_policy_owner: String,
    required_profile: String,
    resource_policy: EmbeddedResourcePolicy,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmbeddedResourcePolicy {
    memory_high: String,
    memory_max: String,
    memory_swap_max: String,
    oom_policy: String,
    kill_mode: String,
    tasks_max: u64,
    restart: String,
    restart_sec: String,
    start_limit_interval_sec: String,
    start_limit_burst: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmbeddedAsrDescriptor {
    schema_version: u16,
    descriptor_id: String,
    descriptor_revision: String,
    unit: String,
    fragment_suffix: String,
    executable_suffix: String,
    script_suffix: String,
    allowed_actions: Vec<String>,
    resource_policy_owner: String,
    resource_policy: EmbeddedAsrResourcePolicy,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmbeddedAsrResourcePolicy {
    memory_high: String,
    memory_max: String,
    memory_swap_max: String,
    oom_policy: String,
    kill_mode: String,
    tasks_max: u64,
    restart: String,
    restart_sec: String,
    start_limit_interval_sec: String,
    start_limit_burst: u64,
}

fn validate_embedded_descriptor() -> Result<(), String> {
    let descriptor: EmbeddedDescriptor =
        serde_json::from_str(DESCRIPTOR_JSON).map_err(|error| format!("descriptor: {error}"))?;
    let expected_actions = ["status", "launch"];
    if descriptor.schema_version != 1
        || descriptor.descriptor_id != ManagedHostWork::PinvouApp.descriptor_id()
        || descriptor.descriptor_revision != ManagedHostWork::PinvouApp.descriptor_revision()
        || descriptor.unit != APP_UNIT
        || descriptor.executable != "/usr/bin/pinvou3-tauri"
        || descriptor.allowed_actions != expected_actions
        || descriptor.resource_policy_owner
            != "base app unit plus explicit MegaBook canary deployment drop-in"
        || descriptor.required_profile != "megabook-canary-v1"
    {
        return Err("embedded descriptor identity/action contract mismatch".to_string());
    }
    let policy = descriptor.resource_policy;
    if policy.memory_high != "4G"
        || policy.memory_max != "8G"
        || policy.memory_swap_max != "2G"
        || policy.oom_policy != "kill"
        || policy.kill_mode != "control-group"
        || policy.tasks_max != 512
        || policy.restart != "on-failure"
        || policy.restart_sec != "15s"
        || policy.start_limit_interval_sec != "300s"
        || policy.start_limit_burst != 3
    {
        return Err("embedded descriptor resource policy contract mismatch".to_string());
    }
    let asr: EmbeddedAsrDescriptor = serde_json::from_str(ASR_DESCRIPTOR_JSON)
        .map_err(|error| format!("ASR descriptor: {error}"))?;
    if asr.schema_version != 1
        || asr.descriptor_id != ManagedHostWork::PinvouAsr.descriptor_id()
        || asr.descriptor_revision != ManagedHostWork::PinvouAsr.descriptor_revision()
        || asr.unit != ASR_UNIT
        || asr.fragment_suffix != "/.config/systemd/user/pinvou-qwen3-asr.service"
        || asr.executable_suffix != "/.pinvou3/asr/qwen3-asr-openvino/runtime/bin/python"
        || asr.script_suffix != "/.pinvou3/asr/qwen3-asr-openvino/qwen3-asr-openvino.py"
        || asr.allowed_actions != ["status", "stop"]
        || asr.resource_policy_owner != "ASR base unit plus pinvou-supervisor package drop-in"
        || asr.resource_policy.memory_high != "20%"
        || asr.resource_policy.memory_max != "35%"
        || asr.resource_policy.memory_swap_max != "2G"
        || asr.resource_policy.oom_policy != "kill"
        || asr.resource_policy.kill_mode != "control-group"
        || asr.resource_policy.tasks_max != 128
        || asr.resource_policy.restart != "on-failure"
        || asr.resource_policy.restart_sec != "3s"
        || asr.resource_policy.start_limit_interval_sec != "60s"
        || asr.resource_policy.start_limit_burst != 3
    {
        return Err("embedded ASR descriptor identity/action contract mismatch".to_string());
    }
    Ok(())
}

const fn fixed_unit(target: ManagedHostWork) -> &'static str {
    match target {
        ManagedHostWork::PinvouApp => APP_UNIT,
        ManagedHostWork::PinvouAsr => ASR_UNIT,
    }
}

#[derive(Debug, Clone)]
struct UnitSnapshot {
    observation: HostWorkObservation,
    control_group: Option<String>,
    integrity_error: Option<String>,
}

trait UnitController: Send + Sync {
    fn status(&self, target: ManagedHostWork) -> Result<UnitSnapshot, String>;
    fn stop(&self, target: ManagedHostWork) -> Result<(), String>;
    fn launch_app(&self) -> Result<(), String>;
}

#[derive(Debug, Default)]
struct SystemdUnitController;

impl UnitController for SystemdUnitController {
    fn status(&self, target: ManagedHostWork) -> Result<UnitSnapshot, String> {
        let unit = fixed_unit(target);
        let output = run_systemctl(&[
            "--user",
            "show",
            unit,
            "--no-pager",
            "--property=ActiveState,SubState,Result,ControlGroup,MainPID,NRestarts,MemoryCurrent,MemoryPeak,MemoryHigh,MemoryMax,MemorySwapMax,OOMPolicy,KillMode,TasksMax,Restart,RestartUSec,StartLimitIntervalUSec,StartLimitBurst,InvocationID,FragmentPath,ExecStart",
        ])?;
        let properties = parse_properties(&output);
        let active_state = properties
            .get("ActiveState")
            .map(String::as_str)
            .unwrap_or("unknown");
        let state = match active_state {
            "active" => ObservedWorkState::Active,
            "activating" => ObservedWorkState::Activating,
            "deactivating" => ObservedWorkState::Deactivating,
            "inactive" => ObservedWorkState::Inactive,
            "failed" => ObservedWorkState::Failed,
            _ => ObservedWorkState::Unknown,
        };
        let control_group = properties
            .get("ControlGroup")
            .filter(|value| !value.is_empty() && value.as_str() != "/")
            .cloned();
        let mut cgroup = control_group
            .as_deref()
            .and_then(cgroup_directory)
            .map(|path| observe_cgroup(&path))
            .unwrap_or_default();
        if cgroup.memory_current_bytes.is_none() {
            cgroup.memory_current_bytes = parse_optional_u64(properties.get("MemoryCurrent"));
        }
        if cgroup.memory_peak_bytes.is_none() {
            cgroup.memory_peak_bytes = parse_optional_u64(properties.get("MemoryPeak"));
        }
        let observation = HostWorkObservation {
            instance_generation: valid_instance_generation(properties.get("InvocationID")),
            state,
            sub_state: bounded_property(properties.get("SubState")),
            unit_result: bounded_property(properties.get("Result")),
            main_pid: parse_optional_u64(properties.get("MainPID"))
                .filter(|pid| *pid > 0)
                .and_then(|pid| u32::try_from(pid).ok()),
            restart_count: parse_optional_u64(properties.get("NRestarts")),
            cgroup,
        };
        let integrity_error = validate_effective_unit(target, &observation, &properties).err();
        Ok(UnitSnapshot {
            observation,
            control_group,
            integrity_error,
        })
    }

    fn stop(&self, target: ManagedHostWork) -> Result<(), String> {
        run_systemctl(&["--user", "stop", fixed_unit(target)]).map(|_| ())
    }

    fn launch_app(&self) -> Result<(), String> {
        run_systemctl(&["--user", "start", APP_UNIT]).map(|_| ())
    }
}

fn fixed_systemctl_path() -> Result<&'static str, String> {
    ["/usr/bin/systemctl", "/bin/systemctl"]
        .into_iter()
        .find(|candidate| Path::new(candidate).is_file())
        .ok_or_else(|| "systemctl is unavailable at an audited absolute path".to_string())
}

fn run_systemctl(arguments: &[&str]) -> Result<String, String> {
    run_systemctl_with_timeout(arguments, SYSTEMCTL_TIMEOUT)
}

fn run_systemctl_with_timeout(arguments: &[&str], timeout: Duration) -> Result<String, String> {
    if timeout.is_zero() {
        return Err("fixed systemd operation has no remaining time budget".to_string());
    }
    let executable = fixed_systemctl_path()?;
    let mut child = Command::new(executable)
        .args(arguments)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn fixed systemd operation: {error}"))?;
    let status = child
        .wait_timeout(timeout)
        .map_err(|error| format!("wait fixed systemd operation: {error}"))?;
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("fixed systemd operation timed out".to_string());
    };
    let stdout = child
        .stdout
        .take()
        .map(read_bounded_output)
        .transpose()
        .map_err(|error| format!("read systemd stdout: {error}"))?
        .unwrap_or_default();
    let stderr = child
        .stderr
        .take()
        .map(read_bounded_output)
        .transpose()
        .map_err(|error| format!("read systemd stderr: {error}"))?
        .unwrap_or_default();
    if status.success() {
        Ok(stdout)
    } else {
        Err(format!(
            "fixed systemd operation exited {status}: {}",
            bound_detail(&stderr)
        ))
    }
}

fn systemd_main_pid(unit: &str, deadline: Instant) -> Result<u32, String> {
    let output = run_systemctl_with_timeout(
        &[
            "--user",
            "show",
            unit,
            "--no-pager",
            "--property=MainPID",
            "--value",
        ],
        remaining_client_budget(deadline)?.min(SYSTEMCTL_TIMEOUT),
    )?;
    output
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0)
        .ok_or_else(|| format!("{unit} has no live MainPID"))
}

fn read_bounded_output(mut reader: impl Read) -> std::io::Result<String> {
    let mut output = Vec::new();
    reader.by_ref().take(16 * 1024).read_to_end(&mut output)?;
    Ok(String::from_utf8_lossy(&output).into_owned())
}

fn parse_properties(output: &str) -> HashMap<String, String> {
    output
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(key, value)| {
            key.len() <= 32
                && value.len() <= 1024
                && key.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn parse_optional_u64(value: Option<&String>) -> Option<u64> {
    value
        .map(String::as_str)
        .filter(|value| !matches!(*value, "" | "[not set]" | "infinity" | "max"))
        .and_then(|value| value.parse().ok())
}

fn bounded_property(value: Option<&String>) -> String {
    value
        .map(|value| value.chars().take(128).collect())
        .unwrap_or_else(|| "unknown".to_string())
}

fn valid_instance_generation(value: Option<&String>) -> Option<String> {
    let value = value?.trim();
    validate_instance_generation(value)
        .is_ok()
        .then(|| value.to_string())
}

fn validate_effective_unit(
    target: ManagedHostWork,
    observation: &HostWorkObservation,
    properties: &HashMap<String, String>,
) -> Result<(), String> {
    validate_unit_identity(target, properties)?;
    validate_effective_restart_policy(target, properties)?;
    let requires_live_cgroup = matches!(
        observation.state,
        ObservedWorkState::Active | ObservedWorkState::Activating | ObservedWorkState::Deactivating
    );
    // systemd 的 effective resource policy 在 unit 尚未启动时也已经可读。Launch
    // preflight 必须先证明 profile 完整可信，不能先启动一个无保护实例再依赖
    // post-action rollback。只有 live/transitional unit 才额外要求 InvocationID 和
    // 实际 cgroup 文件与 systemd property 完全一致。
    validate_effective_protection(target, observation, properties, requires_live_cgroup)?;
    if requires_live_cgroup && observation.instance_generation.is_none() {
        return Err("active unit has no valid systemd InvocationID".to_string());
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct EffectiveRestartPolicy {
    restart: String,
    restart_usec: u64,
    start_limit_interval_usec: u64,
    start_limit_burst: u64,
}

fn descriptor_restart_policy(target: ManagedHostWork) -> Result<EffectiveRestartPolicy, String> {
    let (restart, restart_sec, start_limit_interval_sec, start_limit_burst) = match target {
        ManagedHostWork::PinvouApp => {
            let descriptor: EmbeddedDescriptor = serde_json::from_str(DESCRIPTOR_JSON)
                .map_err(|error| format!("embedded app descriptor: {error}"))?;
            let policy = descriptor.resource_policy;
            (
                policy.restart,
                policy.restart_sec,
                policy.start_limit_interval_sec,
                policy.start_limit_burst,
            )
        }
        ManagedHostWork::PinvouAsr => {
            let descriptor: EmbeddedAsrDescriptor = serde_json::from_str(ASR_DESCRIPTOR_JSON)
                .map_err(|error| format!("embedded ASR descriptor: {error}"))?;
            let policy = descriptor.resource_policy;
            (
                policy.restart,
                policy.restart_sec,
                policy.start_limit_interval_sec,
                policy.start_limit_burst,
            )
        }
    };
    let restart_usec = parse_systemd_timespan_usec(&restart_sec)
        .ok_or_else(|| "descriptor RestartSec is invalid".to_string())?;
    let start_limit_interval_usec = parse_systemd_timespan_usec(&start_limit_interval_sec)
        .ok_or_else(|| "descriptor StartLimitIntervalSec is invalid".to_string())?;
    Ok(EffectiveRestartPolicy {
        restart,
        restart_usec,
        start_limit_interval_usec,
        start_limit_burst,
    })
}

fn validate_effective_restart_policy(
    target: ManagedHostWork,
    properties: &HashMap<String, String>,
) -> Result<(), String> {
    let expected = descriptor_restart_policy(target)?;
    let observed = EffectiveRestartPolicy {
        restart: properties
            .get("Restart")
            .filter(|value| !value.is_empty())
            .cloned()
            .ok_or_else(|| "effective Restart is missing".to_string())?,
        restart_usec: properties
            .get("RestartUSec")
            .and_then(|value| parse_systemd_timespan_usec(value))
            .ok_or_else(|| "effective RestartUSec is missing or invalid".to_string())?,
        start_limit_interval_usec: properties
            .get("StartLimitIntervalUSec")
            .and_then(|value| parse_systemd_timespan_usec(value))
            .ok_or_else(|| "effective StartLimitIntervalUSec is missing or invalid".to_string())?,
        start_limit_burst: parse_optional_u64(properties.get("StartLimitBurst"))
            .ok_or_else(|| "effective StartLimitBurst is missing or invalid".to_string())?,
    };
    if observed != expected {
        return Err("effective Restart/RestartUSec/StartLimit policy mismatch".to_string());
    }
    Ok(())
}

fn descriptor_asr_memory_percentages() -> Result<(u64, u64), String> {
    let descriptor: EmbeddedAsrDescriptor = serde_json::from_str(ASR_DESCRIPTOR_JSON)
        .map_err(|error| format!("embedded ASR descriptor: {error}"))?;
    let high = parse_percentage(&descriptor.resource_policy.memory_high)
        .ok_or_else(|| "descriptor ASR MemoryHigh percentage is invalid".to_string())?;
    let max = parse_percentage(&descriptor.resource_policy.memory_max)
        .ok_or_else(|| "descriptor ASR MemoryMax percentage is invalid".to_string())?;
    if high >= max {
        return Err("descriptor ASR memory percentage ordering is invalid".to_string());
    }
    Ok((high, max))
}

fn parse_percentage(value: &str) -> Option<u64> {
    value
        .strip_suffix('%')?
        .parse::<u64>()
        .ok()
        .filter(|percent| matches!(*percent, 1..=100))
}

fn parse_systemd_timespan_usec(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() || value == "infinity" {
        return None;
    }
    value
        .split_ascii_whitespace()
        .try_fold(0_u64, |total, component| {
            let digits = component.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            let amount = component[..digits].parse::<u64>().ok()?;
            let multiplier = match &component[digits..] {
                "us" | "µs" | "μs" => 1_u64,
                "ms" => 1_000,
                "s" => 1_000_000,
                "min" => 60 * 1_000_000,
                "h" => 60 * 60 * 1_000_000,
                "d" => 24 * 60 * 60 * 1_000_000,
                "w" => 7 * 24 * 60 * 60 * 1_000_000,
                "month" => 30 * 24 * 60 * 60 * 1_000_000,
                "y" => 365 * 24 * 60 * 60 * 1_000_000,
                "" if amount == 0 => 1,
                _ => return None,
            };
            total.checked_add(amount.checked_mul(multiplier)?)
        })
}

fn physical_memory_bytes() -> Result<u64, String> {
    let meminfo = read_small_file(Path::new("/proc/meminfo"), 128 * 1024)
        .ok_or_else(|| "cannot read bounded /proc/meminfo".to_string())?;
    parse_mem_total_bytes(&meminfo)
        .ok_or_else(|| "cannot parse physical MemTotal from /proc/meminfo".to_string())
}

fn parse_mem_total_bytes(meminfo: &str) -> Option<u64> {
    let fields = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?
        .split_ascii_whitespace()
        .collect::<Vec<_>>();
    if fields.len() != 2 || fields[1] != "kB" {
        return None;
    }
    fields[0].parse::<u64>().ok()?.checked_mul(1024)
}

fn system_page_size_bytes() -> Result<u64, String> {
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    u64::try_from(page_size)
        .ok()
        .filter(|value| *value > 0 && value.is_power_of_two())
        .ok_or_else(|| "cannot determine a valid system page size".to_string())
}

fn percentage_page_ceiling_bytes(physical_bytes: u64, percent: u64, page_size: u64) -> Option<u64> {
    if physical_bytes == 0 || !(1..=100).contains(&percent) || !page_size.is_power_of_two() {
        return None;
    }
    let numerator = physical_bytes.checked_mul(percent)?;
    let exact_ceiling = numerator.checked_add(99)?.checked_div(100)?;
    exact_ceiling
        .checked_add(page_size - 1)?
        .checked_div(page_size)?
        .checked_mul(page_size)
}

fn validate_asr_percentage_bounds(
    physical_bytes: u64,
    page_size: u64,
    high: u64,
    max: u64,
) -> Result<(), String> {
    let (high_percent, max_percent) = descriptor_asr_memory_percentages()?;
    let high_ceiling = percentage_page_ceiling_bytes(physical_bytes, high_percent, page_size)
        .ok_or_else(|| "cannot calculate ASR MemoryHigh percentage ceiling".to_string())?;
    let max_ceiling = percentage_page_ceiling_bytes(physical_bytes, max_percent, page_size)
        .ok_or_else(|| "cannot calculate ASR MemoryMax percentage ceiling".to_string())?;
    if high > high_ceiling || max > max_ceiling {
        return Err(
            "ASR effective memory high/max exceeds descriptor percentage ceilings".to_string(),
        );
    }
    Ok(())
}

fn validate_effective_protection(
    target: ManagedHostWork,
    observation: &HostWorkObservation,
    properties: &HashMap<String, String>,
    require_cgroup_match: bool,
) -> Result<(), String> {
    let expected_tasks = match target {
        ManagedHostWork::PinvouApp => 512,
        ManagedHostWork::PinvouAsr => 128,
    };
    if properties.get("OOMPolicy").map(String::as_str) != Some("kill")
        || properties.get("KillMode").map(String::as_str) != Some("control-group")
        || parse_optional_u64(properties.get("TasksMax")) != Some(expected_tasks)
    {
        return Err("effective OOMPolicy/KillMode/TasksMax contract mismatch".to_string());
    }

    let systemd_high = parse_optional_u64(properties.get("MemoryHigh"));
    let systemd_max = parse_optional_u64(properties.get("MemoryMax"));
    let systemd_swap_max = parse_optional_u64(properties.get("MemorySwapMax"));
    let cgroup = &observation.cgroup;
    if require_cgroup_match
        && (systemd_high != cgroup.memory_high_bytes
            || systemd_max != cgroup.memory_max_bytes
            || systemd_swap_max != cgroup.memory_swap_max_bytes)
    {
        return Err(
            "systemd memory protection does not match the effective cgroup files".to_string(),
        );
    }

    const GIB: u64 = 1024 * 1024 * 1024;
    match target {
        ManagedHostWork::PinvouApp => {
            if systemd_high != Some(4 * GIB)
                || systemd_max != Some(8 * GIB)
                || systemd_swap_max != Some(2 * GIB)
            {
                return Err(
                    "MegaBook canary profile 4G/8G/2G is not effectively active".to_string()
                );
            }
        }
        ManagedHostWork::PinvouAsr => {
            let (Some(high), Some(max)) = (systemd_high, systemd_max) else {
                return Err("ASR effective memory high/max protection is missing".to_string());
            };
            if high >= max || systemd_swap_max != Some(2 * GIB) {
                return Err("ASR effective memory high/max/swap protection is invalid".to_string());
            }
            validate_asr_percentage_bounds(
                physical_memory_bytes()?,
                system_page_size_bytes()?,
                high,
                max,
            )?;
        }
    }
    Ok(())
}

fn validate_unit_identity(
    target: ManagedHostWork,
    properties: &HashMap<String, String>,
) -> Result<(), String> {
    let fragment = properties
        .get("FragmentPath")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "systemd FragmentPath is missing".to_string())?;
    let exec_start = properties
        .get("ExecStart")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "systemd ExecStart is missing".to_string())?;
    match target {
        ManagedHostWork::PinvouApp => {
            if fragment != "/usr/lib/systemd/user/pinvou3-app.service"
                || !exec_start_matches(
                    exec_start,
                    "/usr/bin/pinvou3-tauri",
                    &["/usr/bin/pinvou3-tauri"],
                )
            {
                return Err("app FragmentPath/ExecStart identity mismatch".to_string());
            }
            validate_fragment_metadata(Path::new(fragment), Some(0))?;
            validate_executable_metadata(Path::new("/usr/bin/pinvou3-tauri"), Some(0))
        }
        ManagedHostWork::PinvouAsr => {
            let fragment_path = Path::new(fragment);
            let home = fragment_path
                .ancestors()
                .nth(4)
                .filter(|path| path.is_absolute())
                .ok_or_else(|| "ASR FragmentPath has no bounded home root".to_string())?;
            let executable = home.join(".pinvou3/asr/qwen3-asr-openvino/runtime/bin/python");
            let script = home.join(".pinvou3/asr/qwen3-asr-openvino/qwen3-asr-openvino.py");
            let executable_text = executable.to_string_lossy();
            let script_text = script.to_string_lossy();
            if !fragment_path.is_absolute()
                || !fragment_path.ends_with(".config/systemd/user/pinvou-qwen3-asr.service")
                || !exec_start_matches(
                    exec_start,
                    executable_text.as_ref(),
                    &[executable_text.as_ref(), script_text.as_ref(), "serve"],
                )
            {
                return Err("ASR FragmentPath/ExecStart identity mismatch".to_string());
            }
            let expected_uid = unsafe { libc::geteuid() };
            validate_fragment_metadata(fragment_path, Some(expected_uid))?;
            validate_executable_metadata(&executable, Some(expected_uid))?;
            validate_owned_regular_file(&script, expected_uid)
        }
    }
}

/// `systemctl show` renders ExecStart as semicolon-delimited fields such as
/// `{ path=/usr/bin/foo ; argv[]=/usr/bin/foo arg ; ... }`. Substring checks are unsafe:
/// an override could execute `/tmp/evil` while merely carrying the trusted path as an argument.
fn exec_start_matches(exec_start: &str, executable: &str, argv: &[&str]) -> bool {
    systemd_exec_field(exec_start, "path") == Some(executable)
        && systemd_exec_field(exec_start, "argv[]")
            .is_some_and(|observed| observed.split_ascii_whitespace().eq(argv.iter().copied()))
}

fn systemd_exec_field<'a>(exec_start: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    let mut found = None;
    for segment in exec_start.split(';') {
        let segment = segment.trim_matches(|character: char| {
            character.is_ascii_whitespace() || matches!(character, '{' | '}')
        });
        let Some(value) = segment.strip_prefix(&prefix).map(str::trim) else {
            continue;
        };
        if value.is_empty() || found.replace(value).is_some() {
            return None;
        }
    }
    found
}

fn validate_executable_metadata(path: &Path, expected_uid: Option<u32>) -> Result<(), String> {
    let link = fs::symlink_metadata(path)
        .map_err(|error| format!("inspect managed executable link: {error}"))?;
    if expected_uid.is_some_and(|uid| link.uid() != uid) {
        return Err("managed executable link owner is not trusted".to_string());
    }
    let resolved =
        fs::canonicalize(path).map_err(|error| format!("resolve managed executable: {error}"))?;
    let metadata = resolved
        .metadata()
        .map_err(|error| format!("inspect managed executable: {error}"))?;
    if !metadata.file_type().is_file()
        || metadata.mode() & 0o111 == 0
        || metadata.mode() & 0o022 != 0
    {
        return Err("managed executable type/mode is not trusted".to_string());
    }
    Ok(())
}

fn validate_owned_regular_file(path: &Path, expected_uid: u32) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("inspect managed script: {error}"))?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != expected_uid
        || metadata.mode() & 0o022 != 0
    {
        return Err("managed script owner/type/mode is not trusted".to_string());
    }
    Ok(())
}

fn validate_fragment_metadata(path: &Path, expected_uid: Option<u32>) -> Result<(), String> {
    let link_metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspect systemd fragment metadata: {error}"))?;
    if !link_metadata.file_type().is_file() || link_metadata.file_type().is_symlink() {
        return Err("systemd fragment must be a regular non-symlink file".to_string());
    }
    if expected_uid.is_some_and(|uid| link_metadata.uid() != uid)
        || link_metadata.mode() & 0o022 != 0
    {
        return Err("systemd fragment owner/mode is not trusted".to_string());
    }
    Ok(())
}

fn cgroup_directory(control_group: &str) -> Option<PathBuf> {
    if control_group.len() > 512 || !control_group.starts_with('/') {
        return None;
    }
    let relative_text = control_group.strip_prefix('/')?;
    if relative_text.is_empty() {
        return None;
    }
    let relative = Path::new(relative_text);
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    Some(Path::new("/sys/fs/cgroup").join(relative))
}

fn observe_cgroup(path: &Path) -> CgroupObservation {
    CgroupObservation {
        memory_current_bytes: read_u64_file(&path.join("memory.current")),
        memory_peak_bytes: read_u64_file(&path.join("memory.peak")),
        memory_events: read_memory_events(&path.join("memory.events")),
        memory_pressure: read_memory_pressure(&path.join("memory.pressure")),
        pids_current: read_u64_file(&path.join("pids.current")),
        memory_high_bytes: read_limit_file(&path.join("memory.high")),
        memory_max_bytes: read_limit_file(&path.join("memory.max")),
        memory_swap_max_bytes: read_limit_file(&path.join("memory.swap.max")),
    }
}

fn read_small_file(path: &Path, max_bytes: usize) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take((max_bytes + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > max_bytes {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn read_u64_file(path: &Path) -> Option<u64> {
    read_small_file(path, 64)?.trim().parse().ok()
}

fn read_limit_file(path: &Path) -> Option<u64> {
    let value = read_small_file(path, 64)?;
    value.trim().parse().ok()
}

fn read_memory_events(path: &Path) -> BTreeMap<String, u64> {
    read_small_file(path, 4096)
        .into_iter()
        .flat_map(|content| {
            content
                .lines()
                .take(16)
                .filter_map(|line| {
                    let (key, value) = line.split_once(' ')?;
                    if key.len() > 32
                        || !key
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                    {
                        return None;
                    }
                    Some((key.to_string(), value.parse().ok()?))
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn read_memory_pressure(path: &Path) -> Option<MemoryPressure> {
    let content = read_small_file(path, 2048)?;
    let mut pressure = MemoryPressure::default();
    for line in content.lines().take(2) {
        let mut fields = line.split_ascii_whitespace();
        let kind = fields.next()?;
        let mut parsed = PressureLine::default();
        for field in fields.take(4) {
            let (key, value) = field.split_once('=')?;
            match key {
                "avg10" => parsed.avg10 = value.parse().ok(),
                "avg60" => parsed.avg60 = value.parse().ok(),
                "avg300" => parsed.avg300 = value.parse().ok(),
                "total" => parsed.total = value.parse().ok(),
                _ => {}
            }
        }
        match kind {
            "some" => pressure.some = Some(parsed),
            "full" => pressure.full = Some(parsed),
            _ => {}
        }
    }
    Some(pressure)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestFingerprint {
    request_id: String,
    target: ManagedHostWork,
    descriptor_revision: String,
    expected_instance_generation: Option<String>,
    action: SupervisorAction,
}

impl From<&SupervisorRequest> for RequestFingerprint {
    fn from(request: &SupervisorRequest) -> Self {
        Self {
            request_id: request.request_id.clone(),
            target: request.target,
            descriptor_revision: request.descriptor_revision.clone(),
            expected_instance_generation: request.expected_instance_generation.clone(),
            action: request.action,
        }
    }
}

#[derive(Debug, Clone)]
enum StoredRequest {
    Pending(RequestFingerprint),
    Completed(RequestFingerprint, Box<SupervisorReceipt>),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum ControlEvent {
    ControlPending {
        recorded_at_unix_ms: u64,
        fingerprint: RequestFingerprint,
        before: Option<HostWorkObservation>,
    },
    /// One durable terminal tombstone per request id is sufficient to preserve idempotence.
    ControlCompletedTombstone {
        recorded_at_unix_ms: u64,
        fingerprint: RequestFingerprint,
        receipt: SupervisorReceipt,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum ObservationEvent {
    Observation {
        recorded_at_unix_ms: u64,
        target: ManagedHostWork,
        descriptor_revision: String,
        observation: HostWorkObservation,
        control_group_present: bool,
        integrity_error: Option<String>,
    },
}

struct ControlLedger {
    state_directory: PathBuf,
    path: PathBuf,
    stored: HashMap<String, StoredRequest>,
    max_stored_requests: usize,
    max_bytes: u64,
    #[cfg(test)]
    fail_next_completion_sync: bool,
}

impl ControlLedger {
    fn open(state_directory: &Path) -> Result<Self, String> {
        secure_state_directory(state_directory)?;
        let path = state_directory.join("control-v1.jsonl");
        let mut ledger = Self {
            state_directory: state_directory.to_path_buf(),
            path,
            stored: HashMap::new(),
            max_stored_requests: MAX_STORED_REQUESTS,
            max_bytes: MAX_CONTROL_JOURNAL_BYTES,
            #[cfg(test)]
            fail_next_completion_sync: false,
        };
        ledger.load()?;
        Ok(ledger)
    }

    fn load(&mut self) -> Result<(), String> {
        let file = match open_state_file_read(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("open supervisor control ledger: {error}")),
        };
        let file_bytes = file
            .metadata()
            .map_err(|error| format!("inspect supervisor control ledger: {error}"))?
            .len();
        let recovery_bound = self.max_bytes.saturating_add(MAX_RESPONSE_BYTES as u64);
        if file_bytes > recovery_bound {
            return Err("supervisor control ledger exceeds its recovery bound".to_string());
        }
        let mut contents = Vec::with_capacity(file_bytes as usize);
        file.take(recovery_bound.saturating_add(1))
            .read_to_end(&mut contents)
            .map_err(|error| format!("read supervisor control ledger: {error}"))?;
        if contents.len() as u64 > recovery_bound {
            return Err("supervisor control ledger grew beyond its recovery bound".to_string());
        }

        // A frame becomes recoverable only with its trailing newline commit marker. A crash can
        // leave one unterminated tail, which is ignored while the prior durable Pending remains.
        // Conversely, silently skipping a newline-terminated malformed frame could forget the
        // only Pending record and replay a destructive control, so complete corruption is fatal.
        let committed_length = contents
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        if committed_length > 0 {
            for (index, frame) in contents[..committed_length - 1]
                .split(|byte| *byte == b'\n')
                .enumerate()
            {
                if frame.is_empty() || frame.len() > MAX_RESPONSE_BYTES {
                    return Err(format!(
                        "supervisor control ledger frame {} violates its bound",
                        index + 1
                    ));
                }
                let event = serde_json::from_slice::<ControlEvent>(frame).map_err(|error| {
                    format!(
                        "decode committed supervisor control ledger frame {}: {error}",
                        index + 1
                    )
                })?;
                match event {
                    ControlEvent::ControlPending { fingerprint, .. } => {
                        self.remember(StoredRequest::Pending(fingerprint));
                    }
                    ControlEvent::ControlCompletedTombstone {
                        fingerprint,
                        receipt,
                        ..
                    } => {
                        self.remember(StoredRequest::Completed(fingerprint, Box::new(receipt)));
                    }
                }
            }
        }
        if self.stored.len() > self.max_stored_requests {
            return Err("supervisor control ledger exceeds directive-id capacity".to_string());
        }
        Ok(())
    }

    fn append(&mut self, event: &ControlEvent, completion: bool) -> Result<(), String> {
        let _ = completion;
        let mut encoded = serde_json::to_vec(event)
            .map_err(|error| format!("encode supervisor control evidence: {error}"))?;
        if encoded.len() + 1 > MAX_RESPONSE_BYTES {
            return Err("supervisor control evidence frame exceeds bound".to_string());
        }
        encoded.push(b'\n');
        let current_bytes = self.path.metadata().map(|meta| meta.len()).unwrap_or(0);
        if current_bytes.saturating_add(encoded.len() as u64) > self.max_bytes {
            self.compact()?;
        }
        let compacted_bytes = self.path.metadata().map(|meta| meta.len()).unwrap_or(0);
        if compacted_bytes.saturating_add(encoded.len() as u64) > self.max_bytes {
            return Err("supervisor control ledger byte capacity is full".to_string());
        }
        let existed = self.path.exists();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&self.path)
            .map_err(|error| format!("open supervisor control ledger append: {error}"))?;
        validate_state_file(&file)?;
        file.write_all(&encoded)
            .map_err(|error| format!("write supervisor control ledger: {error}"))?;
        #[cfg(test)]
        if completion && self.fail_next_completion_sync {
            self.fail_next_completion_sync = false;
            return Err("injected completion fsync failure".to_string());
        }
        file.sync_all()
            .map_err(|error| format!("fsync supervisor control ledger: {error}"))?;
        if !existed {
            fsync_directory(&self.state_directory)?;
        }
        Ok(())
    }

    fn compact(&self) -> Result<(), String> {
        let temporary = self.state_directory.join(format!(
            "control-v1.jsonl.tmp-{}-{}",
            std::process::id(),
            COMPACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)
            .map_err(|error| format!("create supervisor control compaction: {error}"))?;
        let mut request_ids: Vec<_> = self.stored.keys().cloned().collect();
        request_ids.sort();
        for request_id in request_ids {
            let event = match self.stored.get(&request_id) {
                Some(StoredRequest::Pending(fingerprint)) => ControlEvent::ControlPending {
                    recorded_at_unix_ms: now_unix_ms(),
                    fingerprint: fingerprint.clone(),
                    before: None,
                },
                Some(StoredRequest::Completed(fingerprint, receipt)) => {
                    ControlEvent::ControlCompletedTombstone {
                        recorded_at_unix_ms: now_unix_ms(),
                        fingerprint: fingerprint.clone(),
                        receipt: receipt.as_ref().clone(),
                    }
                }
                None => continue,
            };
            let mut encoded = serde_json::to_vec(&event)
                .map_err(|error| format!("encode supervisor control compaction: {error}"))?;
            encoded.push(b'\n');
            file.write_all(&encoded)
                .map_err(|error| format!("write supervisor control compaction: {error}"))?;
        }
        file.sync_all()
            .map_err(|error| format!("fsync supervisor control compaction: {error}"))?;
        fs::rename(&temporary, &self.path)
            .map_err(|error| format!("install supervisor control compaction: {error}"))?;
        fsync_directory(&self.state_directory)
    }

    fn lookup(&self, request_id: &str) -> Option<StoredRequest> {
        self.stored.get(request_id).cloned()
    }

    fn remember(&mut self, stored: StoredRequest) {
        let request_id = match &stored {
            StoredRequest::Pending(fingerprint) | StoredRequest::Completed(fingerprint, _) => {
                fingerprint.request_id.clone()
            }
        };
        self.stored.insert(request_id, stored);
    }

    fn record_pending(
        &mut self,
        fingerprint: RequestFingerprint,
        before: Option<HostWorkObservation>,
    ) -> Result<(), String> {
        if !self.stored.contains_key(&fingerprint.request_id)
            && self.stored.len() >= self.max_stored_requests
        {
            return Err("supervisor control ledger directive-id capacity is full".to_string());
        }
        self.append(
            &ControlEvent::ControlPending {
                recorded_at_unix_ms: now_unix_ms(),
                fingerprint: fingerprint.clone(),
                before,
            },
            false,
        )?;
        self.remember(StoredRequest::Pending(fingerprint));
        Ok(())
    }

    fn record_completed(
        &mut self,
        fingerprint: RequestFingerprint,
        receipt: SupervisorReceipt,
    ) -> Result<(), String> {
        self.append(
            &ControlEvent::ControlCompletedTombstone {
                recorded_at_unix_ms: now_unix_ms(),
                fingerprint: fingerprint.clone(),
                receipt: receipt.clone(),
            },
            true,
        )?;
        self.remember(StoredRequest::Completed(fingerprint, Box::new(receipt)));
        if self.path.metadata().map(|meta| meta.len()).unwrap_or(0) > self.max_bytes {
            let _ = self.compact();
        }
        Ok(())
    }
}

struct ObservationJournal {
    state_directory: PathBuf,
    path: PathBuf,
    max_bytes: u64,
}

impl ObservationJournal {
    fn open(state_directory: &Path) -> Result<Self, String> {
        secure_state_directory(state_directory)?;
        Ok(Self {
            state_directory: state_directory.to_path_buf(),
            path: state_directory.join("observations-v1.jsonl"),
            max_bytes: MAX_OBSERVATION_JOURNAL_BYTES,
        })
    }

    fn record_observation(
        &mut self,
        target: ManagedHostWork,
        snapshot: &UnitSnapshot,
    ) -> Result<(), String> {
        let mut encoded = serde_json::to_vec(&ObservationEvent::Observation {
            recorded_at_unix_ms: now_unix_ms(),
            target,
            descriptor_revision: target.descriptor_revision().to_string(),
            observation: snapshot.observation.clone(),
            control_group_present: snapshot.control_group.is_some(),
            integrity_error: snapshot.integrity_error.clone(),
        })
        .map_err(|error| format!("encode supervisor observation: {error}"))?;
        if encoded.len() + 1 > MAX_RESPONSE_BYTES {
            return Err("supervisor observation frame exceeds bound".to_string());
        }
        encoded.push(b'\n');
        self.rotate_if_needed(encoded.len() as u64)?;
        let existed = self.path.exists();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&self.path)
            .map_err(|error| format!("open supervisor observation append: {error}"))?;
        validate_state_file(&file)?;
        file.write_all(&encoded)
            .and_then(|_| file.sync_data())
            .map_err(|error| format!("persist supervisor observation: {error}"))?;
        if !existed {
            fsync_directory(&self.state_directory)?;
        }
        Ok(())
    }

    fn rotate_if_needed(&self, additional_bytes: u64) -> Result<(), String> {
        let current_bytes = self.path.metadata().map(|meta| meta.len()).unwrap_or(0);
        if current_bytes == 0 || current_bytes.saturating_add(additional_bytes) <= self.max_bytes {
            return Ok(());
        }
        let rotated = self.path.with_extension("jsonl.1");
        match fs::remove_file(&rotated) {
            Ok(()) => fsync_directory(&self.state_directory)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("remove oldest supervisor observations: {error}")),
        }
        fs::rename(&self.path, rotated)
            .map_err(|error| format!("rotate supervisor observations: {error}"))?;
        fsync_directory(&self.state_directory)
    }
}

fn secure_state_directory(state_directory: &Path) -> Result<(), String> {
    let existed = state_directory.exists();
    fs::create_dir_all(state_directory)
        .map_err(|error| format!("create supervisor state directory: {error}"))?;
    let metadata = fs::symlink_metadata(state_directory)
        .map_err(|error| format!("inspect supervisor state directory: {error}"))?;
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err("supervisor state directory owner/type is not trusted".to_string());
    }
    fs::set_permissions(state_directory, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("secure supervisor state directory: {error}"))?;
    if !existed {
        if let Some(parent) = state_directory.parent() {
            fsync_directory(parent)?;
        }
    }
    Ok(())
}

fn fsync_directory(directory: &Path) -> Result<(), String> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("fsync supervisor state directory: {error}"))
}

fn open_state_file_read(path: &Path) -> std::io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    validate_state_file(&file).map_err(std::io::Error::other)?;
    Ok(file)
}

fn validate_state_file(file: &File) -> Result<(), String> {
    let metadata = file
        .metadata()
        .map_err(|error| format!("inspect supervisor state file: {error}"))?;
    if !metadata.file_type().is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err("supervisor state file owner/type/mode is not trusted".to_string());
    }
    Ok(())
}

struct Supervisor {
    controller: Arc<dyn UnitController>,
    control_ledger: Mutex<ControlLedger>,
    observation_journal: Mutex<ObservationJournal>,
    control_serial: Mutex<()>,
}

impl Supervisor {
    fn process(&self, request: &SupervisorRequest, peer_pid: u32) -> SupervisorReceipt {
        if let Err(error) = request.validate() {
            return rejected_receipt(request, protocol_error_detail(error));
        }
        if request.action == SupervisorAction::Status {
            return self.status_receipt(request);
        }
        if let Err(error) = self.authorize_control(request, peer_pid) {
            return rejected_receipt(request, error);
        }
        let _serial = self
            .control_serial
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fingerprint = RequestFingerprint::from(request);
        let stored = self
            .control_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .lookup(&request.request_id);
        if let Some(stored) = stored {
            return self.reconcile_stored(request, &fingerprint, stored);
        }

        let before = match self.controller.status(request.target) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return rejected_receipt(
                    request,
                    format!("cannot establish trusted pre-action status: {error}"),
                );
            }
        };
        if let Some(error) = before.integrity_error.as_deref() {
            return rejected_receipt(
                request,
                format!("fixed target failed effective-policy validation: {error}"),
            );
        }
        let launch_was_initially_attributable =
            request.action == SupervisorAction::Launch && launch_precondition_allows_start(&before);
        if request.action == SupervisorAction::Stop {
            let observed_generation = before.observation.instance_generation.as_deref();
            if observed_generation != request.expected_instance_generation.as_deref() {
                return rejected_receipt(
                    request,
                    "expected instance generation does not match current systemd InvocationID",
                );
            }
        }
        if desired_state_reached(request.action, &before) {
            let confirmed_receipt = receipt(
                request,
                SupervisorOutcome::AlreadyApplied,
                Some(before.observation),
                "fixed target already has the requested observed state",
            );
            if let Err(error) = self.persist_completed(fingerprint, confirmed_receipt.clone()) {
                return receipt(
                    request,
                    SupervisorOutcome::OutcomeUnknown,
                    confirmed_receipt.observation,
                    format!("observed state is known but completion evidence failed: {error}"),
                );
            }
            return confirmed_receipt;
        }
        if request.action == SupervisorAction::Launch && !launch_was_initially_attributable {
            return rejected_receipt(
                request,
                "launch requires an inactive or failed app unit with no live MainPID",
            );
        }
        if let Err(error) = self
            .control_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .record_pending(fingerprint.clone(), Some(before.observation.clone()))
        {
            return rejected_receipt(
                request,
                format!("cannot persist directive before action: {error}"),
            );
        }

        // Persistence can take long enough for either the caller app or target unit to restart.
        // Re-read both immediately before the side effect; a stale InvocationID is terminally
        // rejected and never allowed to fall through to systemctl stop.
        if let Err(error) = self.authorize_control(request, peer_pid) {
            return self.persist_rejection_after_pending(request, fingerprint, error, None);
        }
        let action_preflight = match self.controller.status(request.target) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return receipt(
                    request,
                    SupervisorOutcome::OutcomeUnknown,
                    None,
                    format!("cannot re-observe fixed target before action: {error}"),
                );
            }
        };
        if let Some(error) = action_preflight.integrity_error.as_deref() {
            return self.persist_rejection_after_pending(
                request,
                fingerprint,
                format!("effective policy changed before action: {error}"),
                Some(action_preflight.observation),
            );
        }
        if request.action == SupervisorAction::Stop
            && action_preflight.observation.instance_generation.as_deref()
                != request.expected_instance_generation.as_deref()
        {
            return self.persist_rejection_after_pending(
                request,
                fingerprint,
                "instance generation changed before action",
                Some(action_preflight.observation),
            );
        }
        if desired_state_reached(request.action, &action_preflight) {
            let confirmed = receipt(
                request,
                SupervisorOutcome::AlreadyApplied,
                Some(action_preflight.observation),
                "fixed target reached the requested state before action",
            );
            return match self.persist_completed(fingerprint, confirmed.clone()) {
                Ok(()) => confirmed,
                Err(error) => receipt(
                    request,
                    SupervisorOutcome::OutcomeUnknown,
                    confirmed.observation,
                    format!("pre-action state is known but evidence failed: {error}"),
                ),
            };
        }
        if request.action == SupervisorAction::Launch
            && !launch_precondition_allows_start(&action_preflight)
        {
            return self.persist_rejection_after_pending(
                request,
                fingerprint,
                "app state no longer has an attributable launch precondition",
                Some(action_preflight.observation),
            );
        }

        let action_result = match request.action {
            SupervisorAction::Stop => self.controller.stop(request.target),
            SupervisorAction::Launch => self.controller.launch_app(),
            SupervisorAction::Status => unreachable!("status returned before control path"),
        };
        let after_result = self.controller.status(request.target);
        if request.action == SupervisorAction::Launch {
            let after_error = match &after_result {
                Ok(snapshot) => snapshot
                    .integrity_error
                    .as_ref()
                    .map(|error| format!("effective policy is untrusted: {error}")),
                Err(error) => Some(format!("post-launch status is unavailable: {error}")),
            };
            if let Some(error) = after_error {
                let definitely_started_by_request = action_result.is_ok()
                    && launch_was_initially_attributable
                    && launch_precondition_allows_start(&action_preflight);
                let rollback_detail = if definitely_started_by_request {
                    self.controller
                        .stop(ManagedHostWork::PinvouApp)
                        .map(|()| "unprotected app was stopped".to_string())
                        .unwrap_or_else(|stop_error| {
                            format!("failed to stop unprotected app: {stop_error}")
                        })
                } else {
                    "launch ownership was not proven; no rollback stop was issued".to_string()
                };
                return receipt(
                    request,
                    SupervisorOutcome::OutcomeUnknown,
                    after_result.ok().map(|snapshot| snapshot.observation),
                    format!("post-launch validation failed: {error}; {rollback_detail}"),
                );
            }
        }
        let after = after_result.ok();
        let reached = after
            .as_ref()
            .is_some_and(|snapshot| desired_state_reached(request.action, snapshot));
        if reached {
            let outcome = if action_result.is_ok() {
                SupervisorOutcome::Applied
            } else {
                SupervisorOutcome::Reconciled
            };
            let confirmed_receipt = receipt(
                request,
                outcome,
                after.map(|snapshot| snapshot.observation),
                action_result
                    .err()
                    .map(|error| format!("action response failed but status reconciled: {error}"))
                    .unwrap_or_else(|| {
                        "fixed systemd action acknowledged and observed".to_string()
                    }),
            );
            if let Err(error) = self.persist_completed(fingerprint, confirmed_receipt.clone()) {
                receipt(
                    request,
                    SupervisorOutcome::OutcomeUnknown,
                    confirmed_receipt.observation,
                    format!("observed state is known but completion evidence failed: {error}"),
                )
            } else {
                confirmed_receipt
            }
        } else {
            let detail = action_result
                .err()
                .unwrap_or_else(|| "action returned without requested observed state".to_string());
            receipt(
                request,
                SupervisorOutcome::OutcomeUnknown,
                after.as_ref().map(|snapshot| snapshot.observation.clone()),
                &detail,
            )
        }
    }

    fn authorize_control(&self, request: &SupervisorRequest, peer_pid: u32) -> Result<(), String> {
        if request.action == SupervisorAction::Launch
            && request.target == ManagedHostWork::PinvouApp
        {
            // The installed desktop entry can only construct this fixed action/target pair.
            return Ok(());
        }
        let app = self
            .controller
            .status(ManagedHostWork::PinvouApp)
            .map_err(|error| format!("cannot authenticate app MainPID: {error}"))?;
        if let Some(error) = app.integrity_error {
            return Err(format!(
                "app unit identity/protection is not trusted: {error}"
            ));
        }
        if app.observation.state != ObservedWorkState::Active
            || app.observation.main_pid != Some(peer_pid)
        {
            return Err("control caller PID is not pinvou3-app.service MainPID".to_string());
        }
        Ok(())
    }

    fn persist_rejection_after_pending(
        &self,
        request: &SupervisorRequest,
        fingerprint: RequestFingerprint,
        detail: impl AsRef<str>,
        observation: Option<HostWorkObservation>,
    ) -> SupervisorReceipt {
        let rejected = receipt(
            request,
            SupervisorOutcome::Rejected,
            observation,
            detail.as_ref(),
        );
        match self.persist_completed(fingerprint, rejected.clone()) {
            Ok(()) => rejected,
            Err(error) => receipt(
                request,
                SupervisorOutcome::OutcomeUnknown,
                rejected.observation,
                format!("control rejected but terminal evidence failed: {error}"),
            ),
        }
    }

    fn status_receipt(&self, request: &SupervisorRequest) -> SupervisorReceipt {
        match self.controller.status(request.target) {
            Ok(snapshot) => {
                let evidence_error = self
                    .observation_journal
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .record_observation(request.target, &snapshot)
                    .err();
                if let Some(error) = snapshot.integrity_error {
                    receipt(
                        request,
                        SupervisorOutcome::OutcomeUnknown,
                        Some(snapshot.observation),
                        format!("fixed target status is untrusted: {error}"),
                    )
                } else if let Some(error) = evidence_error {
                    receipt(
                        request,
                        SupervisorOutcome::OutcomeUnknown,
                        Some(snapshot.observation),
                        format!(
                            "fixed target was observed but evidence persistence failed: {error}"
                        ),
                    )
                } else {
                    receipt(
                        request,
                        SupervisorOutcome::Reconciled,
                        Some(snapshot.observation),
                        "fixed target status observed",
                    )
                }
            }
            Err(error) => receipt(request, SupervisorOutcome::OutcomeUnknown, None, &error),
        }
    }

    fn reconcile_stored(
        &self,
        request: &SupervisorRequest,
        fingerprint: &RequestFingerprint,
        stored: StoredRequest,
    ) -> SupervisorReceipt {
        match stored {
            StoredRequest::Completed(existing, receipt) => {
                if existing == *fingerprint {
                    *receipt
                } else {
                    rejected_receipt(
                        request,
                        "request id was already used for a different control",
                    )
                }
            }
            StoredRequest::Pending(existing) => {
                if existing != *fingerprint {
                    return rejected_receipt(
                        request,
                        "pending request id belongs to a different control",
                    );
                }
                let after = self.controller.status(request.target).ok();
                if after
                    .as_ref()
                    .is_some_and(|snapshot| desired_state_reached(request.action, snapshot))
                {
                    let confirmed_receipt = receipt(
                        request,
                        SupervisorOutcome::Reconciled,
                        after.map(|snapshot| snapshot.observation),
                        "pending directive reconciled by status without replaying the action",
                    );
                    if let Err(error) =
                        self.persist_completed(fingerprint.clone(), confirmed_receipt.clone())
                    {
                        receipt(
                            request,
                            SupervisorOutcome::OutcomeUnknown,
                            confirmed_receipt.observation,
                            format!(
                                "reconciled state is known but completion evidence failed: {error}"
                            ),
                        )
                    } else {
                        confirmed_receipt
                    }
                } else {
                    receipt(
                        request,
                        SupervisorOutcome::OutcomeUnknown,
                        after.map(|snapshot| snapshot.observation),
                        "pending directive is not reconciled; action was not replayed",
                    )
                }
            }
        }
    }

    fn persist_completed(
        &self,
        fingerprint: RequestFingerprint,
        receipt: SupervisorReceipt,
    ) -> Result<(), String> {
        self.control_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .record_completed(fingerprint, receipt)
    }
}

fn desired_state_reached(action: SupervisorAction, snapshot: &UnitSnapshot) -> bool {
    if snapshot.integrity_error.is_some() {
        return false;
    }
    let observation = &snapshot.observation;
    match action {
        SupervisorAction::Status => true,
        SupervisorAction::Stop => {
            matches!(
                observation.state,
                ObservedWorkState::Inactive | ObservedWorkState::Failed
            ) && observation.main_pid.is_none()
        }
        SupervisorAction::Launch => observation.state == ObservedWorkState::Active,
    }
}

fn launch_precondition_allows_start(snapshot: &UnitSnapshot) -> bool {
    snapshot.integrity_error.is_none()
        && matches!(
            snapshot.observation.state,
            ObservedWorkState::Inactive | ObservedWorkState::Failed
        )
        && snapshot.observation.main_pid.is_none()
}

fn receipt(
    request: &SupervisorRequest,
    outcome: SupervisorOutcome,
    observation: Option<HostWorkObservation>,
    detail: impl AsRef<str>,
) -> SupervisorReceipt {
    SupervisorReceipt {
        protocol_version: PROTOCOL_VERSION,
        request_id: request.request_id.clone(),
        target: request.target,
        descriptor_revision: request.descriptor_revision.clone(),
        expected_instance_generation: request.expected_instance_generation.clone(),
        action: request.action,
        outcome,
        observation,
        detail: bound_detail(detail.as_ref()),
        observed_at_unix_ms: now_unix_ms(),
    }
}

fn rejected_receipt(request: &SupervisorRequest, detail: impl AsRef<str>) -> SupervisorReceipt {
    receipt(request, SupervisorOutcome::Rejected, None, detail.as_ref())
}

fn protocol_error_detail(error: ProtocolError) -> &'static str {
    match error {
        ProtocolError::UnsupportedVersion => "unsupported protocol version",
        ProtocolError::DescriptorRevisionMismatch => "descriptor revision mismatch",
        ProtocolError::InvalidRequestId => "request id violates the bounded id contract",
        ProtocolError::InvalidInstanceGeneration => "instance generation is not an InvocationID",
        ProtocolError::MissingInstanceGeneration => "Stop requires an expected InvocationID",
        ProtocolError::UnexpectedInstanceGeneration => {
            "this action must not carry an instance generation"
        }
        ProtocolError::ActionNotAllowed => "action is not allowed for the fixed descriptor",
    }
}

fn bound_detail(detail: &str) -> String {
    if detail.len() <= MAX_DETAIL_BYTES {
        return detail.to_string();
    }
    let mut end = MAX_DETAIL_BYTES;
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    detail[..end].to_string()
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn state_directory() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("PINVOU_SUPERVISOR_STATE_DIR") {
        if !path.is_empty() {
            return validate_absolute_state_path(PathBuf::from(path));
        }
    }
    if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
        if !path.is_empty() {
            return validate_absolute_state_path(PathBuf::from(path).join("pinvou-supervisor"));
        }
    }
    std::env::var_os("HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join(".local/state/pinvou-supervisor"))
        .ok_or_else(|| "supervisor state directory is unavailable".to_string())
        .and_then(validate_absolute_state_path)
}

fn validate_absolute_state_path(path: PathBuf) -> Result<PathBuf, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err("supervisor state directory must be an absolute normalized path".to_string());
    }
    Ok(path)
}

fn activated_listener() -> Result<UnixListener, String> {
    let listen_pid = std::env::var("LISTEN_PID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok());
    let listen_fds = std::env::var("LISTEN_FDS")
        .ok()
        .and_then(|value| value.parse::<u32>().ok());
    if listen_pid != Some(std::process::id()) || listen_fds != Some(1) {
        return Err("daemon requires exactly one systemd-activated listener".to_string());
    }
    std::env::remove_var("LISTEN_PID");
    std::env::remove_var("LISTEN_FDS");
    std::env::remove_var("LISTEN_FDNAMES");
    const SYSTEMD_LISTEN_FD: RawFd = 3;
    let listener = unsafe { UnixListener::from_raw_fd(SYSTEMD_LISTEN_FD) };
    set_close_on_exec(listener.as_raw_fd())?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("inspect activated listener: {error}"))?;
    let expected_socket = fixed_runtime_directory()?.join(SOCKET_RELATIVE_PATH);
    if address.as_pathname() != Some(expected_socket.as_path()) {
        return Err("activated listener is not the fixed supervisor socket".to_string());
    }
    Ok(listener)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PeerCredentials {
    pid: u32,
    uid: u32,
}

// Ancillary-data headers require native word alignment even though the payload is bytes.
#[repr(align(8))]
struct AncillaryBuffer([u8; 128]);

fn peer_credentials(stream: &UnixStream) -> Result<PeerCredentials, String> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            std::os::fd::AsRawFd::as_raw_fd(stream),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::addr_of_mut!(credentials).cast(),
            &mut length,
        )
    };
    if result != 0 || length as usize != std::mem::size_of::<libc::ucred>() || credentials.pid <= 0
    {
        return Err("cannot verify Unix peer credentials".to_string());
    }
    Ok(PeerCredentials {
        pid: credentials.pid as u32,
        uid: credentials.uid,
    })
}

fn verify_same_uid(stream: &UnixStream) -> Result<PeerCredentials, String> {
    let credentials = peer_credentials(stream)?;
    let expected_uid = unsafe { libc::geteuid() };
    if credentials.uid != expected_uid {
        return Err("client peer credential does not match supervisor uid".to_string());
    }
    Ok(credentials)
}

fn verify_supervisor_sender(credentials: PeerCredentials, deadline: Instant) -> Result<(), String> {
    let expected_uid = unsafe { libc::geteuid() };
    let expected_pid = systemd_main_pid(SUPERVISOR_UNIT, deadline)?;
    verify_sender_identity(credentials, expected_uid, expected_pid)
}

fn verify_sender_identity(
    credentials: PeerCredentials,
    expected_uid: u32,
    expected_pid: u32,
) -> Result<(), String> {
    if credentials.uid != expected_uid || credentials.pid != expected_pid {
        return Err(format!(
            "response sender uid/pid {}/{} is not expected {expected_uid}/{expected_pid}",
            credentials.uid, credentials.pid
        ));
    }
    Ok(())
}

fn set_close_on_exec(fd: RawFd) -> Result<(), String> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(format!(
            "read file-descriptor flags: {}",
            std::io::Error::last_os_error()
        ));
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(format!(
            "set FD_CLOEXEC: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn enable_passcred(stream: &UnixStream) -> Result<(), String> {
    let enabled: libc::c_int = 1;
    if unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PASSCRED,
            std::ptr::addr_of!(enabled).cast(),
            std::mem::size_of_val(&enabled) as libc::socklen_t,
        )
    } != 0
    {
        return Err(format!(
            "enable SO_PASSCRED: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn read_response_with_credentials(
    stream: &UnixStream,
    deadline: Instant,
) -> Result<(Vec<u8>, PeerCredentials), String> {
    let mut response = Vec::new();
    let mut sender = None;
    loop {
        // SO_RCVTIMEO applies independently to each recvmsg. Derive every chunk timeout from the
        // same absolute client deadline so a slow peer cannot multiply the advertised 8s budget.
        stream
            .set_read_timeout(Some(remaining_client_budget(deadline)?))
            .map_err(|error| format!("set client read timeout: {error}"))?;
        let mut bytes = [0_u8; 8192];
        let mut control = AncillaryBuffer([0_u8; 128]);
        let mut io = libc::iovec {
            iov_base: bytes.as_mut_ptr().cast(),
            iov_len: bytes.len(),
        };
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = std::ptr::addr_of_mut!(io);
        message.msg_iovlen = 1;
        message.msg_control = control.0.as_mut_ptr().cast();
        message.msg_controllen = control.0.len();
        let received = unsafe { libc::recvmsg(stream.as_raw_fd(), &mut message, 0) };
        if received < 0 {
            return Err(format!(
                "recvmsg supervisor response: {}",
                std::io::Error::last_os_error()
            ));
        }
        if received == 0 {
            return Err("supervisor closed before a complete response frame".to_string());
        }
        if message.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err("supervisor response credentials were truncated".to_string());
        }
        let credentials = credentials_from_message(&message)
            .ok_or_else(|| "supervisor response carried no SCM_CREDENTIALS".to_string())?;
        if sender.is_some_and(|existing| existing != credentials) {
            return Err("supervisor response sender credentials changed mid-frame".to_string());
        }
        sender = Some(credentials);
        response.extend_from_slice(&bytes[..received as usize]);
        if response.len() > MAX_RESPONSE_BYTES {
            return Err("supervisor response exceeds protocol bound".to_string());
        }
        if response.contains(&b'\n') {
            break;
        }
    }
    Ok((
        response,
        sender.expect("credential checked for every response chunk"),
    ))
}

fn credentials_from_message(message: &libc::msghdr) -> Option<PeerCredentials> {
    let mut header = unsafe { libc::CMSG_FIRSTHDR(message) };
    while !header.is_null() {
        let current = unsafe { &*header };
        if current.cmsg_level == libc::SOL_SOCKET
            && current.cmsg_type == libc::SCM_CREDENTIALS
            && current.cmsg_len
                >= unsafe { libc::CMSG_LEN(std::mem::size_of::<libc::ucred>() as _) } as usize
        {
            let credentials =
                unsafe { std::ptr::read_unaligned(libc::CMSG_DATA(header).cast::<libc::ucred>()) };
            if credentials.pid > 0 {
                return Some(PeerCredentials {
                    pid: credentials.pid as u32,
                    uid: credentials.uid,
                });
            }
        }
        header = unsafe { libc::CMSG_NXTHDR(message, header) };
    }
    None
}

fn handle_client(mut stream: UnixStream, supervisor: &Supervisor) -> Result<(), String> {
    set_close_on_exec(stream.as_raw_fd())?;
    let peer = verify_same_uid(&stream)?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|error| format!("set request timeout: {error}"))?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|error| format!("set response timeout: {error}"))?;
    let mut frame = Vec::new();
    BufReader::new(&stream)
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_until(b'\n', &mut frame)
        .map_err(|error| format!("read bounded request: {error}"))?;
    if frame.is_empty() || frame.len() > MAX_REQUEST_BYTES || !frame.ends_with(b"\n") {
        return Err("request is empty, truncated, or oversized".to_string());
    }
    frame.pop();
    let request: SupervisorRequest = serde_json::from_slice(&frame)
        .map_err(|error| format!("decode bounded request: {error}"))?;
    let receipt = supervisor.process(&request, peer.pid);
    let mut response =
        serde_json::to_vec(&receipt).map_err(|error| format!("encode response: {error}"))?;
    if response.len() + 1 > MAX_RESPONSE_BYTES {
        return Err("response exceeds protocol bound".to_string());
    }
    response.push(b'\n');
    stream
        .write_all(&response)
        .and_then(|_| stream.flush())
        .map_err(|error| format!("write response: {error}"))
}

fn spawn_monitor(supervisor: Arc<Supervisor>) {
    thread::spawn(move || {
        let mut previous: HashMap<ManagedHostWork, HostWorkObservation> = HashMap::new();
        let mut last_recorded: HashMap<ManagedHostWork, SystemTime> = HashMap::new();
        loop {
            thread::sleep(MONITOR_INTERVAL);
            for target in [ManagedHostWork::PinvouApp, ManagedHostWork::PinvouAsr] {
                let Ok(snapshot) = supervisor.controller.status(target) else {
                    continue;
                };
                let changed = previous.get(&target) != Some(&snapshot.observation);
                let heartbeat_due = last_recorded
                    .get(&target)
                    .and_then(|recorded| recorded.elapsed().ok())
                    .map(|elapsed| elapsed >= MONITOR_HEARTBEAT)
                    .unwrap_or(true);
                if changed || heartbeat_due {
                    if let Err(error) = supervisor
                        .observation_journal
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .record_observation(target, &snapshot)
                    {
                        eprintln!("pinvou-supervisor monitor evidence failed: {error}");
                    } else {
                        last_recorded.insert(target, SystemTime::now());
                    }
                }
                previous.insert(target, snapshot.observation);
            }
        }
    });
}

fn serve(listener: UnixListener, supervisor: Arc<Supervisor>) -> Result<(), String> {
    let active_clients = Arc::new(AtomicUsize::new(0));
    for incoming in listener.incoming() {
        let stream = incoming.map_err(|error| format!("accept supervisor client: {error}"))?;
        set_close_on_exec(stream.as_raw_fd())?;
        if active_clients.fetch_add(1, Ordering::AcqRel) >= MAX_ACTIVE_CLIENTS {
            active_clients.fetch_sub(1, Ordering::AcqRel);
            continue;
        }
        let active_clients = Arc::clone(&active_clients);
        let supervisor = Arc::clone(&supervisor);
        thread::spawn(move || {
            if let Err(error) = handle_client(stream, &supervisor) {
                eprintln!(
                    "pinvou-supervisor rejected client: {}",
                    bound_detail(&error)
                );
            }
            active_clients.fetch_sub(1, Ordering::AcqRel);
        });
    }
    Ok(())
}

pub fn run_daemon() -> Result<(), String> {
    validate_embedded_descriptor()?;
    let listener = activated_listener()?;
    let state_directory = state_directory()?;
    let control_ledger = ControlLedger::open(&state_directory)?;
    let observation_journal = ObservationJournal::open(&state_directory)?;
    let supervisor = Arc::new(Supervisor {
        controller: Arc::new(SystemdUnitController),
        control_ledger: Mutex::new(control_ledger),
        observation_journal: Mutex::new(observation_journal),
        control_serial: Mutex::new(()),
    });
    spawn_monitor(Arc::clone(&supervisor));
    serve(listener, supervisor)
}

fn client_socket_path() -> Result<PathBuf, String> {
    Ok(fixed_runtime_directory()?.join(SOCKET_RELATIVE_PATH))
}

fn fixed_runtime_directory() -> Result<PathBuf, String> {
    let uid = unsafe { libc::geteuid() };
    let runtime = PathBuf::from(format!("/run/user/{uid}"));
    let metadata = fs::symlink_metadata(&runtime)
        .map_err(|error| format!("inspect fixed user runtime directory: {error}"))?;
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != uid
        || metadata.mode() & 0o022 != 0
    {
        return Err("fixed /run/user/<uid> directory is not trusted".to_string());
    }
    Ok(runtime)
}

fn activate_socket(deadline: Instant) -> Result<(), String> {
    run_systemctl_with_timeout(
        &["--user", "start", SOCKET_UNIT],
        remaining_client_budget(deadline)?.min(SYSTEMCTL_TIMEOUT),
    )
    .map(|_| ())
}

pub fn send_client_request(request: &SupervisorRequest) -> Result<SupervisorReceipt, String> {
    let deadline = Instant::now()
        .checked_add(IO_TIMEOUT)
        .ok_or_else(|| "supervisor client deadline overflow".to_string())?;
    send_client_request_until(request, deadline)
}

fn send_client_request_until(
    request: &SupervisorRequest,
    deadline: Instant,
) -> Result<SupervisorReceipt, String> {
    request
        .validate()
        .map_err(|error| format!("invalid bounded request: {error:?}"))?;
    let socket = client_socket_path()?;
    let mut stream = match connect_with_deadline(&socket, deadline) {
        Ok(stream) => stream,
        Err(first_error) => {
            activate_socket(deadline).map_err(|activation_error| {
                format!("connect failed ({first_error}); activation failed ({activation_error})")
            })?;
            retry_client_connect_until_with(
                deadline,
                first_error,
                |request_deadline| connect_with_deadline(&socket, request_deadline),
                Instant::now,
                thread::sleep,
            )?
        }
    };
    set_close_on_exec(stream.as_raw_fd())?;
    verify_same_uid(&stream)?;
    enable_passcred(&stream)?;
    let mut encoded = serde_json::to_vec(request).map_err(|error| error.to_string())?;
    if encoded.len() + 1 > MAX_REQUEST_BYTES {
        return Err("request exceeds protocol bound".to_string());
    }
    encoded.push(b'\n');
    write_all_with_deadline(&mut stream, &encoded, deadline)?;
    let (mut response, sender) = read_response_with_credentials(&stream, deadline)?;
    verify_supervisor_sender(sender, deadline)?;
    if response.is_empty() || response.len() > MAX_RESPONSE_BYTES || !response.ends_with(b"\n") {
        return Err("response is empty, truncated, or oversized".to_string());
    }
    response.pop();
    let receipt: SupervisorReceipt =
        serde_json::from_slice(&response).map_err(|error| error.to_string())?;
    receipt.validate_for(request).map_err(str::to_string)?;
    Ok(receipt)
}

fn write_all_with_deadline(
    stream: &mut UnixStream,
    bytes: &[u8],
    deadline: Instant,
) -> Result<(), String> {
    let mut written = 0;
    while written < bytes.len() {
        stream
            .set_write_timeout(Some(remaining_client_budget(deadline)?))
            .map_err(|error| format!("set client write timeout: {error}"))?;
        match stream.write(&bytes[written..]) {
            Ok(0) => return Err("supervisor socket closed while writing request".to_string()),
            Ok(count) => written += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("write supervisor request: {error}")),
        }
    }
    Ok(())
}

fn remaining_client_budget(deadline: Instant) -> Result<Duration, String> {
    remaining_client_budget_at(deadline, Instant::now())
}

fn remaining_client_budget_at(deadline: Instant, now: Instant) -> Result<Duration, String> {
    deadline
        .checked_duration_since(now)
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| "supervisor client exhausted its total deadline".to_string())
}

fn retry_client_connect_until_with<T, Connect, Now, Sleep>(
    deadline: Instant,
    mut last_error: std::io::Error,
    mut connect: Connect,
    mut now: Now,
    mut sleep: Sleep,
) -> Result<T, String>
where
    Connect: FnMut(Instant) -> std::io::Result<T>,
    Now: FnMut() -> Instant,
    Sleep: FnMut(Duration),
{
    for attempt_index in 0..CLIENT_CONNECT_RETRIES {
        if remaining_client_budget_at(deadline, now()).is_err() {
            return Err(format!(
                "supervisor connect retry budget was exhausted after: {last_error}"
            ));
        }
        match connect(deadline) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = error,
        }
        if attempt_index + 1 < CLIENT_CONNECT_RETRIES {
            let Ok(remaining) = remaining_client_budget_at(deadline, now()) else {
                break;
            };
            sleep(CLIENT_CONNECT_RETRY_DELAY.min(remaining));
        }
    }
    Err(format!(
        "activated supervisor socket did not accept: {last_error}"
    ))
}

fn connect_with_deadline(socket_path: &Path, deadline: Instant) -> std::io::Result<UnixStream> {
    let path = socket_path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if path.is_empty()
        || path.contains(&0)
        || path.len() >= address.sun_path.len()
        || remaining_client_budget_io(deadline).is_err()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Unix socket path or client deadline is invalid",
        ));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (destination, source) in address.sun_path.iter_mut().zip(path.iter().copied()) {
        *destination = source as libc::c_char;
    }

    let raw_fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if raw_fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let owned_fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    let address_length = std::mem::offset_of!(libc::sockaddr_un, sun_path)
        .saturating_add(path.len())
        .saturating_add(1);
    let connected = unsafe {
        libc::connect(
            owned_fd.as_raw_fd(),
            std::ptr::addr_of!(address).cast(),
            address_length as libc::socklen_t,
        )
    };
    if connected != 0 {
        let error = std::io::Error::last_os_error();
        let retryable = error.raw_os_error().is_some_and(|code| {
            code == libc::EINPROGRESS
                || code == libc::EAGAIN
                || code == libc::EWOULDBLOCK
                || code == libc::EINTR
        });
        if !retryable {
            return Err(error);
        }
        poll_connected(owned_fd.as_raw_fd(), deadline)?;
    }
    let flags = unsafe { libc::fcntl(owned_fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe {
            libc::fcntl(
                owned_fd.as_raw_fd(),
                libc::F_SETFL,
                flags & !libc::O_NONBLOCK,
            )
        } < 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { UnixStream::from_raw_fd(owned_fd.into_raw_fd()) })
}

fn poll_connected(fd: RawFd, deadline: Instant) -> std::io::Result<()> {
    loop {
        let remaining = remaining_client_budget_io(deadline)?;
        let timeout_ms = remaining.as_millis().max(1).min(libc::c_int::MAX as u128) as libc::c_int;
        let mut descriptor = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let result = unsafe { libc::poll(std::ptr::addr_of_mut!(descriptor), 1, timeout_ms) };
        if result == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Unix socket connect deadline elapsed",
            ));
        }
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        let mut socket_error: libc::c_int = 0;
        let mut length = std::mem::size_of_val(&socket_error) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                std::ptr::addr_of_mut!(socket_error).cast(),
                &mut length,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        return if socket_error == 0 {
            Ok(())
        } else {
            Err(std::io::Error::from_raw_os_error(socket_error))
        };
    }
}

fn remaining_client_budget_io(deadline: Instant) -> std::io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "supervisor client deadline elapsed",
            )
        })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread;

    use super::*;

    const APP_MAIN_PID: u32 = 42;
    const APP_INVOCATION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const ASR_INVOCATION: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const STALE_INVOCATION: &str = "cccccccccccccccccccccccccccccccc";
    static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(1);

    struct FakeController {
        observations: Mutex<HashMap<ManagedHostWork, HostWorkObservation>>,
        injected_integrity_errors: Mutex<HashMap<ManagedHostWork, String>>,
        app_stop_count: AtomicUsize,
        asr_stop_count: AtomicUsize,
        launch_count: AtomicUsize,
        app_status_count: AtomicUsize,
        fail_app_status_on: AtomicUsize,
        change_app_observation_on_second_status: Mutex<Option<HostWorkObservation>>,
        fail_launch: AtomicBool,
        asr_status_count: AtomicUsize,
        fail_asr_stop: AtomicBool,
        change_asr_generation_on_second_status: AtomicBool,
        invalid_app_protection: AtomicBool,
    }

    impl FakeController {
        fn active() -> Self {
            let observations = HashMap::from([
                (
                    ManagedHostWork::PinvouApp,
                    observation(
                        ObservedWorkState::Active,
                        Some(APP_MAIN_PID),
                        Some(APP_INVOCATION),
                    ),
                ),
                (
                    ManagedHostWork::PinvouAsr,
                    observation(ObservedWorkState::Active, Some(84), Some(ASR_INVOCATION)),
                ),
            ]);
            Self {
                observations: Mutex::new(observations),
                injected_integrity_errors: Mutex::new(HashMap::new()),
                app_stop_count: AtomicUsize::new(0),
                asr_stop_count: AtomicUsize::new(0),
                launch_count: AtomicUsize::new(0),
                app_status_count: AtomicUsize::new(0),
                fail_app_status_on: AtomicUsize::new(0),
                change_app_observation_on_second_status: Mutex::new(None),
                fail_launch: AtomicBool::new(false),
                asr_status_count: AtomicUsize::new(0),
                fail_asr_stop: AtomicBool::new(false),
                change_asr_generation_on_second_status: AtomicBool::new(false),
                invalid_app_protection: AtomicBool::new(false),
            }
        }

        fn set_observation(&self, target: ManagedHostWork, value: HostWorkObservation) {
            self.observations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(target, value);
        }

        fn set_integrity_error(&self, target: ManagedHostWork, value: String) {
            self.injected_integrity_errors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(target, value);
        }

        fn change_app_observation_on_second_status(&self, value: HostWorkObservation) {
            *self
                .change_app_observation_on_second_status
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(value);
        }
    }

    impl UnitController for FakeController {
        fn status(&self, target: ManagedHostWork) -> Result<UnitSnapshot, String> {
            if target == ManagedHostWork::PinvouApp {
                let call = self.app_status_count.fetch_add(1, Ordering::SeqCst) + 1;
                if self.fail_app_status_on.load(Ordering::SeqCst) == call {
                    return Err(format!("injected app status failure on call {call}"));
                }
                if call == 2 {
                    let changed = self
                        .change_app_observation_on_second_status
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take();
                    if let Some(changed) = changed {
                        self.set_observation(ManagedHostWork::PinvouApp, changed);
                    }
                }
            }
            if target == ManagedHostWork::PinvouAsr
                && self.asr_status_count.fetch_add(1, Ordering::SeqCst) == 1
                && self
                    .change_asr_generation_on_second_status
                    .load(Ordering::SeqCst)
            {
                self.set_observation(
                    ManagedHostWork::PinvouAsr,
                    observation(ObservedWorkState::Active, Some(85), Some(STALE_INVOCATION)),
                );
            }
            let observation = self
                .observations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&target)
                .expect("fixed fake target")
                .clone();
            let integrity_error = self
                .injected_integrity_errors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&target)
                .cloned()
                .or_else(|| {
                    (target == ManagedHostWork::PinvouApp
                        && observation.state == ObservedWorkState::Active
                        && self.invalid_app_protection.load(Ordering::SeqCst))
                    .then(|| "injected app protection mismatch".to_string())
                });
            Ok(UnitSnapshot {
                observation,
                control_group: Some("/test".to_string()),
                integrity_error,
            })
        }

        fn stop(&self, target: ManagedHostWork) -> Result<(), String> {
            match target {
                ManagedHostWork::PinvouApp => {
                    self.app_stop_count.fetch_add(1, Ordering::SeqCst);
                }
                ManagedHostWork::PinvouAsr => {
                    self.asr_stop_count.fetch_add(1, Ordering::SeqCst);
                    if self.fail_asr_stop.load(Ordering::SeqCst) {
                        return Err("injected ASR stop timeout".to_string());
                    }
                }
            }
            self.set_observation(target, observation(ObservedWorkState::Inactive, None, None));
            Ok(())
        }

        fn launch_app(&self) -> Result<(), String> {
            self.launch_count.fetch_add(1, Ordering::SeqCst);
            if self.fail_launch.load(Ordering::SeqCst) {
                return Err("injected app launch failure".to_string());
            }
            self.set_observation(
                ManagedHostWork::PinvouApp,
                observation(
                    ObservedWorkState::Active,
                    Some(APP_MAIN_PID),
                    Some(APP_INVOCATION),
                ),
            );
            Ok(())
        }
    }

    fn observation(
        state: ObservedWorkState,
        main_pid: Option<u32>,
        instance_generation: Option<&str>,
    ) -> HostWorkObservation {
        HostWorkObservation {
            instance_generation: instance_generation.map(str::to_string),
            state,
            sub_state: "test".to_string(),
            unit_result: "success".to_string(),
            main_pid,
            restart_count: Some(0),
            cgroup: CgroupObservation::default(),
        }
    }

    fn test_supervisor(controller: Arc<FakeController>, root: &Path) -> Supervisor {
        Supervisor {
            controller,
            control_ledger: Mutex::new(ControlLedger::open(root).expect("control ledger")),
            observation_journal: Mutex::new(
                ObservationJournal::open(root).expect("observation journal"),
            ),
            control_serial: Mutex::new(()),
        }
    }

    fn temp_directory(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pinvou-supervisor-{label}-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("temp dir");
        path
    }

    fn asr_stop(request_id: &str) -> SupervisorRequest {
        SupervisorRequest::stop_pinvou_asr(request_id, ASR_INVOCATION)
    }

    fn valid_restart_properties(target: ManagedHostWork) -> HashMap<String, String> {
        let (restart_usec, start_limit_interval_usec) = match target {
            ManagedHostWork::PinvouApp => ("15s", "5min"),
            ManagedHostWork::PinvouAsr => ("3s", "1min"),
        };
        HashMap::from([
            ("Restart".to_string(), "on-failure".to_string()),
            ("RestartUSec".to_string(), restart_usec.to_string()),
            (
                "StartLimitIntervalUSec".to_string(),
                start_limit_interval_usec.to_string(),
            ),
            ("StartLimitBurst".to_string(), "3".to_string()),
        ])
    }

    fn valid_app_resource_properties() -> HashMap<String, String> {
        const GIB: u64 = 1024 * 1024 * 1024;
        HashMap::from([
            ("OOMPolicy".to_string(), "kill".to_string()),
            ("KillMode".to_string(), "control-group".to_string()),
            ("TasksMax".to_string(), "512".to_string()),
            ("MemoryHigh".to_string(), (4 * GIB).to_string()),
            ("MemoryMax".to_string(), (8 * GIB).to_string()),
            ("MemorySwapMax".to_string(), (2 * GIB).to_string()),
        ])
    }

    #[test]
    fn embedded_descriptor_is_closed_and_consistent() {
        assert!(validate_embedded_descriptor().is_ok());
        assert_eq!(APP_UNIT, "pinvou3-app.service");
        assert_eq!(ASR_UNIT, "pinvou-qwen3-asr.service");
    }

    #[test]
    fn repeated_stop_uses_same_receipt_without_second_side_effect() {
        let root = temp_directory("idempotent-stop");
        let controller = Arc::new(FakeController::active());
        let supervisor = test_supervisor(Arc::clone(&controller), &root);
        let request = asr_stop("governor:stop-1");
        let first = supervisor.process(&request, APP_MAIN_PID);
        let second = supervisor.process(&request, APP_MAIN_PID);
        assert_eq!(first, second);
        assert_eq!(first.outcome, SupervisorOutcome::Applied);
        assert_eq!(controller.asr_stop_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn stale_expected_invocation_is_rejected_before_stop() {
        let root = temp_directory("stale-invocation");
        let controller = Arc::new(FakeController::active());
        let supervisor = test_supervisor(Arc::clone(&controller), &root);
        let request = SupervisorRequest::stop_pinvou_asr("governor:stale-1", STALE_INVOCATION);
        let receipt = supervisor.process(&request, APP_MAIN_PID);
        assert_eq!(receipt.outcome, SupervisorOutcome::Rejected);
        assert_eq!(controller.asr_stop_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn invocation_change_after_pending_is_rechecked_before_stop() {
        let root = temp_directory("pre-action-invocation");
        let controller = Arc::new(FakeController::active());
        controller
            .change_asr_generation_on_second_status
            .store(true, Ordering::SeqCst);
        let supervisor = test_supervisor(Arc::clone(&controller), &root);
        let receipt = supervisor.process(&asr_stop("governor:pre-action-1"), APP_MAIN_PID);
        assert_eq!(receipt.outcome, SupervisorOutcome::Rejected);
        assert_eq!(controller.asr_stop_count.load(Ordering::SeqCst), 0);
        let repeated = supervisor.process(&asr_stop("governor:pre-action-1"), APP_MAIN_PID);
        assert_eq!(repeated, receipt);
        assert_eq!(controller.asr_stop_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn non_app_peer_pid_cannot_control_asr() {
        let root = temp_directory("peer-pid");
        let controller = Arc::new(FakeController::active());
        let supervisor = test_supervisor(Arc::clone(&controller), &root);
        let receipt = supervisor.process(&asr_stop("governor:peer-1"), APP_MAIN_PID + 1);
        assert_eq!(receipt.outcome, SupervisorOutcome::Rejected);
        assert_eq!(controller.asr_stop_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn crash_after_pending_reconciles_without_replaying_stop() {
        let root = temp_directory("pending-restart");
        let first_controller = Arc::new(FakeController::active());
        first_controller.fail_asr_stop.store(true, Ordering::SeqCst);
        let request = asr_stop("governor:pending-1");
        let first =
            test_supervisor(Arc::clone(&first_controller), &root).process(&request, APP_MAIN_PID);
        assert_eq!(first.outcome, SupervisorOutcome::OutcomeUnknown);
        assert_eq!(first_controller.asr_stop_count.load(Ordering::SeqCst), 1);

        let second_controller = Arc::new(FakeController::active());
        second_controller.set_observation(
            ManagedHostWork::PinvouAsr,
            observation(ObservedWorkState::Inactive, None, None),
        );
        let restarted = test_supervisor(Arc::clone(&second_controller), &root);
        let reconciled = restarted.process(&request, APP_MAIN_PID);
        assert_eq!(reconciled.outcome, SupervisorOutcome::Reconciled);
        assert_eq!(second_controller.asr_stop_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn completion_fsync_failure_is_unknown_then_status_reconciled() {
        let root = temp_directory("completion-fsync");
        let controller = Arc::new(FakeController::active());
        let supervisor = test_supervisor(Arc::clone(&controller), &root);
        supervisor
            .control_ledger
            .lock()
            .expect("ledger")
            .fail_next_completion_sync = true;
        let request = asr_stop("governor:fsync-1");
        assert_eq!(
            supervisor.process(&request, APP_MAIN_PID).outcome,
            SupervisorOutcome::OutcomeUnknown
        );
        assert_eq!(
            supervisor.process(&request, APP_MAIN_PID).outcome,
            SupervisorOutcome::Reconciled
        );
        assert_eq!(controller.asr_stop_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn outcome_unknown_same_id_never_replays_action() {
        let root = temp_directory("unknown-no-replay");
        let controller = Arc::new(FakeController::active());
        controller.fail_asr_stop.store(true, Ordering::SeqCst);
        let supervisor = test_supervisor(Arc::clone(&controller), &root);
        let request = asr_stop("governor:unknown-1");
        assert_eq!(
            supervisor.process(&request, APP_MAIN_PID).outcome,
            SupervisorOutcome::OutcomeUnknown
        );
        assert_eq!(
            supervisor.process(&request, APP_MAIN_PID).outcome,
            SupervisorOutcome::OutcomeUnknown
        );
        assert_eq!(controller.asr_stop_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn rolling_observations_do_not_forget_completed_control_after_restart() {
        let root = temp_directory("observation-rotation");
        let controller = Arc::new(FakeController::active());
        let supervisor = test_supervisor(Arc::clone(&controller), &root);
        let request = asr_stop("governor:rotation-1");
        let completed = supervisor.process(&request, APP_MAIN_PID);
        {
            let mut observations = supervisor.observation_journal.lock().expect("observations");
            observations.max_bytes = 512;
            let snapshot = controller
                .status(ManagedHostWork::PinvouApp)
                .expect("snapshot");
            for _ in 0..40 {
                observations
                    .record_observation(ManagedHostWork::PinvouApp, &snapshot)
                    .expect("rotating observation");
            }
        }
        assert!(root.join("observations-v1.jsonl.1").is_file());
        drop(supervisor);

        let restarted_controller = Arc::new(FakeController::active());
        let restarted = test_supervisor(Arc::clone(&restarted_controller), &root);
        assert_eq!(restarted.process(&request, APP_MAIN_PID), completed);
        assert_eq!(
            restarted_controller.asr_stop_count.load(Ordering::SeqCst),
            0
        );
    }

    #[test]
    fn full_control_ledger_rejects_new_id_before_side_effect() {
        let root = temp_directory("ledger-capacity");
        let controller = Arc::new(FakeController::active());
        let supervisor = test_supervisor(Arc::clone(&controller), &root);
        supervisor
            .control_ledger
            .lock()
            .expect("ledger")
            .max_stored_requests = 0;
        let receipt = supervisor.process(&asr_stop("governor:capacity-1"), APP_MAIN_PID);
        assert_eq!(receipt.outcome, SupervisorOutcome::Rejected);
        assert_eq!(controller.asr_stop_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn atomic_control_compaction_keeps_pending_and_completed_tombstone() {
        let root = temp_directory("control-compaction");
        let completed_request = asr_stop("governor:compact-completed");
        let pending_request = asr_stop("governor:compact-pending");
        let mut ledger = ControlLedger::open(&root).expect("ledger");
        let completed_fingerprint = RequestFingerprint::from(&completed_request);
        ledger
            .record_pending(completed_fingerprint.clone(), None)
            .expect("completed pending");
        ledger
            .record_completed(
                completed_fingerprint,
                receipt(
                    &completed_request,
                    SupervisorOutcome::Applied,
                    None,
                    "test completion",
                ),
            )
            .expect("completed tombstone");
        ledger
            .record_pending(RequestFingerprint::from(&pending_request), None)
            .expect("pending");
        ledger.compact().expect("atomic compaction");
        drop(ledger);

        let restarted = ControlLedger::open(&root).expect("restarted ledger");
        assert!(matches!(
            restarted.lookup(&completed_request.request_id),
            Some(StoredRequest::Completed(_, _))
        ));
        assert!(matches!(
            restarted.lookup(&pending_request.request_id),
            Some(StoredRequest::Pending(_))
        ));
        assert!(!root.join("control-v1.jsonl.1").exists());
    }

    #[test]
    fn committed_control_ledger_corruption_fails_closed() {
        let root = temp_directory("committed-corruption");
        let request = asr_stop("governor:corrupt-committed");
        let mut ledger = ControlLedger::open(&root).expect("ledger");
        ledger
            .record_pending(RequestFingerprint::from(&request), None)
            .expect("durable pending");
        drop(ledger);

        let path = root.join("control-v1.jsonl");
        let mut file = OpenOptions::new()
            .append(true)
            .open(path)
            .expect("append corrupt frame");
        file.write_all(b"{not-valid-json}\n")
            .expect("write corrupt committed frame");
        file.sync_all().expect("sync corrupt committed frame");
        drop(file);

        assert!(ControlLedger::open(&root).is_err());
    }

    #[test]
    fn unterminated_crash_tail_keeps_prior_pending_and_never_replays() {
        let root = temp_directory("unterminated-tail");
        let request = asr_stop("governor:torn-tail");
        let mut ledger = ControlLedger::open(&root).expect("ledger");
        ledger
            .record_pending(RequestFingerprint::from(&request), None)
            .expect("durable pending");
        drop(ledger);

        let path = root.join("control-v1.jsonl");
        let mut file = OpenOptions::new()
            .append(true)
            .open(path)
            .expect("append torn frame");
        file.write_all(b"{\"event\":\"control_completed_tombstone\"")
            .expect("write unterminated tail");
        file.sync_all().expect("make crash tail observable");
        drop(file);

        let controller = Arc::new(FakeController::active());
        let restarted = test_supervisor(Arc::clone(&controller), &root);
        assert_eq!(
            restarted.process(&request, APP_MAIN_PID).outcome,
            SupervisorOutcome::OutcomeUnknown
        );
        assert_eq!(controller.asr_stop_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn newly_launched_unprotected_app_is_stopped_fail_closed() {
        let root = temp_directory("unprotected-launch");
        let controller = Arc::new(FakeController::active());
        controller.set_observation(
            ManagedHostWork::PinvouApp,
            observation(ObservedWorkState::Inactive, None, None),
        );
        controller
            .invalid_app_protection
            .store(true, Ordering::SeqCst);
        let supervisor = test_supervisor(Arc::clone(&controller), &root);
        let request = SupervisorRequest::launch_pinvou_app("desktop-launch:test");
        let receipt = supervisor.process(&request, 9999);
        assert_eq!(receipt.outcome, SupervisorOutcome::OutcomeUnknown);
        assert_eq!(controller.launch_count.load(Ordering::SeqCst), 1);
        assert_eq!(controller.app_stop_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn inactive_or_failed_untrusted_app_profile_never_starts_or_stops_app() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let inactive = observation(ObservedWorkState::Inactive, None, None);
        let mut missing = valid_app_resource_properties();
        missing.remove("MemoryMax");
        let mut widened = valid_app_resource_properties();
        widened.insert("MemoryMax".to_string(), (9 * GIB).to_string());
        let mut untrusted = valid_app_resource_properties();
        untrusted.insert("OOMPolicy".to_string(), "stop".to_string());
        let invalid_profiles = [
            ("missing", missing),
            ("widened", widened),
            ("untrusted", untrusted),
        ];

        for state in [ObservedWorkState::Inactive, ObservedWorkState::Failed] {
            for (label, properties) in &invalid_profiles {
                let integrity_error = validate_effective_protection(
                    ManagedHostWork::PinvouApp,
                    &inactive,
                    properties,
                    false,
                )
                .expect_err("invalid inactive profile must fail before launch");
                let root = temp_directory(&format!("launch-{state:?}-{label}"));
                let controller = Arc::new(FakeController::active());
                controller
                    .set_observation(ManagedHostWork::PinvouApp, observation(state, None, None));
                controller.set_integrity_error(ManagedHostWork::PinvouApp, integrity_error);
                let supervisor = test_supervisor(Arc::clone(&controller), &root);
                let receipt = supervisor.process(
                    &SupervisorRequest::launch_pinvou_app(format!(
                        "desktop-launch:{state:?}:{label}"
                    )),
                    APP_MAIN_PID,
                );
                assert_eq!(receipt.outcome, SupervisorOutcome::Rejected);
                assert!(receipt.detail.contains("effective-policy validation"));
                assert_eq!(controller.launch_count.load(Ordering::SeqCst), 0);
                assert_eq!(controller.app_stop_count.load(Ordering::SeqCst), 0);
            }
        }
    }

    #[test]
    fn unavailable_initial_launch_status_never_starts_or_stops_app() {
        let root = temp_directory("launch-before-unavailable");
        let controller = Arc::new(FakeController::active());
        controller.fail_app_status_on.store(1, Ordering::SeqCst);
        let supervisor = test_supervisor(Arc::clone(&controller), &root);
        let receipt = supervisor.process(
            &SupervisorRequest::launch_pinvou_app("desktop-launch:before-unavailable"),
            APP_MAIN_PID,
        );
        assert_eq!(receipt.outcome, SupervisorOutcome::Rejected);
        assert!(receipt.detail.contains("trusted pre-action status"));
        assert_eq!(controller.launch_count.load(Ordering::SeqCst), 0);
        assert_eq!(controller.app_stop_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn transitional_unknown_or_pid_owning_app_states_never_launch() {
        for (index, (state, main_pid, generation)) in [
            (
                ObservedWorkState::Activating,
                Some(APP_MAIN_PID),
                Some(APP_INVOCATION),
            ),
            (
                ObservedWorkState::Deactivating,
                Some(APP_MAIN_PID),
                Some(APP_INVOCATION),
            ),
            (ObservedWorkState::Unknown, None, None),
            (
                ObservedWorkState::Failed,
                Some(APP_MAIN_PID),
                Some(APP_INVOCATION),
            ),
            (
                ObservedWorkState::Inactive,
                Some(APP_MAIN_PID),
                Some(APP_INVOCATION),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let root = temp_directory(&format!("launch-ineligible-{index}"));
            let controller = Arc::new(FakeController::active());
            controller.set_observation(
                ManagedHostWork::PinvouApp,
                observation(state, main_pid, generation),
            );
            let supervisor = test_supervisor(Arc::clone(&controller), &root);
            let receipt = supervisor.process(
                &SupervisorRequest::launch_pinvou_app(format!("desktop-launch:ineligible-{index}")),
                APP_MAIN_PID,
            );
            assert_eq!(receipt.outcome, SupervisorOutcome::Rejected);
            assert!(receipt.detail.contains("no live MainPID"));
            assert_eq!(controller.launch_count.load(Ordering::SeqCst), 0);
            assert_eq!(controller.app_stop_count.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn failed_app_without_main_pid_is_attributable_to_launch_request() {
        let root = temp_directory("launch-failed-no-pid");
        let controller = Arc::new(FakeController::active());
        controller.set_observation(
            ManagedHostWork::PinvouApp,
            observation(ObservedWorkState::Failed, None, None),
        );
        let supervisor = test_supervisor(Arc::clone(&controller), &root);
        let receipt = supervisor.process(
            &SupervisorRequest::launch_pinvou_app("desktop-launch:failed-no-pid"),
            APP_MAIN_PID,
        );
        assert_eq!(receipt.outcome, SupervisorOutcome::Applied);
        assert_eq!(controller.launch_count.load(Ordering::SeqCst), 1);
        assert_eq!(controller.app_stop_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn second_preflight_must_still_have_attributable_launch_state() {
        let root = temp_directory("launch-second-preflight");
        let controller = Arc::new(FakeController::active());
        controller.set_observation(
            ManagedHostWork::PinvouApp,
            observation(ObservedWorkState::Inactive, None, None),
        );
        controller.change_app_observation_on_second_status(observation(
            ObservedWorkState::Activating,
            Some(APP_MAIN_PID),
            Some(APP_INVOCATION),
        ));
        let supervisor = test_supervisor(Arc::clone(&controller), &root);
        let receipt = supervisor.process(
            &SupervisorRequest::launch_pinvou_app("desktop-launch:preflight-transition"),
            APP_MAIN_PID,
        );
        assert_eq!(receipt.outcome, SupervisorOutcome::Rejected);
        assert!(receipt.detail.contains("attributable launch precondition"));
        assert_eq!(controller.launch_count.load(Ordering::SeqCst), 0);
        assert_eq!(controller.app_stop_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn unavailable_status_after_owned_launch_rolls_back_app() {
        let root = temp_directory("launch-after-unavailable");
        let controller = Arc::new(FakeController::active());
        controller.set_observation(
            ManagedHostWork::PinvouApp,
            observation(ObservedWorkState::Inactive, None, None),
        );
        controller.fail_app_status_on.store(3, Ordering::SeqCst);
        let supervisor = test_supervisor(Arc::clone(&controller), &root);
        let receipt = supervisor.process(
            &SupervisorRequest::launch_pinvou_app("desktop-launch:after-unavailable"),
            APP_MAIN_PID,
        );
        assert_eq!(receipt.outcome, SupervisorOutcome::OutcomeUnknown);
        assert!(receipt.detail.contains("post-launch status is unavailable"));
        assert!(receipt.detail.contains("unprotected app was stopped"));
        assert_eq!(controller.launch_count.load(Ordering::SeqCst), 1);
        assert_eq!(controller.app_stop_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failed_launch_with_unavailable_status_does_not_claim_rollback_ownership() {
        let root = temp_directory("failed-launch-after-unavailable");
        let controller = Arc::new(FakeController::active());
        controller.set_observation(
            ManagedHostWork::PinvouApp,
            observation(ObservedWorkState::Inactive, None, None),
        );
        controller.fail_launch.store(true, Ordering::SeqCst);
        controller.fail_app_status_on.store(3, Ordering::SeqCst);
        let supervisor = test_supervisor(Arc::clone(&controller), &root);
        let receipt = supervisor.process(
            &SupervisorRequest::launch_pinvou_app("desktop-launch:failed-unavailable"),
            APP_MAIN_PID,
        );
        assert_eq!(receipt.outcome, SupervisorOutcome::OutcomeUnknown);
        assert!(receipt.detail.contains("launch ownership was not proven"));
        assert_eq!(controller.launch_count.load(Ordering::SeqCst), 1);
        assert_eq!(controller.app_stop_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn untrusted_active_preflight_never_claims_launch_or_stops_existing_app() {
        let root = temp_directory("launch-active-untrusted");
        let controller = Arc::new(FakeController::active());
        controller
            .invalid_app_protection
            .store(true, Ordering::SeqCst);
        let supervisor = test_supervisor(Arc::clone(&controller), &root);
        let receipt = supervisor.process(
            &SupervisorRequest::launch_pinvou_app("desktop-launch:active-untrusted"),
            APP_MAIN_PID,
        );
        assert_eq!(receipt.outcome, SupervisorOutcome::Rejected);
        assert_eq!(controller.launch_count.load(Ordering::SeqCst), 0);
        assert_eq!(controller.app_stop_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn effective_restart_policy_rejects_each_drop_in_override() {
        for target in [ManagedHostWork::PinvouApp, ManagedHostWork::PinvouAsr] {
            let properties = valid_restart_properties(target);
            assert!(validate_effective_restart_policy(target, &properties).is_ok());

            for (key, value) in [
                ("Restart", "no"),
                ("RestartUSec", "10s"),
                ("StartLimitIntervalUSec", "299s"),
                ("StartLimitBurst", "4"),
            ] {
                let mut overridden = properties.clone();
                overridden.insert(key.to_string(), value.to_string());
                assert!(
                    validate_effective_restart_policy(target, &overridden).is_err(),
                    "{target:?} must reject an effective {key} override"
                );
            }

            for missing in [
                "Restart",
                "RestartUSec",
                "StartLimitIntervalUSec",
                "StartLimitBurst",
            ] {
                let mut incomplete = properties.clone();
                incomplete.remove(missing);
                assert!(
                    validate_effective_restart_policy(target, &incomplete).is_err(),
                    "{target:?} must reject missing effective {missing}"
                );
            }
        }
    }

    #[test]
    fn restart_drop_in_override_makes_status_outcome_unknown() {
        for (index, (key, value)) in [
            ("Restart", "no"),
            ("RestartUSec", "10s"),
            ("StartLimitIntervalUSec", "299s"),
            ("StartLimitBurst", "4"),
        ]
        .into_iter()
        .enumerate()
        {
            let root = temp_directory(&format!("restart-override-status-{index}"));
            let controller = Arc::new(FakeController::active());
            let mut properties = valid_restart_properties(ManagedHostWork::PinvouApp);
            properties.insert(key.to_string(), value.to_string());
            let integrity_error =
                validate_effective_restart_policy(ManagedHostWork::PinvouApp, &properties)
                    .expect_err("override must not satisfy the descriptor");
            controller.set_integrity_error(ManagedHostWork::PinvouApp, integrity_error);
            let supervisor = test_supervisor(Arc::clone(&controller), &root);
            let request = SupervisorRequest::status(
                format!("status:restart-override-{index}"),
                ManagedHostWork::PinvouApp,
            );
            let receipt = supervisor.process(&request, APP_MAIN_PID);
            assert_eq!(receipt.outcome, SupervisorOutcome::OutcomeUnknown);
            assert!(receipt.detail.contains("status is untrusted"));
        }
    }

    #[test]
    fn systemd_timespan_parser_is_semantic_and_fail_closed() {
        assert_eq!(parse_systemd_timespan_usec("300s"), Some(300_000_000));
        assert_eq!(parse_systemd_timespan_usec("5min"), Some(300_000_000));
        assert_eq!(parse_systemd_timespan_usec("1min 30s"), Some(90_000_000));
        assert_eq!(parse_systemd_timespan_usec("1500ms"), Some(1_500_000));
        assert_eq!(parse_systemd_timespan_usec("infinity"), None);
        assert_eq!(parse_systemd_timespan_usec("1.5s"), None);
        assert_eq!(parse_systemd_timespan_usec("15seconds"), None);
        assert_eq!(parse_systemd_timespan_usec(""), None);
    }

    #[test]
    fn asr_memory_percentages_reject_wider_limits_and_allow_page_rounding() {
        let gib = 1024 * 1024 * 1024_u64;
        let physical = 32 * gib;
        let page = 4096;
        let high_ceiling = percentage_page_ceiling_bytes(physical, 20, page).expect("20% ceiling");
        let max_ceiling = percentage_page_ceiling_bytes(physical, 35, page).expect("35% ceiling");

        assert!(
            validate_asr_percentage_bounds(physical, page, high_ceiling, max_ceiling).is_ok(),
            "one-page percentage rounding must remain valid"
        );
        assert!(validate_asr_percentage_bounds(physical, page, 6 * gib, 11 * gib).is_ok());
        assert!(validate_asr_percentage_bounds(physical, page, 128 * 1024 * 1024, gib).is_ok());
        assert!(
            validate_asr_percentage_bounds(physical, page, high_ceiling + page, max_ceiling,)
                .is_err()
        );
        assert!(
            validate_asr_percentage_bounds(physical, page, high_ceiling, max_ceiling + page,)
                .is_err()
        );

        let ninety_percent =
            percentage_page_ceiling_bytes(physical, 90, page).expect("90% ceiling");
        let ninety_five_percent =
            percentage_page_ceiling_bytes(physical, 95, page).expect("95% ceiling");
        assert!(validate_asr_percentage_bounds(
            physical,
            page,
            ninety_percent,
            ninety_five_percent,
        )
        .is_err());
    }

    #[test]
    fn wider_asr_memory_percentage_makes_status_outcome_unknown() {
        let gib = 1024 * 1024 * 1024_u64;
        let physical = 32 * gib;
        let page = 4096;
        let high = percentage_page_ceiling_bytes(physical, 90, page).expect("90% ceiling");
        let max = percentage_page_ceiling_bytes(physical, 95, page).expect("95% ceiling");
        let integrity_error = validate_asr_percentage_bounds(physical, page, high, max)
            .expect_err("90%/95% must be wider than the ASR descriptor");

        let root = temp_directory("asr-wide-percentage-status");
        let controller = Arc::new(FakeController::active());
        controller.set_integrity_error(ManagedHostWork::PinvouAsr, integrity_error);
        let supervisor = test_supervisor(Arc::clone(&controller), &root);
        let request =
            SupervisorRequest::status("status:asr-wide-percentage", ManagedHostWork::PinvouAsr);
        let receipt = supervisor.process(&request, APP_MAIN_PID);
        assert_eq!(receipt.outcome, SupervisorOutcome::OutcomeUnknown);
        assert!(receipt.detail.contains("status is untrusted"));
    }

    #[test]
    fn physical_memory_parser_requires_exact_memtotal_kib() {
        assert_eq!(
            parse_mem_total_bytes("MemTotal:       32768000 kB\nMemFree: 1 kB\n"),
            Some(32_768_000 * 1024)
        );
        assert_eq!(parse_mem_total_bytes("MemTotal: 32 GB\n"), None);
        assert_eq!(parse_mem_total_bytes("MemFree: 32 kB\n"), None);
    }

    #[test]
    fn effective_asr_protection_checks_fragment_exec_and_real_cgroup_values() {
        let root = temp_directory("asr-fragment");
        let fragment = root.join(".config/systemd/user/pinvou-qwen3-asr.service");
        fs::create_dir_all(fragment.parent().expect("fragment parent")).expect("fragment dir");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&fragment)
            .expect("fragment");
        file.sync_all().expect("fragment sync");
        let executable = root.join(".pinvou3/asr/qwen3-asr-openvino/runtime/bin/python");
        let script = root.join(".pinvou3/asr/qwen3-asr-openvino/qwen3-asr-openvino.py");
        fs::create_dir_all(executable.parent().expect("executable parent"))
            .expect("executable dir");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&executable)
            .expect("executable");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&script)
            .expect("script");
        let physical = physical_memory_bytes().expect("physical memory");
        let page = system_page_size_bytes().expect("page size");
        let high = percentage_page_ceiling_bytes(physical, 20, page).expect("20% ceiling");
        let max = percentage_page_ceiling_bytes(physical, 35, page).expect("35% ceiling");
        let swap = 2 * 1024 * 1024 * 1024;
        let mut properties = HashMap::from([
            ("FragmentPath".to_string(), fragment.display().to_string()),
            (
                "ExecStart".to_string(),
                format!(
                    "path={}/.pinvou3/asr/qwen3-asr-openvino/runtime/bin/python ; argv[]={}/.pinvou3/asr/qwen3-asr-openvino/runtime/bin/python {}/.pinvou3/asr/qwen3-asr-openvino/qwen3-asr-openvino.py serve",
                    root.display(), root.display(), root.display()
                ),
            ),
            ("OOMPolicy".to_string(), "kill".to_string()),
            ("KillMode".to_string(), "control-group".to_string()),
            ("TasksMax".to_string(), "128".to_string()),
            ("MemoryHigh".to_string(), high.to_string()),
            ("MemoryMax".to_string(), max.to_string()),
            ("MemorySwapMax".to_string(), swap.to_string()),
        ]);
        properties.extend(valid_restart_properties(ManagedHostWork::PinvouAsr));
        for state in [ObservedWorkState::Inactive, ObservedWorkState::Failed] {
            let inactive = observation(state, None, None);
            assert!(
                validate_effective_unit(ManagedHostWork::PinvouAsr, &inactive, &properties).is_ok(),
                "inactive/failed ASR must validate its effective policy without a live cgroup"
            );

            let mut missing_policy = properties.clone();
            missing_policy.remove("MemoryMax");
            assert!(
                validate_effective_unit(ManagedHostWork::PinvouAsr, &inactive, &missing_policy,)
                    .is_err(),
                "inactive/failed ASR must fail closed when its effective policy is incomplete"
            );
        }
        let mut observed = observation(ObservedWorkState::Active, Some(84), Some(ASR_INVOCATION));
        observed.cgroup.memory_high_bytes = Some(high);
        observed.cgroup.memory_max_bytes = Some(max);
        observed.cgroup.memory_swap_max_bytes = Some(swap);
        assert!(
            validate_effective_unit(ManagedHostWork::PinvouAsr, &observed, &properties).is_ok()
        );
        observed.cgroup.memory_max_bytes = Some(max + 4096);
        assert!(
            validate_effective_unit(ManagedHostWork::PinvouAsr, &observed, &properties).is_err()
        );

        observed.cgroup.memory_max_bytes = Some(max);
        properties.insert(
            "ExecStart".to_string(),
            format!(
                "path=/tmp/evil ; argv[]=/tmp/evil {}/.pinvou3/asr/qwen3-asr-openvino/runtime/bin/python {}/.pinvou3/asr/qwen3-asr-openvino/qwen3-asr-openvino.py serve",
                root.display(), root.display()
            ),
        );
        assert!(
            validate_effective_unit(ManagedHostWork::PinvouAsr, &observed, &properties).is_err(),
            "trusted paths used only as arguments must not authenticate a different executable"
        );
    }

    #[test]
    fn exec_start_identity_is_exact_and_rejects_duplicate_path_fields() {
        let expected = "/usr/bin/pinvou3-tauri";
        assert!(exec_start_matches(
            "{ path=/usr/bin/pinvou3-tauri ; argv[]=/usr/bin/pinvou3-tauri ; ignore_errors=no ; }",
            expected,
            &[expected],
        ));
        assert!(!exec_start_matches(
            "{ path=/tmp/evil ; argv[]=/tmp/evil /usr/bin/pinvou3-tauri ; ignore_errors=no ; }",
            expected,
            &[expected],
        ));
        assert!(!exec_start_matches(
            "{ path=/usr/bin/pinvou3-tauri ; path=/tmp/evil ; argv[]=/usr/bin/pinvou3-tauri ; }",
            expected,
            &[expected],
        ));
    }

    #[test]
    fn effective_app_protection_requires_exact_megabook_profile() {
        let gib = 1024 * 1024 * 1024_u64;
        let mut properties = valid_app_resource_properties();
        for state in [ObservedWorkState::Inactive, ObservedWorkState::Failed] {
            let inactive = observation(state, None, None);
            assert!(
                validate_effective_protection(
                    ManagedHostWork::PinvouApp,
                    &inactive,
                    &properties,
                    false,
                )
                .is_ok(),
                "valid effective profile must be accepted before the app has a cgroup"
            );
        }
        let mut observed = observation(
            ObservedWorkState::Active,
            Some(APP_MAIN_PID),
            Some(APP_INVOCATION),
        );
        observed.cgroup.memory_high_bytes = Some(4 * gib);
        observed.cgroup.memory_max_bytes = Some(8 * gib);
        observed.cgroup.memory_swap_max_bytes = Some(2 * gib);
        assert!(validate_effective_protection(
            ManagedHostWork::PinvouApp,
            &observed,
            &properties,
            true,
        )
        .is_ok());

        properties.insert("MemoryMax".to_string(), (9 * gib).to_string());
        assert!(validate_effective_protection(
            ManagedHostWork::PinvouApp,
            &observed,
            &properties,
            true,
        )
        .is_err());
    }

    #[test]
    fn malformed_control_fields_never_reach_controller() {
        let raw = r#"{"protocol_version":1,"request_id":"d:3","target":"pinvou_asr","descriptor_revision":"pinvou-asr-descriptor-v1","expected_instance_generation":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","action":"stop","unit":"ssh.service"}"#;
        assert!(serde_json::from_str::<SupervisorRequest>(raw).is_err());
    }

    #[test]
    fn receipt_detail_bound_is_utf8_safe_and_byte_exact() {
        let bounded = bound_detail(&"错".repeat(MAX_DETAIL_BYTES));
        assert!(bounded.len() <= MAX_DETAIL_BYTES);
        assert!(bounded.is_char_boundary(bounded.len()));
    }

    #[test]
    fn cgroup_path_rejects_escape_and_root() {
        assert!(cgroup_directory("/").is_none());
        assert!(cgroup_directory("/../../etc").is_none());
        assert_eq!(
            cgroup_directory("/user.slice/app.service"),
            Some(PathBuf::from("/sys/fs/cgroup/user.slice/app.service"))
        );
    }

    #[test]
    fn peer_credentials_and_cloexec_are_enforced() {
        let (left, _right) = UnixStream::pair().expect("socketpair");
        let credentials = verify_same_uid(&left).expect("same uid");
        assert_eq!(credentials.pid, std::process::id());
        set_close_on_exec(left.as_raw_fd()).expect("cloexec");
        let flags = unsafe { libc::fcntl(left.as_raw_fd(), libc::F_GETFD) };
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
    }

    #[test]
    fn worker_thread_connection_authenticates_as_process_main_pid() {
        let root = temp_directory("thread-peer-pid");
        let socket_path = root.join("control.sock");
        let listener = UnixListener::bind(&socket_path).expect("listener");
        let worker = thread::spawn(move || UnixStream::connect(socket_path).expect("connect"));
        let (accepted, _) = listener.accept().expect("accept");
        let _client = worker.join().expect("worker");
        let credentials = peer_credentials(&accepted).expect("peer credentials");
        assert_eq!(credentials.pid, std::process::id());
        assert_eq!(credentials.uid, unsafe { libc::geteuid() });
    }

    #[test]
    fn socket_activation_listener_peer_can_differ_from_authenticated_response_sender() {
        let root = temp_directory("socket-activation-credentials");
        let socket_path = root.join("control.sock");
        let listener = UnixListener::bind(&socket_path).expect("listener");
        let listener_fd = listener.as_raw_fd();
        let child_pid = unsafe { libc::fork() };
        assert!(child_pid >= 0, "fork failed");
        if child_pid == 0 {
            let accepted =
                unsafe { libc::accept(listener_fd, std::ptr::null_mut(), std::ptr::null_mut()) };
            if accepted >= 0 {
                const RESPONSE: &[u8] = b"{}\n";
                let _ = unsafe { libc::write(accepted, RESPONSE.as_ptr().cast(), RESPONSE.len()) };
                unsafe { libc::close(accepted) };
            }
            unsafe { libc::_exit((accepted < 0) as i32) };
        }

        let client = UnixStream::connect(&socket_path).expect("client");
        enable_passcred(&client).expect("passcred");
        let listener_peer = peer_credentials(&client).expect("listener peer");
        let (response, response_sender) =
            read_response_with_credentials(&client, Instant::now() + Duration::from_secs(5))
                .expect("credentialed response");
        assert_eq!(response, b"{}\n");
        assert_eq!(listener_peer.pid, std::process::id());
        assert_eq!(response_sender.pid, child_pid as u32);
        assert_ne!(listener_peer.pid, response_sender.pid);
        assert!(verify_sender_identity(
            response_sender,
            unsafe { libc::geteuid() },
            listener_peer.pid,
        )
        .is_err());
        let mut child_status = 0;
        assert_eq!(
            unsafe { libc::waitpid(child_pid, &mut child_status, 0) },
            child_pid
        );
        assert!(libc::WIFEXITED(child_status));
        assert_eq!(libc::WEXITSTATUS(child_status), 0);
    }

    #[test]
    fn every_multichunk_response_read_has_stable_sender_credentials() {
        let (receiver, mut sender) = UnixStream::pair().expect("socketpair");
        enable_passcred(&receiver).expect("passcred");
        let writer = thread::spawn(move || {
            let mut response = vec![b'x'; 20_000];
            response.push(b'\n');
            sender.write_all(&response).expect("large response");
        });
        let (response, credentials) =
            read_response_with_credentials(&receiver, Instant::now() + Duration::from_secs(5))
                .expect("credentialed large response");
        writer.join().expect("writer");
        assert_eq!(response.len(), 20_001);
        assert_eq!(credentials.pid, std::process::id());
    }

    #[test]
    fn cli_response_chunks_share_one_absolute_read_deadline() {
        let (receiver, mut sender) = UnixStream::pair().expect("socketpair");
        enable_passcred(&receiver).expect("passcred");
        let writer = thread::spawn(move || {
            sender.write_all(b"{").expect("first response chunk");
            thread::sleep(Duration::from_millis(100));
            sender.write_all(b"}").expect("second response chunk");
            thread::sleep(Duration::from_millis(100));
            sender.write_all(b"\n").expect("final response chunk");
        });

        let result =
            read_response_with_credentials(&receiver, Instant::now() + Duration::from_millis(150));
        writer.join().expect("writer");
        assert!(
            result.is_err(),
            "three individually timely chunks must not multiply the total deadline"
        );
    }

    #[test]
    fn fake_same_uid_response_sender_pid_is_rejected() {
        let credentials = PeerCredentials {
            pid: APP_MAIN_PID,
            uid: unsafe { libc::geteuid() },
        };
        let expected_supervisor_pid = APP_MAIN_PID + 1;
        assert!(verify_sender_identity(
            credentials,
            unsafe { libc::geteuid() },
            expected_supervisor_pid,
        )
        .is_err());
    }

    #[test]
    fn activated_listener_rejects_missing_systemd_contract() {
        std::env::remove_var("LISTEN_PID");
        std::env::remove_var("LISTEN_FDS");
        assert!(activated_listener().is_err());
    }

    #[test]
    fn cli_connect_retries_share_one_total_deadline() {
        let started = Instant::now();
        let deadline = started + IO_TIMEOUT;
        let clock = Cell::new(started);
        let attempts = Cell::new(0_usize);
        let sleeps = Cell::new(0_usize);
        let result = retry_client_connect_until_with::<(), _, _, _>(
            deadline,
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "initial"),
            |received_deadline| {
                assert_eq!(received_deadline, deadline);
                attempts.set(attempts.get() + 1);
                clock.set(clock.get() + Duration::from_secs(3));
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "injected slow connect",
                ))
            },
            || clock.get(),
            |duration| {
                sleeps.set(sleeps.get() + 1);
                clock.set(clock.get() + duration);
            },
        );

        assert!(result.is_err());
        assert_eq!(
            attempts.get(),
            3,
            "deadline must stop retries before 30 attempts"
        );
        assert_eq!(sleeps.get(), 2);
        assert!(clock.get() >= deadline);
    }

    #[test]
    fn cli_exhausted_deadline_has_no_long_tail_connect_or_sleep() {
        let started = Instant::now();
        let deadline = started + IO_TIMEOUT;
        let clock = Cell::new(started);
        let attempts = Cell::new(0_usize);
        let sleeps = Cell::new(0_usize);
        let result = retry_client_connect_until_with::<(), _, _, _>(
            deadline,
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "initial"),
            |_received_deadline| {
                attempts.set(attempts.get() + 1);
                clock.set(deadline);
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "injected connect consumed the budget",
                ))
            },
            || clock.get(),
            |_duration| sleeps.set(sleeps.get() + 1),
        );

        assert!(result.is_err());
        assert_eq!(attempts.get(), 1);
        assert_eq!(sleeps.get(), 0);
    }
}
