//! BGE-M3 模型清单与下载实现。
//!
//! 桌面本地知识库和共享知识库服务都通过本模块下载同一固定 revision 的模型文件，
//! 避免两端分别维护下载地址、摘要和目录布局。

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use url::Url;

pub const KNOWLEDGE_MODEL_HF_BASE_URL: &str = "https://huggingface.co";
/// 国内可达的 Hugging Face 兼容镜像（路径结构与官方源完全一致，默认首选）。
pub const KNOWLEDGE_MODEL_HF_MIRROR_BASE_URL: &str = "https://hf-mirror.com";
pub const KNOWLEDGE_MODEL_HF_REPOSITORY: &str = "onnx-community/bge-m3-ONNX";
pub const KNOWLEDGE_MODEL_HF_REVISION: &str = "25b9af8e87a38eb120cfe87125383677b9cd309e";
pub const KNOWLEDGE_MODEL_HF_BASE_URL_ENV: &str = "PINVOU_KNOWLEDGE_HF_BASE_URL";
pub const KNOWLEDGE_MODEL_DOWNLOAD_BYTES: u64 = 585_565_019;

/// 取消状态的统一报文（面向用户的报错文案，各取消检查点共用）。回退循环
/// 以 `is_cancelled` 标志位（而非错误字符串比对）区分「用户取消」（终止整体
/// 流程）与「单个镜像源故障」（换下一个基地址重试），标志位不依赖本常量。
const CANCELLED: &str = "已取消";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnowledgeModelFile {
    /// 固定 revision 内的源文件路径。
    pub source_path: &'static str,
    /// 候选模型目录内的落盘路径。
    pub destination_path: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

pub const KNOWLEDGE_MODEL_FILES: [KnowledgeModelFile; 5] = [
    KnowledgeModelFile {
        source_path: "onnx/model_int8.onnx",
        destination_path: "model.onnx",
        bytes: 568_479_395,
        sha256: "2237f770aad5c71bbc1fc2d361a57f9a37400574cc9eff32626f0cdb49234730",
    },
    KnowledgeModelFile {
        source_path: "tokenizer.json",
        destination_path: "tokenizer.json",
        bytes: 17_082_799,
        sha256: "249df0778f236f6ece390de0de746838ef25b9d6954b68c2ee71249e0a9d8fd4",
    },
    KnowledgeModelFile {
        source_path: "config.json",
        destination_path: "config.json",
        bytes: 658,
        sha256: "70dae5884ced999af00244f776ac9eaa71538d68497d3d6a6091e0318cd32905",
    },
    KnowledgeModelFile {
        source_path: "tokenizer_config.json",
        destination_path: "tokenizer_config.json",
        bytes: 1_203,
        sha256: "b87c8703482b0300d3da30e201519aa641f6a450f5eb5bf1e624afbf70c74d80",
    },
    KnowledgeModelFile {
        source_path: "special_tokens_map.json",
        destination_path: "special_tokens_map.json",
        bytes: 964,
        sha256: "8c785abebea9ae3257b61681b4e6fd8365ceafde980c21970d001e834cf10835",
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnowledgeModelDownloadStage {
    Download,
    Verify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnowledgeModelDownloadProgress {
    pub stage: KnowledgeModelDownloadStage,
    /// 整份清单累计完成的下载字节数。
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    /// 从 1 开始的当前文件序号。
    pub file_index: usize,
    pub file_count: usize,
    pub source_path: &'static str,
}

/// 返回按序尝试的 Hugging Face 兼容镜像基地址列表。
///
/// 显式设置 [`KNOWLEDGE_MODEL_HF_BASE_URL_ENV`] 时只返回该地址——用户明确指定的
/// 源不做回退；否则先试国内镜像（[`KNOWLEDGE_MODEL_HF_MIRROR_BASE_URL`]），失败
/// 再回退官方源。每个文件都逐基地址重试，内容始终经逐文件 SHA-256 校验。
pub fn knowledge_model_hf_base_url_candidates() -> Vec<String> {
    ordered_hf_base_url_candidates(
        std::env::var(KNOWLEDGE_MODEL_HF_BASE_URL_ENV)
            .ok()
            .filter(|value| !value.trim().is_empty()),
    )
}

/// [`knowledge_model_hf_base_url_candidates`] 的纯函数核心（便于单测，不触环境变量）。
fn ordered_hf_base_url_candidates(explicit: Option<String>) -> Vec<String> {
    match explicit {
        Some(value) => vec![value],
        None => vec![
            KNOWLEDGE_MODEL_HF_MIRROR_BASE_URL.to_string(),
            KNOWLEDGE_MODEL_HF_BASE_URL.to_string(),
        ],
    }
}

/// 将固定 revision 的五个文件下载并逐一校验到一个新建的候选目录。
///
/// `candidate` 必须不存在。任何失败或取消都会清理本次创建的候选目录；调用方在
/// 返回成功后负责真实加载候选模型，并将其原子替换到正式目录。
///
/// `hf_base_urls` 是按序尝试的镜像基地址列表（见
/// [`knowledge_model_hf_base_url_candidates`]）：单个文件在某个基地址上下载或
/// 校验失败时，自动换下一个基地址重试，全部失败才整体报错。
pub async fn download_knowledge_model_candidate<P, C>(
    candidate: &Path,
    hf_base_urls: &[String],
    on_progress: P,
    is_cancelled: C,
) -> Result<(), String>
where
    P: FnMut(KnowledgeModelDownloadProgress) + Send,
    C: Fn() -> bool + Send + Sync,
{
    crate::ensure_tls_crypto_provider();
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(90))
        .timeout(Duration::from_secs(3 * 60 * 60))
        // 重定向只跟随 HTTPS 目标（与 connectors / marketplace 下载路径的
        // 策略同口径）：镜像被劫持时不得把 586MB 模型流重定向到明文 HTTP。
        // 跨源 HTTPS 重定向仍允许——hf-mirror 现阶段会把 /resolve/ 308 到
        // 官方源，落盘字节始终经 SHA-256 门禁。
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if hf_redirect_follow_allowed(attempt.previous().len(), attempt.url().scheme()) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .user_agent(concat!("pinvou-knowledge/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| format!("无法创建模型下载客户端: {error}"))?;
    download_knowledge_model_candidate_with(
        &client,
        candidate,
        hf_base_urls,
        &KNOWLEDGE_MODEL_FILES,
        on_progress,
        is_cancelled,
    )
    .await
}

/// Check whether a model directory contains the ONNX and tokenizer files
/// PINVOU needs at runtime.
///
/// The directory may come from caller configuration (e.g. a server CLI flag),
/// so canonicalize it first: the existence checks then answer for the
/// directory the path actually resolves to, and a missing directory keeps the
/// incomplete semantics. Symlinked directories are followed on purpose, like
/// every other file operation on this path: there is no trusted root to
/// enforce here (versioned symlink layouts such as `current -> models/v3`
/// are a supported override shape), so the probe only reports incomplete for
/// paths that do not resolve.
pub fn model_directory_is_complete(dir: &Path) -> bool {
    let Ok(dir) = std::fs::canonicalize(dir) else {
        return false;
    };
    let onnx = dir.join("model.onnx").is_file()
        || dir.join("onnx").join("model_int8.onnx").is_file()
        || dir.join("onnx").join("model.onnx").is_file();
    onnx && [
        "tokenizer.json",
        "config.json",
        "special_tokens_map.json",
        "tokenizer_config.json",
    ]
    .iter()
    .all(|file| dir.join(file).is_file())
}

/// 恢复上次在目录切换窗口中中断的模型安装，并清理旧版服务遗留的随机备份。
pub fn recover_model_directory(destination: &Path) -> Result<Option<String>, String> {
    let backup = destination.with_extension("backup");
    if backup.exists() {
        if destination.exists() {
            std::fs::remove_dir_all(&backup).map_err(|error| {
                format!("清理上次遗留的模型备份失败({}): {error}", backup.display())
            })?;
        } else {
            std::fs::rename(&backup, destination).map_err(|error| {
                format!(
                    "恢复上次中断留下的模型备份失败({} -> {}): {error}",
                    backup.display(),
                    destination.display()
                )
            })?;
        }
    }

    let Some(parent) = destination.parent() else {
        return Ok(None);
    };
    if !parent.exists() {
        return Ok(None);
    }
    let Some(name) = destination.file_name().and_then(|value| value.to_str()) else {
        return Ok(None);
    };
    let legacy_prefix = format!(".{name}.backup-");
    let mut legacy = std::fs::read_dir(parent)
        .map_err(|error| format!("无法检查模型父目录({}): {error}", parent.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.starts_with(&legacy_prefix))
        })
        .collect::<Vec<_>>();
    legacy.sort();
    if !destination.exists() {
        if legacy.len() == 1 {
            std::fs::rename(&legacy[0], destination).map_err(|error| {
                format!(
                    "恢复旧版中断留下的模型备份失败({} -> {}): {error}",
                    legacy[0].display(),
                    destination.display()
                )
            })?;
            legacy.clear();
        } else if !legacy.is_empty() {
            return Err("发现多份旧版模型备份，无法安全判断应恢复哪一份".to_string());
        }
    }
    let failed = legacy
        .into_iter()
        .filter_map(|path| {
            std::fs::remove_dir_all(&path)
                .err()
                .map(|error| format!("{}: {error}", path.display()))
        })
        .collect::<Vec<_>>();
    Ok((!failed.is_empty()).then(|| format!("清理旧版模型备份失败：{}", failed.join("；"))))
}

/// 将已通过真实加载验证的候选目录原子换入正式目录，失败时恢复旧模型。
pub fn install_model_candidate(
    candidate: &Path,
    destination: &Path,
) -> Result<Option<String>, String> {
    let recovery_warning = recover_model_directory(destination)?;
    let backup = destination.with_extension("backup");
    let had_destination = destination.exists();
    if had_destination {
        std::fs::rename(destination, &backup)
            .map_err(|error| format!("备份现有模型失败: {error}"))?;
    }
    if let Err(error) = std::fs::rename(candidate, destination) {
        if had_destination && let Err(rollback_error) = std::fs::rename(&backup, destination) {
            return Err(format!(
                "部署模型失败: {error}; 回滚旧模型也失败: {rollback_error}; 旧模型仍保留在 {}",
                backup.display()
            ));
        }
        return Err(format!("部署模型失败: {error}"));
    }
    let cleanup_warning = had_destination
        .then(|| std::fs::remove_dir_all(&backup))
        .and_then(Result::err)
        .map(|error| {
            format!(
                "新模型已部署，但清理旧模型备份失败({}): {error}",
                backup.display()
            )
        });
    Ok(match (recovery_warning, cleanup_warning) {
        (Some(left), Some(right)) => Some(format!("{left}；{right}")),
        (Some(warning), None) | (None, Some(warning)) => Some(warning),
        (None, None) => None,
    })
}

async fn download_knowledge_model_candidate_with<P, C>(
    client: &reqwest::Client,
    candidate: &Path,
    hf_base_urls: &[String],
    manifest: &[KnowledgeModelFile],
    mut on_progress: P,
    is_cancelled: C,
) -> Result<(), String>
where
    P: FnMut(KnowledgeModelDownloadProgress) + Send,
    C: Fn() -> bool + Send + Sync,
{
    // 全部基地址先整体校验：任何一个镜像配置非法都在触网前失败，
    // 不允许「前两个基地址下了一半，第三个才发现配置写错」。
    if hf_base_urls.is_empty() {
        return Err("镜像基地址列表为空".to_string());
    }
    let mut base_urls = Vec::with_capacity(hf_base_urls.len());
    for value in hf_base_urls {
        base_urls.push(validate_hf_base_url(value)?);
    }
    let total_bytes = manifest.iter().map(|file| file.bytes).sum();
    let parent = candidate
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("无法创建模型父目录({}): {error}", parent.display()))?;
    std::fs::create_dir(candidate).map_err(|error| {
        format!(
            "无法创建模型候选目录({}，目录必须不存在): {error}",
            candidate.display()
        )
    })?;

    let result = async {
        let mut completed_bytes = 0_u64;
        for (index, file) in manifest.iter().enumerate() {
            if is_cancelled() {
                return Err(CANCELLED.to_string());
            }
            let destination = safe_candidate_path(candidate, file.destination_path)?;
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    format!("无法创建模型文件目录({}): {error}", parent.display())
                })?;
            }
            let partial = destination.with_extension(format!(
                "{}.part",
                destination
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or("download")
            ));

            // 按序尝试各镜像基地址：当前基地址上下载或校验失败（含镜像内容被
            // 篡改导致的 SHA-256 不符）都换下一个重试；全部失败才整体报错，
            // 错误里带上源域名与已耗尽的源数，否则镜像被篡改这类失败会被误读
            // 成官方源故障。取消是全局意图，任何一处出现都立即终止，不当作
            // 镜像故障。
            let mut failures: Vec<String> = Vec::new();
            let mut succeeded = false;
            // 换基地址重试时该文件从头下载，进度若原样透传，前端看到的累计
            // 字节会倒退（进度条回跳）；用跨基地址的峰值钳制保持单调不减。
            let mut peak_downloaded = completed_bytes;
            for base_url in &base_urls {
                if is_cancelled() {
                    return Err(CANCELLED.to_string());
                }
                // 上一个基地址的半截 `.part` 必须清掉再重试，避免续写混杂来源的字节。
                let _ = std::fs::remove_file(&partial);
                let url = knowledge_model_file_url(base_url, file.source_path)?;
                let mut on_progress = |event: KnowledgeModelDownloadProgress| {
                    peak_downloaded = peak_downloaded.max(event.downloaded_bytes);
                    on_progress(KnowledgeModelDownloadProgress {
                        downloaded_bytes: peak_downloaded,
                        ..event
                    });
                };
                match download_and_verify_manifest_file(
                    client,
                    &url,
                    &partial,
                    &destination,
                    file,
                    completed_bytes,
                    total_bytes,
                    index,
                    manifest.len(),
                    &mut on_progress,
                    &is_cancelled,
                )
                .await
                {
                    Ok(()) => {
                        succeeded = true;
                        break;
                    }
                    // 取消是全局意图：标志位一旦置位就整体终止（即便本次错误
                    // 本身不是取消报文），不当作镜像故障换下一基地址。
                    Err(_) if is_cancelled() => return Err(CANCELLED.to_string()),
                    Err(error) => {
                        let host = match base_url.port() {
                            // 非默认端口写进前缀，避免同机多端口候选在报错里
                            // 无法区分；默认端口省略（与 URL 显示习惯一致）。
                            Some(port) => {
                                format!(
                                    "{}:{port}",
                                    base_url.host_str().unwrap_or_else(|| base_url.as_str())
                                )
                            }
                            None => base_url
                                .host_str()
                                .unwrap_or_else(|| base_url.as_str())
                                .to_string(),
                        };
                        failures.push(format!("[{host}] {error}"));
                    }
                }
            }
            if !succeeded {
                // base_urls 非空且每次失败都会写入 failures；兜底文案不得伪称
                // 「已取消」。
                let detail = failures.join("；");
                return Err(if base_urls.len() > 1 {
                    format!("{} 个下载源均失败：{detail}", base_urls.len())
                } else if detail.is_empty() {
                    "模型下载失败".to_string()
                } else {
                    detail
                });
            }

            if is_cancelled() {
                return Err(CANCELLED.to_string());
            }
            completed_bytes += file.bytes;
        }
        Ok(())
    }
    .await;

    if result.is_err() {
        let _ = std::fs::remove_dir_all(candidate);
    }
    result
}

/// 单文件在单一基地址上的完整尝试：下载到 `.part` → verify 进度事件 →
/// SHA-256 校验 → 原子 `rename` 到 `destination`。任一步失败都返回 `Err`，
/// 由调用方决定是否换下一个基地址重试。
#[allow(clippy::too_many_arguments)]
async fn download_and_verify_manifest_file<P, C>(
    client: &reqwest::Client,
    url: &Url,
    partial: &Path,
    destination: &Path,
    file: &KnowledgeModelFile,
    completed_bytes: u64,
    total_bytes: u64,
    file_index: usize,
    file_count: usize,
    on_progress: &mut P,
    is_cancelled: &C,
) -> Result<(), String>
where
    P: FnMut(KnowledgeModelDownloadProgress) + Send,
    C: Fn() -> bool + Send + Sync,
{
    download_manifest_file(
        client,
        url,
        partial,
        file,
        completed_bytes,
        total_bytes,
        file_index,
        file_count,
        on_progress,
        is_cancelled,
    )
    .await?;

    if is_cancelled() {
        return Err(CANCELLED.to_string());
    }
    on_progress(KnowledgeModelDownloadProgress {
        stage: KnowledgeModelDownloadStage::Verify,
        downloaded_bytes: completed_bytes + file.bytes,
        total_bytes,
        file_index: file_index + 1,
        file_count,
        source_path: file.source_path,
    });
    let verify_path = partial.to_path_buf();
    let actual = tokio::task::spawn_blocking(move || sha256_file(&verify_path))
        .await
        .map_err(|error| format!("模型校验任务失败: {error}"))??;
    if !actual.eq_ignore_ascii_case(file.sha256) {
        return Err(format!(
            "模型文件校验失败({}): 期望 {}，实际 {}",
            file.source_path, file.sha256, actual
        ));
    }
    if is_cancelled() {
        return Err(CANCELLED.to_string());
    }
    std::fs::rename(partial, destination).map_err(|error| {
        format!(
            "无法完成模型文件写入({} -> {}): {error}",
            partial.display(),
            destination.display()
        )
    })?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn download_manifest_file<P, C>(
    client: &reqwest::Client,
    url: &Url,
    destination: &Path,
    manifest_file: &KnowledgeModelFile,
    completed_bytes: u64,
    total_bytes: u64,
    file_index: usize,
    file_count: usize,
    on_progress: &mut P,
    is_cancelled: &C,
) -> Result<(), String>
where
    P: FnMut(KnowledgeModelDownloadProgress) + Send,
    C: Fn() -> bool + Send + Sync,
{
    let response = client
        .get(url.clone())
        .send()
        .await
        .map_err(|error| format!("连接模型源失败({}): {error}", manifest_file.source_path))?
        .error_for_status()
        .map_err(|error| format!("模型源响应异常({}): {error}", manifest_file.source_path))?;
    if let Some(actual) = response.content_length()
        && actual != manifest_file.bytes
    {
        return Err(format!(
            "模型文件大小不符({}): 期望 {} 字节，服务端返回 {} 字节",
            manifest_file.source_path, manifest_file.bytes, actual
        ));
    }

    let mut output = tokio::fs::File::create(destination)
        .await
        .map_err(|error| format!("无法创建模型文件({}): {error}", destination.display()))?;
    let mut stream = response.bytes_stream();
    let mut file_bytes = 0_u64;
    let mut last_emitted = 0_u64;
    while let Some(chunk) = stream.next().await {
        if is_cancelled() {
            return Err(CANCELLED.to_string());
        }
        let chunk = chunk
            .map_err(|error| format!("模型下载中断({}): {error}", manifest_file.source_path))?;
        file_bytes = file_bytes
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| "模型文件大小溢出".to_string())?;
        if file_bytes > manifest_file.bytes {
            return Err(format!(
                "模型文件超过预期大小({}): 期望 {} 字节",
                manifest_file.source_path, manifest_file.bytes
            ));
        }
        output
            .write_all(&chunk)
            .await
            .map_err(|error| format!("写入模型文件失败({}): {error}", destination.display()))?;
        if file_bytes.saturating_sub(last_emitted) >= 2 * 1024 * 1024
            || file_bytes == manifest_file.bytes
        {
            last_emitted = file_bytes;
            on_progress(KnowledgeModelDownloadProgress {
                stage: KnowledgeModelDownloadStage::Download,
                downloaded_bytes: completed_bytes + file_bytes,
                total_bytes,
                file_index: file_index + 1,
                file_count,
                source_path: manifest_file.source_path,
            });
        }
    }
    output
        .sync_all()
        .await
        .map_err(|error| format!("同步模型文件失败({}): {error}", destination.display()))?;
    drop(output);
    if file_bytes != manifest_file.bytes {
        return Err(format!(
            "模型文件大小不符({}): 期望 {} 字节，实际 {} 字节",
            manifest_file.source_path, manifest_file.bytes, file_bytes
        ));
    }
    Ok(())
}

/// 重定向跟随判定（纯函数核心，便于单测）：只跟随 HTTPS 目标，且跳数有界。
/// 初始请求本身不受此限制（本地/测试服务器可用 HTTP），仅约束重定向链。
fn hf_redirect_follow_allowed(previous_hops: usize, scheme: &str) -> bool {
    previous_hops < 10 && scheme == "https"
}

/// 显式配置的镜像基地址可能来自共享服务端或桌面端的两个环境变量之一（校验
/// 发生在共享 crate 内，无法区分来源），错误文案同时点名两者，避免对桌面端
/// 用户误报成另一个变量。
const HF_BASE_URL_ENV_HINT: &str =
    "镜像基地址环境变量（PINVOU_KNOWLEDGE_HF_BASE_URL / PINVOU3_KB_HF_BASE_URL）";

fn validate_hf_base_url(value: &str) -> Result<Url, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{HF_BASE_URL_ENV_HINT} 不能为空"));
    }
    let mut url = Url::parse(value)
        .map_err(|error| format!("{HF_BASE_URL_ENV_HINT} 不是有效 URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(format!(
            "{HF_BASE_URL_ENV_HINT} 必须是不含账号、查询参数和片段的 HTTP(S) 基地址"
        ));
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

fn knowledge_model_file_url(base_url: &Url, source_path: &str) -> Result<Url, String> {
    let mut url = base_url.clone();
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| "Hugging Face 镜像基地址不能作为目录基址".to_string())?;
        segments.pop_if_empty();
        for segment in KNOWLEDGE_MODEL_HF_REPOSITORY.split('/') {
            segments.push(segment);
        }
        segments.push("resolve");
        segments.push(KNOWLEDGE_MODEL_HF_REVISION);
        for segment in source_path.split('/') {
            segments.push(segment);
        }
    }
    url.query_pairs_mut().append_pair("download", "true");
    Ok(url)
}

fn safe_candidate_path(candidate: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative.components().any(|component| {
            !matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err("模型清单包含不安全的落盘路径".to_string());
    }
    Ok(candidate.join(relative))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)
        .map_err(|error| format!("无法打开模型校验文件({}): {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("读取模型校验文件失败({}): {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread;

    use super::*;

    fn serve_model_files(
        bodies: Vec<&'static [u8]>,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (requests_tx, requests_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            for body in bodies {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                loop {
                    let read = stream.read(&mut buffer).unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                requests_tx
                    .send(String::from_utf8_lossy(&request).into_owned())
                    .unwrap();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(body).unwrap();
                stream.flush().unwrap();
            }
        });
        (format!("http://{address}/hf"), requests_rx, handle)
    }

    #[test]
    fn pinned_manifest_has_expected_total_and_unique_destinations() {
        assert_eq!(
            KNOWLEDGE_MODEL_FILES
                .iter()
                .map(|file| file.bytes)
                .sum::<u64>(),
            KNOWLEDGE_MODEL_DOWNLOAD_BYTES
        );
        let mut destinations = KNOWLEDGE_MODEL_FILES
            .iter()
            .map(|file| file.destination_path)
            .collect::<Vec<_>>();
        destinations.sort_unstable();
        destinations.dedup();
        assert_eq!(destinations.len(), KNOWLEDGE_MODEL_FILES.len());
        assert_eq!(KNOWLEDGE_MODEL_FILES[0].destination_path, "model.onnx");
        assert!(
            KNOWLEDGE_MODEL_FILES
                .iter()
                .all(|file| file.sha256.len() == 64)
        );
    }

    #[test]
    fn mirror_base_keeps_prefix_and_encodes_fixed_manifest_path() {
        let base = validate_hf_base_url("https://mirror.example/hf").unwrap();
        let url = knowledge_model_file_url(&base, "onnx/model_int8.onnx").unwrap();
        assert_eq!(
            url.as_str(),
            concat!(
                "https://mirror.example/hf/onnx-community/bge-m3-ONNX/resolve/",
                "25b9af8e87a38eb120cfe87125383677b9cd309e/onnx/model_int8.onnx?download=true"
            )
        );
    }

    #[test]
    fn mirror_base_rejects_ambiguous_or_unsafe_values() {
        for value in [
            "",
            "file:///tmp/models",
            "https://user:secret@example.com",
            "https://example.com?repo=other",
            "https://example.com/#fragment",
        ] {
            assert!(validate_hf_base_url(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn ordered_candidates_explicit_source_wins_over_mirror_chain() {
        // 显式环境变量 = 用户明确指定的源，不做任何回退。
        assert_eq!(
            ordered_hf_base_url_candidates(Some("https://internal.example/hf".to_string())),
            vec!["https://internal.example/hf".to_string()]
        );
        // 未指定：国内镜像优先，官方源兜底。
        assert_eq!(
            ordered_hf_base_url_candidates(None),
            vec![
                KNOWLEDGE_MODEL_HF_MIRROR_BASE_URL.to_string(),
                KNOWLEDGE_MODEL_HF_BASE_URL.to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn cancelled_download_removes_its_candidate_directory() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("candidate");
        let cancelled = AtomicBool::new(true);
        let client = reqwest::Client::new();
        let result = download_knowledge_model_candidate_with(
            &client,
            &candidate,
            &["http://127.0.0.1:9".to_string()],
            &[KnowledgeModelFile {
                source_path: "config.json",
                destination_path: "config.json",
                bytes: 2,
                sha256: "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
            }],
            |_| {},
            || cancelled.load(Ordering::Relaxed),
        )
        .await;
        assert_eq!(result.unwrap_err(), "已取消");
        assert!(!candidate.exists());
    }

    #[tokio::test]
    async fn manifest_download_verifies_files_and_reports_monotonic_cumulative_progress() {
        let (base_url, requests, server) = serve_model_files(vec![b"abc", b"{}"]);
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("candidate");
        let manifest = [
            KnowledgeModelFile {
                source_path: "onnx/model_int8.onnx",
                destination_path: "model.onnx",
                bytes: 3,
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            },
            KnowledgeModelFile {
                source_path: "config.json",
                destination_path: "config.json",
                bytes: 2,
                sha256: "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
            },
        ];
        let mut progress = Vec::new();

        download_knowledge_model_candidate_with(
            &reqwest::Client::new(),
            &candidate,
            &[base_url.clone()],
            &manifest,
            |value| progress.push(value),
            || false,
        )
        .await
        .unwrap();

        server.join().unwrap();
        assert_eq!(std::fs::read(candidate.join("model.onnx")).unwrap(), b"abc");
        assert_eq!(std::fs::read(candidate.join("config.json")).unwrap(), b"{}");
        assert_eq!(
            progress
                .iter()
                .map(|value| value.downloaded_bytes)
                .collect::<Vec<_>>(),
            vec![3, 3, 5, 5]
        );
        assert!(
            progress
                .windows(2)
                .all(|pair| pair[0].downloaded_bytes <= pair[1].downloaded_bytes)
        );
        assert_eq!(
            progress.iter().map(|value| value.stage).collect::<Vec<_>>(),
            vec![
                KnowledgeModelDownloadStage::Download,
                KnowledgeModelDownloadStage::Verify,
                KnowledgeModelDownloadStage::Download,
                KnowledgeModelDownloadStage::Verify,
            ]
        );
        let first_request = requests.recv().unwrap();
        let second_request = requests.recv().unwrap();
        assert!(first_request.contains(concat!(
            "GET /hf/onnx-community/bge-m3-ONNX/resolve/",
            "25b9af8e87a38eb120cfe87125383677b9cd309e/onnx/model_int8.onnx?download=true "
        )));
        assert!(second_request.contains("/config.json?download=true "));
    }

    #[tokio::test]
    async fn sha_mismatch_removes_candidate_directory() {
        let (base_url, _requests, server) = serve_model_files(vec![b"abc"]);
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("candidate");
        let result = download_knowledge_model_candidate_with(
            &reqwest::Client::new(),
            &candidate,
            &[base_url.clone()],
            &[KnowledgeModelFile {
                source_path: "config.json",
                destination_path: "config.json",
                bytes: 3,
                sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            }],
            |_| {},
            || false,
        )
        .await;
        server.join().unwrap();

        assert!(result.unwrap_err().contains("模型文件校验失败"));
        assert!(!candidate.exists());
    }

    /// 主镜像不可达（连接拒绝）时，单个文件必须自动换下一个基地址重试成功，
    /// 且只向存活的镜像发请求。
    #[tokio::test]
    async fn unreachable_mirror_falls_back_to_next_base() {
        let (base_url, requests, server) = serve_model_files(vec![b"abc"]);
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("candidate");
        // 127.0.0.1:1 无监听，连接立即被拒，等价于镜像宕机。
        let bases = vec!["http://127.0.0.1:1".to_string(), base_url.clone()];
        download_knowledge_model_candidate_with(
            &reqwest::Client::new(),
            &candidate,
            &bases,
            &[KnowledgeModelFile {
                source_path: "onnx/model_int8.onnx",
                destination_path: "model.onnx",
                bytes: 3,
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            }],
            |_| {},
            || false,
        )
        .await
        .unwrap();
        server.join().unwrap();

        assert_eq!(std::fs::read(candidate.join("model.onnx")).unwrap(), b"abc");
        let request = requests.recv().unwrap();
        assert!(request.contains("GET /hf/onnx-community/bge-m3-ONNX/resolve/"));
    }

    /// 镜像返回被篡改/损坏的字节（SHA-256 不符）时同样回退到下一个基地址，
    /// 最终落盘内容必须来自通过校验的源。
    #[tokio::test]
    async fn mirror_serving_corrupt_bytes_falls_back_to_next_base() {
        let (bad_base, _bad_requests, bad_server) = serve_model_files(vec![b"zzz"]);
        let (good_base, _good_requests, good_server) = serve_model_files(vec![b"abc"]);
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("candidate");
        let bases = vec![bad_base, good_base];
        download_knowledge_model_candidate_with(
            &reqwest::Client::new(),
            &candidate,
            &bases,
            &[KnowledgeModelFile {
                source_path: "config.json",
                destination_path: "config.json",
                bytes: 3,
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            }],
            |_| {},
            || false,
        )
        .await
        .unwrap();
        bad_server.join().unwrap();
        good_server.join().unwrap();

        assert_eq!(
            std::fs::read(candidate.join("config.json")).unwrap(),
            b"abc"
        );
    }

    /// 全部下载源都失败时，聚合报错必须点名每一个失败的源（`[host]` 前缀）
    /// 并给出源总数，而不是只保留最后一个源的报错——否则用户无法分辨是
    /// 镜像坏了还是官方源也坏了。
    #[tokio::test]
    async fn exhausted_sources_error_names_every_failed_base() {
        let (bad_base, _bad_requests, bad_server) = serve_model_files(vec![b"zzz"]);
        let (worse_base, _worse_requests, worse_server) = serve_model_files(vec![b"yyy"]);
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("candidate");
        let bases = vec![bad_base.clone(), worse_base.clone()];
        let result = download_knowledge_model_candidate_with(
            &reqwest::Client::new(),
            &candidate,
            &bases,
            &[KnowledgeModelFile {
                source_path: "config.json",
                destination_path: "config.json",
                bytes: 3,
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            }],
            |_| {},
            || false,
        )
        .await;
        bad_server.join().unwrap();
        worse_server.join().unwrap();

        let error = result.unwrap_err();
        assert!(error.contains("2 个下载源均失败"), "{error}");
        // 前缀 host 含非默认端口（本地服务器），与聚合文案的 [host:port] 一致。
        let bad_authority = bad_base
            .split("//")
            .nth(1)
            .and_then(|r| r.split('/').next())
            .unwrap();
        let worse_authority = worse_base
            .split("//")
            .nth(1)
            .and_then(|r| r.split('/').next())
            .unwrap();
        assert!(error.contains(&format!("[{bad_authority}]")), "{error}");
        assert!(error.contains(&format!("[{worse_authority}]")), "{error}");
        assert!(!candidate.exists());
    }

    /// 模型下载的重定向策略：只跟随 HTTPS 目标、跳数有界（与
    /// connectors/marketplace 下载路径同口径；初始请求不受限，本地测试
    /// 服务器可用 HTTP）。
    #[test]
    fn redirects_follow_only_https_targets_within_hop_budget() {
        assert!(hf_redirect_follow_allowed(0, "https"));
        assert!(hf_redirect_follow_allowed(9, "https"));
        assert!(!hf_redirect_follow_allowed(10, "https"));
        assert!(!hf_redirect_follow_allowed(0, "http"));
        assert!(!hf_redirect_follow_allowed(0, "ftp"));
    }

    /// 换基地址重试时该文件从头下载：进度事件的累计字节必须保持单调不减
    /// （峰值钳制），否则前端进度条会在回退瞬间从已累计的高位倒跳回低位。
    /// 首源发出 2MiB/4MiB/5MiB 三次事件后在 SHA 校验失败，重启源的事件若
    /// 原样透传会从 2MiB 重新开始——钳制失效时本测试的窗口断言即失败。
    #[tokio::test]
    async fn progress_events_stay_monotonic_across_base_fallback() {
        const FILE_BYTES: usize = 5 * 1024 * 1024;
        let good_body: &'static [u8] = Vec::leak(vec![b'a'; FILE_BYTES]);
        let bad_body: &'static [u8] = Vec::leak(vec![b'z'; FILE_BYTES]);
        let (bad_base, _bad_requests, bad_server) = serve_model_files(vec![bad_body]);
        let (good_base, _good_requests, good_server) = serve_model_files(vec![good_body]);
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("candidate");
        let (events_tx, events_rx) = mpsc::channel();
        let bases = vec![bad_base, good_base];
        download_knowledge_model_candidate_with(
            &reqwest::Client::new(),
            &candidate,
            &bases,
            &[KnowledgeModelFile {
                source_path: "onnx/model_int8.onnx",
                destination_path: "model.onnx",
                bytes: FILE_BYTES as u64,
                sha256: {
                    let mut hasher = Sha256::new();
                    hasher.update(good_body);
                    let hex: String = hasher
                        .finalize()
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect();
                    Box::leak(hex.into_boxed_str())
                },
            }],
            move |event| {
                if event.stage == KnowledgeModelDownloadStage::Download {
                    let _ = events_tx.send(event.downloaded_bytes);
                }
            },
            || false,
        )
        .await
        .unwrap();
        bad_server.join().unwrap();
        good_server.join().unwrap();

        assert_eq!(
            std::fs::read(candidate.join("model.onnx")).unwrap(),
            good_body
        );
        let events: Vec<u64> = events_rx.into_iter().collect();
        assert_eq!(
            events.len(),
            6,
            "两个源各发 2MiB/4MiB/5MiB 三次事件: {events:?}"
        );
        for pair in events.windows(2) {
            assert!(
                pair[0] <= pair[1],
                "跨源回退时进度事件必须单调不减: {events:?}"
            );
        }
    }

    /// 非法基地址必须在触网前整体失败：哪怕它排在存活镜像之后，也不允许
    /// 「下到一半才报配置错误」——存活源必须一个请求都收不到。候选目录必须
    /// 保持未创建。
    #[tokio::test]
    async fn invalid_base_url_fails_before_any_download() {
        // 排在首位的是存活的服务端：若基地址校验被错误地推迟到逐源下载阶段，
        // 第一个源就会先被真实请求，本测试借请求通道抓住这一回归。
        let (live_base, live_requests, live_server) = serve_model_files(vec![b"abc"]);
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("candidate");
        let bases = vec![live_base, "https://user:secret@example.com".to_string()];
        let result = download_knowledge_model_candidate_with(
            &reqwest::Client::new(),
            &candidate,
            &bases,
            &[KnowledgeModelFile {
                source_path: "config.json",
                destination_path: "config.json",
                bytes: 3,
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            }],
            |_| {},
            || false,
        )
        .await;

        assert!(
            result.unwrap_err().contains("必须是不含账号"),
            "非法基地址必须在触网前失败"
        );
        assert!(
            live_requests.try_recv().is_err(),
            "存活源排在非法基地址之前也不得收到任何请求"
        );
        // 服务线程此刻仍阻塞在 accept()（校验失败 = 永远不会有请求到来），
        // 不得 join，泄漏到测试进程结束即可。
        drop(live_server);
        assert!(!candidate.exists(), "候选目录不应在基地址校验前创建");
    }

    /// 镜像尝试进行中用户取消：必须整体终止（不换下一基地址重试），
    /// 候选目录照常清理。这是回退循环里最关键的语义分支。
    #[tokio::test]
    async fn cancel_during_mirror_attempt_aborts_without_falling_through() {
        let (first_base, _first_requests, first_server) = serve_model_files(vec![b"abc"]);
        let (second_base, second_requests, _second_server) = serve_model_files(vec![b"abc"]);
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("candidate");
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel_flag = std::sync::Arc::clone(&cancelled);
        let bases = vec![first_base, second_base];
        let result = download_knowledge_model_candidate_with(
            &reqwest::Client::new(),
            &candidate,
            &bases,
            &[KnowledgeModelFile {
                source_path: "config.json",
                destination_path: "config.json",
                bytes: 3,
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            }],
            move |_| {
                // 首个进度事件即视为用户点了取消（模拟下载中途取消）。
                cancel_flag.store(true, std::sync::atomic::Ordering::Release);
            },
            || cancelled.load(std::sync::atomic::Ordering::Acquire),
        )
        .await;

        assert_eq!(result.unwrap_err(), "已取消");
        // 第二个基地址必须完全没有被请求。
        assert!(
            second_requests.try_recv().is_err(),
            "取消后不得再尝试下一基地址"
        );
        assert!(!candidate.exists(), "取消后候选目录必须被清理");
        first_server.join().unwrap();
    }

    #[tokio::test]
    async fn size_mismatch_removes_candidate_directory() {
        let (base_url, _requests, server) = serve_model_files(vec![b"abcd"]);
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("candidate");
        let result = download_knowledge_model_candidate_with(
            &reqwest::Client::new(),
            &candidate,
            &[base_url.clone()],
            &[KnowledgeModelFile {
                source_path: "config.json",
                destination_path: "config.json",
                bytes: 3,
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            }],
            |_| {},
            || false,
        )
        .await;
        server.join().unwrap();

        assert!(result.unwrap_err().contains("模型文件大小不符"));
        assert!(!candidate.exists());
    }

    #[test]
    fn installation_recovers_stable_backup_and_removes_it_after_replace() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("bge-m3");
        let backup = destination.with_extension("backup");
        let candidate = root.path().join("candidate");
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(backup.join("model.onnx"), b"old").unwrap();
        std::fs::create_dir_all(&candidate).unwrap();
        std::fs::write(candidate.join("model.onnx"), b"new").unwrap();

        assert!(
            install_model_candidate(&candidate, &destination)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            std::fs::read(destination.join("model.onnx")).unwrap(),
            b"new"
        );
        assert!(!backup.exists());
    }

    #[test]
    fn recovery_cleans_legacy_random_service_backup() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("bge-m3");
        let legacy = root.path().join(".bge-m3.backup-old-service");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();

        assert!(recover_model_directory(&destination).unwrap().is_none());
        assert!(!legacy.exists());
    }

    // Symlink resolution is Unix-only in this suite: creating a directory
    // symlink on Windows requires privileges the test runner does not have.
    #[cfg(unix)]
    #[test]
    fn completeness_probe_resolves_a_symlinked_model_directory() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("bge-m3-v3");
        std::fs::create_dir_all(target.join("onnx")).unwrap();
        std::fs::write(target.join("onnx").join("model_int8.onnx"), b"onnx").unwrap();
        for file in [
            "tokenizer.json",
            "config.json",
            "special_tokens_map.json",
            "tokenizer_config.json",
        ] {
            std::fs::write(target.join(file), b"x").unwrap();
        }
        let link = root.path().join("current");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        // The probe follows the operating system's path resolution on
        // purpose: a versioned symlink layout reports complete when its
        // target holds the required files.
        assert!(model_directory_is_complete(&link));

        std::fs::remove_file(target.join("tokenizer.json")).unwrap();
        assert!(!model_directory_is_complete(&link));
        // A dangling symlink does not resolve and stays incomplete.
        std::fs::remove_dir_all(&target).unwrap();
        assert!(!model_directory_is_complete(&link));
    }
}
