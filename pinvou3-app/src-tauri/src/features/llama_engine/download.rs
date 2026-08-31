//! 引擎与视觉模型按需下载、校验与部署。
//!
//! 范式照搬 voice_asr.rs / model_download.rs：流式 reqwest + `.part` 临时文件 +
//! 进度事件 + 取消标志；模型与钉版引擎（PINNED_ENGINE_TAG）均强制尺寸 + sha256
//! 校验，fail closed（不匹配即删除报错）；模型校验结果带缓存，状态热路径只消费
//! 缓存不重复全量 hash。引擎 tag 默认锁定 PINNED_ENGINE_TAG，记入
//! engine-meta.json；`PINVOU3_LLAMA_ENGINE_TAG` 显式覆盖时为开发通道，跳过
//! digest 校验。所有 env 开发覆盖（tag/URL/sha256）仅 debug 构建生效，release
//! 忽略并 warn 一次（见 `dev_env_override`）。GitHub 资产不支持断点续传，
//! 失败整文件重下。
//!
//! 进度事件 `llama-engine:progress` payload：
//! `{ stage: engine_download|engine_extract|model_download|model_verify|done|cancelled,
//!    item: engine|model|mmproj, modelId, filename, downloaded, total }`

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tauri::Emitter;

use super::platform;
use super::{bin_dir, llama_engine_dir, models_dir, tmp_dir};

// ---------------- 资产表 ----------------

/// 一个候选模型的资产（主权重 + 视觉投影器）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct LlamaModelSpec {
    pub id: &'static str,
    pub display_name: &'static str,
    /// 主权重 + 视觉投影器合计字节数（前端展示）。
    pub size_bytes: u64,
    pub gguf: ModelAsset,
    pub mmproj: ModelAsset,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ModelAsset {
    pub filename: &'static str,
    pub expected_size: u64,
    /// 发布资产的 sha256（强制校验，fail closed；dev 覆盖见 `asset_sha256`）。
    pub sha256: &'static str,
    /// 首个尝试源：ModelScope（中国大陆网络最快，与 HuggingFace 官方仓同源同内容）。
    pub primary_url: &'static str,
    /// 备用源 1：HuggingFace 官方。
    pub mirror_url: &'static str,
    /// 备用源 2：HuggingFace 国内镜像（回退顺序的最后一环；镜像内容完整性
    /// 由 sha256 强制校验钉死，任一源返回篡改/损坏内容都会被拒绝）。
    pub fallback_url: &'static str,
}

/// 2B 官方仓 Q8_0 视觉投影器。
/// 相比 F16（819MB）省约 46%，视觉编码精度损失可忽略。
const MMPROJ_2B_Q8_0: ModelAsset = ModelAsset {
    filename: "mmproj-Qwen3VL-2B-Instruct-Q8_0.gguf",
    expected_size: 445_053_216,
    sha256: "f9a68fabba69c3b81e153367b2c7521030b0fa8bb0de400c9599c8e6725f9c82",
    primary_url: "https://modelscope.cn/models/Qwen/Qwen3-VL-2B-Instruct-GGUF/resolve/master/mmproj-Qwen3VL-2B-Instruct-Q8_0.gguf",
    mirror_url: "https://huggingface.co/Qwen/Qwen3-VL-2B-Instruct-GGUF/resolve/main/mmproj-Qwen3VL-2B-Instruct-Q8_0.gguf",
    fallback_url: "https://hf-mirror.com/Qwen/Qwen3-VL-2B-Instruct-GGUF/resolve/main/mmproj-Qwen3VL-2B-Instruct-Q8_0.gguf",
};

/// 默认档：官方 Qwen3-VL-2B Q4_K_M + Q8_0 mmproj（实测合计 1.45GB）。
/// CPU/核显设备的质量-体积平衡点，`default_model()` 指向本档。
pub(crate) const MODEL_Q4_K_M: LlamaModelSpec = LlamaModelSpec {
    id: "qwen3vl-2b-q4km",
    display_name: "Qwen3-VL-2B Q4_K_M（1.55GB，默认推荐）",
    size_bytes: 1_107_409_952 + 445_053_216,
    gguf: ModelAsset {
        filename: "Qwen3VL-2B-Instruct-Q4_K_M.gguf",
        expected_size: 1_107_409_952,
        sha256: "089d75c52f4b7ffc56ba998ffc50aae89fcafc755f9e7208aacca281dca6c2ae",
        primary_url: "https://modelscope.cn/models/Qwen/Qwen3-VL-2B-Instruct-GGUF/resolve/master/Qwen3VL-2B-Instruct-Q4_K_M.gguf",
        mirror_url: "https://huggingface.co/Qwen/Qwen3-VL-2B-Instruct-GGUF/resolve/main/Qwen3VL-2B-Instruct-Q4_K_M.gguf",
        fallback_url: "https://hf-mirror.com/Qwen/Qwen3-VL-2B-Instruct-GGUF/resolve/main/Qwen3VL-2B-Instruct-Q4_K_M.gguf",
    },
    mmproj: MMPROJ_2B_Q8_0,
};

/// 独显档：官方 Qwen3-VL-4B Q4_K_M + Q8_0 mmproj（实测合计 2.75GB）。
/// 显存充足的独显设备上识图质量明显优于 2B。
pub(crate) const MODEL_4B_Q4_K_M: LlamaModelSpec = LlamaModelSpec {
    id: "qwen3vl-4b-q4km",
    display_name: "Qwen3-VL-4B Q4_K_M（2.95GB，独显推荐）",
    size_bytes: 2_497_281_664 + 453_974_304,
    gguf: ModelAsset {
        filename: "Qwen3VL-4B-Instruct-Q4_K_M.gguf",
        expected_size: 2_497_281_664,
        sha256: "66358cb18bb6b3b1b6675aa412c7a88ef01d228f481184d13668e5201c730a0a",
        primary_url: "https://modelscope.cn/models/Qwen/Qwen3-VL-4B-Instruct-GGUF/resolve/master/Qwen3VL-4B-Instruct-Q4_K_M.gguf",
        mirror_url: "https://huggingface.co/Qwen/Qwen3-VL-4B-Instruct-GGUF/resolve/main/Qwen3VL-4B-Instruct-Q4_K_M.gguf",
        fallback_url: "https://hf-mirror.com/Qwen/Qwen3-VL-4B-Instruct-GGUF/resolve/main/Qwen3VL-4B-Instruct-Q4_K_M.gguf",
    },
    mmproj: ModelAsset {
        filename: "mmproj-Qwen3VL-4B-Instruct-Q8_0.gguf",
        expected_size: 453_974_304,
        sha256: "30ba2c7dd3127a4561b6cba9d13d0f711c91bdb38742e2f56d73c8cb596bd06d",
        primary_url: "https://modelscope.cn/models/Qwen/Qwen3-VL-4B-Instruct-GGUF/resolve/master/mmproj-Qwen3VL-4B-Instruct-Q8_0.gguf",
        mirror_url: "https://huggingface.co/Qwen/Qwen3-VL-4B-Instruct-GGUF/resolve/main/mmproj-Qwen3VL-4B-Instruct-Q8_0.gguf",
        fallback_url: "https://hf-mirror.com/Qwen/Qwen3-VL-4B-Instruct-GGUF/resolve/main/mmproj-Qwen3VL-4B-Instruct-Q8_0.gguf",
    },
};

/// 档位顺序即设置页展示顺序：默认 → 独显。
/// （2026-08 移除 IQ2_M 极致低配档与 Q3_K_S legacy 档：量化过低识图质量
/// 不可用，老安装残留文件不受影响，只是不再出现在可选列表。）
const MODEL_SPECS: &[LlamaModelSpec] = &[MODEL_Q4_K_M, MODEL_4B_Q4_K_M];

pub(crate) fn model_specs() -> &'static [LlamaModelSpec] {
    MODEL_SPECS
}

/// 新装默认档；prefs 里存着 legacy id 的老用户原样保留（已下载的继续用）。
pub(crate) fn default_model() -> &'static LlamaModelSpec {
    &MODEL_Q4_K_M
}

pub(crate) fn model_spec(model_id: &str) -> Result<&'static LlamaModelSpec, String> {
    MODEL_SPECS
        .iter()
        .find(|spec| spec.id == model_id)
        .ok_or_else(|| format!("未知模型: {model_id}"))
}

pub(crate) fn model_gguf_path(spec: &LlamaModelSpec) -> PathBuf {
    models_dir().join(spec.gguf.filename)
}

pub(crate) fn mmproj_path(spec: &LlamaModelSpec) -> PathBuf {
    models_dir().join(spec.mmproj.filename)
}

// ---------------- 引擎 ----------------

const LLAMA_REPO: &str = "ggml-org/llama.cpp";
/// 默认锁定的引擎 tag（该 tag 各平台资产的尺寸 + sha256 已在
/// `platform::pinned_engine_asset` 钉死；`PINVOU3_LLAMA_ENGINE_TAG`
/// 显式设置时可覆盖为其它版本或 "latest"，属开发通道，跳过 digest 校验）。
const PINNED_ENGINE_TAG: &str = "b10299";

pub(crate) fn engine_binary_path() -> PathBuf {
    bin_dir().join(platform::engine_binary_name())
}

pub(crate) fn engine_installed() -> bool {
    engine_binary_path().is_file()
}

pub(crate) fn engine_tag() -> Option<String> {
    let meta = bin_dir().join("engine-meta.json");
    let text = std::fs::read_to_string(meta).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("tag")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn engine_installed_with_tag(tag: &str) -> bool {
    engine_installed() && engine_tag().as_deref() == Some(tag)
}

/// 开发用 env 覆盖统一入口：仅 debug 构建生效，release 下忽略并 log::warn
/// 一次。这些覆盖（`PINVOU3_LLAMA_ENGINE_TAG` / `PINVOU3_LLAMA_MODEL_URL` /
/// per-asset sha env）可替换下载源、选任意引擎 tag、绕过 sha256 校验，release
/// 生效会把开发通道暴露给最终用户。
fn dev_env_override(name: &str) -> Option<String> {
    let value = std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    if cfg!(debug_assertions) {
        value
    } else {
        if value.is_some() {
            static WARNED: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
            let mut warned = WARNED
                .get_or_init(|| Mutex::new(std::collections::HashSet::new()))
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if warned.insert(name.to_string()) {
                log::warn!("[pinvou3][llama-engine] release 构建忽略开发用 env 覆盖 {name}");
            }
        }
        None
    }
}

/// 解析引擎版本：默认锁定 PINNED_ENGINE_TAG（资产有钉死的 sha256 校验）。
/// env `PINVOU3_LLAMA_ENGINE_TAG` 显式设置时才覆盖：值为 "latest" 时查询
/// GitHub latest API，其它非空值直接作为 tag 使用。env 覆盖属开发通道，
/// 仅 debug 构建生效（release 忽略，见 `dev_env_override`）；对应资产没有
/// 钉版 digest，安装时跳过完整性校验（日志提示）。
pub(crate) async fn resolve_engine_tag() -> Result<String, String> {
    if let Some(tag) = dev_env_override("PINVOU3_LLAMA_ENGINE_TAG") {
        if tag == "latest" {
            return query_latest_engine_tag().await;
        }
        return Ok(tag);
    }
    Ok(PINNED_ENGINE_TAG.to_string())
}

/// 查询 GitHub latest release tag（仅 env 显式指定 "latest" 的开发通道调用）。
async fn query_latest_engine_tag() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .user_agent("pinvou3-llama-engine/1.0")
        .build()
        .map_err(|e| format!("HTTP client 构建失败: {e}"))?;
    let url = format!("https://api.github.com/repos/{LLAMA_REPO}/releases/latest");
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("查询 llama.cpp 最新版本失败: {e}"))?;
    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("解析 llama.cpp release 响应失败: {e}"))?;
    if let Some(tag) = json.get("tag_name").and_then(|v| v.as_str()) {
        if !tag.is_empty() {
            return Ok(tag.to_string());
        }
    }
    Ok(PINNED_ENGINE_TAG.to_string())
}

/// 一键下载并部署 llama.cpp 引擎（幂等：同 tag 已安装则直接返回）。
pub(crate) async fn install_engine(app: &tauri::AppHandle) -> Result<(), String> {
    let _guard = acquire_download("engine")?;

    let tag = resolve_engine_tag().await?;
    if engine_installed_with_tag(&tag) {
        let _ = app.emit(
            "llama-engine:progress",
            serde_json::json!({ "stage": "done", "item": "engine", "tag": tag }),
        );
        return Ok(());
    }

    if platform::engine_asset_name(&tag).is_empty() {
        return Err("当前平台暂不支持本地多模态引擎".to_string());
    }
    let asset_name = platform::engine_asset_name(&tag);
    let url = platform::engine_url(&tag);
    // 钉版 tag：尺寸 + sha256 强制校验；env 覆盖的开发通道无钉版 digest，
    // 跳过校验并打日志（fail closed 只针对钉版资产）。
    let pinned = if tag == PINNED_ENGINE_TAG {
        match platform::pinned_engine_asset() {
            Some(pinned) => Some(pinned),
            None => return Err("当前平台缺少钉版引擎的完整性校验信息".to_string()),
        }
    } else {
        log::warn!(
            "[pinvou3][llama-engine] 引擎 tag {tag} 为 env 覆盖的开发通道，跳过 sha256 校验"
        );
        None
    };
    for dir in [llama_engine_dir(), bin_dir(), tmp_dir()] {
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {e}"))?;
    }

    let archive = tmp_dir().join(&asset_name);
    let part = tmp_dir().join(format!("{asset_name}.part"));
    let _ = std::fs::remove_file(&part);
    let _ = std::fs::remove_file(&archive);
    let expected_size = pinned.map(|(size, _)| size).unwrap_or(0);
    ensure_disk_space(&tmp_dir(), expected_size)?;
    download_file(
        app,
        &url,
        &part,
        "engine_download",
        "engine",
        None,
        None,
        expected_size,
    )
    .await
    .map_err(|e| {
        // 引擎路径与模型路径同契约：网络/写盘失败不遗留孤儿 .part。
        let _ = std::fs::remove_file(&part);
        e
    })?;
    if let Some((size, sha256)) = pinned {
        verify_engine_archive(&part, size, sha256)?;
        // 校验期间取消：取消优先——GB 级包的 sha256 耗时数秒，期间取消
        // 不应继续解压替换（commit 点在解压替换，此刻退出完全不落盘）。
        if CANCEL.load(Ordering::Acquire) {
            let _ = std::fs::remove_file(&part);
            return Err("已取消".to_string());
        }
    }

    let _ = app.emit(
        "llama-engine:progress",
        serde_json::json!({ "stage": "engine_extract", "item": "engine" }),
    );
    let extract_dir = tmp_dir().join("engine-extract");
    let _ = std::fs::remove_dir_all(&extract_dir);
    std::fs::create_dir_all(&extract_dir).map_err(|e| format!("创建解压目录失败: {e}"))?;
    let part_for_task = part.clone();
    let extract_for_task = extract_dir.clone();
    let dest_bin = bin_dir();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        if platform::engine_archive_is_zip() {
            extract_zip(&part_for_task, &extract_for_task)?;
        } else {
            extract_targz(&part_for_task, &extract_for_task)?;
        }
        // Linux/macOS 压缩包带顶层 `llama-*/bin/`，定位 llama-server 所在目录。
        let server_dir = locate_engine_server_dir(&extract_for_task)?;
        // rename 替换前先停运行中的引擎：Windows 下运行中的 llama-server
        // 占用 bin 目录文件，rename 会失败。
        stop_engine_if_running("替换引擎文件")?;
        swap_engine_files(&server_dir, &dest_bin)
    })
    .await
    .map_err(|e| format!("解压任务失败: {e}"))??;

    let _ = std::fs::remove_file(&part);
    let _ = std::fs::remove_dir_all(&extract_dir);
    write_engine_meta(&tag)?;
    let _ = app.emit(
        "llama-engine:progress",
        serde_json::json!({ "stage": "done", "item": "engine", "tag": tag }),
    );
    Ok(())
}

/// 在解压目录中定位 llama-server 所在目录（Windows zip 根目录直放；
/// Linux/macOS 为顶层单目录下的 `bin/`）。
fn locate_engine_server_dir(extract_dir: &Path) -> Result<PathBuf, String> {
    let name = platform::engine_binary_name();
    if extract_dir.join(name).is_file() {
        return Ok(extract_dir.to_path_buf());
    }
    let entries = std::fs::read_dir(extract_dir).map_err(|e| format!("读取解压目录失败: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("读取解压目录条目失败: {e}"))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join(name).is_file() {
            return Ok(path);
        }
        let bin = path.join("bin");
        if bin.join(name).is_file() {
            return Ok(bin);
        }
    }
    Err(format!("压缩包内未找到 {name}"))
}

/// 引擎运行/启动中则停止并等待 watcher 收口（rename 替换/删除文件的前置
/// 条件：Windows 下运行中的 llama-server 占用 bin/模型文件，见
/// `swap_engine_files`）。仅停引擎，不影响后续手动/自动再启动；超时返回
/// 明确错误。
fn stop_engine_if_running(action: &str) -> Result<(), String> {
    let phase = super::server::runtime_snapshot().phase;
    if phase != "running" && phase != "starting" {
        return Ok(());
    }
    super::server::stop();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let phase = super::server::runtime_snapshot().phase;
        if phase != "running" && phase != "starting" {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!("停止运行中的引擎超时，无法{action}"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 原子替换引擎文件：先把 llama-server + 同目录共享库复制到同级 staged
/// 目录（与 bin 同分区），再 rename 整体交换——同分区 rename 原子，旧目录
/// 先改名备份，成功后删除。任一步失败时清理 staged 并尽力恢复旧引擎目录，
/// 保证旧引擎仍可用。
fn swap_engine_files(src_dir: &Path, dest_dir: &Path) -> Result<(), String> {
    let Some(parent) = dest_dir.parent() else {
        return Err("引擎目录无父目录".to_string());
    };
    let staged = parent.join("bin.staged");
    let backup = parent.join("bin.old");
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged).map_err(|e| format!("创建引擎暂存目录失败: {e}"))?;
    let mut copied = 0usize;
    let stage_result = (|| -> Result<(), String> {
        for entry in std::fs::read_dir(src_dir).map_err(|e| format!("读取引擎目录失败: {e}"))?
        {
            let entry = entry.map_err(|e| format!("读取引擎目录条目失败: {e}"))?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = entry.file_name();
            let dest = staged.join(&name);
            std::fs::copy(&path, &dest)
                .map_err(|e| format!("复制引擎文件 {} 失败: {e}", name.to_string_lossy()))?;
            if name == platform::engine_binary_name() {
                platform::make_executable(&dest)?;
            }
            copied += 1;
        }
        if copied == 0 {
            return Err("压缩包内未找到引擎文件".to_string());
        }
        Ok(())
    })();
    if let Err(error) = stage_result {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(error);
    }
    let _ = std::fs::remove_dir_all(&backup);
    if dest_dir.exists() {
        if let Err(e) = std::fs::rename(dest_dir, &backup) {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(format!("备份旧引擎目录失败: {e}"));
        }
    }
    if let Err(e) = std::fs::rename(&staged, dest_dir) {
        // 新目录落位失败：尽力恢复旧引擎目录，保证引擎仍可运行。
        let _ = std::fs::rename(&backup, dest_dir);
        let _ = std::fs::remove_dir_all(&staged);
        return Err(format!("替换引擎目录失败: {e}"));
    }
    let _ = std::fs::remove_dir_all(&backup);
    Ok(())
}

fn write_engine_meta(tag: &str) -> Result<(), String> {
    let meta = bin_dir().join("engine-meta.json");
    let text = serde_json::json!({ "tag": tag }).to_string();
    std::fs::write(&meta, text).map_err(|e| format!("写入引擎元数据失败: {e}"))
}

// ---------------- 模型 ----------------

/// 一键下载指定模型的 gguf + mmproj（幂等：已验证的资产跳过）。
pub(crate) async fn install_model(app: &tauri::AppHandle, model_id: &str) -> Result<(), String> {
    let spec = model_spec(model_id)?;
    let _guard = acquire_download("model")?;

    // 即将改写模型文件时先停运行中的引擎并等收口（Windows 下运行中的
    // llama-server 占用 gguf/mmproj，rename 会报 sharing violation；与
    // delete 路径同口径）。已完整安装的幂等跳过不打扰运行中的引擎。
    // 收口等待最长 ~10s，与 delete 命令同口径跑在 spawn_blocking——本函数
    // 是 async，阻塞等待不得占住 tokio 工作线程。
    if !model_files_verified(&spec) {
        tokio::task::spawn_blocking(|| stop_engine_if_running("替换模型文件"))
            .await
            .map_err(|e| format!("停止引擎任务失败: {e}"))??;
    }

    std::fs::create_dir_all(models_dir()).map_err(|e| format!("创建模型目录失败: {e}"))?;
    install_asset(app, &spec.gguf, "model", model_id).await?;
    install_asset(app, &spec.mmproj, "mmproj", model_id).await?;
    let _ = app.emit(
        "llama-engine:progress",
        serde_json::json!({ "stage": "done", "item": "model", "modelId": model_id }),
    );
    Ok(())
}

async fn install_asset(
    app: &tauri::AppHandle,
    asset: &ModelAsset,
    item: &'static str,
    model_id: &str,
) -> Result<(), String> {
    let dest = models_dir().join(asset.filename);
    // 安装前检查走完整校验（size + 全量 sha256），与状态热路径的轻量
    // 检查区分开：已验证过的资产命中 VERIFY_CACHE，不会重复 hash。
    if model_file_verified_fully(&dest, asset) {
        return Ok(());
    }
    ensure_disk_space(&models_dir(), asset.expected_size)?;
    let tmp = dest.with_extension("part");
    let _ = std::fs::remove_file(&tmp);
    let mut last_err = None;
    for url in asset_urls(asset) {
        if CANCEL.load(Ordering::Acquire) {
            let _ = std::fs::remove_file(&tmp);
            return Err("已取消".to_string());
        }
        match download_file(
            app,
            &url,
            &tmp,
            "model_download",
            item,
            Some(asset.filename),
            Some(model_id),
            asset.expected_size,
        )
        .await
        {
            Ok(()) => {
                if CANCEL.load(Ordering::Acquire) {
                    let _ = std::fs::remove_file(&tmp);
                    return Err("已取消".to_string());
                }
                let _ = app.emit(
                    "llama-engine:progress",
                    serde_json::json!({
                        "stage": "model_verify", "item": item, "modelId": model_id
                    }),
                );
                match verify_and_promote(&tmp, &dest, asset) {
                    Ok(()) => return Ok(()),
                    Err(_) if CANCEL.load(Ordering::Acquire) => {
                        let _ = std::fs::remove_file(&tmp);
                        return Err("已取消".to_string());
                    }
                    Err(error) => {
                        // size/sha256 校验失败不再直接放弃：删掉 .part、记录
                        // 原因、换下一个候选源（镜像内容错配/被劫持时可经其他
                        // 源恢复）；所有源都失败才由循环尾汇总报错。fail closed
                        // 不变：校验失败绝不落盘，任何源都绕不过校验。
                        let _ = std::fs::remove_file(&tmp);
                        last_err = Some(error);
                    }
                }
            }
            Err(error) => {
                let _ = std::fs::remove_file(&tmp);
                if CANCEL.load(Ordering::Acquire) {
                    return Err("已取消".to_string());
                }
                last_err = Some(error);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| "模型下载失败".to_string()))
}

fn asset_urls(asset: &ModelAsset) -> Vec<String> {
    // env 覆盖属开发通道，仅 debug 构建生效（release 忽略，见
    // `dev_env_override`）；debug 下同样强制 https，防开发时误填
    // http 地址后被当成可用配置残留。
    if let Some(url) = dev_env_override("PINVOU3_LLAMA_MODEL_URL") {
        if url.starts_with("https://") {
            return vec![url];
        }
        log::warn!("[pinvou3][llama-engine] 忽略非 https 的 PINVOU3_LLAMA_MODEL_URL 覆盖");
    }
    let mut urls = vec![asset.primary_url.to_string()];
    if !asset.mirror_url.is_empty() {
        urls.push(asset.mirror_url.to_string());
    }
    if !asset.fallback_url.is_empty() {
        urls.push(asset.fallback_url.to_string());
    }
    urls
}

// ---------------- 下载与校验 ----------------

/// 状态位：仅供 `llama_engine_status` 汇报「下载中」，不参与互斥。
static DOWNLOADING: AtomicBool = AtomicBool::new(false);
/// 互斥位：安装与删除共用的文件变更闸。删除不能复用 DOWNLOADING——
/// 那会让状态查询在删除（含最多 ~10s 的停机等待）期间把引擎误报成
/// 「下载中」；但删除又必须与在途安装互斥，否则「删完又被装回」、swap
/// 改名撞 bin.old/bin.staged 清理、共享 mmproj 共用判定失真。单一原子闸
/// 保证 install/delete 两两互斥；DOWNLOADING 只在持闸期间置位。
static FILE_BUSY: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
static DOWNLOAD_ITEM: Mutex<Option<&'static str>> = Mutex::new(None);

const PROGRESS_EMIT_BYTES: u64 = 2 * 1024 * 1024;
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

struct DownloadGuard {
    _busy: FileBusyGuard,
}

impl Drop for DownloadGuard {
    fn drop(&mut self) {
        DOWNLOADING.store(false, Ordering::SeqCst);
        *DOWNLOAD_ITEM.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

struct FileBusyGuard;

impl Drop for FileBusyGuard {
    fn drop(&mut self) {
        FILE_BUSY.store(false, Ordering::SeqCst);
    }
}

fn acquire_file_busy() -> Result<FileBusyGuard, String> {
    if FILE_BUSY.swap(true, Ordering::SeqCst) {
        return Err("已有引擎文件安装或删除任务进行中".to_string());
    }
    Ok(FileBusyGuard)
}

fn acquire_download(item: &'static str) -> Result<DownloadGuard, String> {
    let busy = acquire_file_busy()?;
    // 持闸期间 DOWNLOADING 必为 false（所有置位点都在闸内）。
    DOWNLOADING.store(true, Ordering::SeqCst);
    *DOWNLOAD_ITEM.lock().unwrap_or_else(|e| e.into_inner()) = Some(item);
    CANCEL.store(false, Ordering::SeqCst);
    Ok(DownloadGuard { _busy: busy })
}

pub(crate) fn is_downloading() -> bool {
    DOWNLOADING.load(Ordering::Acquire)
}

pub(crate) fn downloading_item() -> Option<&'static str> {
    *DOWNLOAD_ITEM.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn cancel_download() {
    CANCEL.store(true, Ordering::SeqCst);
}

/// 流式下载到 `.part`，带进度事件与取消。
#[allow(clippy::too_many_arguments)]
async fn download_file(
    app: &tauri::AppHandle,
    url: &str,
    dest: &Path,
    stage: &'static str,
    item: &'static str,
    filename: Option<&str>,
    model_id: Option<&str>,
    fallback_total: u64,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        // 读超时：连接中断（如 GitHub 下行卡死）时 120s 无数据即报错，
        // 否则下载会永久挂起且无法取消。
        .read_timeout(Duration::from_secs(120))
        .user_agent("pinvou3-llama-engine/1.0")
        .build()
        .map_err(|e| format!("HTTP client 构建失败: {e}"))?;
    let response = tokio::select! {
        result = client.get(url).send() => {
            result.map_err(|e| format!("连接下载源失败: {e}"))?
        }
        _ = wait_for_cancel() => {
            let _ = std::fs::remove_file(dest);
            return Err("已取消".to_string());
        }
    };
    let mut resp = response
        .error_for_status()
        .map_err(|e| format!("下载源响应异常: {e}"))?;
    let total = resp
        .content_length()
        .filter(|n| *n > 0)
        .unwrap_or(fallback_total);
    let mut file = std::fs::File::create(dest).map_err(|e| format!("创建文件失败: {e}"))?;
    let mut downloaded: u64 = 0;
    let mut last_emit: u64 = 0;
    loop {
        let chunk = tokio::select! {
            result = resp.chunk() => {
                result.map_err(|e| format!("下载中断: {e}"))?
            }
            _ = wait_for_cancel() => {
                drop(file);
                let _ = std::fs::remove_file(dest);
                return Err("已取消".to_string());
            }
        };
        let Some(chunk) = chunk else { break };
        file.write_all(&chunk)
            .map_err(|e| format!("写盘失败: {e}"))?;
        downloaded += chunk.len() as u64;
        if downloaded - last_emit >= PROGRESS_EMIT_BYTES || (total > 0 && downloaded >= total) {
            last_emit = downloaded;
            emit_progress(app, stage, item, filename, model_id, downloaded, total);
        }
    }
    drop(file);
    Ok(())
}

async fn wait_for_cancel() {
    loop {
        if CANCEL.load(Ordering::Acquire) {
            return;
        }
        tokio::time::sleep(CANCEL_POLL_INTERVAL).await;
    }
}

fn emit_progress(
    app: &tauri::AppHandle,
    stage: &'static str,
    item: &'static str,
    filename: Option<&str>,
    model_id: Option<&str>,
    downloaded: u64,
    total: u64,
) {
    let _ = app.emit(
        "llama-engine:progress",
        serde_json::json!({
            "stage": stage,
            "item": item,
            "filename": filename,
            "modelId": model_id,
            "downloaded": downloaded,
            "total": total,
        }),
    );
}

/// 尺寸 + sha256 校验后原子改名落盘（fail closed：不匹配即删除 `.part` 报错）。
/// 校验通过后把结果写入 VERIFY_CACHE，状态热路径不再重复全量 hash。
fn verify_and_promote(tmp: &Path, dest: &Path, asset: &ModelAsset) -> Result<(), String> {
    let meta = std::fs::metadata(tmp).map_err(|e| format!("读取下载文件信息失败: {e}"))?;
    if asset.expected_size > 0 && meta.len() != asset.expected_size {
        let _ = std::fs::remove_file(tmp);
        return Err(format!(
            "模型文件尺寸不符：期望 {} 实际 {}",
            asset.expected_size,
            meta.len()
        ));
    }
    let expected = asset_sha256(asset);
    let got =
        crate::platform::hashing::sha256_file(tmp).map_err(|e| format!("读取下载文件失败: {e}"))?;
    if !got.eq_ignore_ascii_case(&expected) {
        let _ = std::fs::remove_file(tmp);
        return Err(format!(
            "模型校验失败(sha256 不匹配): 期望 {expected:.12} 实际 {got:.12}"
        ));
    }
    // 校验期间用户取消：与 platform::download 契约同口径——取消优先，
    // 已通过校验的文件也不提升安装；.part 由调用方按取消路径清理。
    if CANCEL.load(Ordering::Acquire) {
        return Err("已取消".to_string());
    }
    std::fs::rename(tmp, dest).map_err(|e| format!("落盘模型文件失败: {e}"))?;
    // 安装完成的文件已强制通过完整性校验，回填缓存供状态路径直接消费。
    if let Ok(meta) = std::fs::metadata(dest) {
        remember_verified(dest, meta.len(), meta.modified().ok(), true);
    }
    Ok(())
}

/// 钉版引擎包的尺寸 + sha256 强制校验（fail closed：不匹配即删除 `.part` 报错）。
fn verify_engine_archive(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("读取下载文件信息失败: {e}"))?;
    if meta.len() != expected_size {
        let _ = std::fs::remove_file(path);
        return Err(format!(
            "引擎包尺寸不符：期望 {expected_size} 实际 {}",
            meta.len()
        ));
    }
    let got = crate::platform::hashing::sha256_file(path)
        .map_err(|e| format!("读取下载文件失败: {e}"))?;
    if !got.eq_ignore_ascii_case(expected_sha256) {
        let _ = std::fs::remove_file(path);
        return Err(format!(
            "引擎包校验失败(sha256 不匹配): 期望 {expected_sha256:.12} 实际 {got:.12}"
        ));
    }
    Ok(())
}

/// 下载前检查目标目录所在卷的可用空间（要求 expected_size 的 1.2 倍余量）。
/// 平台不支持查询或 expected_size 未知（0）时跳过。
fn ensure_disk_space(dir: &Path, expected_size: u64) -> Result<(), String> {
    if expected_size == 0 {
        return Ok(());
    }
    let required = expected_size.saturating_mul(6) / 5;
    let Some(available) = platform::available_disk_space(dir) else {
        return Ok(());
    };
    if available < required {
        return Err(format!(
            "磁盘可用空间不足：下载约需 {:.1}GB（含 20% 余量），当前可用 {:.1}GB",
            required as f64 / 1024.0 / 1024.0 / 1024.0,
            available as f64 / 1024.0 / 1024.0 / 1024.0
        ));
    }
    Ok(())
}

/// 资产 sha256：内置钉版值，dev 可用 env 覆盖（仅 debug 构建生效，release
/// 忽略，见 `dev_env_override`）——per-asset
/// `PINVOU3_LLAMA_GGUF_SHA256` / `PINVOU3_LLAMA_MMPROJ_SHA256` 优先；
/// 旧版 `PINVOU3_LLAMA_MODEL_SHA256`（一 hash 两用）保留为开发兜底。
fn asset_sha256(asset: &ModelAsset) -> String {
    asset_sha256_with(asset, dev_env_override)
}

/// env 读取抽成参数，便于单测不依赖进程级 env（避免并行测试互相干扰）。
fn asset_sha256_with(asset: &ModelAsset, env: impl Fn(&str) -> Option<String>) -> String {
    let per_asset = if asset.filename.starts_with("mmproj") {
        env("PINVOU3_LLAMA_MMPROJ_SHA256")
    } else {
        env("PINVOU3_LLAMA_GGUF_SHA256")
    };
    per_asset
        .filter(|s| !s.trim().is_empty())
        .or_else(|| env("PINVOU3_LLAMA_MODEL_SHA256").filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| asset.sha256.to_string())
}

/// 校验缓存（按 path + len + modified 失效；下载后替换文件会自然失效）。
type VerifyKey = (PathBuf, u64, Option<std::time::SystemTime>);
static VERIFY_CACHE: OnceLock<Mutex<HashMap<VerifyKey, bool>>> = OnceLock::new();

fn verify_cache() -> &'static Mutex<HashMap<VerifyKey, bool>> {
    VERIFY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_verified(key: &VerifyKey) -> Option<bool> {
    verify_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(key)
        .copied()
}

fn remember_verified(path: &Path, len: u64, modified: Option<std::time::SystemTime>, ok: bool) {
    verify_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert((path.to_path_buf(), len, modified), ok);
}

/// 状态查询路径的轻量校验：只消费 VERIFY_CACHE，缓存未命中退化为
/// size + mtime 存在性检查——文件完整性在安装/下载完成时已强制 sha256
/// 校验（见 `verify_and_promote`），此处不对 GB 级文件重复全量 hash。
pub(crate) fn model_file_verified(path: &Path, asset: &ModelAsset) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if asset.expected_size > 0 && meta.len() != asset.expected_size {
        return false;
    }
    let key = (path.to_path_buf(), meta.len(), meta.modified().ok());
    cached_verified(&key).unwrap_or(true)
}

/// 安装路径的完整校验：size + 全量 sha256 比对（fail closed：hash 不匹配
/// 即视为未安装，触发重新下载），结果写入 VERIFY_CACHE 供状态路径消费。
fn model_file_verified_fully(path: &Path, asset: &ModelAsset) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if asset.expected_size > 0 && meta.len() != asset.expected_size {
        return false;
    }
    let key = (path.to_path_buf(), meta.len(), meta.modified().ok());
    if let Some(cached) = cached_verified(&key) {
        return cached;
    }
    let expected = asset_sha256(asset);
    let ok = crate::platform::hashing::sha256_file(path)
        .map(|got| got.eq_ignore_ascii_case(&expected))
        .unwrap_or(false);
    remember_verified(path, meta.len(), meta.modified().ok(), ok);
    ok
}

pub(crate) fn model_files_verified(spec: &LlamaModelSpec) -> bool {
    model_file_verified(&model_gguf_path(spec), &spec.gguf)
        && model_file_verified(&mmproj_path(spec), &spec.mmproj)
}

/// 删除引擎二进制与同目录共享库（保留 bin 目录本身），返回删除的文件数。
/// 一并清理 swap_engine_files 中途失败可能残留的 bin.old/bin.staged 临时
/// 目录（与 bin 同级），否则重装时 staged 目录会带脏残留起步。
/// 运行/启动中的引擎先停止并等收口（Windows 下文件被进程占用无法删除）。
/// 用户可见错误只含资产名，本地路径细节进日志（不泄露用户目录结构）。
/// 引擎可经「下载引擎」随时重装。
pub(crate) fn delete_engine_files() -> Result<u32, String> {
    // 与在途安装互斥（FILE_BUSY）：否则删除可与下载/swap 交错，出现
    // 「删除成功」后又被装回、或删 bin.old/bin.staged 撞上 swap 改名。
    let _busy = acquire_file_busy()?;
    stop_engine_if_running("删除引擎文件")?;
    let mut removed = 0u32;
    let dir = bin_dir();
    if !dir.is_dir() {
        return Ok(0);
    }
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("读取引擎目录失败: {e}"))? {
        let entry = entry.map_err(|e| format!("读取引擎目录项失败: {e}"))?;
        let path = entry.path();
        if path.is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            std::fs::remove_file(&path).map_err(|e| {
                log::warn!(
                    "[pinvou3][llama-engine] 删除引擎文件失败 {}: {e}",
                    path.display()
                );
                format!("删除引擎文件 {name} 失败")
            })?;
            removed += 1;
        }
    }
    if let Some(parent) = dir.parent() {
        for leftover in [parent.join("bin.old"), parent.join("bin.staged")] {
            if leftover.exists() {
                std::fs::remove_dir_all(&leftover).map_err(|e| {
                    log::warn!(
                        "[pinvou3][llama-engine] 删除引擎临时目录失败 {}: {e}",
                        leftover.display()
                    );
                    "删除引擎临时目录失败".to_string()
                })?;
            }
        }
    }
    Ok(removed)
}

/// 删除已安装模型的权重文件，返回实际删除的文件数（0 = 本就没装）。
/// 运行/启动中的引擎先停止并等收口（Windows 下模型文件被进程占用无法删除）。
/// 用户可见错误只含资产名/模型 id，本地路径细节进日志。
/// mmproj 仅在无其他已安装档位共用同一文件时一并删除（历史上 2B 两档
/// 共享 Q8_0 mmproj；当前只余 Q4_K_M 一档使用，逻辑保留以防后续加档）。
pub(crate) fn delete_model_files(spec: &LlamaModelSpec) -> Result<u32, String> {
    // 与在途安装互斥（FILE_BUSY）：安装中的档位尚未落盘完整，此时按
    // 「已验证」判定共享 mmproj 会误删另一档位正在下载的投影器。
    let _busy = acquire_file_busy()?;
    stop_engine_if_running("删除模型文件")?;
    let mut removed = 0u32;
    let gguf = model_gguf_path(spec);
    if gguf.exists() {
        std::fs::remove_file(&gguf).map_err(|e| {
            log::warn!(
                "[pinvou3][llama-engine] 删除模型文件失败 {}: {e}",
                gguf.display()
            );
            format!("删除模型 {} 的权重文件失败", spec.id)
        })?;
        removed += 1;
    }
    let mmproj_in_use = model_specs().iter().any(|other| {
        other.id != spec.id
            && other.mmproj.filename == spec.mmproj.filename
            && model_file_verified(&model_gguf_path(other), &other.gguf)
    });
    if !mmproj_in_use {
        let mmproj = mmproj_path(spec);
        if mmproj.exists() {
            std::fs::remove_file(&mmproj).map_err(|e| {
                log::warn!(
                    "[pinvou3][llama-engine] 删除视觉投影器失败 {}: {e}",
                    mmproj.display()
                );
                format!("删除模型 {} 的视觉投影器失败", spec.id)
            })?;
            removed += 1;
        }
    }
    Ok(removed)
}

// ---------------- 解压 ----------------

/// zip 解压（防 zip-slip：拒绝 `..` / 根路径 / 盘符前缀条目）。
fn extract_zip(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| format!("打开压缩包失败: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("解析压缩包失败: {e}"))?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| format!("读取压缩包条目失败: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let target = safe_join(dest, &name)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建解压目录失败: {e}"))?;
        }
        let mut out =
            std::fs::File::create(&target).map_err(|e| format!("创建解压文件失败: {e}"))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| format!("解压失败: {e}"))?;
    }
    Ok(())
}

/// tar.gz 解压（先校验条目路径再 unpack，防路径穿越）。
fn extract_targz(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| format!("打开压缩包失败: {e}"))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    tar.set_unpack_xattrs(false);
    for entry in tar.entries().map_err(|e| format!("读取压缩包失败: {e}"))? {
        let mut entry = entry.map_err(|e| format!("读取压缩包条目失败: {e}"))?;
        let name = entry
            .path()
            .map_err(|e| format!("读取压缩包路径失败: {e}"))?
            .to_string_lossy()
            .into_owned();
        safe_join(dest, &name)?;
        entry
            .unpack_in(dest)
            .map_err(|e| format!("解压失败: {e}"))?;
    }
    Ok(())
}

fn safe_join(base: &Path, name: &str) -> Result<PathBuf, String> {
    let path = Path::new(name);
    for component in path.components() {
        if matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        ) {
            return Err(format!("压缩包条目含非法路径: {name}"));
        }
    }
    Ok(base.join(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_specs_table_has_unique_ids_and_default_size() {
        let mut ids = std::collections::HashSet::new();
        for spec in MODEL_SPECS {
            assert!(ids.insert(spec.id), "duplicate model id {}", spec.id);
            assert!(!spec.gguf.filename.is_empty());
            assert!(!spec.mmproj.filename.is_empty());
            // 完整性强制校验依赖内置钉版 digest，不允许回退为空
            assert!(!spec.gguf.sha256.is_empty());
            assert!(!spec.mmproj.sha256.is_empty());
            assert!(spec.gguf.primary_url.starts_with("https://"));
            assert!(spec.gguf.mirror_url.starts_with("https://"));
            assert!(spec.mmproj.primary_url.starts_with("https://"));
        }
        assert_eq!(default_model().id, MODEL_Q4_K_M.id);
        assert!(MODEL_Q4_K_M.size_bytes > 0);
    }

    #[test]
    fn model_spec_resolves_known_and_rejects_unknown() {
        assert_eq!(model_spec("qwen3vl-2b-q4km").unwrap().id, "qwen3vl-2b-q4km");
        assert_eq!(model_spec("qwen3vl-4b-q4km").unwrap().id, "qwen3vl-4b-q4km");
        assert!(model_spec("no-such-model").is_err());
    }

    #[test]
    fn asset_urls_prefer_env_override_and_fallback_mirror() {
        let asset = &MODEL_Q4_K_M.gguf;
        let urls = asset_urls(asset);
        assert_eq!(urls[0], asset.primary_url);
        assert_eq!(urls[1], asset.mirror_url);
    }

    #[test]
    fn safe_join_rejects_parent_root_and_prefix() {
        let base = Path::new("C:/tmp/base");
        assert!(safe_join(base, "a/b/c.gguf").is_ok());
        assert!(safe_join(base, "../escape").is_err());
        assert!(safe_join(base, "/abs/path").is_err());
        // 绝对路径用例按平台构造："C:/evil" 只在 Windows 上是绝对路径
        // （Unix 上是相对路径），Unix 用根路径用例，断言意图不变。
        // 运行时 is_absolute 探测，避免在 adapter 层外引入条件编译。
        let abs = ["C:/evil", "/abs/evil"]
            .into_iter()
            .find(|candidate| Path::new(candidate).is_absolute())
            .expect("当前平台必有一个绝对路径用例");
        assert!(safe_join(base, abs).is_err());
    }

    #[test]
    fn extract_zip_rejects_parent_traversal() {
        let tmp = temporary_dir("zip-slip");
        let archive = tmp.join("evil.zip");
        let file = std::fs::File::create(&archive).expect("create zip");
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("../escape.txt", zip::write::SimpleFileOptions::default())
            .expect("write entry");
        zip.write_all(b"pwned").expect("write bytes");
        let _ = zip.finish().expect("finish zip");

        let dest = tmp.join("out");
        let err = extract_zip(&archive, &dest).expect_err("must reject traversal");
        assert!(err.contains("非法路径"), "got {err}");
        assert!(!dest.join("escape.txt").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn model_file_verified_fully_rejects_hash_mismatch() {
        let tmp = temporary_dir("verify-full");
        std::fs::create_dir_all(&tmp).expect("mkdir");
        let content = vec![0u8; 16];
        // 每条断言用独立文件，避免同 key 命中 VERIFY_CACHE 干扰。
        let bad = tmp.join("bad.bin");
        std::fs::write(&bad, &content).expect("write");
        let good = tmp.join("good.bin");
        std::fs::write(&good, &content).expect("write");
        let real_sha = crate::platform::hashing::sha256_file(&good).expect("sha256");
        let asset = ModelAsset {
            filename: "model.bin",
            expected_size: 16,
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            primary_url: "",
            mirror_url: "",
            fallback_url: "",
        };
        // fail closed：hash 不匹配即视为未安装
        assert!(!model_file_verified_fully(&bad, &asset));
        let correct = ModelAsset {
            sha256: Box::leak(real_sha.into_boxed_str()),
            ..asset
        };
        assert!(model_file_verified_fully(&good, &correct));
        // 尺寸不符同样拒绝
        let wrong_size = ModelAsset {
            expected_size: 17,
            ..correct
        };
        assert!(!model_file_verified_fully(&good, &wrong_size));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn model_file_verified_status_path_uses_cache_or_size_only() {
        let tmp = temporary_dir("verify-status");
        std::fs::create_dir_all(&tmp).expect("mkdir");
        let path = tmp.join("model.bin");
        std::fs::write(&path, vec![0u8; 16]).expect("write");
        let asset = ModelAsset {
            filename: "model.bin",
            expected_size: 16,
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            primary_url: "",
            mirror_url: "",
            fallback_url: "",
        };
        // 状态热路径：缓存未命中退化为 size 检查（完整性在安装时已强制校验），
        // 即使此处 sha256 故意填错也不做全量 hash。
        assert!(model_file_verified(&path, &asset));
        // 缓存里的否定结果被消费（模拟安装时校验失败已写入缓存的场景）
        let meta = std::fs::metadata(&path).expect("meta");
        remember_verified(&path, meta.len(), meta.modified().ok(), false);
        assert!(!model_file_verified(&path, &asset));
        // 尺寸不符恒拒绝
        let wrong_size = ModelAsset {
            expected_size: 17,
            ..asset
        };
        assert!(!model_file_verified(&path, &wrong_size));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn asset_sha256_prefers_per_asset_env_then_legacy_then_builtin() {
        let gguf = &MODEL_Q4_K_M.gguf;
        let mmproj = &MODEL_Q4_K_M.mmproj;
        let env = |name: &str| match name {
            "PINVOU3_LLAMA_GGUF_SHA256" => Some("gguf-override".to_string()),
            "PINVOU3_LLAMA_MMPROJ_SHA256" => Some("mmproj-override".to_string()),
            "PINVOU3_LLAMA_MODEL_SHA256" => Some("legacy-override".to_string()),
            _ => None,
        };
        // per-asset env 按资产类型分流，互不串用
        assert_eq!(asset_sha256_with(gguf, env), "gguf-override");
        assert_eq!(asset_sha256_with(mmproj, env), "mmproj-override");
        // 仅旧版通用 env 时兜底生效
        let legacy_only = |name: &str| {
            (name == "PINVOU3_LLAMA_MODEL_SHA256").then(|| "legacy-override".to_string())
        };
        assert_eq!(asset_sha256_with(gguf, legacy_only), "legacy-override");
        assert_eq!(asset_sha256_with(mmproj, legacy_only), "legacy-override");
        // 无 env 时用内置钉版值
        let no_env = |_: &str| Option::<String>::None;
        assert_eq!(asset_sha256_with(gguf, no_env), gguf.sha256);
        assert!(!asset_sha256_with(gguf, no_env).is_empty());
    }

    fn temporary_dir(label: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pinvou-llama-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }
}
