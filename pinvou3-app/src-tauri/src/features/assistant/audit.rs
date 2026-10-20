// architecture-guard: allow-target-cfg -- the round-21 FIFO-pin test
// below needs mkfifo/S_IFIFO (POSIX-only, runtime-skipped elsewhere) to
// pin the append open's planted-FIFO refusal; the gate itself lives in
// the platform primitive open_private_append_file.
//!
//! 助手审计流水：append-only JSONL。
//!
//! 一行 = 一个事件：{"ts":"`<RFC3339>`","kind":"...","role":"...","detail":{...}}
//! 文件在 session workspace 根：`<ws>/workflow_audit.jsonl`。
//! 只追加不改写；写失败只 eprintln 不 panic（审计绝不能弄死工作流）。
//! 单 run 实测量级 <100KB,暂不轮转;若未来常驻累积 >50MB 再加 rotation。

use std::io::Write;
use std::path::Path;

/// 追加一条审计记录。`entry` 不必带 ts，本函数注入。
pub fn append(ws: &Path, kind: &str, role: &str, detail: serde_json::Value) {
    let line = serde_json::json!({
        "ts": chrono::Utc::now().to_rfc3339(),
        "kind": kind,
        "role": role,
        "detail": detail,
    });
    let path = ws.join("workflow_audit.jsonl");
    // 单次 write_all(含换行)——writeln 是两次 write(2),并发 append 时
    // 另一个写者可插在 JSON 体和换行之间,撕裂行边界。
    let mut buf = line.to_string();
    buf.push('\n');
    // Round-17 MAJOR-2: the append open is a path open on a user-writable
    // trust boundary — a planted FIFO at the jsonl path blocked the
    // caller's open forever (the watcher's single drain task on the
    // scheduled-creation paths). The platform gated open refuses
    // non-regular entries (O_NOFOLLOW|O_NONBLOCK, 0600) and the portable
    // fstat below refuses anything non-regular that still made it
    // through. The write-failure contract (eprintln, never panic) is
    // unchanged.
    let r = crate::platform::filesystem::open_private_append_file(&path)
        .and_then(|mut file| file.write_all(buf.as_bytes()));
    if let Err(e) = r {
        // Round-26 minor 3, round-31 minor 10 wording: the audit stream
        // must be observable where logs go, not only on a stderr the app
        // may not surface — the write-failure CONTRACT is unchanged (log,
        // never panic); only the channel moved from eprintln to log::warn.
        log::warn!("[audit] append failed ({}): {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_creates_file_and_appends_lines() {
        struct TempDir(std::path::PathBuf);
        impl Drop for TempDir {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).ok();
            }
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let guard = TempDir(std::env::temp_dir().join(format!(
            "audit_test_{}_{}",
            std::process::id(),
            nanos
        )));
        let dir = guard.0.clone();
        std::fs::create_dir_all(&dir).unwrap();
        append(
            &dir,
            "dispatch",
            "slide_writer",
            serde_json::json!({"note":"第一条"}),
        );
        append(
            &dir,
            "token",
            "slide_writer",
            serde_json::json!({"input":100,"output":50}),
        );
        let content = std::fs::read_to_string(dir.join("workflow_audit.jsonl")).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        // Round-22 M1 (fixing the round-21 pin, which planted the FIFO at
        // an unused .fifo path — append only ever opens the jsonl, so that
        // pin was vacuous): plant the FIFO at the EXACT jsonl path after
        // the legitimate lines are asserted. The gated open must refuse
        // (fail, not block — the test completing is the no-wedge pin), the
        // append must not write into the pipe buffer, and the FIFO must be
        // untouched (no rename can install it as the log).
        #[cfg(unix)]
        {
            let jsonl = dir.join("workflow_audit.jsonl");
            std::fs::remove_file(&jsonl).unwrap();
            let c = std::ffi::CString::new(jsonl.as_os_str().to_str().unwrap()).unwrap();
            assert_eq!(
                unsafe { libc::mkfifo(c.as_ptr(), libc::S_IFIFO | 0o644) },
                0,
                "fixture: the FIFO must be created"
            );
            append(&dir, "dispatch", "slide_writer", serde_json::json!({"n":1}));
            let meta = std::fs::metadata(&jsonl).unwrap();
            use std::os::unix::fs::FileTypeExt as _;
            assert!(meta.file_type().is_fifo(), "the FIFO must be untouched");
        }
        for l in &lines {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            assert!(v.get("ts").and_then(|t| t.as_str()).is_some());
            assert!(v.get("kind").is_some());
        }
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["kind"], "dispatch");
        assert_eq!(first["role"], "slide_writer");
    }

    #[test]
    fn append_to_missing_dir_does_not_panic() {
        // 写失败只打日志不 panic —— 审计绝不能弄死工作流
        let dir = std::path::PathBuf::from("/nonexistent_dir_for_audit_test");
        append(&dir, "x", "y", serde_json::json!({}));
    }
}
