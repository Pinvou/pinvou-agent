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
}

// ---------------- 状态 ----------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LlamaEngineStatus {
    pub engine_installed: bool,
    pub engine_tag: Option<String>,
    pub models: Vec<ModelInstallStatus>,
    /// idle | downloading | starting | running | stopped
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
        phase: if downloading {
            "downloading"
        } else {
            snapshot.phase
        },
        port: snapshot.port,
        pid: snapshot.pid,
        device: snapshot.device,
        active_model: snapshot.active_model,
        downloading,
        downloading_item: download::downloading_item(),
        error: snapshot.last_error,
        stderr_tail: snapshot.stderr_tail,
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
    server::start(&app, &model, device).await
}

pub(crate) fn llama_engine_stop() {
    server::stop();
}

// ---------------- bridge 接线点 ----------------

/// 引擎运行中返回本地 OpenAI 兼容端点，否则 None。
/// bridge.rs 在会话 spawn 时调用（EngineConfig 快照语义）。
pub(crate) fn vision_endpoint() -> Option<String> {
    server::running_endpoint()
}
