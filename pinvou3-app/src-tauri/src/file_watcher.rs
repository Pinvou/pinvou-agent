//! 文件系统 watcher：监听 `~/.pinvou3/sessions/` 目录树，发现新文件 / 修改时
//! emit `artifact:disk` 事件到前端，由前端按 session_id 归属到对应产物列表。
//!
//! 为什么需要：write_file 工具调用前端能捕获，但 AI 产出 docx/xlsx 时
//! 走 exec_shell + pandoc，args 是命令字符串没有结构化 path,前端识别不到。
//! 用 OS file watcher 兜底:任何方式落到 session workspace 的文件都能被发现。
//!
//! 路径推断：`~/.pinvou3/sessions/<id>/workspace/<file>` → session_id = <id>。
//! 不在 workspace 子目录下的事件(如 sessions/<id>.json session 元数据本身)忽略。
//!
//! Debouncing：notify 后端(inotify)对单个文件写入可能 fire 多次事件
//! (Create + 多次 Modify(Data) + 最后 Modify(Metadata))。watcher 端不 debounce,
//! 由前端按 path 去重(trackArtifact 已经处理重复 path)。

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::json;
use tauri::{AppHandle, Emitter};

/// 启动后台 watcher 线程。spawn 后 return,watcher 自己跑直到 app 退出。
pub fn spawn(app: AppHandle, sessions_root: PathBuf) {
    thread::spawn(move || {
        if let Err(e) = std::fs::create_dir_all(&sessions_root) {
            eprintln!(
                "[file_watcher] create sessions_root {} failed: {e}",
                sessions_root.display()
            );
            return;
        }

        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        let mut watcher: RecommendedWatcher = match notify::recommended_watcher(tx) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("[file_watcher] init failed: {e}");
                return;
            }
        };

        if let Err(e) = watcher.watch(&sessions_root, RecursiveMode::Recursive) {
            eprintln!(
                "[file_watcher] watch({}) failed: {e}",
                sessions_root.display()
            );
            return;
        }
        eprintln!(
            "[file_watcher] watching {}",
            sessions_root.display()
        );

        for result in rx {
            match result {
                Ok(event) => handle_event(&app, &event, &sessions_root),
                Err(e) => eprintln!("[file_watcher] event error: {e}"),
            }
        }
    });
}

fn handle_event(app: &AppHandle, ev: &Event, root: &Path) {
    // 只处理这几类事件:
    // - Create        → 新文件
    // - Modify(Data)  → 文件写入
    // - Modify(Name)  → rename (Nautilus 删除 = mv 到 trash,触发这个)
    // - Remove        → unlink
    // 实际 removed/upsert 由 path 当前是否存在决定 (rename out 后 path 不存在)。
    if !matches!(
        ev.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) {
        return;
    }
    for path in &ev.paths {
        let Some((session_id, rel)) = parse_session_relative(path, root) else {
            continue;
        };
        // 只关心落在 sessions/<id>/workspace/ 下的产物
        if !rel.starts_with("workspace/") && !rel.starts_with("workspace\\") {
            continue;
        }
        // 跳过 LibreOffice/Word/编辑器临时文件 (.~lock / ~$ / .swp 等)
        let basename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if should_skip(basename) {
            continue;
        }
        // 按 path 当前存在性判定事件类型 ——
        // - Nautilus 删除 = mv to trash → Modify(Name) + 文件不存在 → removed
        // - LLM 写完 = Create / Modify(Data) → 文件存在 → upsert
        let event_type = if path.exists() { "upsert" } else { "removed" };
        let _ = app.emit(
            "artifact:disk",
            json!({
                "session_id": session_id,
                "path": path.to_string_lossy(),
                "event": event_type,
            }),
        );
    }
}

/// 跳过隐藏 / 编辑器临时文件 —— LibreOffice `.~lock.xxx#` / MS Word `~$xxx`
/// / vim `.xxx.swp` / 通用 `.` 开头的隐藏文件 / `.tmp` 后缀。
fn should_skip(basename: &str) -> bool {
    if basename.is_empty() { return true; }
    if basename.starts_with('.') { return true; }    // .~lock / 任意 dot file
    if basename.starts_with("~$") { return true; }   // MS Word 临时锁
    if basename.ends_with('~') { return true; }      // emacs backup
    if basename.ends_with(".swp") || basename.ends_with(".swo") { return true; }
    if basename.ends_with(".tmp") || basename.ends_with(".bak") { return true; }
    false
}

/// 从绝对 path 提取 `(session_id, relative_path)`。
/// 路径形如 `<sessions_root>/<id>/workspace/foo.md` → ("id", "workspace/foo.md")。
fn parse_session_relative(path: &Path, root: &Path) -> Option<(String, String)> {
    let rel = path.strip_prefix(root).ok()?;
    let mut comps = rel.components();
    let session = comps.next()?.as_os_str().to_str()?.to_string();
    let after = comps.as_path().to_string_lossy().to_string();
    if session.is_empty() || after.is_empty() {
        return None;
    }
    Some((session, after))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_relative_basic() {
        let root = PathBuf::from("/home/x/.pinvou3/sessions");
        let p = PathBuf::from("/home/x/.pinvou3/sessions/abc123/workspace/foo.md");
        let (id, rel) = parse_session_relative(&p, &root).unwrap();
        assert_eq!(id, "abc123");
        assert_eq!(rel, "workspace/foo.md");
    }

    #[test]
    fn parse_relative_artifacts_subdir() {
        let root = PathBuf::from("/home/x/.pinvou3/sessions");
        let p = PathBuf::from("/home/x/.pinvou3/sessions/abc/artifacts/y.txt");
        let (id, rel) = parse_session_relative(&p, &root).unwrap();
        assert_eq!(id, "abc");
        assert_eq!(rel, "artifacts/y.txt");
    }

    #[test]
    fn should_skip_temp_files() {
        assert!(should_skip(".~lock.foo.docx#"));   // LibreOffice
        assert!(should_skip("~$report.docx"));      // MS Word
        assert!(should_skip(".hidden"));            // 通用 dot file
        assert!(should_skip(".bashrc.swp"));        // vim swap
        assert!(should_skip("draft.tmp"));
        assert!(!should_skip("report.docx"));
        assert!(!should_skip("人类文档.docx"));     // 正常中文文件名
        assert!(!should_skip("plan.md"));
    }

    #[test]
    fn parse_relative_rejects_outside() {
        let root = PathBuf::from("/home/x/.pinvou3/sessions");
        let p = PathBuf::from("/etc/passwd");
        assert!(parse_session_relative(&p, &root).is_none());
    }
}
