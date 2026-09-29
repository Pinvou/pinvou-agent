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
/// Hugging Face-compatible mirror reachable from mainland China (path layout
/// identical to the official source; preferred by default).
pub const KNOWLEDGE_MODEL_HF_MIRROR_BASE_URL: &str = "https://hf-mirror.com";
pub const KNOWLEDGE_MODEL_HF_REPOSITORY: &str = "onnx-community/bge-m3-ONNX";
pub const KNOWLEDGE_MODEL_HF_REVISION: &str = "25b9af8e87a38eb120cfe87125383677b9cd309e";
pub const KNOWLEDGE_MODEL_HF_BASE_URL_ENV: &str = "PINVOU_KNOWLEDGE_HF_BASE_URL";
pub const KNOWLEDGE_MODEL_DOWNLOAD_BYTES: u64 = 585_565_019;

/// Shared message for the cancelled state (user-facing error copy, used by every
/// cancellation checkpoint). The fallback loop tells "user cancel" (abort the whole
/// flow) apart from "single mirror source failure" (retry the next base URL) via the
/// `is_cancelled` flag rather than error-string matching; the flag does not depend
/// on this constant.
const CANCELLED: &str = "cancelled";

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

/// Returns the ordered list of Hugging Face-compatible mirror base URLs to try.
///
/// When [`KNOWLEDGE_MODEL_HF_BASE_URL_ENV`] is set explicitly, only that URL is
/// returned — an explicitly chosen source never falls back; otherwise the mainland
/// China mirror ([`KNOWLEDGE_MODEL_HF_MIRROR_BASE_URL`]) is tried first, falling
/// back to the official source on failure. Every file is retried per base URL, and
/// content is always verified with per-file SHA-256.
pub fn knowledge_model_hf_base_url_candidates() -> Vec<String> {
    ordered_hf_base_url_candidates(
        std::env::var(KNOWLEDGE_MODEL_HF_BASE_URL_ENV)
            .ok()
            .filter(|value| !value.trim().is_empty()),
    )
}

/// Pure core of [`knowledge_model_hf_base_url_candidates`] (for unit testing; does
/// not touch environment variables).
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
/// `hf_base_urls` is the ordered list of mirror base URLs to try (see
/// [`knowledge_model_hf_base_url_candidates`]): when a single file fails to download
/// or verify on one base URL, the next base URL is retried automatically, and the
/// whole operation errors out only after all of them fail.
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
        // Redirects follow HTTPS targets only (same policy as the connectors /
        // marketplace download paths): if the mirror is hijacked, the 586MB model
        // stream must not be redirected to plaintext HTTP. Cross-origin HTTPS
        // redirects are still allowed — hf-mirror currently 308s /resolve/ to the
        // official source, and the bytes written to disk always pass the SHA-256
        // gate.
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

/// 上次安装中断遗留候选目录的处置结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptedCandidate {
    /// 目录与清单逐文件一致（存在性 + 大小 + SHA-256），可直接进入真实加载 + 部署。
    Reuse,
    /// 目录不存在，或校验不符已被清理；调用方应重新下载。
    Cleaned,
}

/// 崩溃恢复：安装流程在「五文件下载加逐文件 SHA-256 校验完成」之后、「真实加载
/// 与部署」之前被中断（进程崩溃 / 被杀 / 断电）时，候选目录会完整留在磁盘上。
/// 下载阶段本就逐文件校验通过后才重命名落位，因此与清单一致的候选目录必然来自
/// 一次完整下载——重校验（大小 + SHA-256，~585MB 秒级）通过后直接复用即可续上
/// 安装，不必每次重试都重新下载全量模型；任何不符（含下载中途的 `.part` 残留、
/// 磁盘损坏、应用升级后清单换版）都会清掉该目录并按 `Cleaned` 走全新下载。
/// 清单之外的多余条目不参与校验，`Reuse` 时会随部署一并保留——加载器只读固定
/// 文件名、清单内文件哈希钉死，多余文件仅占磁盘。清单为编译期常量，非空由
/// 调用方保证。
///
/// `on_progress(file_index, file_count)` 在每个文件校验通过后回调（`file_index`
/// 从 1 开始），供调用方把复查进度映射到既有进度事件。调用方负责跨进程安装锁；
/// 返回 `Reuse` 后候选目录的所有权移交调用方（与下载成功后的语义一致）。
pub fn recover_interrupted_candidate_dir(
    candidate: &Path,
    manifest: &[KnowledgeModelFile],
    mut on_progress: impl FnMut(usize, usize),
) -> Result<InterruptedCandidate, String> {
    if !candidate.exists() {
        return Ok(InterruptedCandidate::Cleaned);
    }
    if candidate.is_file() {
        // 残留是同名普通文件（异常产物）：清掉即可走全新下载，不必让整次安装失败。
        std::fs::remove_file(candidate).map_err(|error| {
            format!(
                "清理残留的模型候选文件失败({}): {error}",
                candidate.display()
            )
        })?;
        return Ok(InterruptedCandidate::Cleaned);
    }
    if let Err(reason) = verify_manifest_dir(candidate, manifest, &mut on_progress) {
        // 复查不通过意味着要付出 ~585MB 全新下载的代价，失败原因必须留痕可查。
        eprintln!("[knowledge] 模型候选目录复查未通过，清理后重新下载: {reason}");
        std::fs::remove_dir_all(candidate).map_err(|error| {
            format!(
                "清理未通过校验的模型候选目录失败({}): {error}",
                candidate.display()
            )
        })?;
        return Ok(InterruptedCandidate::Cleaned);
    }
    Ok(InterruptedCandidate::Reuse)
}

/// 逐文件校验候选目录与清单一致（存在、是文件、大小一致、SHA-256 一致），
/// 首个不一致即返回 `Err`。
fn verify_manifest_dir(
    dir: &Path,
    manifest: &[KnowledgeModelFile],
    on_progress: &mut impl FnMut(usize, usize),
) -> Result<(), String> {
    for (index, file) in manifest.iter().enumerate() {
        let path = safe_candidate_path(dir, file.destination_path)?;
        let metadata = std::fs::metadata(&path)
            .map_err(|error| format!("模型候选文件缺失({}): {error}", path.display()))?;
        if !metadata.is_file() {
            return Err(format!("模型候选路径不是文件({})", path.display()));
        }
        if metadata.len() != file.bytes {
            return Err(format!(
                "模型候选文件大小不符({}): 期望 {} 字节，实际 {} 字节",
                path.display(),
                file.bytes,
                metadata.len()
            ));
        }
        let actual = sha256_file(&path)?;
        if !actual.eq_ignore_ascii_case(file.sha256) {
            return Err(format!(
                "模型候选文件校验失败({}): 期望 {}，实际 {}",
                file.destination_path, file.sha256, actual
            ));
        }
        on_progress(index + 1, manifest.len());
    }
    Ok(())
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
    // Validate all base URLs up front: any invalid mirror configuration must fail
    // before any network access, so "half a download from the first two base URLs
    // before the third one reveals the misconfiguration" cannot happen.
    if hf_base_urls.is_empty() {
        return Err("mirror base URL list is empty".to_string());
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

            // Try the mirror base URLs in order: a download or verification failure
            // on the current base URL (including an SHA-256 mismatch caused by
            // tampered mirror content) retries the next one; only after all fail does
            // the whole operation error out, with the source host and the number of
            // exhausted sources in the error — otherwise a failure such as a tampered
            // mirror would be misread as an official-source outage. Cancellation is a
            // global intent: wherever it appears, it terminates immediately and is
            // never treated as a mirror failure.
            let mut failures: Vec<String> = Vec::new();
            let mut succeeded = false;
            // Retrying on a different base URL restarts this file from scratch; if
            // progress events passed through unchanged, the cumulative bytes seen by
            // the frontend would go backwards (the progress bar would jump back).
            // Peak clamping across base URLs keeps the value monotonically
            // non-decreasing.
            let mut peak_downloaded = completed_bytes;
            for base_url in &base_urls {
                if is_cancelled() {
                    return Err(CANCELLED.to_string());
                }
                // The previous base URL's partial `.part` must be removed before
                // retrying, so the append never mixes bytes from different sources.
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
                    // Cancellation is a global intent: once the flag is set, abort the
                    // whole operation (even if this particular error is not the cancel
                    // message); never treat it as a mirror failure and move on to the
                    // next base URL.
                    Err(_) if is_cancelled() => return Err(CANCELLED.to_string()),
                    Err(error) => {
                        let host = match base_url.port() {
                            // Non-default ports are included in the prefix so
                            // same-host candidates on different ports stay
                            // distinguishable in errors; default ports are omitted
                            // (consistent with URL display conventions).
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
                // base_urls is non-empty and every failure appends to failures; the
                // fallback message must not falsely claim "cancelled".
                let detail = failures.join("; ");
                return Err(if base_urls.len() > 1 {
                    format!("{} download sources all failed: {detail}", base_urls.len())
                } else if detail.is_empty() {
                    "Model download failed".to_string()
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

/// Complete attempt for a single file on a single base URL: download to `.part` →
/// verify progress event → SHA-256 verification → atomic `rename` to `destination`.
/// Any failing step returns `Err`, and the caller decides whether to retry the next
/// base URL.
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
        .map_err(|error| format!("Model verification task failed: {error}"))??;
    if !actual.eq_ignore_ascii_case(file.sha256) {
        return Err(format!(
            "Model file verification failed ({}): expected {}, actual {}",
            file.source_path, file.sha256, actual
        ));
    }
    if is_cancelled() {
        return Err(CANCELLED.to_string());
    }
    std::fs::rename(partial, destination).map_err(|error| {
        format!(
            "Failed to finish writing model file ({} -> {}): {error}",
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

/// Redirect-follow decision (pure core for unit testing): follow HTTPS targets only,
/// with a bounded hop count. The initial request itself is not restricted (local and
/// test servers may use HTTP); only the redirect chain is constrained.
fn hf_redirect_follow_allowed(previous_hops: usize, scheme: &str) -> bool {
    previous_hops < 10 && scheme == "https"
}

/// An explicitly configured mirror base URL can come from either of two environment
/// variables (shared server or desktop; validation happens inside the shared crate,
/// which cannot tell the origin apart), so the error message names both and avoids
/// reporting the wrong variable to desktop users.
const HF_BASE_URL_ENV_HINT: &str =
    "mirror base URL environment variable (PINVOU_KNOWLEDGE_HF_BASE_URL / PINVOU3_KB_HF_BASE_URL)";

fn validate_hf_base_url(value: &str) -> Result<Url, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{HF_BASE_URL_ENV_HINT} must not be empty"));
    }
    let mut url = Url::parse(value)
        .map_err(|error| format!("{HF_BASE_URL_ENV_HINT} is not a valid URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(format!(
            "{HF_BASE_URL_ENV_HINT} must be an HTTP(S) base URL without credentials, query parameters, or fragments"
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
        // Explicit environment variable = a source the user chose explicitly; no fallback.
        assert_eq!(
            ordered_hf_base_url_candidates(Some("https://internal.example/hf".to_string())),
            vec!["https://internal.example/hf".to_string()]
        );
        // Unset: mainland China mirror first, official source as the fallback.
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
        assert_eq!(result.unwrap_err(), CANCELLED);
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

        assert!(
            result
                .unwrap_err()
                .contains("Model file verification failed")
        );
        assert!(!candidate.exists());
    }

    /// When the primary mirror is unreachable (connection refused), a single file
    /// must automatically retry the next base URL and succeed, and only the alive
    /// mirror may receive requests.
    #[tokio::test]
    async fn unreachable_mirror_falls_back_to_next_base() {
        let (base_url, requests, server) = serve_model_files(vec![b"abc"]);
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("candidate");
        // 127.0.0.1:1 has no listener, so the connection is refused immediately —
        // equivalent to the mirror being down.
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

    /// When a mirror serves tampered/corrupted bytes (SHA-256 mismatch), fall back to
    /// the next base URL the same way; the content finally written to disk must come
    /// from a source that passed verification.
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

    /// When every download source fails, the aggregated error must name each failed
    /// source (the `[host]` prefix) and give the total source count, instead of
    /// keeping only the last source's error — otherwise users cannot tell whether
    /// the mirror or the official source is broken.
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
        assert!(error.contains("2 download sources all failed"), "{error}");
        // The prefix host includes a non-default port (local server), matching
        // [host:port] in the aggregated message.
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

    /// Redirect policy for model downloads: follow HTTPS targets only, with a bounded
    /// hop count (same as the connectors/marketplace download paths; the initial
    /// request is unrestricted, and local test servers may use HTTP).
    #[test]
    fn redirects_follow_only_https_targets_within_hop_budget() {
        assert!(hf_redirect_follow_allowed(0, "https"));
        assert!(hf_redirect_follow_allowed(9, "https"));
        assert!(!hf_redirect_follow_allowed(10, "https"));
        assert!(!hf_redirect_follow_allowed(0, "http"));
        assert!(!hf_redirect_follow_allowed(0, "ftp"));
    }

    /// Retrying on a different base URL restarts the file from scratch: the
    /// cumulative bytes in progress events must stay monotonically non-decreasing
    /// (peak clamping), otherwise the frontend progress bar jumps from the already
    /// accumulated high value back down at the fallback moment. The first source
    /// emits three events (2MiB/4MiB/5MiB) and then fails SHA verification; if the
    /// restarted source's events passed through unchanged, they would restart from
    /// 2MiB — with clamping broken, this test's window assertion fails.
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
        // Do not pin the event count: transport chunk sizes are an implementation
        // detail, and a single chunk larger than 2MiB merges threshold events. Broken
        // clamping shows up as events restarting from a low value after the fallback,
        // which the monotonic assertion below catches; here we only require that the
        // event stream genuinely advances to the completion value.
        assert!(
            events.last() == Some(&(FILE_BYTES as u64)),
            "progress events must advance to the completion value ({FILE_BYTES} bytes): {events:?}"
        );
        for pair in events.windows(2) {
            assert!(
                pair[0] <= pair[1],
                "progress events must be monotonically non-decreasing across source fallback: {events:?}"
            );
        }
    }

    /// An invalid base URL must fail the whole operation before any network access:
    /// even when ordered after an alive mirror, "half a download before reporting the
    /// configuration error" is not allowed — the alive source must not receive a
    /// single request. The candidate directory must remain uncreated.
    #[tokio::test]
    async fn invalid_base_url_fails_before_any_download() {
        // The alive server comes first: if base URL validation were wrongly deferred
        // to the per-source download stage, the first source would receive a real
        // request first; this test uses the request channel to catch that regression.
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
            result.unwrap_err().contains("must be an HTTP(S) base URL"),
            "an invalid base URL must fail before any network access"
        );
        assert!(
            live_requests.try_recv().is_err(),
            "an alive source ordered before an invalid base URL must not receive any requests"
        );
        // The server thread is still blocked in accept() at this point (validation
        // failure = no request will ever arrive); do not join it — leaking until the
        // test process exits is fine.
        drop(live_server);
        assert!(
            !candidate.exists(),
            "candidate directory must not be created before base URL validation"
        );
    }

    /// User cancels during a mirror attempt: the whole operation must abort (no retry
    /// on the next base URL), and the candidate directory is still cleaned up. This
    /// is the most critical semantic branch in the fallback loop.
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
                // The first progress event counts as the user clicking cancel
                // (simulates cancelling mid-download).
                cancel_flag.store(true, std::sync::atomic::Ordering::Release);
            },
            || cancelled.load(std::sync::atomic::Ordering::Acquire),
        )
        .await;

        assert_eq!(result.unwrap_err(), CANCELLED);
        // The second base URL must not have been requested at all.
        assert!(
            second_requests.try_recv().is_err(),
            "the next base URL must not be attempted after cancellation"
        );
        assert!(
            !candidate.exists(),
            "candidate directory must be cleaned up after cancellation"
        );
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

    fn write_manifest_fixture(dir: &std::path::Path, manifest: &[KnowledgeModelFile]) {
        for file in manifest {
            let path = dir.join(file.destination_path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"payload").unwrap();
        }
    }

    fn tiny_manifest() -> Vec<KnowledgeModelFile> {
        let payload = b"payload";
        // 清单结构的 sha256 是 &'static str（生产清单是编译期常量）；测试清单
        // 用 Box::leak 换得等价生命周期（测试进程一次性的少量泄漏，无碍）。
        let digest: &'static str = Box::leak(
            Sha256::digest(payload)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
                .into_boxed_str(),
        );
        vec![
            KnowledgeModelFile {
                source_path: "onnx/model_int8.onnx",
                destination_path: "model.onnx",
                bytes: payload.len() as u64,
                sha256: digest,
            },
            KnowledgeModelFile {
                source_path: "config.json",
                destination_path: "config.json",
                bytes: payload.len() as u64,
                sha256: digest,
            },
        ]
    }

    // 回归锚点：安装中断（下载校验完成、加载/部署前进程死亡）留下的完整候选
    // 目录必须被识别为可复用——否则每次重试都会清掉 ~585MB 已验证数据重新下载。
    #[test]
    fn interrupted_candidate_matching_manifest_is_reused() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("bge-m3.tmp");
        let manifest = tiny_manifest();
        write_manifest_fixture(&candidate, &manifest);

        let mut progress = Vec::new();
        let outcome = recover_interrupted_candidate_dir(&candidate, &manifest, |done, total| {
            progress.push((done, total));
        })
        .unwrap();

        assert_eq!(outcome, InterruptedCandidate::Reuse);
        assert_eq!(progress, vec![(1, 2), (2, 2)]);
        assert!(candidate.exists(), "复用语义不得删除候选目录");
    }

    #[test]
    fn interrupted_candidate_missing_file_is_cleaned_for_fresh_download() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("bge-m3.tmp");
        let manifest = tiny_manifest();
        write_manifest_fixture(&candidate, &manifest);
        std::fs::remove_file(candidate.join("config.json")).unwrap();

        let outcome = recover_interrupted_candidate_dir(&candidate, &manifest, |_, _| {}).unwrap();

        assert_eq!(outcome, InterruptedCandidate::Cleaned);
        assert!(!candidate.exists(), "不完整的残留必须清理后才能重新下载");
    }

    #[test]
    fn interrupted_candidate_corrupted_content_is_cleaned() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("bge-m3.tmp");
        let manifest = tiny_manifest();
        write_manifest_fixture(&candidate, &manifest);
        // 同长度篡改：绕过大小检查，确保命中 SHA-256 比对分支（清单校验的安全核心）。
        std::fs::write(candidate.join("model.onnx"), b"payloaX").unwrap();

        let outcome = recover_interrupted_candidate_dir(&candidate, &manifest, |_, _| {}).unwrap();

        assert_eq!(outcome, InterruptedCandidate::Cleaned);
        assert!(!candidate.exists());
    }

    #[test]
    fn interrupted_candidate_non_file_entry_is_cleaned() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("bge-m3.tmp");
        let manifest = tiny_manifest();
        write_manifest_fixture(&candidate, &manifest);
        std::fs::remove_file(candidate.join("config.json")).unwrap();
        std::fs::create_dir(candidate.join("config.json")).unwrap();

        let outcome = recover_interrupted_candidate_dir(&candidate, &manifest, |_, _| {}).unwrap();

        assert_eq!(outcome, InterruptedCandidate::Cleaned);
        assert!(!candidate.exists());
    }

    #[test]
    fn interrupted_candidate_stray_file_is_cleaned_for_fresh_download() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("bge-m3.tmp");
        std::fs::write(&candidate, b"not a directory").unwrap();

        let outcome =
            recover_interrupted_candidate_dir(&candidate, &tiny_manifest(), |_, _| {}).unwrap();

        assert_eq!(outcome, InterruptedCandidate::Cleaned);
        assert!(!candidate.exists(), "同名普通文件残留也应被清理");
    }

    #[test]
    fn interrupted_candidate_extra_entries_are_carried_by_reuse() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("bge-m3.tmp");
        let manifest = tiny_manifest();
        write_manifest_fixture(&candidate, &manifest);
        std::fs::write(candidate.join("stale.part"), b"leftover").unwrap();

        let outcome = recover_interrupted_candidate_dir(&candidate, &manifest, |_, _| {}).unwrap();

        assert_eq!(
            outcome,
            InterruptedCandidate::Reuse,
            "清单外多余条目不阻断复用（部署后仅占磁盘，见 recover 文档）"
        );
        assert!(candidate.join("stale.part").exists());
    }

    #[test]
    fn interrupted_candidate_wrong_size_is_cleaned() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("bge-m3.tmp");
        let manifest = tiny_manifest();
        write_manifest_fixture(&candidate, &manifest);
        std::fs::write(candidate.join("config.json"), b"payload-extra").unwrap();

        let outcome = recover_interrupted_candidate_dir(&candidate, &manifest, |_, _| {}).unwrap();

        assert_eq!(outcome, InterruptedCandidate::Cleaned);
        assert!(!candidate.exists());
    }

    #[test]
    fn interrupted_candidate_absent_dir_reports_cleaned() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("bge-m3.tmp");

        let outcome =
            recover_interrupted_candidate_dir(&candidate, &tiny_manifest(), |_, _| {}).unwrap();

        assert_eq!(outcome, InterruptedCandidate::Cleaned);
        assert!(!candidate.exists());
    }
}
