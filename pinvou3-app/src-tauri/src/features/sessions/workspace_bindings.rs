//! 普通 chat 会话的用户工作目录绑定 sidecar。
//!
//! 普通（assistant/engine）会话创建时，前端可让用户选择一个工作目录；绑定后
//! [`SessionStore::session_roots`] 把该会话的 execution 根解析到绑定目录
//! （engine cwd），账本根仍为会话私有目录——与原生代码会话的项目目录绑定共享
//! `session_roots_for` 的双根语义，但数据来源是本模块自持有的
//! `_session_workspaces.json`（`{session_id: path_string}` JSON 对象），不经
//! `execution_root_resolver` 闭包（那只解析 codex_acp 原生代码会话）。
//!
//! 底座 `SavedSession` 不能加字段，故与 `_session_models.json` 一样走独立
//! sidecar。绑定即权威：绑定目录被删后由 engine spawn 路径重建，不做回退。

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::{Context, Result};

use super::SessionStore;

const SESSION_WORKSPACES_FILE: &str = "_session_workspaces.json";

impl SessionStore {
    /// 绑定会话工作目录并原子落盘。落盘失败时回滚内存插入并返回 Err——
    /// 调用方（create_session 命令）据此删除刚建的空会话，不留下内存与盘面
    /// 不一致的「看似绑定成功、重启后丢失」的会话。
    pub fn bind_session_workspace(&self, id: &str, path: PathBuf) -> Result<()> {
        let mut bindings = self.session_workspaces.write();
        let previous = bindings.insert(id.to_string(), path);
        if let Err(error) = Self::persist_session_workspaces(&bindings) {
            match previous {
                Some(previous) => {
                    bindings.insert(id.to_string(), previous);
                }
                None => {
                    bindings.remove(id);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    /// 读取会话的用户工作目录绑定（无绑定 → None，execution 根回退会话私有目录）。
    pub fn session_workspace_binding(&self, id: &str) -> Option<PathBuf> {
        self.session_workspaces.read().get(id).cloned()
    }

    /// 移除绑定；有变化才落盘（空表时删除 sidecar 文件）。
    pub(crate) fn remove_session_workspace(&self, id: &str) {
        if self.session_workspaces.write().remove(id).is_some() {
            self.save_session_workspaces();
        }
    }

    pub(crate) fn persist_session_workspaces(bindings: &HashMap<String, PathBuf>) -> Result<()> {
        let file = crate::platform::paths::sessions_root().join(SESSION_WORKSPACES_FILE);
        if bindings.is_empty() {
            return match std::fs::remove_file(&file) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error).with_context(|| format!("remove {}", file.display())),
            };
        }
        let payload =
            serde_json::to_vec_pretty(bindings).context("serialize session workspace bindings")?;
        crate::platform::filesystem::atomic_write(&file, &payload)
            .with_context(|| format!("persist session workspace bindings to {}", file.display()))
    }

    pub(crate) fn save_session_workspaces(&self) {
        if let Err(error) = Self::persist_session_workspaces(&self.session_workspaces.read()) {
            eprintln!("[sessions] save_session_workspaces failed: {error:#}");
        }
    }

    /// boot 时恢复工作目录绑定。清理 ghost 条目（对应 `<id>.json` 已不存在的
    /// 绑定——会话在进程外被删的残留）并重写文件，模式同 `load_multi_agent_flags`。
    pub fn load_session_workspaces(&self) {
        let file = crate::platform::paths::sessions_root().join(SESSION_WORKSPACES_FILE);
        let Ok(content) = std::fs::read_to_string(&file) else {
            return;
        };
        let bindings: HashMap<String, PathBuf> = match serde_json::from_str(&content) {
            Ok(bindings) => bindings,
            Err(e) => {
                eprintln!("[sessions] load_session_workspaces failed: {e}");
                return;
            }
        };
        let sessions_dir = self.manager.sessions_dir().to_path_buf();
        let mut ghosts = false;
        {
            let mut persisted = self.session_workspaces.write();
            persisted.clear();
            for (id, path) in bindings {
                if sessions_dir.join(format!("{id}.json")).is_file() {
                    persisted.insert(id, path);
                } else {
                    ghosts = true;
                }
            }
        }
        if ghosts {
            self.save_session_workspaces();
        }
    }
}
