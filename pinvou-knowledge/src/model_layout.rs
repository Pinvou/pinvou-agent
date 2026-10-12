//! Single source of truth for the model directory layout.
//!
//! Independent of the `client`/`server`/`local-embed` feature gates: the
//! download manifest (`model_download`), the completeness probe, and the
//! in-process Embedder load (`embedding`) share one layout description
//! instead of each maintaining its own copy.

use std::path::{Path, PathBuf};

/// ONNX file candidates needed at load time (relative to the model directory,
/// in priority order): prefer a root-level single file, then the `onnx/`
/// subdirectory. Shared by the download layout, the completeness probe, and
/// the Embedder load so the list is maintained in one place.
pub const MODEL_ONNX_CANDIDATES: [&str; 3] =
    ["model.onnx", "onnx/model_int8.onnx", "onnx/model.onnx"];

/// Tokenizer/config files needed at load time (relative to the model
/// directory).
pub const MODEL_TOKENIZER_FILES: [&str; 4] = [
    "tokenizer.json",
    "config.json",
    "special_tokens_map.json",
    "tokenizer_config.json",
];

/// Locate a loadable ONNX file inside the model directory, in
/// [`MODEL_ONNX_CANDIDATES`] order.
pub fn find_onnx_in(dir: &Path) -> Result<PathBuf, String> {
    MODEL_ONNX_CANDIDATES
        .iter()
        .map(|relative| dir.join(relative))
        .find(|path| path.is_file())
        .ok_or_else(|| {
            format!(
                "单文件 ONNX 未找到(model.onnx 或 onnx/model_int8.onnx): {}",
                dir.display()
            )
        })
}
