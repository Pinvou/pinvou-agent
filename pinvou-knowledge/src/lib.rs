//! Pinvou 的可复用知识库核心。
//!
//! 本 crate 不依赖 Tauri 或 Pinvou 桌面应用。桌面端通过 [`client`] 访问远程服务，
//! 服务端通过 [`KnowledgeService`] 持有源文档、索引与授权状态。

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

#[cfg(feature = "discovery")]
pub mod discovery;
pub mod embedding;
pub mod model;
#[cfg(feature = "client")]
pub mod model_download;
#[cfg(feature = "server")]
pub mod parser;
#[cfg(feature = "server")]
pub mod store;
#[cfg(feature = "server")]
pub mod tls;

#[cfg(feature = "server")]
pub mod backup;
#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "server")]
pub mod service;

pub use embedding::Embedder;
pub use model::*;
#[cfg(feature = "server")]
pub use service::{KnowledgeService, ServiceBoot};

/// 客户端和服务端共同执行的单文件上传上限。
pub const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;
pub const EXPECTED_SERVER_ID_HEADER: &str = "x-pinvou-expected-server-id";
#[cfg(feature = "server")]
pub(crate) const MAX_VECTOR_DIMENSIONS: usize = 4096;
#[cfg(feature = "server")]
pub(crate) const MAX_VECTOR_BLOB_BYTES: usize = MAX_VECTOR_DIMENSIONS * std::mem::size_of::<f32>();

#[cfg(feature = "server")]
pub(crate) fn managed_relative_path(value: &str) -> Option<&Path> {
    let path = Path::new(value);
    (!value.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_))))
    .then_some(path)
}

/// 本地知识库与共享知识库共用模型目录时使用的跨进程安装锁。
///
/// 锁文件位于模型父目录中，因此 Linux 共享服务即使通过 bind mount 使用不同路径，
/// 仍会锁定同一个底层文件。锁文件本身可以长期保留；进程退出或崩溃时操作系统会
/// 自动释放文件锁。
pub const KNOWLEDGE_MODEL_INSTALL_BUSY: &str = "另一进程正在准备知识库模型，请稍后刷新并重试";
const KNOWLEDGE_MODEL_INSTALL_LOCK_FILE: &str = ".bge-m3.install.lock";

/// 共享知识库数据目录被另一进程（通常是正在运行的服务）占用时的提示。
pub const KNOWLEDGE_DATA_DIR_BUSY: &str =
    "共享知识库服务正在运行或另一进程正在操作数据目录，请先停止服务后重试";

#[derive(Debug)]
pub struct KnowledgeModelInstallLock {
    file: File,
}

impl Drop for KnowledgeModelInstallLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// 尝试获取模型目录的跨进程排他安装锁。
///
/// 使用非阻塞锁，避免在 Tauri/Tokio 异步线程上等待数百 MB 的下载过程。调用方应
/// 将返回的 guard 持有到候选模型验证、目录替换与 Embedder 构造全部完成。
pub fn try_lock_knowledge_model_install(
    model_dir: &Path,
) -> Result<KnowledgeModelInstallLock, String> {
    let parent = knowledge_model_lock_parent(model_dir)?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("无法创建模型父目录({}): {error}", parent.display()))?;
    let lock_path = parent.join(KNOWLEDGE_MODEL_INSTALL_LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| format!("无法打开模型安装锁({}): {error}", lock_path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(KnowledgeModelInstallLock { file }),
        Err(TryLockError::WouldBlock) => Err(KNOWLEDGE_MODEL_INSTALL_BUSY.to_string()),
        Err(TryLockError::Error(error)) => Err(format!(
            "无法锁定模型目录({}): {error}",
            model_dir.display()
        )),
    }
}

fn knowledge_model_lock_parent(model_dir: &Path) -> Result<&Path, String> {
    match model_dir.parent() {
        Some(parent) if parent.as_os_str().is_empty() => Ok(Path::new(".")),
        Some(parent) => Ok(parent),
        None => Err("模型目录无父目录".to_string()),
    }
}

#[derive(Debug)]
pub struct KnowledgeDataDirLock {
    file: File,
}

impl Drop for KnowledgeDataDirLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// 尝试获取共享知识库数据目录的跨进程排他锁。
///
/// 服务端在 boot 期间持有该锁直至退出，裸二进制的备份/恢复路径用同一把锁
/// 感知服务是否在运行，避免在服务在线时直接替换数据目录造成数据分裂。
/// 锁文件位于数据目录的父目录中，因此 restore 对数据目录本身的重命名
/// （换入 staging、回滚）不会影响锁；进程退出或崩溃时操作系统自动释放。
pub fn try_lock_knowledge_data_dir(data_dir: &Path) -> Result<KnowledgeDataDirLock, String> {
    let parent = match data_dir.parent() {
        Some(parent) if parent.as_os_str().is_empty() => Path::new("."),
        Some(parent) => parent,
        None => return Err("共享知识库数据目录无父目录".to_string()),
    };
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("无法创建数据目录父目录({}): {error}", parent.display()))?;
    let name = data_dir
        .file_name()
        .ok_or_else(|| "共享知识库数据目录无效".to_string())?;
    let lock_path = parent.join(format!(".{}.data.lock", name.to_string_lossy()));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| format!("无法打开数据目录锁({}): {error}", lock_path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(KnowledgeDataDirLock { file }),
        Err(TryLockError::WouldBlock) => Err(KNOWLEDGE_DATA_DIR_BUSY.to_string()),
        Err(TryLockError::Error(error)) => Err(format!(
            "无法锁定共享知识库数据目录({}): {error}",
            data_dir.display()
        )),
    }
}

/// Select one process-wide rustls provider before either reqwest or the
/// embedded HTTPS server builds a TLS configuration. Linux pulls both ring
/// and AWS-LC through transitive dependencies, so feature-based automatic
/// selection is intentionally not relied upon.
#[cfg(feature = "client")]
pub fn ensure_tls_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// 服务端能够解析并索引的文档类型。桌面端文件夹导入使用同一规则，
/// 避免先上传明知无法解析的文件。
pub fn is_supported_document_path(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "txt"
            | "md"
            | "markdown"
            | "csv"
            | "tsv"
            | "json"
            | "jsonl"
            | "yaml"
            | "yml"
            | "toml"
            | "xml"
            | "html"
            | "htm"
            | "log"
            | "ini"
            | "conf"
            | "rs"
            | "py"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "java"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "go"
            | "sh"
            | "ps1"
            | "sql"
            | "xlsx"
            | "xls"
            | "xlsb"
            | "ods"
            | "pdf"
            | "doc"
            | "docx"
            | "odt"
            | "rtf"
            | "ppt"
            | "pptx"
            | "odp"
            | "epub"
            | "png"
            | "jpg"
            | "jpeg"
            | "bmp"
            | "tif"
            | "tiff"
            | "webp"
    )
}

/// Detect private-key material even if it was renamed to a normal text file.
/// Certificates are intentionally allowed because they are public material.
pub fn looks_like_secret_material(bytes: &[u8]) -> bool {
    const MARKERS: &[&[u8]] = &[
        b"-----BEGIN PRIVATE KEY-----",
        b"-----BEGIN RSA PRIVATE KEY-----",
        b"-----BEGIN DSA PRIVATE KEY-----",
        b"-----BEGIN EC PRIVATE KEY-----",
        b"-----BEGIN OPENSSH PRIVATE KEY-----",
        b"-----BEGIN ENCRYPTED PRIVATE KEY-----",
        b"-----BEGIN PGP PRIVATE KEY BLOCK-----",
        b"openssh-key-v1",
    ];
    let head = &bytes[..bytes.len().min(8192)];
    MARKERS
        .iter()
        .any(|marker| head.windows(marker.len()).any(|window| window == *marker))
}

/// 与桌面本地知识库保持一致的切块规则。
pub fn chunk_text(text: &str, max_chars: usize, overlap: usize) -> Vec<String> {
    if text.trim().is_empty() || max_chars == 0 {
        return Vec::new();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let mut end = (start + max_chars).min(chars.len());
        if end < chars.len() {
            let floor = start + max_chars.saturating_mul(3) / 5;
            if let Some(boundary) = (floor..end)
                .rev()
                .find(|index| matches!(chars[*index], '\n' | '。' | '！' | '？' | '.' | '!' | '?'))
            {
                end = boundary + 1;
            }
        }
        let chunk: String = chars[start..end].iter().collect();
        if !chunk.trim().is_empty() {
            chunks.push(chunk);
        }
        if end >= chars.len() {
            break;
        }
        let next = end.saturating_sub(overlap.min(end - start));
        start = next.max(start + 1);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};

    use super::{
        chunk_text, is_supported_document_path, knowledge_model_lock_parent,
        try_lock_knowledge_data_dir, try_lock_knowledge_model_install, KNOWLEDGE_DATA_DIR_BUSY,
        KNOWLEDGE_MODEL_INSTALL_BUSY,
    };

    #[test]
    fn chunking_keeps_overlap_without_stalling() {
        let source = "一二三四五六七八九十";
        let chunks = chunk_text(source, 6, 2);
        assert_eq!(chunks, vec!["一二三四五六", "五六七八九十"]);
    }

    #[test]
    fn folder_import_uses_the_server_parser_allowlist() {
        assert!(is_supported_document_path(Path::new("report.PDF")));
        assert!(is_supported_document_path(Path::new("notes.md")));
        assert!(is_supported_document_path(Path::new("sheet.xlsx")));
        assert!(!is_supported_document_path(Path::new("archive.zip")));
        assert!(!is_supported_document_path(Path::new("secret.key")));
    }

    #[test]
    fn chunking_prefers_sentence_boundaries_after_sixty_percent() {
        assert_eq!(
            chunk_text("abcdefghi.jklmnop", 12, 2),
            vec!["abcdefghi.", "i.jklmnop"]
        );
    }

    #[test]
    fn renamed_private_keys_are_rejected_by_shared_core() {
        assert!(super::looks_like_secret_material(
            b"notes\n-----BEGIN OPENSSH PRIVATE KEY-----\nsecret"
        ));
        assert!(!super::looks_like_secret_material(
            b"-----BEGIN CERTIFICATE-----\npublic"
        ));
    }

    #[test]
    fn relative_model_directory_locks_in_the_current_directory() {
        assert_eq!(
            knowledge_model_lock_parent(Path::new("bge-m3")).unwrap(),
            Path::new(".")
        );
    }

    #[test]
    fn model_install_lock_child_process() {
        let Some(model_dir) = std::env::var_os("PINVOU_TEST_MODEL_LOCK_DIR").map(PathBuf::from)
        else {
            return;
        };
        let ready = PathBuf::from(
            std::env::var_os("PINVOU_TEST_MODEL_LOCK_READY").expect("child ready path"),
        );
        let release = PathBuf::from(
            std::env::var_os("PINVOU_TEST_MODEL_LOCK_RELEASE").expect("child release path"),
        );
        let _guard = try_lock_knowledge_model_install(&model_dir).expect("child acquires lock");
        std::fs::write(&ready, b"ready").expect("child reports acquired lock");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !release.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn model_install_lock_serializes_processes_and_releases_on_exit() {
        let root = tempfile::tempdir().expect("temporary model parent");
        let model_dir = root.path().join("bge-m3");
        let ready = root.path().join("child-ready");
        let release = root.path().join("child-release");
        let mut child = Command::new(std::env::current_exe().expect("current test executable"))
            .arg("--exact")
            .arg("tests::model_install_lock_child_process")
            .arg("--nocapture")
            .env("PINVOU_TEST_MODEL_LOCK_DIR", &model_dir)
            .env("PINVOU_TEST_MODEL_LOCK_READY", &ready)
            .env("PINVOU_TEST_MODEL_LOCK_RELEASE", &release)
            .spawn()
            .expect("spawn lock holder");

        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if !ready.exists() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child did not acquire the model lock");
        }

        let contention = try_lock_knowledge_model_install(&model_dir).unwrap_err();
        std::fs::write(&release, b"release").expect("release child lock");
        let status = child.wait().expect("wait for lock holder");
        assert!(status.success());
        assert_eq!(contention, KNOWLEDGE_MODEL_INSTALL_BUSY);

        let guard = try_lock_knowledge_model_install(&model_dir)
            .expect("process exit must release the model lock");
        drop(guard);
    }

    #[test]
    fn data_dir_lock_child_process() {
        let Some(data_dir) = std::env::var_os("PINVOU_TEST_DATA_LOCK_DIR").map(PathBuf::from)
        else {
            return;
        };
        let ready = PathBuf::from(
            std::env::var_os("PINVOU_TEST_DATA_LOCK_READY").expect("child ready path"),
        );
        let release = PathBuf::from(
            std::env::var_os("PINVOU_TEST_DATA_LOCK_RELEASE").expect("child release path"),
        );
        let _guard = try_lock_knowledge_data_dir(&data_dir).expect("child acquires lock");
        std::fs::write(&ready, b"ready").expect("child reports acquired lock");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !release.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn data_dir_lock_serializes_processes_and_releases_on_exit() {
        let root = tempfile::tempdir().expect("temporary data parent");
        let data_dir = root.path().join("knowledge-data");
        let ready = root.path().join("child-ready");
        let release = root.path().join("child-release");
        let mut child = Command::new(std::env::current_exe().expect("current test executable"))
            .arg("--exact")
            .arg("tests::data_dir_lock_child_process")
            .arg("--nocapture")
            .env("PINVOU_TEST_DATA_LOCK_DIR", &data_dir)
            .env("PINVOU_TEST_DATA_LOCK_READY", &ready)
            .env("PINVOU_TEST_DATA_LOCK_RELEASE", &release)
            .spawn()
            .expect("spawn lock holder");

        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if !ready.exists() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child did not acquire the data dir lock");
        }

        let contention = try_lock_knowledge_data_dir(&data_dir).unwrap_err();
        std::fs::write(&release, b"release").expect("release child lock");
        let status = child.wait().expect("wait for lock holder");
        assert!(status.success());
        assert_eq!(contention, KNOWLEDGE_DATA_DIR_BUSY);

        let guard = try_lock_knowledge_data_dir(&data_dir)
            .expect("process exit must release the data dir lock");
        drop(guard);
    }
}
