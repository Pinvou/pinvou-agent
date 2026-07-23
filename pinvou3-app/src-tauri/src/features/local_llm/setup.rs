//! MegaCube(GB10) 本地大模型一键引导。
//!
//! 整机出厂会在 `/opt/.h3c/packages/vllm/` 预装推理引擎容器压缩包 + `models/` 下
//! 模型压缩包 + `startup.sh` 可执行启动脚本,但开箱后并未拉起。本模块负责:
//!  1. `detect_local_vllm_setup` —— 首屏区分 ready / starting / stopped / failed，
//!     只在真正未启动或启动失败时弹框，避免开机加载模型期间误提示。
//!  2. `bootstrap_local_vllm` —— 用户点"启用"后,把 startup.sh 装成 systemd 服务拉起引擎+
//!     开机自启、探到就绪后把本地 vLLM 写进模型配置设为默认。
//!
//! 设计要点:
//! - 触发不以机型(DMI product_name)为门槛:GB10 底层是 NVIDIA DGX Spark,H3C 不同批次固件
//!   DMI 串写法不一(实测 `SCI-CUBE` / `NVIDIA_DGX_Spark`),易漏判。改以 `/opt/.h3c/packages/vllm`
//!   这套预装为真凭据——出厂镜像独有,普通机不会有。product_name 仅回填 `is_megacube` 供诊断。
//! - 提权只发生在引导阶段、且**一次 pkexec** 办完(给 startup.sh 加可执行位+装单元+reload+enable+restart),
//!   复用 `super_permission::run_pkexec` 的 pkexec 范式;**不走** sudoers 开关,二者无关。
//! - startup.sh 以 root 跑,内容来自出厂预装目录——属可信来源。该脚本须可重复
//!   执行(docker load 幂等 / docker run 固定 --name 前先 docker rm -f),是预装侧编写约定。

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::Emitter;

use crate::platform::prefs::{ModelPreset, SavedModel, UserPrefs};
use crate::platform::credential_store::CredentialState;
use crate::features::monitor::{self, VllmStatus};

/// 引导阶段事件名(前端 listen 更新步骤指示 + 计时)。
const PHASE_EVENT: &str = "vllm-setup:phase";

const H3C_VLLM_DIR: &str = "/opt/.h3c/packages/vllm";
const STARTUP_SH: &str = "/opt/.h3c/packages/vllm/startup.sh";
const MODELS_DIR: &str = "/opt/.h3c/packages/vllm/models";
const SERVICE_NAME: &str = "pinvou3-vllm.service";
const CONTAINER_NAME: &str = "pinvou3-vllm-sim";
const PRODUCT_NAME_PATH: &str = "/sys/class/dmi/id/product_name";
const VLLM_PORTS: [u16; 3] = [8000, 8001, 8002];
const RUNTIME_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
/// 首启通常 5-6 分钟，bootstrap 的用户等待上限为 10 分钟；额外留 2 分钟余量后，
/// API 仍离线的 running/activating 才视为卡死，避免永远停在 starting。
const STARTING_STALE_AFTER: Duration = Duration::from_secs(12 * 60);
/// 探到在线但 /v1/models 没回模型名时的兜底(正常 vLLM 一定有 served name,极少用到)。
const FALLBACK_MODEL: &str = "qwen36_35b_256k";
/// 引导成功后写入的固定模型 id(再次引导按 id upsert,幂等)。
const BOOTSTRAP_MODEL_ID: &str = "local-vllm-megacube";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LocalVllmEngineState {
    Ready,
    Starting,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalVllmSetupStatus {
    /// eligible:预装包齐全 + 引擎确实停止/失败 + 没成功跑过引导。
    /// 机型仅作诊断,不进门槛。
    pub eligible: bool,
    pub is_megacube: bool,
    pub has_packages: bool,
    pub vllm_online: bool,
    pub engine_state: LocalVllmEngineState,
    pub already_bootstrapped: bool,
    /// 与运行态无关的产品 Gate：预装齐全、未成功引导且用户未选择不再提醒。
    /// 前端仅在 starting 超过硬截止后用它决定是否恢复启用入口。
    pub may_offer_setup: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BootstrapResult {
    pub base_url: String,
    pub model: String,
}

/// 归一化 product_name 后判型:去空白、转大写、剔除非字母数字(吸收 `-`/`_`/空格差异)。
/// GB10 底层是 NVIDIA DGX Spark 参考平台,H3C 不同批次固件 DMI 串写法不一:实测有
/// `SCI-CUBE`(113 真机)与 `NVIDIA_DGX_Spark`(cube-bb89 真机)两种,故认这一组已知串。
/// ⚠️ 仅供 `is_megacube` 诊断展示——eligible **不再**以机型为门槛(见 detect_local_vllm_setup),
/// 故即便以后冒出第三种串、这里返 false,也不影响引导框触发。
fn product_is_megacube(raw: &str) -> bool {
    let norm: String = raw
        .trim()
        .to_ascii_uppercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    matches!(norm.as_str(), "SCICUBE" | "NVIDIADGXSPARK")
}

/// 读 `/sys/class/dmi/id/product_name` 判机型。
fn is_megacube() -> bool {
    std::fs::read_to_string(PRODUCT_NAME_PATH)
        .map(|s| product_is_megacube(&s))
        .unwrap_or(false)
}

/// 预装三件齐全:startup.sh 存在 + models/ 非空 + vllm/ 下有引擎压缩包。
/// startup.sh 是预装侧提供的可执行启动脚本,bootstrap 直接拿它当 systemd ExecStart。
fn has_packages() -> bool {
    Path::new(STARTUP_SH).is_file() && dir_non_empty(MODELS_DIR) && has_engine_archive(H3C_VLLM_DIR)
}

fn dir_non_empty(dir: &str) -> bool {
    std::fs::read_dir(dir)
        .map(|mut rd| rd.next().is_some())
        .unwrap_or(false)
}

/// dir 下是否有常见压缩包(引擎容器包)。`.tar.gz` 以 `.gz` 结尾故已覆盖。
fn has_engine_archive(dir: &str) -> bool {
    const EXTS: [&str; 5] = ["tar", "gz", "tgz", "zip", "7z"];
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten().any(|e| {
                let p = e.path();
                p.is_file()
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| {
                            let lower = n.to_ascii_lowercase();
                            EXTS.iter().any(|ext| lower.ends_with(&format!(".{ext}")))
                        })
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// 探本机 vLLM 候选端口,返回首个在线实例 (base_url, served_model)。
/// 在线 = `/v1/models` 有响应(status 非 Offline,Mismatch 也算在线)。
async fn probe_online() -> Option<(String, Option<String>)> {
    for port in VLLM_PORTS {
        let base = format!("http://127.0.0.1:{port}/v1");
        if let Some(snap) = monitor::vllm_snapshot(&base, None).await {
            if !matches!(snap.status, VllmStatus::Offline) {
                return Some((snap.upstream, snap.model));
            }
        }
    }
    None
}

#[derive(Debug, Default)]
struct RuntimeSnapshot {
    container_status: Option<String>,
    /// true 表示 Docker 查询成功；此时 status=None 才能可靠解释为“容器不存在”。
    container_observed: bool,
    container_age_secs: Option<u64>,
    service_load: Option<String>,
    service_active: Option<String>,
    service_sub: Option<String>,
    service_result: Option<String>,
    service_exec_status: Option<i32>,
    service_age_secs: Option<u64>,
}

fn command_output_with_timeout(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<Output, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program} 启动失败: {e}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|e| format!("{program} 读取输出失败: {e}"));
            }
            Ok(None) if started.elapsed() < timeout => {
                thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{program} 执行超过 {}ms", timeout.as_millis()));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{program} 等待失败: {e}"));
            }
        }
    }
}

fn output_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn age_from_rfc3339(raw: &str) -> Option<u64> {
    let started = chrono::DateTime::parse_from_rfc3339(raw).ok()?;
    chrono::Utc::now()
        .signed_duration_since(started.with_timezone(&chrono::Utc))
        .to_std()
        .ok()
        .map(|age| age.as_secs())
}

fn monotonic_age_secs(started_micros: u64) -> Option<u64> {
    if started_micros == 0 {
        return None;
    }
    let raw = std::fs::read_to_string("/proc/uptime").ok()?;
    let uptime_secs: f64 = raw.split_whitespace().next()?.parse().ok()?;
    let now_micros = (uptime_secs * 1_000_000.0) as u64;
    (now_micros >= started_micros).then(|| (now_micros - started_micros) / 1_000_000)
}

fn runtime_snapshot() -> RuntimeSnapshot {
    let mut snapshot = RuntimeSnapshot::default();

    // 先用 docker ps 区分“查询成功但容器不存在”和“无 Docker 权限/daemon 不可用”。
    // 直接 inspect 的所有非零退出都折叠成 None，会把权限错误误判为容器退出。
    let name_filter = format!("name=^/{CONTAINER_NAME}$");
    if let Ok(list) = command_output_with_timeout(
        "docker",
        &["ps", "-a", "--filter", &name_filter, "--format={{.ID}}"],
        RUNTIME_COMMAND_TIMEOUT,
    ) {
        if list.status.success() {
            snapshot.container_observed = true;
            if !output_text(&list).is_empty() {
                if let Ok(inspect) = command_output_with_timeout(
                    "docker",
                    &[
                        "inspect",
                        "--format={{.State.Status}}|{{.State.StartedAt}}",
                        CONTAINER_NAME,
                    ],
                    RUNTIME_COMMAND_TIMEOUT,
                ) {
                    if inspect.status.success() {
                        let raw = output_text(&inspect);
                        let mut parts = raw.splitn(2, '|');
                        snapshot.container_status = parts
                            .next()
                            .filter(|value| !value.is_empty())
                            .map(str::to_string);
                        snapshot.container_age_secs = parts.next().and_then(age_from_rfc3339);
                        if snapshot.container_status.is_none() {
                            snapshot.container_observed = false;
                        }
                    } else {
                        snapshot.container_observed = false;
                    }
                } else {
                    snapshot.container_observed = false;
                }
            }
        }
    }

    if let Ok(output) = command_output_with_timeout(
        "systemctl",
        &[
            "show",
            SERVICE_NAME,
            "--property=LoadState",
            "--property=ActiveState",
            "--property=SubState",
            "--property=Result",
            "--property=ExecMainStatus",
            "--property=ExecMainStartTimestampMonotonic",
        ],
        RUNTIME_COMMAND_TIMEOUT,
    ) {
        if !output.status.success() {
            return snapshot;
        }
        let raw = output_text(&output);
        let mut service_started_micros = None;
        for line in raw.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = (!value.is_empty()).then(|| value.to_string());
            match key {
                "LoadState" => snapshot.service_load = value,
                "ActiveState" => snapshot.service_active = value,
                "SubState" => snapshot.service_sub = value,
                "Result" => snapshot.service_result = value,
                "ExecMainStatus" => {
                    snapshot.service_exec_status = value.and_then(|raw| raw.parse().ok())
                }
                "ExecMainStartTimestampMonotonic" => {
                    service_started_micros = value.and_then(|raw| raw.parse().ok())
                }
                _ => {}
            }
        }
        snapshot.service_age_secs = service_started_micros.and_then(monotonic_age_secs);
    }
    snapshot
}

fn starting_is_stale(snapshot: &RuntimeSnapshot) -> bool {
    [snapshot.container_age_secs, snapshot.service_age_secs]
        .into_iter()
        .flatten()
        .any(|age| age >= STARTING_STALE_AFTER.as_secs())
}

fn classify_runtime_state(snapshot: &RuntimeSnapshot) -> LocalVllmEngineState {
    match snapshot.container_status.as_deref() {
        Some("exited" | "dead" | "removing" | "paused") => {
            return LocalVllmEngineState::Failed;
        }
        _ => {}
    }

    if snapshot.service_active.as_deref() == Some("failed")
        || snapshot
            .service_result
            .as_deref()
            .is_some_and(|result| !matches!(result, "success" | "done"))
        || snapshot
            .service_exec_status
            .is_some_and(|status| status != 0)
    {
        return LocalVllmEngineState::Failed;
    }

    if matches!(
        snapshot.container_status.as_deref(),
        Some("running" | "restarting" | "created")
    ) {
        return if starting_is_stale(snapshot) {
            LocalVllmEngineState::Failed
        } else {
            LocalVllmEngineState::Starting
        };
    }

    match snapshot.service_active.as_deref() {
        Some("activating") => {
            if starting_is_stale(snapshot) {
                LocalVllmEngineState::Failed
            } else {
                LocalVllmEngineState::Starting
            }
        }
        // oneshot + RemainAfterExit 正常启动后就是 active(exited)。Docker 可观测且容器
        // 不存在时才能立即判失败；Docker 权限不足时先按启动中处理，再由年龄上限收敛。
        Some("active") if snapshot.service_sub.as_deref() == Some("exited") => {
            if (snapshot.container_observed && snapshot.container_status.is_none())
                || starting_is_stale(snapshot)
            {
                LocalVllmEngineState::Failed
            } else {
                LocalVllmEngineState::Starting
            }
        }
        Some("active") => {
            if starting_is_stale(snapshot) {
                LocalVllmEngineState::Failed
            } else {
                LocalVllmEngineState::Starting
            }
        }
        _ => LocalVllmEngineState::Stopped,
    }
}

async fn detect_engine_state() -> LocalVllmEngineState {
    if probe_online().await.is_some() {
        return LocalVllmEngineState::Ready;
    }
    tokio::task::spawn_blocking(|| classify_runtime_state(&runtime_snapshot()))
        .await
        .unwrap_or(LocalVllmEngineState::Stopped)
}

fn systemd_unit() -> String {
    format!(
        "[Unit]\n\
         Description=pinvou3 local vLLM inference engine (MegaCube)\n\
         After=network-online.target docker.service\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         RemainAfterExit=yes\n\
         WorkingDirectory={H3C_VLLM_DIR}\n\
         ExecStart={STARTUP_SH}\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

fn privileged_start_script() -> &'static str {
    "set -e\n\
     chmod +x /opt/.h3c/packages/vllm/startup.sh\n\
     install -m 0644 -o root -g root \"$1\" /etc/systemd/system/pinvou3-vllm.service\n\
     systemctl daemon-reload\n\
     systemctl enable pinvou3-vllm.service\n\
     systemctl restart pinvou3-vllm.service"
}

/// 跑 pkexec 并捕获输出(失败时把 stderr 带回前端)。
/// 退出码约定同 super_permission:126 用户取消 / 127 未授权或 pkexec 不可用。
fn run_pkexec_capture(args: &[&str]) -> Result<(), String> {
    let out = Command::new("pkexec")
        .args(args)
        .output()
        .map_err(|e| format!("pkexec 启动失败: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let code = out.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(match code {
        126 => "用户取消了授权".to_string(),
        127 => "系统授权失败，或当前会话无法显示授权框 (pkexec exit 127)".to_string(),
        _ => format!("启动脚本执行失败 (exit {code}): {}", stderr.trim()),
    })
}

/// 首屏检测:预装齐全 + 引擎 stopped/failed + 没成功引导过 + 没婉拒过
/// → eligible(前端据此弹引导框)。starting 时只后台等待，不误弹框。
///
/// 机型不再作硬门槛——`/opt/.h3c/packages/vllm` 这套预装是 H3C 出厂镜像独有的强凭据,比 DMI
/// product_name 可靠(同型号不同批次 DMI 串都不一样)。`is_megacube` 仍回填供诊断。
/// 端口探测只在预装齐全、没跑过引导、没婉拒时才做——普通机(无预装)直接短路,不白等 3 个端口各 3s 超时。
/// 注:`declined` 只压住**开机自动弹框**;设置页「检测本机 vLLM」仍据 `has_packages` 提供手动启用入口。
pub async fn detect_local_vllm_setup() -> Result<LocalVllmSetupStatus, String> {
    let is_megacube = is_megacube();
    let has_packages = has_packages();
    let prefs = UserPrefs::load();
    let already_bootstrapped = prefs.advanced.local_vllm_bootstrapped;
    let declined = prefs.advanced.local_vllm_setup_declined;

    let engine_state = if has_packages {
        detect_engine_state().await
    } else {
        LocalVllmEngineState::Stopped
    };
    let vllm_online = engine_state == LocalVllmEngineState::Ready;
    let can_offer_start = matches!(
        engine_state,
        LocalVllmEngineState::Stopped | LocalVllmEngineState::Failed
    );
    let may_offer_setup = has_packages && !already_bootstrapped && !declined;
    let eligible = may_offer_setup && can_offer_start;

    Ok(LocalVllmSetupStatus {
        eligible,
        is_megacube,
        has_packages,
        vllm_online,
        engine_state,
        already_bootstrapped,
        may_offer_setup,
    })
}

/// 用户在引导框点「不再提醒 → 确认」:持久置 declined,开机引导框不再自动弹。
/// 不影响设置页「检测本机 vLLM」的手动启用入口(那条按 has_packages 提供,与 declined 无关)。
pub fn decline_local_vllm_setup() -> Result<(), String> {
    let mut prefs = UserPrefs::load();
    prefs.advanced.local_vllm_setup_declined = true;
    prefs.save().map_err(|e| format!("保存失败: {e:?}"))?;
    Ok(())
}

/// 用户点"启用"后执行:弹 pkexec 系统授权框，把预装 startup.sh 装成
/// systemd 服务并 restart（已是 active(exited) 也会真正重跑）
/// → 轮询就绪 → 写模型配置设默认 + 置 bootstrapped 标记。
///
/// 全程发 `vllm-setup:phase` 事件(authorizing → waiting{attempt} → ready),
/// 前端据此显示步骤指示;计时由前端自跑(pkexec 阻塞期间也能看到秒数在涨)。
pub async fn bootstrap_local_vllm(app: tauri::AppHandle) -> Result<BootstrapResult, String> {
    // 1. 重校验(detect 与点击之间状态可能变)。机型不卡,只认预装齐全。
    if !has_packages() {
        return Err("未找到完整的本地大模型预装包(startup.sh / 引擎包 / models)。".to_string());
    }

    // 2. 点击与执行之间再校验一次。若引擎已被开机服务拉起，直接接管等待，
    //    不用授权后的 restart 打断正在加载的模型。只有确认 stopped/failed 才弹系统授权框。
    let already_online = probe_online().await;
    if already_online.is_none() {
        let runtime_state =
            tokio::task::spawn_blocking(|| classify_runtime_state(&runtime_snapshot()))
                .await
                .unwrap_or(LocalVllmEngineState::Stopped);
        if matches!(
            runtime_state,
            LocalVllmEngineState::Stopped | LocalVllmEngineState::Failed
        ) {
            // 写 systemd 单元到临时文件(用户态),交 pkexec 用 install 提权落到系统路径。
            let tmp_unit = std::env::temp_dir().join("pinvou3-vllm.service");
            std::fs::write(&tmp_unit, systemd_unit())
                .map_err(|e| format!("写服务单元失败: {e}"))?;

            // 事先发 authorizing，前端先显示「等待系统授权」，紧接着由 pkexec 弹系统框。
            // enable 和 restart 分开：restart 可确保 oneshot 处于 active(exited) 时也重跑。
            let _ = app.emit(
                PHASE_EVENT,
                serde_json::json!({ "phase": "authorizing", "attempt": 0 }),
            );
            let tu = tmp_unit.to_string_lossy().to_string();
            let auth_result =
                run_pkexec_capture(&["bash", "-c", privileged_start_script(), "pinvou3", &tu]);
            let _ = std::fs::remove_file(&tmp_unit);
            auth_result?;
        }
    }

    // 4. 健康轮询,每轮发 waiting{attempt} 事件。
    //    超时给 10 分钟:113 真机实测 GB10 NVFP4 首启 ~5-6min(权重 117s + torch.compile 33s
    //    + flashinfer autotune + cudagraph capture),5 分钟会刚好误杀;二次启动有
    //    ~/.cache/vllm 编译/autotune 缓存会快很多,故 600s 仅首启用得满。
    let _ = app.emit(
        PHASE_EVENT,
        serde_json::json!({ "phase": "waiting", "attempt": 0 }),
    );
    let deadline = Instant::now() + Duration::from_secs(600);
    let mut attempt: u32 = 0;
    let (base_url, served) = loop {
        if let Some(found) = already_online.clone().or(probe_online().await) {
            break found;
        }
        let runtime_state =
            tokio::task::spawn_blocking(|| classify_runtime_state(&runtime_snapshot()))
                .await
                .unwrap_or(LocalVllmEngineState::Stopped);
        if runtime_state == LocalVllmEngineState::Failed {
            return Err(format!(
                "推理引擎启动后已退出。可重试，或终端查看 `docker logs {CONTAINER_NAME}`。"
            ));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "推理引擎已拉起,但 10 分钟内未就绪。可稍后在设置里手动探测,或终端查看 `systemctl status {SERVICE_NAME}`。"
            ));
        }
        attempt += 1;
        let _ = app.emit(
            PHASE_EVENT,
            serde_json::json!({ "phase": "waiting", "attempt": attempt }),
        );
        tokio::time::sleep(Duration::from_secs(3)).await;
    };
    let _ = app.emit(
        PHASE_EVENT,
        serde_json::json!({ "phase": "ready", "attempt": attempt }),
    );
    let model = served.unwrap_or_else(|| FALLBACK_MODEL.to_string());

    // 5. 写模型配置 + 设默认 + 置标记。
    let mut prefs = UserPrefs::load();
    prefs.upsert_model(SavedModel {
        id: BOOTSTRAP_MODEL_ID.to_string(),
        name: "本地大模型".to_string(),
        preset: ModelPreset::LocalVllm,
        context_window_tokens: None,
        max_output_tokens: None,
        model: model.clone(),
        base_url: base_url.clone(),
        api_key: String::new(),
        // 本地 vLLM 无 key,凭证字段同 prefs.rs 内置模型的无 key 缺省
        credential_ref: None,
        credential_state: CredentialState::Missing,
        has_secret: false,
        credential_action: None,
    });
    prefs.advanced.active_model_id = Some(BOOTSTRAP_MODEL_ID.to_string());
    prefs.advanced.local_vllm_bootstrapped = true;
    prefs
        .save()
        .map_err(|e| format!("保存模型配置失败: {e:?}"))?;

    Ok(BootstrapResult { base_url, model })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_is_megacube_accepts_separator_variants() {
        assert!(product_is_megacube("SCI-CUBE")); // 113 真机实测(连字符)
        assert!(product_is_megacube("SCI_CUBE")); // 下划线写法
        assert!(product_is_megacube("  SCI-CUBE\n")); // 带空白/换行
        assert!(product_is_megacube("sci cube")); // 大小写/空格
        assert!(product_is_megacube("NVIDIA_DGX_Spark")); // cube-bb89 真机实测(GB10=DGX Spark)
        assert!(product_is_megacube("NVIDIA DGX Spark")); // 空格写法
        assert!(!product_is_megacube("System Product Name"));
        assert!(!product_is_megacube("SCI-CUBE-PRO")); // 非精确同名不误判
        assert!(!product_is_megacube(""));
    }

    #[test]
    fn systemd_unit_has_required_fields() {
        let u = systemd_unit();
        assert!(u.contains("Type=oneshot"));
        assert!(u.contains("RemainAfterExit=yes"));
        assert!(u.contains("ExecStart=/opt/.h3c/packages/vllm/startup.sh"));
        assert!(u.contains("WantedBy=multi-user.target"));
    }

    #[test]
    fn runtime_state_distinguishes_loading_stopped_and_failed() {
        let running = RuntimeSnapshot {
            container_status: Some("running".into()),
            container_observed: true,
            container_age_secs: Some(60),
            service_active: Some("active".into()),
            service_sub: Some("exited".into()),
            ..RuntimeSnapshot::default()
        };
        assert_eq!(
            classify_runtime_state(&running),
            LocalVllmEngineState::Starting
        );

        let activating = RuntimeSnapshot {
            service_active: Some("activating".into()),
            ..RuntimeSnapshot::default()
        };
        assert_eq!(
            classify_runtime_state(&activating),
            LocalVllmEngineState::Starting
        );

        let stale_running = RuntimeSnapshot {
            container_status: Some("running".into()),
            container_observed: true,
            container_age_secs: Some(STARTING_STALE_AFTER.as_secs()),
            ..RuntimeSnapshot::default()
        };
        assert_eq!(
            classify_runtime_state(&stale_running),
            LocalVllmEngineState::Failed
        );

        let stale_oneshot = RuntimeSnapshot {
            container_status: Some("exited".into()),
            service_active: Some("active".into()),
            service_sub: Some("exited".into()),
            ..RuntimeSnapshot::default()
        };
        assert_eq!(
            classify_runtime_state(&stale_oneshot),
            LocalVllmEngineState::Failed
        );

        assert_eq!(
            classify_runtime_state(&RuntimeSnapshot::default()),
            LocalVllmEngineState::Stopped
        );
    }

    #[test]
    fn runtime_state_does_not_treat_docker_permission_error_as_missing_container() {
        let docker_unobservable = RuntimeSnapshot {
            container_observed: false,
            service_active: Some("active".into()),
            service_sub: Some("exited".into()),
            service_result: Some("success".into()),
            service_exec_status: Some(0),
            service_age_secs: Some(60),
            ..RuntimeSnapshot::default()
        };
        assert_eq!(
            classify_runtime_state(&docker_unobservable),
            LocalVllmEngineState::Starting
        );

        let missing_container = RuntimeSnapshot {
            container_observed: true,
            service_active: Some("active".into()),
            service_sub: Some("exited".into()),
            service_result: Some("success".into()),
            service_exec_status: Some(0),
            service_age_secs: Some(60),
            ..RuntimeSnapshot::default()
        };
        assert_eq!(
            classify_runtime_state(&missing_container),
            LocalVllmEngineState::Failed
        );

        let stale_unobservable = RuntimeSnapshot {
            service_active: Some("active".into()),
            service_sub: Some("exited".into()),
            service_age_secs: Some(STARTING_STALE_AFTER.as_secs()),
            ..RuntimeSnapshot::default()
        };
        assert_eq!(
            classify_runtime_state(&stale_unobservable),
            LocalVllmEngineState::Failed
        );
    }

    #[test]
    fn runtime_commands_have_a_hard_timeout() {
        if !crate::platform::capabilities::is_unix() {
            return;
        }
        let success =
            command_output_with_timeout("sh", &["-c", "printf ready"], Duration::from_millis(500))
                .expect("quick command should complete");
        assert!(success.status.success());
        assert_eq!(output_text(&success), "ready");

        let started = Instant::now();
        let result =
            command_output_with_timeout("sh", &["-c", "sleep 1"], Duration::from_millis(40));
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn privileged_start_always_restarts_existing_oneshot() {
        let script = privileged_start_script();
        assert!(script.contains("systemctl enable pinvou3-vllm.service"));
        assert!(script.contains("systemctl restart pinvou3-vllm.service"));
        assert!(!script.contains("enable --now"));
    }
}
