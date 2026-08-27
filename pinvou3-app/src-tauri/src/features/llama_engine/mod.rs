//! 本地多模态引擎：按需下载 llama.cpp 引擎与 GGUF 视觉模型，spawn llama-server
//! 提供 OpenAI 兼容端点，供底座 `image_analyze` 工具向纯文本 LLM 描述图像。
//!
//! 目录布局（惯例仿 voice_asr.rs）：
//!   `~/.pinvou3/llama-engine/bin/`    llama-server(.exe) + 共享库 + engine-meta.json
//!   `~/.pinvou3/llama-engine/models/`  视觉模型 gguf + mmproj gguf
//!   `~/.pinvou3/llama-engine/tmp/`     下载/解压中间目录
//!
//! 一期只做视觉（[`Modality::Vision`]）；modality 抽象为音频（ASR/Omni）预留：
//! 后续引擎/模型/端点按 modality 分区扩展，`image_analyze` 只消费 vision 端点。
//!
//! 接线点：`features/assistant/platform/bridge.rs` 构建 EngineConfig 时调用
//! [`vision_endpoint`]，引擎运行中则把 `image_analyze` 的 base_url 指到本地
//! `http://127.0.0.1:{port}/v1`（会话 spawn 时快照，引擎启停后下次会话生效）。

use std::path::PathBuf;

use serde::Serialize;

pub(crate) mod download;
pub(crate) mod platform;
pub(crate) mod server;

// ---------------- 目录 ----------------

pub(crate) fn llama_engine_dir() -> PathBuf {
    crate::platform::paths::pinvou3_home().join("llama-engine")
}

pub(crate) fn bin_dir() -> PathBuf {
    llama_engine_dir().join("bin")
}

pub(crate) fn models_dir() -> PathBuf {
    llama_engine_dir().join("models")
}

pub(crate) fn tmp_dir() -> PathBuf {
    llama_engine_dir().join("tmp")
}

// ---------------- modality 抽象 ----------------

/// 引擎可服务的模态。一期只实现视觉；音频（ASR/Omni）后续在同一引擎
/// 管理框架内扩展（引擎/模型/端点按 modality 分区）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Modality {
    Vision,
}

impl Modality {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Modality::Vision => "vision",
        }
    }
}

// ---------------- 设备 ----------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum EngineDevice {
    Cpu,
    Gpu,
}

impl EngineDevice {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "cpu" => Ok(EngineDevice::Cpu),
            "gpu" => Ok(EngineDevice::Gpu),
            other => Err(format!("未知设备: {other}（可选 cpu / gpu）")),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            EngineDevice::Cpu => "cpu",
            EngineDevice::Gpu => "gpu",
        }
    }
}

/// 设备自动选择：平台 GPU 分级一次检测后进程内缓存（硬件不会热插拔式变化）。
/// 强核显白名单档按 GPU 处理——首版微基准终裁（`-ngl 0` vs `-ngl 99` 小图
/// 探测）尚未实现，先按白名单定档（TODO: 引擎安装完成后跑一次探测并缓存
/// 到 prefs，谁快选谁）。
pub(crate) fn auto_detect_device() -> EngineDevice {
    use std::sync::OnceLock;
    static CACHE: OnceLock<EngineDevice> = OnceLock::new();
    *CACHE.get_or_init(|| match crate::platform::os::gpu_class() {
        crate::platform::os::GpuClass::Dedicated | crate::platform::os::GpuClass::StrongIgpu => {
            EngineDevice::Gpu
        }
        crate::platform::os::GpuClass::Integrated | crate::platform::os::GpuClass::None => {
            EngineDevice::Cpu
        }
    })
}

/// 按设备分级推荐模型档：独显 → 4B 档；CPU/核显 → 默认 2B Q4_K_M 档。
/// 设置页据此给对应模型打「推荐」标。
pub(crate) fn recommended_model_id() -> &'static str {
    match crate::platform::os::gpu_class() {
        crate::platform::os::GpuClass::Dedicated => download::MODEL_4B_Q4_K_M.id,
        _ => download::MODEL_Q4_K_M.id,
    }
}

// ---------------- 状态 ----------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LlamaEngineStatus {
    pub engine_installed: bool,
    pub engine_tag: Option<String>,
    pub models: Vec<ModelInstallStatus>,
    /// 引擎进程相位：idle | starting | running | stopped（下载态由
    /// `downloading` / `downloading_item` 独立表达，不复读到本字段）。
    pub phase: &'static str,
    pub port: Option<u16>,
    pub pid: Option<u32>,
    pub device: Option<EngineDevice>,
    pub active_model: Option<String>,
    pub downloading: bool,
    /// engine | model | mmproj
    pub downloading_item: Option<&'static str>,
    /// 最近一次失败/停止原因（含 stderr 尾行）。
    pub error: Option<String>,
    pub stderr_tail: Vec<String>,
    /// 设备自动检测结果（「自动」档实际生效的设备；用户显式 cpu/gpu 时
    /// 仍回填检测值，仅供设置页展示推荐）。
    pub detected_device: EngineDevice,
    /// 按设备分级推荐的模型档 id（设置页「推荐」标：仅独显推荐 4B；
    /// 其余设备与默认档相同，设置页据此只显示「默认」不显示「推荐」）。
    pub recommended_model: &'static str,
    /// 新装默认档 id（设置页「默认」标；恒为 2B Q4_K_M，不随配置漂移）。
    pub default_model: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelInstallStatus {
    pub id: &'static str,
    pub display_name: &'static str,
    pub size_bytes: u64,
    pub installed: bool,
}

// ---------------- tauri 命令的域函数 ----------------
// （app/commands/llama_engine.rs 经 prelude 宏透传调用，保持同名同签名）

pub(crate) fn llama_engine_status() -> LlamaEngineStatus {
    let downloading = download::is_downloading();
    let snapshot = server::runtime_snapshot();
    LlamaEngineStatus {
        engine_installed: download::engine_installed(),
        engine_tag: download::engine_tag(),
        models: download::model_specs()
            .iter()
            .map(|spec| ModelInstallStatus {
                id: spec.id,
                display_name: spec.display_name,
                size_bytes: spec.size_bytes,
                installed: download::model_files_verified(spec),
            })
            .collect(),
        // phase 只表达引擎进程相位,不复读下载态——下载有自己的
        // downloading/downloading_item 字段。此前下载中 phase 被覆盖成
        // "downloading",并发下载时运行中的引擎对外隐身:前端发送门
        // (startEngineAndWait 轮询 phase === 'running')和 capability 的
        // local_engine_state 永远等不到 running,自动唤起必须等下载结束。
        phase: snapshot.phase,
        port: snapshot.port,
        pid: snapshot.pid,
        device: snapshot.device,
        active_model: snapshot.active_model,
        downloading,
        downloading_item: download::downloading_item(),
        error: snapshot.last_error,
        stderr_tail: snapshot.stderr_tail,
        detected_device: auto_detect_device(),
        recommended_model: recommended_model_id(),
        default_model: download::default_model().id,
    }
}

pub(crate) async fn llama_engine_install_engine(app: tauri::AppHandle) -> Result<(), String> {
    download::install_engine(&app).await
}

pub(crate) async fn llama_engine_install_model(
    app: tauri::AppHandle,
    model: String,
) -> Result<(), String> {
    download::install_model(&app, &model).await
}

pub(crate) fn llama_engine_cancel_download() {
    download::cancel_download();
}

pub(crate) async fn llama_engine_start(
    app: tauri::AppHandle,
    model: String,
    device: String,
) -> Result<(), String> {
    let device = EngineDevice::parse(&device)?;
    // 结构化错误在 tauri 命令边界转回文案（AlreadyRunning 的用户可见
    // 文案保持"引擎已在运行或启动中"不变）。
    server::start(&app, &model, device)
        .await
        .map_err(server::StartError::into_message)
}

pub(crate) fn llama_engine_stop() {
    server::stop();
}

/// 删除模型：delete_model_files 内部先停运行/启动中的引擎并等收口
/// （Windows 下模型文件被进程占用无法删除，超时返回明确错误），再删权重
/// 文件；安装状态由下次 `llama_engine_status` 按文件存在性重算。
pub(crate) fn llama_engine_delete_model(model: String) -> Result<(), String> {
    let spec = download::model_spec(&model)?;
    download::delete_model_files(spec).map(|_| ())
}

/// 卸载引擎：delete_engine_files 内部先停运行/启动中的引擎并等收口，再删
/// 引擎二进制与共享库；模型文件保留，重装引擎后即可直接使用。
pub(crate) fn llama_engine_delete_engine() -> Result<(), String> {
    download::delete_engine_files().map(|_| ())
}

// ---------------- bridge 接线点 ----------------

/// 引擎运行中返回本地 OpenAI 兼容端点，否则 None。
/// bridge.rs 在会话 spawn 时调用（EngineConfig 快照语义）。
pub(crate) fn vision_endpoint() -> Option<String> {
    server::running_endpoint()
}

/// 本地引擎四档状态（capability 查询与前端发送门共用口径）。
/// - `unused`：当前模型未选本地识图引擎（且全局兜底未开），前端不介入
/// - `running`：引擎运行中，直接使用
/// - `not_running`：已安装但未运行，按自动启动策略处理
/// - `not_installed`：引擎或默认模型未就绪，引导安装
pub(crate) fn local_engine_state(
    prefer_local: bool,
    engine_installed: bool,
    model_ready: bool,
    phase: &str,
) -> &'static str {
    if !prefer_local {
        return "unused";
    }
    if phase == "running" {
        return "running";
    }
    if !engine_installed || !model_ready {
        return "not_installed";
    }
    "not_running"
}

/// 解析自动启动/发送兜底用的默认引擎模型与设备：prefs 持久化的
/// 设置页选择（未知模型 id 回落 `download::default_model()`；
/// 设备 None/"auto"/非法值 → 平台自动检测，显式 cpu/gpu 优先）。
pub(crate) fn resolve_default_engine_plan(
    prefs: &crate::platform::prefs::AdvancedPrefs,
) -> (String, EngineDevice) {
    let model_id = prefs
        .llama_engine_default_model
        .as_deref()
        .filter(|id| download::model_spec(id).is_ok())
        .map(str::to_owned)
        .unwrap_or_else(|| download::default_model().id.to_owned());
    let device = match prefs.llama_engine_default_device.as_deref() {
        Some(value) => EngineDevice::parse(value).unwrap_or_else(|_| auto_detect_device()),
        None => auto_detect_device(),
    };
    (model_id, device)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::prefs::AdvancedPrefs;

    #[test]
    fn local_engine_state_four_levels() {
        // 未选本地识图 → unused（前端不介入）
        assert_eq!(local_engine_state(false, true, true, "idle"), "unused");
        assert_eq!(local_engine_state(false, false, false, "stopped"), "unused");
        // 引擎运行 → running
        assert_eq!(local_engine_state(true, true, true, "running"), "running");
        // 已安装未运行 → not_running
        assert_eq!(local_engine_state(true, true, true, "idle"), "not_running");
        assert_eq!(
            local_engine_state(true, true, true, "stopped"),
            "not_running"
        );
        // 引擎或模型缺失 → not_installed
        assert_eq!(
            local_engine_state(true, false, true, "idle"),
            "not_installed"
        );
        assert_eq!(
            local_engine_state(true, true, false, "idle"),
            "not_installed"
        );
    }

    #[test]
    fn resolve_default_engine_plan_falls_back() {
        // 未配置 → 默认模型 + 自动检测设备（cpu/gpu 依平台检测结果，两值皆合法）
        let (model, device) = resolve_default_engine_plan(&AdvancedPrefs::default());
        assert_eq!(model, download::default_model().id);
        assert!(matches!(device, EngineDevice::Cpu | EngineDevice::Gpu));

        // 非法模型 id → 回落默认；非法设备 → 自动检测（不报错、不硬编码 gpu）
        let prefs = AdvancedPrefs {
            llama_engine_default_model: Some("no-such-model".into()),
            llama_engine_default_device: Some("tpu".into()),
            ..Default::default()
        };
        let (model, device) = resolve_default_engine_plan(&prefs);
        assert_eq!(model, download::default_model().id);
        assert!(matches!(device, EngineDevice::Cpu | EngineDevice::Gpu));

        // 合法配置透传（显式 cpu/gpu 优先于自动检测）
        let prefs = AdvancedPrefs {
            llama_engine_default_model: Some("qwen3vl-2b-q4km".into()),
            llama_engine_default_device: Some("cpu".into()),
            ..Default::default()
        };
        let (model, device) = resolve_default_engine_plan(&prefs);
        assert_eq!(model, "qwen3vl-2b-q4km");
        assert_eq!(device, EngineDevice::Cpu);
    }
}
