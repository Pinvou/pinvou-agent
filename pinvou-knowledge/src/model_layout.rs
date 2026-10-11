//! 模型目录布局的单一事实来源。
//!
//! 独立于 `client`/`server`/`local-embed` 特性门：下载清单（`model_download`）、
//! 完整性探测与进程内 Embedder 装载（`embedding`）共用同一份布局描述，
//! 避免各处分别维护同一份目录布局。

use std::path::{Path, PathBuf};

/// 运行时加载所需的 ONNX 文件候选（相对模型目录，按优先级排列）：优先根目录
/// 单文件，其次 onnx/ 子目录。下载落盘布局、完整性探测与 Embedder 装载共用，
/// 避免各处分别维护同一份目录布局。
pub const MODEL_ONNX_CANDIDATES: [&str; 3] =
    ["model.onnx", "onnx/model_int8.onnx", "onnx/model.onnx"];

/// 运行时加载所需的 tokenizer/config 文件（相对模型目录）。
pub const MODEL_TOKENIZER_FILES: [&str; 4] = [
    "tokenizer.json",
    "config.json",
    "special_tokens_map.json",
    "tokenizer_config.json",
];

/// 按 [`MODEL_ONNX_CANDIDATES`] 顺序定位模型目录中可加载的 ONNX 文件。
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
