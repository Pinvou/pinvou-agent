//! Pinvou 的可复用知识库核心。
//!
//! 本 crate 不依赖 Tauri 或 Pinvou 桌面应用。桌面端通过 [`client`] 访问远程服务，
//! 服务端通过 [`KnowledgeService`] 持有源文档、索引与授权状态。

use std::path::Path;

pub mod discovery;
pub mod embedding;
pub mod model;
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

/// 本地知识库与共享知识库共同使用的 BGE-M3 发布包。
///
/// 下载源、摘要和体积必须保持为同一份不可变资产，避免桌面端与服务端
/// 各自维护一套模型来源。两端仍分别支持环境变量覆盖，用于企业内网镜像。
pub const KNOWLEDGE_MODEL_ARCHIVE_URL: &str =
    "https://github.com/Pinvou/pinvou-agent/releases/download/kb-model-v1/bge-m3.tar.gz";
pub const KNOWLEDGE_MODEL_ARCHIVE_SHA256: &str =
    "86438791d1ee7c9989c75878d3623ab28a7e4cd57aa3a7816480043d1de62efe";
pub const KNOWLEDGE_MODEL_ARCHIVE_BYTES: u64 = 407_925_014;

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
    use std::path::Path;

    use super::{chunk_text, is_supported_document_path};

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
}
