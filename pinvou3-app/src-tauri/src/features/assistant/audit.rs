//! 助手审计流水：append-only JSONL。
//!
//! 一行 = 一个事件：{"ts":"<RFC3339>","kind":"...","role":"...","detail":{...}}
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
    let r = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(buf.as_bytes()));
    if let Err(e) = r {
        eprintln!("[audit] append failed ({}): {e}", path.display());
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
