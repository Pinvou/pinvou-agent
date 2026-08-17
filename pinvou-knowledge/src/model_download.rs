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
pub const KNOWLEDGE_MODEL_HF_REPOSITORY: &str = "onnx-community/bge-m3-ONNX";
pub const KNOWLEDGE_MODEL_HF_REVISION: &str = "25b9af8e87a38eb120cfe87125383677b9cd309e";
pub const KNOWLEDGE_MODEL_HF_BASE_URL_ENV: &str = "PINVOU_KNOWLEDGE_HF_BASE_URL";
pub const KNOWLEDGE_MODEL_DOWNLOAD_BYTES: u64 = 585_565_019;

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

/// 返回当前进程应使用的 Hugging Face 兼容镜像基地址。
pub fn knowledge_model_hf_base_url() -> String {
    std::env::var(KNOWLEDGE_MODEL_HF_BASE_URL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| KNOWLEDGE_MODEL_HF_BASE_URL.to_string())
}

/// 将固定 revision 的五个文件下载并逐一校验到一个新建的候选目录。
///
/// `candidate` 必须不存在。任何失败或取消都会清理本次创建的候选目录；调用方在
/// 返回成功后负责真实加载候选模型，并将其原子替换到正式目录。
pub async fn download_knowledge_model_candidate<P, C>(
    candidate: &Path,
    hf_base_url: &str,
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
        .user_agent(concat!("pinvou-knowledge/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| format!("无法创建模型下载客户端: {error}"))?;
    download_knowledge_model_candidate_with(
        &client,
        candidate,
        hf_base_url,
        &KNOWLEDGE_MODEL_FILES,
        on_progress,
        is_cancelled,
    )
    .await
}

/// 检查模型目录是否包含 PINVOU 运行所需的 ONNX 与 tokenizer 文件。
pub fn model_directory_is_complete(dir: &Path) -> bool {
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
        if had_destination {
            if let Err(rollback_error) = std::fs::rename(&backup, destination) {
                return Err(format!(
                    "部署模型失败: {error}; 回滚旧模型也失败: {rollback_error}; 旧模型仍保留在 {}",
                    backup.display()
                ));
            }
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
    hf_base_url: &str,
    manifest: &[KnowledgeModelFile],
    mut on_progress: P,
    is_cancelled: C,
) -> Result<(), String>
where
    P: FnMut(KnowledgeModelDownloadProgress) + Send,
    C: Fn() -> bool + Send + Sync,
{
    let base_url = validate_hf_base_url(hf_base_url)?;
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
                return Err("已取消".to_string());
            }
            let url = knowledge_model_file_url(&base_url, file.source_path)?;
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
            download_manifest_file(
                client,
                &url,
                &partial,
                file,
                completed_bytes,
                total_bytes,
                index,
                manifest.len(),
                &mut on_progress,
                &is_cancelled,
            )
            .await?;

            if is_cancelled() {
                return Err("已取消".to_string());
            }
            on_progress(KnowledgeModelDownloadProgress {
                stage: KnowledgeModelDownloadStage::Verify,
                downloaded_bytes: completed_bytes + file.bytes,
                total_bytes,
                file_index: index + 1,
                file_count: manifest.len(),
                source_path: file.source_path,
            });
            let verify_path = partial.clone();
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
                return Err("已取消".to_string());
            }
            std::fs::rename(&partial, &destination).map_err(|error| {
                format!(
                    "无法完成模型文件写入({} -> {}): {error}",
                    partial.display(),
                    destination.display()
                )
            })?;
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
    if let Some(actual) = response.content_length() {
        if actual != manifest_file.bytes {
            return Err(format!(
                "模型文件大小不符({}): 期望 {} 字节，服务端返回 {} 字节",
                manifest_file.source_path, manifest_file.bytes, actual
            ));
        }
    }

    let mut output = tokio::fs::File::create(destination)
        .await
        .map_err(|error| format!("无法创建模型文件({}): {error}", destination.display()))?;
    let mut stream = response.bytes_stream();
    let mut file_bytes = 0_u64;
    let mut last_emitted = 0_u64;
    while let Some(chunk) = stream.next().await {
        if is_cancelled() {
            return Err("已取消".to_string());
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

fn validate_hf_base_url(value: &str) -> Result<Url, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{KNOWLEDGE_MODEL_HF_BASE_URL_ENV} 不能为空"));
    }
    let mut url = Url::parse(value)
        .map_err(|error| format!("{KNOWLEDGE_MODEL_HF_BASE_URL_ENV} 不是有效 URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(format!(
            "{KNOWLEDGE_MODEL_HF_BASE_URL_ENV} 必须是不含账号、查询参数和片段的 HTTP(S) 基地址"
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
        assert!(KNOWLEDGE_MODEL_FILES
            .iter()
            .all(|file| file.sha256.len() == 64));
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

    #[tokio::test]
    async fn cancelled_download_removes_its_candidate_directory() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("candidate");
        let cancelled = AtomicBool::new(true);
        let client = reqwest::Client::new();
        let result = download_knowledge_model_candidate_with(
            &client,
            &candidate,
            "http://127.0.0.1:9",
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
            &base_url,
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
        assert!(progress
            .windows(2)
            .all(|pair| pair[0].downloaded_bytes <= pair[1].downloaded_bytes));
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
            &base_url,
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

    #[tokio::test]
    async fn size_mismatch_removes_candidate_directory() {
        let (base_url, _requests, server) = serve_model_files(vec![b"abcd"]);
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("candidate");
        let result = download_knowledge_model_candidate_with(
            &reqwest::Client::new(),
            &candidate,
            &base_url,
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

        assert!(install_model_candidate(&candidate, &destination)
            .unwrap()
            .is_none());
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
}
