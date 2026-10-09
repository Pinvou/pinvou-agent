//! 文件 sha256 摘要，供 voice/native_installer/knowledge 等多处复用。
//!
//! 与各调用方原实现等价：File::open → 1 MiB(或更大)缓冲循环 →
//! crate::platform::encoding::hex_lower(finalize)。返回 io::Result，
//! 由调用方各自转换为 Result<_, String> 以保留原中文错误文案。
//! Round-49: `sha256_file` additionally refuses non-regular paths
//! (symlinks/FIFOs — see its doc) before the open.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

/// 计算字节串 sha256，返回小写十六进制字符串。小缓冲场景（指纹/内容寻址键/
/// 完整性校验）统一走这里；无界输入（文件、下载流）各自流式处理，不得
/// 整体读入内存后再调本函数。
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    crate::platform::encoding::hex_lower(&Sha256::digest(bytes))
}

/// 计算文件 sha256，返回小写十六进制字符串。
///
/// Round-49 hardening — the extract walk's regular-file wall extended to this
/// module's own door: a plain `File::open` FOLLOWS a planted symlink (the
/// target's bytes get hashed as if they were the caller's file) and BLOCKS
/// FOREVER on a planted FIFO (the read loop drains to EOF, and a FIFO never
/// EOFs while a writer lives — every hash-pinned install lane would hang
/// with no deadline). `symlink_metadata` does not follow the final path
/// component, so symlinks are refused even when they point at a regular
/// file, and only regular files are hashed. NOT atomic with the open below
/// (a path swapped in between still reaches `File::open`): it rejects the
/// ordinary case, it does not close the race — the same scope statement
/// `read_text_file_capped` makes on the CLI side. All callers verify
/// downloads, staged archives or store files, where a non-regular path is
/// exactly the plant this gate exists for.
///
/// `pub` + the platform-root re-export: the headless CLI verifies connector
/// CLI binaries and staged voice models with the app's own hashing instead of
/// a drifting copy (same crate-boundary shape as `external_command`).
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "not a regular file (symlinks and special files are refused): {}",
                path.display()
            ),
        ));
    }
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
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"hello world"),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
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

    /// Round-49: a planted FIFO must be refused, not opened — the hash loop
    /// reads to EOF and a FIFO never EOFs while a writer lives, so a plain
    /// open here hung every hash-pinned lane with no deadline. (A regular
    /// file still hashes: `sha256_file_matches_known_vector` and
    /// `sha256_file_empty_file` are that half of the gate's contract.)
    #[test]
    #[cfg(unix)]
    fn sha256_file_refuses_a_fifo() {
        let dir = std::env::temp_dir().join(format!(
            "pinvou3_hashing_fifo_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("planted.fifo");
        let cpath = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes())
            .expect("temp path has no NUL");
        // SAFETY: mkfifo(3) with a valid path; default mode, never opened.
        let rc = unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) };
        assert_eq!(rc, 0, "mkfifo must succeed in the temp dir");
        use std::os::unix::fs::FileTypeExt as _;
        assert!(
            std::fs::symlink_metadata(&fifo)
                .unwrap()
                .file_type()
                .is_fifo(),
            "the fixture must really be a FIFO"
        );
        let result = sha256_file(&fifo);
        let _ = std::fs::remove_dir_all(&dir);
        let error = result.expect_err("a planted FIFO must be refused before any open");
        assert!(
            error.to_string().contains("not a regular file"),
            "the refusal must say why: {error}"
        );
    }

    /// Round-49: a planted symlink is refused even when it points at a
    /// regular file — `symlink_metadata` does not follow the final component,
    /// so the hash never reads bytes the caller did not point at.
    #[test]
    #[cfg(unix)]
    fn sha256_file_refuses_a_symlink_even_to_a_regular_file() {
        let dir = std::env::temp_dir().join(format!(
            "pinvou3_hashing_symlink_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let victim = dir.join("victim.txt");
        std::fs::write(&victim, b"hello world").unwrap();
        let link = dir.join("planted.link");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        let result = sha256_file(&link);
        let _ = std::fs::remove_dir_all(&dir);
        let error = result.expect_err("a planted symlink must be refused, not followed");
        assert!(
            error.to_string().contains("not a regular file"),
            "the refusal must say why: {error}"
        );
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
