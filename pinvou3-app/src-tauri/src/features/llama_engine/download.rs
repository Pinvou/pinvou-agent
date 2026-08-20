//! 引擎与视觉模型按需下载、校验与部署。
//!
//! 范式照搬 voice_asr.rs / model_download.rs：流式 reqwest + `.part` 临时文件 +
//! 进度事件 + 取消标志；模型用 sha256（env 可覆盖）+ 尺寸校验，带结果缓存；
//! 引擎（GitHub release）无固定 sha，用尺寸 + 解压结构校验，tag 记入
//! engine-meta.json。GitHub 资产不支持断点续传，失败整文件重下。
//!
//! 进度事件 `llama-engine:progress` payload：
//! `{ stage: engine_download|engine_extract|model_download|model_verify|done|cancelled,
//!    item: engine|model|mmproj, modelId, filename, downloaded, total }`

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use sha2::{Digest, Sha256};
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
    /// 空串 = 跳过 sha256 校验（发布后回填实测值；dev 本地包用 env 覆盖）。
    pub sha256: &'static str,
    /// 首个尝试源：ModelScope（中国大陆网络最快，参考示例同源）。
    pub primary_url: &'static str,
    /// 备用源 1：HuggingFace 官方。
    pub mirror_url: &'static str,
    /// 备用源 2：HuggingFace 国内镜像（仅中国大陆网络生效）。
    pub fallback_url: &'static str,
}

/// 极致低配档：Qwen3-VL-2B IQ2_M（bartowski 转换，实测 663MB）。
/// ⚠️ 跨仓混用：主权重来自 bartowski 转换仓、mmproj 来自官方仓，两者均由
/// 同源 bf16 权重转出、量化格式兼容；上线前需真机冒烟确认组合可用。
pub(crate) const MODEL_IQ2_M: LlamaModelSpec = LlamaModelSpec {
    id: "qwen3vl-2b-iq2m",
    display_name: "Qwen3-VL-2B IQ2_M（1.14GB，极致低配）",
    size_bytes: 695_182_656 + 445_053_216,
    gguf: ModelAsset {
        filename: "Qwen_Qwen3-VL-2B-Instruct-IQ2_M.gguf",
        expected_size: 695_182_656,
        // HF API 未取到该文件 lfs oid，先以尺寸校验兜底（发布后回填实测 sha256）。
        sha256: "",
        primary_url: "https://modelscope.cn/models/bartowski/Qwen_Qwen3-VL-2B-Instruct-GGUF/resolve/master/Qwen_Qwen3-VL-2B-Instruct-IQ2_M.gguf",
        mirror_url: "https://huggingface.co/bartowski/Qwen_Qwen3-VL-2B-Instruct-GGUF/resolve/main/Qwen_Qwen3-VL-2B-Instruct-IQ2_M.gguf",
        fallback_url: "https://hf-mirror.com/bartowski/Qwen_Qwen3-VL-2B-Instruct-GGUF/resolve/main/Qwen_Qwen3-VL-2B-Instruct-IQ2_M.gguf",
    },
    mmproj: MMPROJ_2B_Q8_0,
};

/// 2B 官方仓 Q8_0 视觉投影器（IQ2_M 与 Q4_K_M 两档共用，磁盘上只存一份）。
/// 相比 F16（819MB）省约 46%，视觉编码精度损失可忽略。
const MMPROJ_2B_Q8_0: ModelAsset = ModelAsset {
    filename: "mmproj-Qwen3VL-2B-Instruct-Q8_0.gguf",
    expected_size: 445_053_216,
    sha256: "",
    primary_url: "https://modelscope.cn/models/Qwen/Qwen3-VL-2B-Instruct-GGUF/resolve/master/mmproj-Qwen3VL-2B-Instruct-Q8_0.gguf",
    mirror_url: "https://huggingface.co/Qwen/Qwen3-VL-2B-Instruct-GGUF/resolve/main/mmproj-Qwen3VL-2B-Instruct-Q8_0.gguf",
    fallback_url: "https://hf-mirror.com/Qwen/Qwen3-VL-2B-Instruct-GGUF/resolve/main/mmproj-Qwen3VL-2B-Instruct-Q8_0.gguf",
};

/// 旧版档（legacy）：Qwen3-VL-2B Q3_K_S + F16 mmproj（bartowski 转换）。
/// 仅为兼容已下载的老安装保留，不再推荐；新装请用 q4km / iq2m。
pub(crate) const MODEL_Q3_K_S: LlamaModelSpec = LlamaModelSpec {
    id: "qwen3vl-2b-q3k-s",
    display_name: "Qwen3-VL-2B Q3_K_S（1.61GB，旧版不推荐）",
    size_bytes: 867_253_568 + 819_394_848,
    gguf: ModelAsset {
        filename: "Qwen_Qwen3-VL-2B-Instruct-Q3_K_S.gguf",
        expected_size: 867_253_568,
        sha256: "",
        primary_url: "https://modelscope.cn/models/bartowski/Qwen_Qwen3-VL-2B-Instruct-GGUF/resolve/master/Qwen_Qwen3-VL-2B-Instruct-Q3_K_S.gguf",
        mirror_url: "https://huggingface.co/bartowski/Qwen_Qwen3-VL-2B-Instruct-GGUF/resolve/main/Qwen_Qwen3-VL-2B-Instruct-Q3_K_S.gguf",
        fallback_url: "https://hf-mirror.com/bartowski/Qwen_Qwen3-VL-2B-Instruct-GGUF/resolve/main/Qwen_Qwen3-VL-2B-Instruct-Q3_K_S.gguf",
    },
    mmproj: ModelAsset {
        filename: "mmproj-Qwen_Qwen3-VL-2B-Instruct-f16.gguf",
        expected_size: 819_394_848,
        sha256: "",
        primary_url: "https://modelscope.cn/models/bartowski/Qwen_Qwen3-VL-2B-Instruct-GGUF/resolve/master/mmproj-Qwen_Qwen3-VL-2B-Instruct-f16.gguf",
        mirror_url: "https://huggingface.co/bartowski/Qwen_Qwen3-VL-2B-Instruct-GGUF/resolve/main/mmproj-Qwen_Qwen3-VL-2B-Instruct-f16.gguf",
        fallback_url: "https://hf-mirror.com/bartowski/Qwen_Qwen3-VL-2B-Instruct-GGUF/resolve/main/mmproj-Qwen_Qwen3-VL-2B-Instruct-f16.gguf",
    },
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
        sha256: "",
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
        sha256: "",
        primary_url: "https://modelscope.cn/models/Qwen/Qwen3-VL-4B-Instruct-GGUF/resolve/master/Qwen3VL-4B-Instruct-Q4_K_M.gguf",
        mirror_url: "https://huggingface.co/Qwen/Qwen3-VL-4B-Instruct-GGUF/resolve/main/Qwen3VL-4B-Instruct-Q4_K_M.gguf",
        fallback_url: "https://hf-mirror.com/Qwen/Qwen3-VL-4B-Instruct-GGUF/resolve/main/Qwen3VL-4B-Instruct-Q4_K_M.gguf",
    },
    mmproj: ModelAsset {
        filename: "mmproj-Qwen3VL-4B-Instruct-Q8_0.gguf",
        expected_size: 453_974_304,
        sha256: "",
        primary_url: "https://modelscope.cn/models/Qwen/Qwen3-VL-4B-Instruct-GGUF/resolve/master/mmproj-Qwen3VL-4B-Instruct-Q8_0.gguf",
        mirror_url: "https://huggingface.co/Qwen/Qwen3-VL-4B-Instruct-GGUF/resolve/main/mmproj-Qwen3VL-4B-Instruct-Q8_0.gguf",
        fallback_url: "https://hf-mirror.com/Qwen/Qwen3-VL-4B-Instruct-GGUF/resolve/main/mmproj-Qwen3VL-4B-Instruct-Q8_0.gguf",
    },
};

/// 档位顺序即设置页展示顺序：极致低配 → 默认 → 独显 → legacy 垫底。
const MODEL_SPECS: &[LlamaModelSpec] = &[
    MODEL_IQ2_M,
    MODEL_Q4_K_M,
    MODEL_4B_Q4_K_M,
    MODEL_Q3_K_S,
];

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
/// GitHub latest API 不可达时的兜底 tag（资产 404 时安装会给出明确错误，
/// 可用 `PINVOU3_LLAMA_ENGINE_TAG` 指定其它版本）。
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
    value.get("tag").and_then(|v| v.as_str()).map(str::to_string)
}

fn engine_installed_with_tag(tag: &str) -> bool {
    engine_installed() && engine_tag().as_deref() == Some(tag)
}

/// 解析引擎版本：env `PINVOU3_LLAMA_ENGINE_TAG` > GitHub latest API > PINNED_ENGINE_TAG。
pub(crate) async fn resolve_engine_tag() -> Result<String, String> {
    if let Ok(tag) = std::env::var("PINVOU3_LLAMA_ENGINE_TAG") {
        if !tag.trim().is_empty() {
            return Ok(tag.trim().to_string());
        }
    }
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
    for dir in [llama_engine_dir(), bin_dir(), tmp_dir()] {
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {e}"))?;
    }

    let archive = tmp_dir().join(&asset_name);
    let part = tmp_dir().join(format!("{asset_name}.part"));
    let _ = std::fs::remove_file(&part);
    let _ = std::fs::remove_file(&archive);
    download_file(app, &url, &part, "engine_download", "engine", None, None, 0).await?;

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
    let entries =
        std::fs::read_dir(extract_dir).map_err(|e| format!("读取解压目录失败: {e}"))?;
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

/// 只搬 llama-server + 同目录共享库（不拷贝其它平台目录）。
fn swap_engine_files(src_dir: &Path, dest_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| format!("创建引擎目录失败: {e}"))?;
    let mut copied = 0usize;
    for entry in std::fs::read_dir(src_dir).map_err(|e| format!("读取引擎目录失败: {e}"))? {
        let entry = entry.map_err(|e| format!("读取引擎目录条目失败: {e}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name();
        let dest = dest_dir.join(&name);
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
    if model_file_verified(&dest, asset) {
        return Ok(());
    }
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
                    Err(error) => return Err(error),
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
    if let Ok(url) = std::env::var("PINVOU3_LLAMA_MODEL_URL") {
        if !url.trim().is_empty() {
            return vec![url];
        }
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

static DOWNLOADING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
static DOWNLOAD_ITEM: Mutex<Option<&'static str>> = Mutex::new(None);

const PROGRESS_EMIT_BYTES: u64 = 2 * 1024 * 1024;
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

struct DownloadGuard;

impl Drop for DownloadGuard {
    fn drop(&mut self) {
        DOWNLOADING.store(false, Ordering::SeqCst);
        *DOWNLOAD_ITEM.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

fn acquire_download(item: &'static str) -> Result<DownloadGuard, String> {
    if DOWNLOADING.swap(true, Ordering::SeqCst) {
        return Err("已有下载任务进行中".to_string());
    }
    *DOWNLOAD_ITEM.lock().unwrap_or_else(|e| e.into_inner()) = Some(item);
    CANCEL.store(false, Ordering::SeqCst);
    Ok(DownloadGuard)
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
    let mut file =
        std::fs::File::create(dest).map_err(|e| format!("创建文件失败: {e}"))?;
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
        file.write_all(&chunk).map_err(|e| format!("写盘失败: {e}"))?;
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

/// 尺寸 + sha256 校验后原子改名落盘。
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
    if let Some(expected) = asset_sha256(asset) {
        let got = sha256_file(tmp)?;
        if !got.eq_ignore_ascii_case(&expected) {
            let _ = std::fs::remove_file(tmp);
            return Err(format!(
                "模型校验失败(sha256 不匹配): 期望 {expected:.12} 实际 {got:.12}"
            ));
        }
    }
    std::fs::rename(tmp, dest).map_err(|e| format!("落盘模型文件失败: {e}"))?;
    Ok(())
}

fn asset_sha256(asset: &ModelAsset) -> Option<String> {
    std::env::var("PINVOU3_LLAMA_MODEL_SHA256")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| (!asset.sha256.is_empty()).then(|| asset.sha256.to_string()))
}

/// 校验缓存（按 path + len + modified 失效；下载后替换文件会自然失效）。
type VerifyKey = (PathBuf, u64, Option<std::time::SystemTime>);
static VERIFY_CACHE: OnceLock<Mutex<HashMap<VerifyKey, bool>>> = OnceLock::new();

pub(crate) fn model_file_verified(path: &Path, asset: &ModelAsset) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if asset.expected_size > 0 && meta.len() != asset.expected_size {
        return false;
    }
    let Some(expected) = asset_sha256(asset) else {
        return true;
    };
    let key = (
        path.to_path_buf(),
        meta.len(),
        meta.modified().ok(),
    );
    if let Some(cached) = VERIFY_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
        .copied()
    {
        return cached;
    }
    let Ok(got) = sha256_file(path) else {
        return false;
    };
    let ok = got.eq_ignore_ascii_case(&expected);
    VERIFY_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, ok);
    ok
}

pub(crate) fn model_files_verified(spec: &LlamaModelSpec) -> bool {
    model_file_verified(&model_gguf_path(spec), &spec.gguf)
        && model_file_verified(&mmproj_path(spec), &spec.mmproj)
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
    for entry in tar
        .entries()
        .map_err(|e| format!("读取压缩包失败: {e}"))?
    {
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

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("打开文件失败: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| format!("读取文件失败: {e}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(crate::platform::encoding::hex_lower(&hasher.finalize()))
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
            assert!(spec.gguf.primary_url.starts_with("https://"));
            assert!(spec.gguf.mirror_url.starts_with("https://"));
            assert!(spec.mmproj.primary_url.starts_with("https://"));
        }
        assert_eq!(default_model().id, MODEL_Q3_K_S.id);
        assert!(MODEL_Q3_K_S.size_bytes > 0);
    }

    #[test]
    fn model_spec_resolves_known_and_rejects_unknown() {
        assert_eq!(model_spec("qwen3vl-2b-q3k-s").unwrap().id, "qwen3vl-2b-q3k-s");
        assert_eq!(model_spec("qwen3vl-2b-q4km").unwrap().id, "qwen3vl-2b-q4km");
        assert!(model_spec("no-such-model").is_err());
    }

    #[test]
    fn asset_urls_prefer_env_override_and_fallback_mirror() {
        let asset = &MODEL_Q3_K_S.gguf;
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
        assert!(safe_join(base, "C:/evil").is_err());
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
    fn model_file_verified_skips_sha_when_empty_and_checks_size() {
        let tmp = temporary_dir("verify");
        let path = tmp.join("model.bin");
        std::fs::write(&path, vec![0u8; 16]).expect("write");
        let asset = ModelAsset {
            filename: "model.bin",
            expected_size: 16,
            sha256: "",
            primary_url: "",
            mirror_url: "",
            fallback_url: "",
        };
        assert!(model_file_verified(&path, &asset));
        let wrong = ModelAsset { expected_size: 17, ..asset };
        assert!(!model_file_verified(&path, &wrong));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    fn temporary_dir(label: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "pinvou-llama-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
