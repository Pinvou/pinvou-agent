//! 文件 sha256 摘要，供 voice/native_installer/knowledge 等多处复用。
//!
//! 与各调用方原实现等价：File::open → 1 MiB(或更大)缓冲循环 →
//! crate::platform::encoding::hex_lower(finalize)。返回 io::Result，
//! 由调用方各自转换为 Result<_, String> 以保留原中文错误文案。

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

/// 计算文件 sha256，返回小写十六进制字符串。
pub(crate) fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(crate::platform::encoding::hex_lower(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用 std::env::temp_dir 生成临时文件（**不新增 tempfile 依赖**——已确认
    /// Cargo.toml 无 tempfile/mockito；遵循「新增依赖须告知用户」公约）。
    fn scratch_file(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "pinvou3_hashing_test_{name}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn sha256_file_matches_known_vector() {
        let p = scratch_file("hello");
        std::fs::write(&p, b"hello world").unwrap();
        // "hello world" 的 sha256:
        let expect = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        assert_eq!(sha256_file(&p).unwrap(), expect);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn sha256_file_missing_path_errors() {
        let r = sha256_file(Path::new("/nonexistent/__definitely_not_here__"));
        assert!(r.is_err());
    }

    #[test]
    fn sha256_file_empty_file() {
        let p = scratch_file("empty");
        std::fs::write(&p, b"").unwrap();
        // 空文件的 sha256:
        let expect = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(sha256_file(&p).unwrap(), expect);
        let _ = std::fs::remove_file(&p);
    }
}
