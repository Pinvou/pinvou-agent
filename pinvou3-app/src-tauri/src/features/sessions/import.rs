//! 会话归档导入：把 `export_session` 产出的 tar.xz（或裸 session .json）
//! 重新装回会话存储，闭合导出/导入循环。
//!
//! 边界：
//! - 已存在的 session id 一律跳过，不做 id 重写——会话 id 被 artifacts
//!   目录、血缘（parent_session_id）、审计多处引用，改写会撕断一致性。
//! - manifest.json / container.json 仅作来源描述，不参与导入；未知成员
//!   静默忽略（向前兼容未来归档格式加成员）。
//! - artifact 成员路径做穿越拒绝（绝对路径 / `..` / 空分量 / 非常规文件），
//!   并施加单成员与总量字节上限，fail-closed。
//! - `aux-` / `sched-` 前缀的记录拒绝导入：aux 是受限只读旁路、scheduled
//!   运行记录依赖本机侧车（profile/最近运行），跨机导入没有意义。

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use deepseek_tui::session_manager::SavedSession;

use super::SessionStore;

/// 单个归档成员的解压字节上限（512 MiB）：一个会话的 artifacts 集合在
/// 正常产品流程下远小于此；超限即视为异常输入，拒绝而不是静默截断。
const MAX_MEMBER_BYTES: u64 = 512 * 1024 * 1024;
/// 整个归档的解压总字节上限（2 GiB），防解压炸弹。
const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct SessionImportFailure {
    pub session_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct SessionImportSummary {
    pub imported: Vec<String>,
    pub skipped_existing: Vec<String>,
    pub failed: Vec<SessionImportFailure>,
}

enum ImportOutcome {
    Imported(String),
    SkippedExisting(String),
    Failed(SessionImportFailure),
}

impl SessionStore {
    /// 从归档文件导入会话。`.json` 后缀按单条 session 记录读取，其余按
    /// `export_session` 的 tar.xz 归档读取。
    pub(crate) fn import_archive(&self, path: &Path) -> Result<SessionImportSummary> {
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            let mut summary = SessionImportSummary::default();
            let bytes = std::fs::read(path).context("read session record")?;
            if bytes.len() as u64 > MAX_MEMBER_BYTES {
                bail!("session record exceeds the {MAX_MEMBER_BYTES}-byte import cap");
            }
            let session: SavedSession = serde_json::from_slice(&bytes)
                .context("parse session record (expected a SavedSession JSON)")?;
            import_one_session(self, session, Vec::new(), &mut summary);
            return Ok(summary);
        }

        let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
        let decoder = liblzma::read::XzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);

        let mut record: Option<SavedSession> = None;
        let mut artifacts: Vec<(String, Vec<u8>)> = Vec::new();
        let mut total_bytes: u64 = 0;
        for entry in archive
            .entries()
            .context("read archive members (is this a tar.xz session archive?)")?
        {
            let entry = entry.context("read archive entry")?;
            if !entry.header().entry_type().is_file() {
                // 目录/链接/设备成员一律拒绝：链接成员无法按内容校验，
                // 是经典的归档投毒面。
                bail!("archive contains a non-regular member; refusing to import");
            }
            let member_name = entry.path()?.to_string_lossy().replace('\\', "/");
            let size = entry.size();
            total_bytes += size;
            if total_bytes > MAX_TOTAL_BYTES {
                bail!("archive exceeds the {MAX_TOTAL_BYTES}-byte total import cap");
            }
            let mut bytes = Vec::new();
            entry
                .take(MAX_MEMBER_BYTES + 1)
                .read_to_end(&mut bytes)
                .with_context(|| format!("read member {member_name}"))?;
            if bytes.len() as u64 > MAX_MEMBER_BYTES {
                bail!("member {member_name} exceeds the {MAX_MEMBER_BYTES}-byte import cap");
            }
            match member_name.as_str() {
                "session.json" => {
                    let session: SavedSession = serde_json::from_slice(&bytes)
                        .context("parse session.json (expected a SavedSession JSON)")?;
                    record = Some(session);
                }
                "manifest.json" | "container.json" => {}
                other => {
                    if let Some(relative) = other.strip_prefix("artifacts/") {
                        artifacts.push((relative.to_string(), bytes));
                    }
                    // 其余未知成员忽略（向前兼容）。
                }
            }
        }

        let Some(session) = record else {
            bail!("archive has no session.json member");
        };
        let mut summary = SessionImportSummary::default();
        import_one_session(self, session, artifacts, &mut summary);
        Ok(summary)
    }
}

fn import_one_session(
    store: &SessionStore,
    session: SavedSession,
    artifacts: Vec<(String, Vec<u8>)>,
    summary: &mut SessionImportSummary,
) {
    match import_one_session_inner(store, session, artifacts) {
        ImportOutcome::Imported(id) => summary.imported.push(id),
        ImportOutcome::SkippedExisting(id) => summary.skipped_existing.push(id),
        ImportOutcome::Failed(failure) => summary.failed.push(failure),
    }
}

fn import_one_session_inner(
    store: &SessionStore,
    session: SavedSession,
    artifacts: Vec<(String, Vec<u8>)>,
) -> ImportOutcome {
    let id = session.metadata.id.clone();
    let fail = |reason: String| SessionImportFailure {
        session_id: id.clone(),
        reason,
    };
    if let Err(reason) = super::validators::validate_session_id(&id) {
        return ImportOutcome::Failed(fail(format!("invalid session id: {reason:#}")));
    }
    if super::validators::is_aux_session_id(&id) || id.starts_with("sched-") {
        return ImportOutcome::Failed(fail(
            "aux / scheduled-run session records are not importable".to_string(),
        ));
    }
    let record_path = store.sessions_dir().join(format!("{id}.json"));
    if record_path.exists() {
        return ImportOutcome::SkippedExisting(id);
    }

    // artifacts 先落盘、记录后落盘：save 失败时尽力回滚 artifacts 目录，
    // 不留下「记录缺失但有 artifacts 目录」的半成品。记录落盘后 artifacts
    // 再失败则保持导入成功、错误只记日志（归档里的原始文件仍在；id 已被
    // 占用，修好磁盘后重新导入会走 skip 分支，不会重复导入记录）。
    let artifacts_result = import_artifacts(store, &id, &artifacts);
    if let Err(error) = store.save(&session) {
        if artifacts_result.is_ok() {
            let _ = std::fs::remove_dir_all(store.sessions_dir().join(&id));
        }
        return ImportOutcome::Failed(fail(format!("persist session record: {error:#}")));
    }
    if let Err(error) = artifacts_result {
        eprintln!(
            "[sessions] import {}: session record imported but artifacts failed: {error:#}",
            id
        );
    }
    ImportOutcome::Imported(id)
}

fn import_artifacts(store: &SessionStore, id: &str, artifacts: &[(String, Vec<u8>)]) -> Result<()> {
    if artifacts.is_empty() {
        return Ok(());
    }
    let root = store.sessions_dir();
    let Some(dir) = deepseek_tui::session_export::session_artifacts_dir(&root, id)? else {
        bail!("session artifacts dir unavailable for {id}");
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    for (relative, bytes) in artifacts {
        let target = validated_artifact_target(&dir, relative)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        crate::platform::filesystem::atomic_write_private(&target, bytes)
            .with_context(|| format!("write artifact {}", target.display()))?;
    }
    Ok(())
}

/// artifact 成员相对路径校验：拒绝绝对路径、`..`、空分量与分隔符开头的
/// 名字；返回目标是 artifacts 目录下的既定路径。
fn validated_artifact_target(artifacts_dir: &Path, relative: &str) -> Result<PathBuf> {
    let normalized = relative.replace('\\', "/");
    if normalized.is_empty() || normalized.starts_with('/') {
        bail!("invalid artifact member path: {relative:?}");
    }
    let mut target = artifacts_dir.to_path_buf();
    for component in normalized.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            bail!("invalid artifact member path: {relative:?}");
        }
        target.push(component);
    }
    Ok(target)
}
