//! 多对话管理 wrapper。
//!
//! 复用 deepseek-tui 上游 [`SessionManager`]（已支持 `new(custom_dir)`），
//! 把 sessions 目录定向到 `~/.pinvou3/sessions/`（隔离 `~/.deepseek/`）。
//!
//! 暴露给 pinvou3-app Tauri commands 的能力：
//! - `list` —— 列出所有会话元数据（前端历史面板）
//! - `create_new` —— 新建空会话（首次未发送消息前）
//! - `load` —— 读完整对话（切换 session 时给 engine 通过 `Op::SyncSession` 注入）
//! - `save` —— 持久化（每轮 turn 完成 auto-save）
//! - `delete` —— 删除会话 + artifacts 目录
//! - `set_title` —— 重命名
//! - `active_id` / `set_active` —— 跟踪当前 active session（chat command 用）
//!
//! **Arc + RwLock 包装**：所有字段都是 `Arc`，整个 `SessionStore` 可以
//! 廉价 Clone 进 Tauri State + 多个 task 共享。

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use deepseek_tui::models::Message;
use deepseek_tui::session_manager::{
    create_saved_session_with_id_and_mode, SavedSession, SessionManager, SessionMetadata,
};
use parking_lot::RwLock;

use super::paths;

/// pinvou3 session 存储：包 SessionManager + active id 跟踪。
#[derive(Clone)]
pub struct SessionStore {
    manager: Arc<SessionManager>,
    active: Arc<RwLock<Option<String>>>,
}

impl SessionStore {
    /// 用 `~/.pinvou3/sessions/` 初始化。如果目录不存在会自动创建。
    pub fn boot() -> Result<Self> {
        let dir = paths::sessions_root();
        let manager = SessionManager::new(dir.clone())
            .with_context(|| format!("SessionManager::new({}) failed", dir.display()))?;
        Ok(Self {
            manager: Arc::new(manager),
            active: Arc::new(RwLock::new(None)),
        })
    }

    /// 列出所有 session 元数据，按 updated_at 倒序（最新在前）。
    pub fn list(&self) -> Result<Vec<SessionMetadata>> {
        let mut out = self
            .manager
            .list_sessions()
            .context("list_sessions failed")?;
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(out)
    }

    /// 加载完整 session（包含所有 messages）。
    pub fn load(&self, id: &str) -> Result<SavedSession> {
        self.manager
            .load_session(id)
            .with_context(|| format!("load_session({id})"))
    }

    /// 落盘整个 session（atomic write 由上游处理）。
    pub fn save(&self, session: &SavedSession) -> Result<PathBuf> {
        self.manager
            .save_session(session)
            .context("save_session failed")
    }

    /// 删除 session（含 artifacts 子目录）。
    pub fn delete(&self, id: &str) -> Result<()> {
        self.manager
            .delete_session(id)
            .with_context(|| format!("delete_session({id})"))?;
        // 如果删的是 active session，清理 active 标记
        let mut active = self.active.write();
        if active.as_deref() == Some(id) {
            *active = None;
        }
        Ok(())
    }

    /// 重命名：load → 改 metadata.title → save。
    pub fn set_title(&self, id: &str, title: String) -> Result<()> {
        let mut session = self.load(id)?;
        session.metadata.title = title;
        self.save(&session)?;
        Ok(())
    }

    /// 新建空 session（无 messages）。返回 SavedSession 让调用方
    /// 立刻 `Op::SyncSession` 同步给 engine，并 set_active(id)。
    /// 上游空消息时 title 默认 "New Session"，pinvou3 覆写成中文。
    pub fn create_new(&self, model: String, workspace: PathBuf) -> Result<SavedSession> {
        let id = generate_session_id();
        let mut session =
            create_saved_session_with_id_and_mode(id, &[], &model, &workspace, 0, None, None);
        session.metadata.title = "新对话".to_string();
        self.save(&session)?;
        Ok(session)
    }

    /// 替换 session 的 messages 数组并刷新 updated_at / message_count。
    /// 前端每轮 TurnComplete 后调用，把 messages 数组同步给后端持久化。
    /// total_tokens 暂时不维护（前端没拿到 usage 数据），保持原值。
    pub fn update_messages(&self, id: &str, messages: Vec<Message>) -> Result<()> {
        let mut session = self.load(id)?;
        session.metadata.message_count = messages.len();
        session.metadata.updated_at = Utc::now();
        session.messages = messages;
        self.save(&session)?;
        Ok(())
    }

    pub fn active_id(&self) -> Option<String> {
        self.active.read().clone()
    }

    pub fn set_active(&self, id: Option<String>) {
        *self.active.write() = id;
    }
}

/// 生成 URL-safe session id（短 8 字节 timestamp + nanos hash）。
/// 上游 `validated_session_path` 只允许 `[A-Za-z0-9_-]`，所以走 base32-like 字符集。
fn generate_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    const ALPHA: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut n = nanos;
    let mut buf = String::with_capacity(13);
    for _ in 0..13 {
        buf.push(ALPHA[(n % 36) as usize] as char);
        n /= 36;
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::paths::tests::ENV_LOCK;

    /// 借用 paths 模块的进程级 env 锁——避免与其他 mutate PINVOU3_HOME
    /// 的测试并行 race。返回带 guard 的 store；guard drop 后才解锁。
    fn isolated_store() -> (SessionStore, std::sync::MutexGuard<'static, ()>) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = format!(
            "/tmp/pinvou3-sessions-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        std::env::set_var("PINVOU3_HOME", &tmp);
        let store = SessionStore::boot().expect("boot");
        // 注意：不 remove_var——锁还没 drop，下面的断言需要 PINVOU3_HOME 仍是这个值。
        (store, guard)
    }

    #[test]
    fn create_new_persists_and_lists() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), std::env::temp_dir())
            .expect("create");
        let list = store.list().expect("list");
        assert!(list.iter().any(|m| m.id == s.metadata.id));
    }

    #[test]
    fn set_title_updates_metadata() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), std::env::temp_dir())
            .expect("create");
        store
            .set_title(&s.metadata.id, "改个名字".into())
            .expect("rename");
        let loaded = store.load(&s.metadata.id).expect("load");
        assert_eq!(loaded.metadata.title, "改个名字");
    }

    #[test]
    fn delete_removes_session() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), std::env::temp_dir())
            .expect("create");
        store.delete(&s.metadata.id).expect("delete");
        assert!(store.load(&s.metadata.id).is_err(), "load after delete");
    }

    #[test]
    fn active_id_tracks_set_active() {
        let (store, _g) = isolated_store();
        assert!(store.active_id().is_none());
        store.set_active(Some("abc".into()));
        assert_eq!(store.active_id().as_deref(), Some("abc"));
        store.set_active(None);
        assert!(store.active_id().is_none());
    }

    #[test]
    fn delete_active_clears_active_id() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), std::env::temp_dir())
            .expect("create");
        store.set_active(Some(s.metadata.id.clone()));
        store.delete(&s.metadata.id).expect("delete");
        assert!(store.active_id().is_none(), "delete active clears tracker");
    }

    #[test]
    fn generate_session_id_url_safe() {
        let id = generate_session_id();
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }
}
