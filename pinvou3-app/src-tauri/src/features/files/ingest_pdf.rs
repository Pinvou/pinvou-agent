//! PDF 摄入：pdftotext 抽正文 + 扫描件 OCR 兜底。
//!
//! 走 `pdftotext -layout`（poppler-utils）抽文字层；无文字层（扫描件）时
//! 委托 [`super::visual_preview`] 的 `ocr_pdf`（pdftoppm 逐页转图 → tesseract）。
//! 缺工具时返回带说明的 warning 占位，不抛错阻塞其他格式。
//!
//! [`super::visual_preview`]: super::visual_preview

use std::path::Path;

use super::ingest_deps::{pdf_tool_command, system_tools};
use super::visual_preview::ocr_pdf;
use super::IngestResult;

/// PDF 摄入主路径：pdftotext 抽正文；空白（扫描件）走 OCR 兜底。
pub(super) fn ingest_pdf(
    path: &Path,
    basename: String,
    path_str: String,
    byte_size: u64,
) -> IngestResult {
    let tools = system_tools();
    if !tools.pdftotext {
        return IngestResult::warning(
            "pdf",
            &basename,
            path,
            byte_size,
            crate::platform::os::pdf_text_missing_message(),
        );
    }
    // pdftotext -layout <path> -  → stdout
    let out = pdf_tool_command("pdftotext")
        .arg("-layout")
        .arg(path)
        .arg("-")
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let content = String::from_utf8_lossy(&o.stdout).into_owned();
            // 扫描件 PDF（图层 PDF）没有文字层，pdftotext 返回空白 —— 此时
            // 走 OCR 兜底：pdftoppm 逐页转图再喂给 tesseract。
            if content.trim().is_empty() {
                return ocr_pdf(path, basename, path_str, byte_size);
            }
            IngestResult::with_markdown("pdf", &basename, path, byte_size, content)
        }
        Ok(o) => IngestResult::warning(
            "pdf",
            &basename,
            path,
            byte_size,
            format!(
                "pdftotext 失败: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ),
        ),
        Err(e) => IngestResult::warning(
            "pdf",
            &basename,
            path,
            byte_size,
            format!("pdftotext 调用失败: {e}"),
        ),
    }
}
