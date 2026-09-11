//! 普通 chat 会话的用户工作目录绑定 sidecar（per-session，随会话目录存续）。
//!
//! 普通（assistant/engine）会话创建时，前端可让用户选择一个工作目录；绑定后
//! [`SessionStore::session_roots`] 把该会话的 execution 根解析到绑定目录
//! （engine cwd），账本根仍为会话私有目录——与原生代码会话的项目目录绑定共享
//! `session_roots_for` 的双根语义。app 组合根注入 bridge/SessionStore 的
//! `execution_root_resolver` 闭包在原生代码会话未命中时回退查这里，故 bridge
//! 侧（提示词环境段、AGENTS.md 注入、连接器 scope、审计根）对两类绑定会话
//! 行为一致——绑定目录同为 prompt-injection 面，安全姿态跟绑定不跟模式。
//!
//! 存储形态（绑定存储收敛）：绑定记录是会话私有目录内的 per-session sidecar
//! `<sessions>/<id>/workspace-binding.json`，与原生代码会话的
//! `code-session.json` 同一机制——绑定随会话目录存续，删目录即随之消失，
//! 不再有全局表的 boot 期 ghost 清理。内存 `session_workspaces` 退化为读
//! 缓存：bind 时写入、读 miss 时从 sidecar 回填（跨进程新绑定同样可见）、
//! 删除/保留策略清理时清除。
//!
//! 存量全局表 `_session_workspaces.json` 是本 PR 开发期的中间格式，从未随
//! `main` 发布（评审 #445 P2）；boot 期
//! [`SessionStore::migrate_legacy_session_workspaces`] 只为收敛中间版本
//! dev build 的 home：活会话条目逐条写成 sidecar 后删除旧文件；写失败的
//! 条目留在旧文件原样不动（本次运行内由内存表接管解析），下次 boot 重试，
//! 不阻断启动。中间版本存量迁完后该迁移即成恒 no-op（文件缺失直接返回）。

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// rebind_workspace_bindings 的结果:已平移 + 逐条目失败隔离(评审 #464
/// MAJOR 4:非法 id 或单条写失败不再中断整轮,进失败名单由命令层并入报告)。
#[derive(Debug, Default)]
pub struct RebindBindingsOutcome {
    pub rebound: Vec<(String, PathBuf)>,
    pub failed_session_ids: Vec<String>,
}

/// 折叠身份键下的「等于或嵌套于」前缀判定:Windows 折叠分隔符与大小写,
/// 与 store 的 key_is_same_or_nested 同约定;空 from 匹配一切(本文件两处
/// 候选扫描共用——评审 #464 MINOR 8:同一闭包曾字节级重复且语义已与
/// 项目层漂移)。
fn folded_covers(from: &Path, path: &Path) -> bool {
    let from_key = crate::platform::os::filesystem_path_identity_key(&from.to_string_lossy());
    let from_trim = from_key.trim_end_matches('/');
    let key = crate::platform::os::filesystem_path_identity_key(&path.to_string_lossy());
    let trim = key.trim_end_matches('/');
    from_trim.is_empty() || trim == from_trim || trim.starts_with(&format!("{from_trim}/"))
}

use super::{SessionStore, validate_session_id};

/// 绑定 sidecar 的 schema 版本；未来字段演进时用于迁移。
const SESSION_WORKSPACE_SIDECAR_VERSION: u32 = 1;
/// 本 PR 中间版本的全局绑定表（从未随 `main` 发布；boot 期迁移成功后删除）。
const LEGACY_SESSION_WORKSPACES_FILE: &str = "_session_workspaces.json";
/// per-session 绑定 sidecar 文件名（位于会话私有目录内）。
const SESSION_WORKSPACE_SIDECAR_FILE: &str = "workspace-binding.json";

/// 绑定 sidecar 内容。`path` 为 `validate_user_workspace_path` canonicalize
/// 后的绝对目录；`bound_at` 仅作元信息，不参与恢复语义。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionWorkspaceSidecar {
    version: u32,
    path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bound_at: Option<i64>,
}

fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

/// 未来高版本格式不能静默按当前版本解析：拒读并按缺失处理（bind 重写为
/// 当前版本即自愈），解析异常均记日志。
fn read_workspace_sidecar(path: &Path) -> Option<SessionWorkspaceSidecar> {
    let payload = std::fs::read(path).ok()?;
    match serde_json::from_slice::<SessionWorkspaceSidecar>(&payload) {
        Ok(sidecar) if sidecar.version <= SESSION_WORKSPACE_SIDECAR_VERSION => Some(sidecar),
        Ok(sidecar) => {
            eprintln!(
                "[sessions] workspace binding sidecar version {} above supported {} ({}), ignored",
                sidecar.version,
                SESSION_WORKSPACE_SIDECAR_VERSION,
                path.display()
            );
            None
        }
        Err(error) => {
            eprintln!(
                "[sessions] parse workspace binding sidecar failed ({}): {error}",
                path.display()
            );
            None
        }
    }
}

impl SessionStore {
    fn session_workspace_sidecar_path(&self, id: &str) -> PathBuf {
        self.manager
            .sessions_dir()
            .join(id)
            .join(SESSION_WORKSPACE_SIDECAR_FILE)
    }

    /// 绑定会话工作目录并原子落盘到会话私有目录内的 sidecar。要求会话的
    /// 持久记录（`<id>.json`）已存在——绑定是会话的从属数据，不为未知 id
    /// 凭空创建目录；调用方（create_session 命令）先建会话再绑定。
    /// 落盘失败返回 Err 且不触碰内存缓存——调用方（create_session 命令）
    /// 据此删除刚建的空会话，不留下「看似绑定成功、重启后丢失」的会话。
    pub fn bind_session_workspace(&self, id: &str, path: PathBuf) -> Result<()> {
        validate_session_id(id)?;
        let record = self.manager.sessions_dir().join(format!("{id}.json"));
        if !record.is_file() {
            anyhow::bail!("cannot bind workspace: session record {id} does not exist");
        }
        let sidecar = SessionWorkspaceSidecar {
            version: SESSION_WORKSPACE_SIDECAR_VERSION,
            path,
            bound_at: Some(now_unix_secs()),
        };
        let file = self.session_workspace_sidecar_path(id);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create session dir {}", parent.display()))?;
        }
        let payload =
            serde_json::to_vec_pretty(&sidecar).context("serialize session workspace binding")?;
        crate::platform::filesystem::atomic_write(&file, &payload)
            .with_context(|| format!("persist session workspace binding to {}", file.display()))?;
        self.session_workspaces
            .write()
            .insert(id.to_string(), sidecar.path);
        Ok(())
    }

    /// 读取会话的用户工作目录绑定（无绑定 → None，execution 根回退会话私有
    /// 目录）。内存缓存 miss 时回读 sidecar；会话持久记录已不存在的残留
    /// sidecar（部分删除失败留下的目录）按 None 处理，与旧 ghost 清理同语义。
    pub fn session_workspace_binding(&self, id: &str) -> Option<PathBuf> {
        if let Some(path) = self.session_workspaces.read().get(id).cloned() {
            return Some(path);
        }
        if validate_session_id(id).is_err()
            || !self
                .manager
                .sessions_dir()
                .join(format!("{id}.json"))
                .is_file()
        {
            return None;
        }
        let sidecar = read_workspace_sidecar(&self.session_workspace_sidecar_path(id))?;
        let path = sidecar.path;
        self.session_workspaces
            .write()
            .insert(id.to_string(), path.clone());
        Some(path)
    }

    /// best-effort 删除绑定 sidecar 文件；NotFound 视为已删除。会话删除
    /// 路径的目录清理通常已把它带走，这里覆盖「会话仍在、仅解绑」与残留
    /// 目录兜底两种情况。
    pub(crate) fn remove_workspace_sidecar_file(&self, id: &str) {
        let file = self.session_workspace_sidecar_path(id);
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => eprintln!(
                "[sessions] remove workspace binding sidecar failed ({}): {error:#}",
                file.display()
            ),
        }
    }

    /// 扫描全部 `workspace-binding.json` sidecar 并并入内存表，列出绑定在
    /// `from` 前缀下的会话（目录重绑定的栅栏候选集；`from` 通常已消失，
    /// 匹配在共享折叠键上进行——Windows 折叠分隔符与大小写，与
    /// SessionAgentStore::sessions_under_workspace 同语义）。不校验
    /// `<id>.json` 存在——残留 sidecar 同样要被重绑定覆盖，否则旧目录复活。
    /// 内存表并入是因为存量迁移未完成时，未迁移条目只存在于内存/旧全局表
    /// （评审 #464 nit：栅栏候选不得漏掉这一组）。
    pub fn workspace_bindings_under(&self, from: &Path) -> Vec<(String, PathBuf)> {
        let covered = |path: &Path| folded_covers(from, path);
        let mut matched = Vec::new();
        if let Ok(entries) = std::fs::read_dir(self.manager.sessions_dir()) {
            for entry in entries.flatten() {
                if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    continue;
                }
                let Some(id) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                let Some(sidecar) =
                    read_workspace_sidecar(&entry.path().join(SESSION_WORKSPACE_SIDECAR_FILE))
                else {
                    continue;
                };
                if covered(&sidecar.path) {
                    matched.push((id, sidecar.path));
                }
            }
        }
        // 内存表并集(去重):覆盖迁移降级路径下未落 sidecar 的条目。
        for (id, path) in self.session_workspaces.read().iter() {
            if covered(path) && !matched.iter().any(|(existing_id, _)| existing_id == id) {
                matched.push((id.clone(), path.clone()));
            }
        }
        matched
    }

    /// 目录重绑定（修断链通道）：把绑定在 `from` 前缀下的普通会话绑定整体
    /// 平移到 `to`（sidecar 原子重写 + 内存缓存同步）。与
    /// SessionAgentStore::rebind_workspace_prefix 同语义、同幂等性——
    /// from→to 重跑无命中即空操作，失败可整体重试。会话元数据
    /// （metadata.workspace 展示字段）由命令层统一经 set_workspace 改写。
    ///
    /// 逐条目隔离（评审 #464 MAJOR 4）：非法 id（boot 迁移会把未校验的失败
    /// 条目留在内存旧表）与单条写失败不再 `?` 中断整轮——项目 root 与 agent
    /// 索引此时已平移，中断会让同一条目在每次重试都重复失败；失败进
    /// `failed_session_ids` 由命令层并入报告。
    pub fn rebind_workspace_bindings(
        &self,
        from: &Path,
        to: &Path,
    ) -> Result<RebindBindingsOutcome> {
        let covered = |path: &Path| folded_covers(from, path);
        let skip = from.components().count();
        let mut outcome = RebindBindingsOutcome::default();
        // 候选 = sidecar 扫描 ∪ 内存旧表:存量迁移未完成的降级路径下,未迁移
        // 条目只存在于内存/旧全局表,漏配会在下次 boot 迁移时以旧目录复活。
        let mut candidates: Vec<(String, PathBuf)> = self.workspace_bindings_under(from);
        for (id, path) in self.session_workspaces.read().iter() {
            if !candidates.iter().any(|(existing_id, _)| existing_id == id) {
                candidates.push((id.clone(), path.clone()));
            }
        }
        for (id, path) in candidates {
            if !covered(&path) {
                continue;
            }
            // id 先过校验再拼路径(与 bind_session_workspace 同闸):遍历形态
            // 的 id 会写出 sessions_dir 之外。
            if let Err(error) = validate_session_id(&id) {
                eprintln!("[sessions] rebind skips invalid session id {id:?}: {error:#}");
                outcome.failed_session_ids.push(id);
                continue;
            }
            let suffix: PathBuf = path.components().skip(skip).collect();
            let next = if suffix.as_os_str().is_empty() {
                to.to_path_buf()
            } else {
                to.join(suffix)
            };
            let sidecar_path = self
                .manager
                .sessions_dir()
                .join(&id)
                .join(SESSION_WORKSPACE_SIDECAR_FILE);
            // 内存旧表条目可能没有会话目录(从未写成 sidecar),atomic_write
            // 不建父目录,先补(与 bind_session_workspace 同)。
            if let Some(parent) = sidecar_path.parent() {
                if let Err(error) = std::fs::create_dir_all(parent) {
                    eprintln!("[sessions] rebind create session dir for {id} failed: {error:#}");
                    outcome.failed_session_ids.push(id);
                    continue;
                }
            }
            // bound_at 仅元信息:原样保留,与 codex 存储的 rebind 同口径,
            // 不再重置为 None(评审 #452 finding 3)。
            let bound_at = read_workspace_sidecar(&sidecar_path).and_then(|s| s.bound_at);
            let updated = SessionWorkspaceSidecar {
                version: SESSION_WORKSPACE_SIDECAR_VERSION,
                path: next.clone(),
                bound_at,
            };
            let write = serde_json::to_vec_pretty(&updated)
                .context("serialize session workspace binding")
                .and_then(|payload| {
                    crate::platform::filesystem::atomic_write(&sidecar_path, &payload).with_context(
                        || {
                            format!(
                                "rebind session workspace binding {}",
                                sidecar_path.display()
                            )
                        },
                    )
                });
            if let Err(error) = write {
                eprintln!("[sessions] rebind workspace binding {id} failed: {error:#}");
                outcome.failed_session_ids.push(id);
                continue;
            }
            self.session_workspaces
                .write()
                .insert(id.clone(), next.clone());
            outcome.rebound.push((id, next));
        }
        // 降级路径对称(评审 #452 finding 9):旧全局表仍在盘上(存量迁移未
        // 完成)时同步重写,否则重绑定会在下次 boot 迁移时被旧表复活。
        self.rewrite_legacy_session_workspaces_if_present();
        Ok(outcome)
    }

    /// 旧全局表的最小重写(仅重绑定的降级路径对称使用)。#445 round-2 移除了
    /// 通用持久化(无其它调用面),这里只保留:表非空则整体原子重写为内存表
    /// 内容,表空则删文件(没有可复活的条目)。写失败只记日志——内存表与
    /// sidecar 已是权威,旧表本来就只是迁移残留。
    fn rewrite_legacy_session_workspaces_if_present(&self) {
        let legacy = crate::platform::paths::sessions_root().join(LEGACY_SESSION_WORKSPACES_FILE);
        if !legacy.is_file() {
            return;
        }
        let bindings = self.session_workspaces.read();
        if bindings.is_empty() {
            if let Err(error) = std::fs::remove_file(&legacy) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("[sessions] remove legacy session workspaces failed: {error:#}");
                }
            }
            return;
        }
        match serde_json::to_vec_pretty(&*bindings) {
            Ok(payload) => {
                if let Err(error) = crate::platform::filesystem::atomic_write(&legacy, &payload) {
                    eprintln!("[sessions] rewrite legacy session workspaces failed: {error:#}");
                }
            }
            Err(error) => {
                eprintln!("[sessions] serialize legacy session workspaces failed: {error:#}");
            }
        }
    }

    /// boot 期存量迁移：全局表 `_session_workspaces.json` → per-session
    /// sidecar（只服务本 PR 中间版本 dev build 的 home，见模块文档）。活会话
    /// 条目逐条写成 sidecar（同值重写幂等），全部迁移成功即删除旧文件；任一
    /// 条目写失败时旧文件原样保留，未迁移条目接管进内存表继续可解析，下次
    /// boot 重试，不阻断启动。ghost 条目（对应 `<id>.json` 已不存在——会话在
    /// 进程外被删的残留）直接丢弃，不迁移。
    pub fn migrate_legacy_session_workspaces(&self) {
        let legacy = crate::platform::paths::sessions_root().join(LEGACY_SESSION_WORKSPACES_FILE);
        let Ok(content) = std::fs::read_to_string(&legacy) else {
            return;
        };
        let bindings: HashMap<String, PathBuf> = match serde_json::from_str(&content) {
            Ok(bindings) => bindings,
            Err(error) => {
                eprintln!("[sessions] parse legacy session workspaces failed: {error}");
                return;
            }
        };
        let mut unmigrated = HashMap::new();
        for (id, path) in bindings {
            if !self
                .manager
                .sessions_dir()
                .join(format!("{id}.json"))
                .is_file()
            {
                continue;
            }
            if let Err(error) = self.bind_session_workspace(&id, path.clone()) {
                eprintln!("[sessions] migrate workspace binding for {id} failed: {error:#}");
                unmigrated.insert(id, path);
            }
        }
        if unmigrated.is_empty() {
            match std::fs::remove_file(&legacy) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => eprintln!(
                    "[sessions] remove legacy session workspaces failed ({}): {error:#}",
                    legacy.display()
                ),
            }
        } else {
            *self.session_workspaces.write() = unmigrated;
        }
    }
}
