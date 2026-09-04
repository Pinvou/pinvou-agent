//! 文件系统 watcher：监听 `~/.pinvou3/sessions/` 目录树，发现新文件 / 修改时
//! emit `artifact:disk` 事件到前端，由前端按 session_id 归属到对应产物列表；
//! 定时会话 JSON 变化时另发一个轻量事件，让全局运行记录及时刷新。
//!
//! 为什么需要：write_file 工具调用前端能捕获，但 AI 产出 docx/xlsx 时
//! 走 exec_shell + pandoc，args 是命令字符串没有结构化 path,前端识别不到。
//! 用 OS file watcher 兜底:任何方式落到 session workspace 的文件都能被发现。
//!
//! 路径推断：
//! - `~/.pinvou3/sessions/<id>/workspace/<file>` → `session_id = <id>`
//! - `~/.pinvou3/sessions/<id>/artifacts/<file>` → `session_id = <id>`
//!
//! 其中 `sessions/default/artifacts/` 是 MCP 办公产物的公共落点；文件系统
//! watcher 不能据此判断真实对话归属，真实归属以带 `session_id` 的工具事件为准。
//! 不在 workspace/artifacts 子目录下的事件(如 `sessions/<id>.json` session 元数据本身)忽略。
//!
//! Debouncing：notify 后端(inotify)对单个文件写入可能 fire 多次事件
//! (Create + 多次 Modify(Data) + 最后 Modify(Metadata))。产物事件(artifact:disk)
//! watcher 端不 debounce,由前端按 path 去重(trackArtifact 已经处理重复 path)。
//! 例外：顶层 `sched-*.json` 在一次定时运行里随每条消息整体重写,成片事件只
//! 应换来一次刷新,因此按 path 做 400ms 尾沿去抖(scheduled_task:run_updated
//! 只驱动前端刷新,payload 的 event 字段无消费者,延迟发射不损语义)。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::json;
use tauri::{AppHandle, Emitter};

/// 顶层 `sched-*.json` 事件的尾沿去抖窗口：最后一次事件后静默满窗口才发射。
const SCHED_DEBOUNCE: Duration = Duration::from_millis(400);

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
        eprintln!("[file_watcher] watching {}", sessions_root.display());

        // sched-*.json 的待发射表：session_id -> (路径, 最后一次事件时刻)。
        // 主循环在等下一条事件与等最早到期之间 recv_timeout，到期的先发射。
        let mut pending_sched: HashMap<String, (PathBuf, Instant)> = HashMap::new();
        loop {
            let now = Instant::now();
            let due: Vec<String> = pending_sched
                .iter()
                .filter(|(_, (_, at))| now.duration_since(*at) >= SCHED_DEBOUNCE)
                .map(|(id, _)| id.clone())
                .collect();
            for id in due {
                if let Some((path, _)) = pending_sched.remove(&id) {
                    emit_scheduled_run_updated(&app, &id, path.exists());
                }
            }
            let deadline = pending_sched
                .values()
                .map(|(_, at)| *at + SCHED_DEBOUNCE)
                .min();
            let event = match deadline {
                Some(deadline) => {
                    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                        Ok(result) => Some(result),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                None => match rx.recv() {
                    Ok(result) => Some(result),
                    Err(_) => break,
                },
            };
            match event {
                Some(Ok(event)) => {
                    handle_event(&app, &event, &sessions_root, &mut pending_sched);
                }
                Some(Err(e)) => eprintln!("[file_watcher] event error: {e}"),
                None => continue,
            }
        }
    });
}

fn handle_event(
    app: &AppHandle,
    ev: &Event,
    root: &Path,
    pending_sched: &mut HashMap<String, (PathBuf, Instant)>,
) {
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
        if let Some(session_id) = scheduled_session_id(path, root) {
            // 只登记、不立即发射：由主循环尾沿去抖后统一发出；event 语义
            // （upsert/removed）延迟到发射时刻按 path 存在性现判。
            pending_sched.insert(session_id, (path.clone(), Instant::now()));
            continue;
        }
        let Some((session_id, rel)) = parse_session_relative(path, root) else {
            continue;
        };
        // 两类要关心的产物:
        // ① `sessions/<id>/workspace/` 下的(write_file 等的常规产出)。
        // ② `sessions/<id>/artifacts/` 下的 session 专属产物。
        // `sessions/default/artifacts/` 是 MCP 办公产物公共落点，前端会跳过它的
        // watcher 事件，真实归属以 chat:tool_end 的 session_id 为准。
        let in_workspace = rel.starts_with("workspace/") || rel.starts_with("workspace\\");
        let in_artifacts = rel.starts_with("artifacts/") || rel.starts_with("artifacts\\");
        if !in_workspace && !in_artifacts {
            continue;
        }
        // 跳过 LibreOffice/Word/编辑器临时文件 (.~lock / ~$ / .swp 等)
        let basename = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if should_skip(basename) {
            continue;
        }
        // 只推**顶层目录的文件**:子目录(tmp/ _state/ .deepseek/ 等 infra)里的文件、
        // 以及目录本身,都不该进产物面板(中间/工作流基础设施,非成品)。
        let sub = if in_workspace {
            rel.trim_start_matches("workspace/")
                .trim_start_matches("workspace\\")
        } else {
            rel.trim_start_matches("artifacts/")
                .trim_start_matches("artifacts\\")
        };
        if sub.contains('/') || sub.contains('\\') {
            continue; // 子目录内容(scratch / 工作流 infra)
        }
        if path.is_dir() {
            continue; // 目录本身不是产物(removed 时路径已不存在 → 放行让前端兜底 untrack)
        }
        // 按 path 当前存在性判定事件类型 ——
        // - Nautilus 删除 = mv to trash → Modify(Name) + 文件不存在 → removed
        // - LLM 写完 = Create / Modify(Data) → 文件存在 → upsert
        let event_type = if path.exists() { "upsert" } else { "removed" };
        let payload = json!({
            "session_id": session_id,
            "path": path.to_string_lossy(),
            "event": event_type,
        });
        let _ = app.emit("artifact:disk", payload.clone());
        crate::platform::app_events::forward_app_event(app, "artifact:disk", payload);
    }
}

/// 发射一次定时运行刷新事件。`exists` 在发射时刻按 path 现判，保留原有
/// upsert/removed 语义（当前前端两个监听方都只据此触发刷新、不读 event）。
fn emit_scheduled_run_updated(app: &AppHandle, session_id: &str, exists: bool) {
    let payload = json!({
        "sessionId": session_id,
        "event": if exists { "upsert" } else { "removed" },
    });
    let _ = app.emit("scheduled_task:run_updated", payload.clone());
    crate::platform::app_events::forward_app_event(app, "scheduled_task:run_updated", payload);
}

/// Match only the top-level persisted JSON for a scheduled conversation.
/// Sidecars and workspace files are deliberately excluded.
fn scheduled_session_id(path: &Path, root: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    if rel.components().count() != 1
        || path.extension().and_then(|ext| ext.to_str()) != Some("json")
    {
        return None;
    }
    let id = path.file_stem()?.to_str()?;
    id.starts_with("sched-").then(|| id.to_string())
}

/// 跳过隐藏 / 编辑器临时文件 —— LibreOffice `.~lock.xxx#` / MS Word `~$xxx`
/// / vim `.xxx.swp` / 通用 `.` 开头的隐藏文件 / `.tmp` 后缀。
fn should_skip(basename: &str) -> bool {
    if basename.is_empty() {
        return true;
    }
    if basename.starts_with('.') {
        return true;
    } // .~lock / 任意 dot file
    if basename.starts_with("~$") {
        return true;
    } // MS Word 临时锁
    if basename.ends_with('~') {
        return true;
    } // emacs backup
    if basename.ends_with(".swp") || basename.ends_with(".swo") {
        return true;
    }
    if basename.ends_with(".tmp") || basename.ends_with(".bak") {
        return true;
    }
    // 工作流内部审计流水（assistant/audit.rs，逐条 TokenUsage 追加），纯基础
    // 设施、前端零消费；不跳过会随每次子代理事件刷一遍产物面板。
    if basename == "workflow_audit.jsonl" {
        return true;
    }
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
        assert!(should_skip(".~lock.foo.docx#")); // LibreOffice
        assert!(should_skip("~$report.docx")); // MS Word
        assert!(should_skip(".hidden")); // 通用 dot file
        assert!(should_skip(".bashrc.swp")); // vim swap
        assert!(should_skip("draft.tmp"));
        assert!(should_skip("workflow_audit.jsonl")); // 工作流内部审计流水
        assert!(!should_skip("report.docx"));
        assert!(!should_skip("人类文档.docx")); // 正常中文文件名
        assert!(!should_skip("plan.md"));
    }

    #[test]
    fn parse_relative_rejects_outside() {
        let root = PathBuf::from("/home/x/.pinvou3/sessions");
        let p = PathBuf::from("/etc/passwd");
        assert!(parse_session_relative(&p, &root).is_none());
    }

    #[test]
    fn scheduled_session_event_matches_only_top_level_payload() {
        let root = PathBuf::from("/home/x/.pinvou3/sessions");
        let payload = root.join("sched-123.json");
        assert_eq!(
            scheduled_session_id(&payload, &root).as_deref(),
            Some("sched-123")
        );
        assert!(scheduled_session_id(&root.join("chat-123.json"), &root).is_none());
        assert!(scheduled_session_id(&root.join("_hidden_sessions.json"), &root).is_none());
        assert!(
            scheduled_session_id(
                &root.join("sched-123").join("workspace").join("report.md"),
                &root
            )
            .is_none()
        );
    }
}
