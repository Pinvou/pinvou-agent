//! Single source of truth for the model directory layout.
//!
//! Independent of the `client`/`server`/`local-embed` feature gates: the
//! download manifest (`model_download`), the completeness probe, and the
//! in-process Embedder load (`embedding`) share one layout description
//! instead of each maintaining its own copy.

use std::path::Path;

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

/// Read the ONNX weights for the Embedder load, trying
/// [`MODEL_ONNX_CANDIDATES`] in priority order and returning the first file
/// that reads successfully.
///
/// Existence alone does not guarantee readability — a leftover artifact can
/// be a regular file yet deny reads — so, like the read chain this module was
/// extracted from, a read error advances to the next candidate instead of
/// failing the load. The last candidate's error is reported when every read
/// fails.
pub fn read_first_onnx(dir: &Path) -> Result<Vec<u8>, String> {
    let mut last_error = None;
    for relative in MODEL_ONNX_CANDIDATES {
        match std::fs::read(dir.join(relative)) {
            Ok(bytes) => return Ok(bytes),
            Err(error) => last_error = Some(error),
        }
    }
    Err(format!(
        "单文件 ONNX 未找到(model.onnx 或 onnx/model_int8.onnx): {}",
        last_error
            .map(|error| error.to_string())
            .unwrap_or_default()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_candidate_wins_when_readable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.onnx"), b"root").unwrap();
        std::fs::create_dir(dir.path().join("onnx")).unwrap();
        std::fs::write(dir.path().join("onnx").join("model_int8.onnx"), b"int8").unwrap();
        std::fs::write(dir.path().join("onnx").join("model.onnx"), b"sub").unwrap();
        assert_eq!(read_first_onnx(dir.path()).unwrap(), b"root");
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_preferred_candidate_falls_through_to_readable_fallback() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.onnx"), b"unreadable").unwrap();
        std::fs::create_dir(dir.path().join("onnx")).unwrap();
        std::fs::write(dir.path().join("onnx").join("model_int8.onnx"), b"fallback").unwrap();
        std::fs::set_permissions(
            dir.path().join("model.onnx"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        assert_eq!(read_first_onnx(dir.path()).unwrap(), b"fallback");
    }

    #[test]
    fn missing_candidates_report_load_error() {
        let dir = tempfile::tempdir().unwrap();
        let error = read_first_onnx(dir.path()).unwrap_err();
        assert!(error.contains("单文件 ONNX 未找到"), "unexpected: {error}");
    }
}
