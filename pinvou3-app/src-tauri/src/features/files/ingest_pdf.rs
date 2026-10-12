//! PDF 摄入：pdftotext 抽正文 + 扫描件 OCR 兜底。
//!
//! 走 `pdftotext -layout`（poppler-utils）抽文字层；无文字层（扫描件）时
//! 委托 [`super::visual_preview`] 的 `ocr_pdf`（pdftoppm 逐页转图 → tesseract）。
//! 缺工具时返回带说明的 warning 占位，不抛错阻塞其他格式。
//!
//! [`super::visual_preview`]: super::visual_preview

use std::path::Path;

use super::IngestResult;
use super::ingest_deps::{pdftotext_extract, system_tools};
use super::visual_preview::ocr_pdf;

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
    // Scanned PDFs (image-layer PDFs) have no text layer, so pdftotext
    // returns blank — fall back to OCR: pdftoppm renders page images that
    // are then fed to tesseract.
    match pdftotext_extract(path) {
        Ok(content) if !content.trim().is_empty() => {
            IngestResult::with_markdown("pdf", &basename, path, byte_size, content)
        }
        Ok(_) => ocr_pdf(path, basename, path_str, byte_size),
        Err(warning) => IngestResult::warning("pdf", &basename, path, byte_size, warning),
    }
}
