//! 会话归档导入：把 `export_session` 产出的 tar.xz（或裸 session .json）
//! 重新装回会话存储，闭合导出/导入循环。
//!
//! 边界：
//! - 已存在的 session id 一律跳过，不做 id 重写——会话 id 被 artifacts
//!   目录、血缘（parent_session_id）、审计多处引用，改写会撕断一致性。
//! - manifest.json / container.json 仅作来源描述，不参与导入；未知成员
//!   静默忽略（向前兼容未来归档格式加成员）。
//! - artifact 成员路径在解析期整体校验（绝对路径 / `..` / 空分量 / 含 `:`
//!   分量 / 非常规文件），任一非法即拒绝整个归档，fail-closed；并施加单
//!   成员与总量字节上限。`:` 拒绝覆盖 Windows 盘符分量（`C:` 会被
//!   `PathBuf::push` 当 prefix 清空已拼路径，逃出 artifacts 目录落到该盘
//!   CWD）与 NTFS ADS 流名。
//! - artifacts 目录先建后用底座 [`deepseek_tui::session_export::
//!   session_artifacts_dir`] 校验：该 helper 是读侧语义（目录不存在返回
//!   `Ok(None)`），写侧必须先落目录再校验「无符号链接 + 规范化仍位于
//!   store 内」，否则新 id 的 artifacts 永远写不进去（review finding）。
//! - 记录 schema 以底座新建记录自带的 schema_version 为上限（运行时从
//!   [`deepseek_tui::session_manager::create_saved_session`] 探针读取，不
//!   本地镜像常量以免底座升版后漂移）：超限记录能导入却永远打不开，还会
//!   占住 id。
//! - `aux-` / `sched-` 记录拒绝导入：aux 是受限只读旁路、scheduled 运行
//!   记录依赖本机侧车（profile/最近运行），跨机导入没有意义。sched 用
//!   别名击穿安全的 [`super::validators::is_sched_session_id`]（大小写
//!   不敏感；round-36 minor 2 的同一口径——`SCHED-x` 落盘后会被大小写
//!   不敏感的调度门归为定时会话，从列表隐身又拒绝删除）。

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use deepseek_tui::session_manager::SavedSession;

use super::SessionStore;

/// 单个归档成员的解压字节上限（512 MiB）：一个会话的 artifacts 集合在
/// 正常产品流程下远小于此；超限即视为异常输入，拒绝而不是静默截断。
const MAX_MEMBER_BYTES: u64 = 512 * 1024 * 1024;
/// 整个归档的解压总字节上限（2 GiB），防解压炸弹。
const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// 底座保留的 sessions 根文件名（`session_boot_owners.json` 注册表）：
/// 拒绝语义（含大小写不敏感的别名口径，见
/// `super::validators::RESERVED_SESSION_ID_STEM` 与 `validate_session_id`）
/// 统一由 validators 执行，本模块不再持有独立字面量以免两处漂移。

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
    /// 记录已落盘但 artifacts 落盘失败（磁盘满/路径异常等）：GUI 据此在
    /// toast 上区分「完整导入」与「缺 artifacts 的部分导入」。此前该失败
    /// 只写 stderr（打包产物里不可见），且重导入命中 skip-existing 后
    /// 缺失的 artifacts 不删会话就无法补齐。
    #[serde(default)]
    pub artifacts_failed: Vec<SessionImportFailure>,
}

enum ImportOutcome {
    /// 记录落盘；`Err` 携带 artifacts 落盘失败的原因（记录导入成功但
    /// artifacts 缺失的部分导入，见 `SessionImportSummary::artifacts_failed`）。
    Imported(String, Option<anyhow::Error>),
    SkippedExisting(String),
    Failed(SessionImportFailure),
}

impl SessionStore {
    /// 从归档文件导入会话。`.json` 后缀按单条 session 记录读取，其余按
    /// `export_session` 的 tar.xz 归档读取。
    pub(crate) fn import_archive(&self, path: &Path) -> Result<SessionImportSummary> {
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            let mut summary = SessionImportSummary::default();
            // 上限在分配之前生效：take 限制读入字节，稀疏文件（truncate 出
            // 的巨文件）不会先整读进内存再被拒。容量按文件声明的长度预置
            // （钳到上限+1），避免 read_to_end 倍增扩容把峰值推到 ~1.5×。
            let file = std::fs::File::open(path).context("read session record")?;
            let declared = file.metadata().map(|meta| meta.len()).unwrap_or(0);
            let mut bytes = Vec::with_capacity(declared.min(MAX_MEMBER_BYTES + 1) as usize);
            file.take(MAX_MEMBER_BYTES + 1)
                .read_to_end(&mut bytes)
                .with_context(|| format!("read {}", path.display()))?;
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
            // 容量按成员声明大小预置（钳到上限+1），理由同上：避免倍增扩容
            // 的瞬时峰值超出 cap 本身 ~50%。
            let mut bytes = Vec::with_capacity(size.min(MAX_MEMBER_BYTES + 1) as usize);
            entry
                .take(MAX_MEMBER_BYTES + 1)
                .read_to_end(&mut bytes)
                .with_context(|| format!("read member {member_name}"))?;
            if bytes.len() as u64 > MAX_MEMBER_BYTES {
                bail!("member {member_name} exceeds the {MAX_MEMBER_BYTES}-byte import cap");
            }
            match member_name.as_str() {
                "session.json" => {
                    // 格式 v1 恰有一个 session.json；第二个成员说明归档被
                    // 拼接/篡改，静默 last-wins 会让 manifest 声明的会话
                    // 无声丢失——fail-closed 拒绝。
                    if record.is_some() {
                        bail!("archive contains multiple session.json members; refusing to import");
                    }
                    let session: SavedSession = serde_json::from_slice(&bytes)
                        .context("parse session.json (expected a SavedSession JSON)")?;
                    record = Some(session);
                }
                "manifest.json" | "container.json" => {}
                other => {
                    if let Some(relative) = other.strip_prefix("artifacts/") {
                        // 解析期校验、任一非法即拒绝整个归档：把坏成员留到
                        // 落盘期只会降级成「记录导入成功但 artifacts 静默
                        // 缺失」的 fail-open（review finding）。
                        validated_artifact_target(Path::new(""), relative).with_context(|| {
                            format!("archive member {member_name} is not importable")
                        })?;
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
        ImportOutcome::Imported(id, artifacts_error) => {
            if let Some(error) = artifacts_error {
                summary.artifacts_failed.push(SessionImportFailure {
                    session_id: id.clone(),
                    reason: format!("{error:#}"),
                });
            }
            summary.imported.push(id);
        }
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
    if super::validators::is_aux_session_id(&id) || super::validators::is_sched_session_id(&id) {
        return ImportOutcome::Failed(fail(
            "aux / scheduled-run session records are not importable".to_string(),
        ));
    }
    // 与 load_session_snapshot 同一上限（见模块文档的探针推导说明）。
    let schema_ceiling = session_schema_version_ceiling();
    if session.schema_version > schema_ceiling {
        return ImportOutcome::Failed(fail(format!(
            "session record schema v{} is newer than supported v{schema_ceiling}",
            session.schema_version
        )));
    }
    let record_path = store.sessions_dir().join(format!("{id}.json"));
    if record_path.exists() {
        return ImportOutcome::SkippedExisting(id);
    }

    // artifacts 先落盘、记录后落盘：save 失败时若 artifacts 已完整写入，
    // 尽力回滚 artifacts 目录，不留「记录缺失但有 artifacts 目录」的半成品；
    // artifacts 本身就写了一半再失败的，不回滚（部分目录留给下次成功导入
    // 以原子覆盖补齐，`atomic_write_private` 同名覆盖语义保证无陈旧混合）。
    // 记录落盘后 artifacts 再失败则保持导入成功，失败进 summary 的
    // artifacts_failed（GUI 据此区分部分导入）并记 stderr；归档里的原始
    // 文件仍在，id 已被占用，重新导入会走 skip 分支。
    let artifacts_result = import_artifacts(store, &id, &artifacts);
    if let Err(error) = store.save(&session) {
        if artifacts_result.is_ok() {
            let _ = std::fs::remove_dir_all(store.sessions_dir().join(&id));
        }
        return ImportOutcome::Failed(fail(format!("persist session record: {error:#}")));
    }
    if let Err(error) = &artifacts_result {
        eprintln!(
            "[sessions] import {}: session record imported but artifacts failed: {error:#}",
            id
        );
    }
    ImportOutcome::Imported(id, artifacts_result.err())
}

fn import_artifacts(store: &SessionStore, id: &str, artifacts: &[(String, Vec<u8>)]) -> Result<()> {
    if artifacts.is_empty() {
        return Ok(());
    }
    let root = store.sessions_dir();
    let session_dir = root.join(id);
    let candidate_dir = session_dir.join("artifacts");
    // 先建目录：session_artifacts_dir 是读侧语义（目录不存在返回 Ok(None)），
    // 不先落目录则新 id 的 artifacts 永远写不进去。建之前先拒符号链接
    // （create_dir_all 会跟随父链，与底座 exporter 的双层链接拒绝同口径）；
    // 建完再交给 helper 做规范化收敛校验（仍位于 store 内）。
    for candidate in [&session_dir, &candidate_dir] {
        if let Ok(meta) = std::fs::symlink_metadata(candidate) {
            anyhow::ensure!(
                !meta.file_type().is_symlink(),
                "{} is a symlink; refusing to import artifacts through it",
                candidate.display()
            );
        }
    }
    std::fs::create_dir_all(&candidate_dir)
        .with_context(|| format!("create {}", candidate_dir.display()))?;
    let Some(dir) = deepseek_tui::session_export::session_artifacts_dir(&root, id)? else {
        bail!("session artifacts dir unavailable for {id}");
    };
    for (relative, bytes) in artifacts {
        let target = validated_artifact_target(&dir, relative)?;
        // 逐级父目录拒绝符号链接：与底座 exporter 的逐条目纪律同口径
        // （exporter 对 symlink 子项直接跳过）。store 内被预先植入的
        // `artifacts/<sub>` → store 外目录的链接（例如残缺删除残留）否则
        // 会被 create_dir_all 跟随，把成员写到 store 之外。
        if let Some(parent) = target.parent() {
            let mut prefix = dir.to_path_buf();
            if let Ok(rel_parent) = parent.strip_prefix(&dir) {
                for component in rel_parent.components() {
                    prefix.push(component);
                    if let Ok(meta) = std::fs::symlink_metadata(&prefix) {
                        anyhow::ensure!(
                            !meta.file_type().is_symlink(),
                            "{} is a symlink; refusing to import artifacts through it",
                            prefix.display()
                        );
                    }
                }
            }
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        crate::platform::filesystem::atomic_write_private(&target, bytes)
            .with_context(|| format!("write artifact {}", target.display()))?;
    }
    Ok(())
}

/// 导出前置校验（闭合导出→导入循环）：逐个 artifacts 文件按导入方的成员
/// 规则（[`validated_artifact_target`]）与恒等性（归一化后不得改变相对
/// 布局，即 POSIX 上文件名里的 `\` 会被导入方当分隔符改名）验证可往返。
/// 任一文件不满足即让导出失败并指名文件，用户改名后重试——归档从此不会
/// 产出「导出成功、导入必拒」的产物（导入侧保持 fail-closed 不变）。
/// 遍历口径与底座 exporter 一致：跳过符号链接（exporter 也不打包）、仅
/// 收常规文件、目录递归。
pub(crate) fn ensure_artifacts_exportable(artifacts_dir: &Path) -> Result<()> {
    fn walk(dir: &Path, relative: &str, problems: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let child = entry.file_name().to_string_lossy().into_owned();
            let child_relative = if relative.is_empty() {
                child.clone()
            } else {
                format!("{relative}/{child}")
            };
            let path = dir.join(&child);
            if file_type.is_dir() {
                walk(&path, &child_relative, problems);
            } else if file_type.is_file() {
                // 与导入方同一归一化：`\` → `/`。恒等性要求归一化不引入
                // 新的分量（否则导入会静默改名换布局）。
                let normalized = child_relative.replace('\\', "/");
                let raw_components = Path::new(&child_relative).components().count();
                let normalized_components = normalized.split('/').count();
                let validation = validated_artifact_target(Path::new(""), &normalized)
                    .map(|_| ())
                    .and_then(|()| {
                        if raw_components == normalized_components {
                            Ok(())
                        } else {
                            bail!(
                                "member path {child_relative:?} would be renamed to {normalized:?} on import"
                            )
                        }
                    });
                if let Err(reason) = validation {
                    problems.push(format!(
                        "artifacts/{child_relative}: {reason} (rename or remove this file and retry the export)"
                    ));
                }
            }
        }
    }
    let mut problems = Vec::new();
    walk(artifacts_dir, "", &mut problems);
    if problems.is_empty() {
        return Ok(());
    }
    bail!(
        "session artifacts contain files that cannot be restored from the archive:\n{}",
        problems.join("\n")
    )
}

/// artifact 成员相对路径校验：拒绝绝对路径、`..`、空分量与含 `:` 的分量
/// （Windows 盘符 prefix 分量会被 `PathBuf::push` 清空已拼路径——
/// `artifacts/C:/evil` 逃出 artifacts 目录；同一检查顺带拒绝 NTFS ADS）；
/// 返回目标是 artifacts 目录下的既定路径。
fn validated_artifact_target(artifacts_dir: &Path, relative: &str) -> Result<std::path::PathBuf> {
    let normalized = relative.replace('\\', "/");
    if normalized.is_empty() || normalized.starts_with('/') {
        bail!("invalid artifact member path: {relative:?}");
    }
    let mut target = artifacts_dir.to_path_buf();
    for component in normalized.split('/') {
        if component.is_empty() || component == "." || component == ".." || component.contains(':')
        {
            bail!("invalid artifact member path: {relative:?}");
        }
        target.push(component);
    }
    Ok(target)
}

/// 底座当前 schema 上限：从底座新建记录的 `schema_version` 探针读取
/// （引擎写入即盖此值），不本地镜像常量——底座升版时本检查自动跟随，
/// 不会出现「新版导出被旧上限误拒」的漂移。
fn session_schema_version_ceiling() -> u32 {
    deepseek_tui::session_manager::create_saved_session(
        &[],
        "import-schema-probe",
        Path::new(""),
        0,
        None,
    )
    .schema_version
}

#[cfg(test)]
mod tests {
    use super::super::{SessionImportFailure, SessionImportSummary};
    use super::{MAX_MEMBER_BYTES, SessionStore, validated_artifact_target};
    use deepseek_tui::models::{ContentBlock, Message, Role};

    use super::session_schema_version_ceiling;

    /// 与 features::sessions::tests::isolated_store 同一隔离模式
    /// （test_support::locked_env 串行化 PINVOU3_HOME；守卫持锁到测试结束）。
    fn isolated_store() -> (
        SessionStore,
        std::sync::MutexGuard<'static, ()>,
        crate::platform::test_support::EnvRestore,
    ) {
        let (lock, env) = crate::platform::test_support::locked_env(&["PINVOU3_HOME"]);
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-import-test-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        // SAFETY: test_support::locked_env 持锁；env 写入进程内串行。
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        (
            SessionStore::boot_with_scheduled_root(tmp.join("scheduled")).expect("boot"),
            lock,
            env,
        )
    }

    fn turn_prompt(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    fn assistant_text(text: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    /// 在源会话 artifacts 目录落一个真实文件（供导出/导入往返）。
    fn seed_artifact(store: &SessionStore, id: &str, name: &str, content: &[u8]) {
        let dir = store
            .sessions_dir()
            .join(id)
            .join("artifacts")
            .join("reports");
        std::fs::create_dir_all(&dir).expect("mkdir artifacts");
        std::fs::write(dir.join(name), content).expect("write artifact");
    }

    fn write_json_record(store: &SessionStore, id: &str) -> std::path::PathBuf {
        let source = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let mut session = store.load(&source.metadata.id).expect("load");
        session.messages = vec![turn_prompt("裸记录导入"), assistant_text("回复")];
        session.metadata.id = id.into();
        let path = std::env::temp_dir().join(format!(
            "pinvou3-import-{}-{}.json",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::fs::write(&path, serde_json::to_vec(&session).expect("to_json")).expect("write json");
        path
    }

    /// 打包一个内存 tar.xz（成员按 (name, bytes) 顺序写入）。
    fn build_archive(members: &[(&str, &[u8])]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pinvou3-import-{}-{}.tar.xz",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let file = std::fs::File::create(&path).expect("create archive");
        let encoder = liblzma::write::XzEncoder::new(file, 1);
        let mut builder = tar::Builder::new(encoder);
        for (name, bytes) in members {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o600);
            header.set_cksum();
            builder
                .append_data(&mut header, name, *bytes)
                .expect("append member");
        }
        builder
            .into_inner()
            .expect("finish archive")
            .finish()
            .expect("finish xz");
        path
    }

    /// 手工 ustar 头写入成员（tar::Builder 拒绝 `..` 成员名，恶意样例必须
    /// 绕过它的清洗直接构造）：name + 声明 size + payload 原样拼装。
    fn archive_with_raw_member(name: &str, payload: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pinvou3-import-raw-{}-{}.tar.xz",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        // size 字段：offset 124，11 位八进制 + NUL。
        let size_field = format!("{:011o}\0", payload.len());
        header[124..124 + size_field.len()].copy_from_slice(size_field.as_bytes());
        header[156] = b'0'; // typeflag: regular file
        // checksum：chksum 字段（148..156）先整体置空格求和，再写 6 位八进制
        // + NUL + 空格。
        header[148..156].fill(b' ');
        let sum: u32 = header.iter().map(|&b| b as u32).sum();
        let chksum = format!("{:06o}\0 ", sum);
        header[148..156].copy_from_slice(chksum.as_bytes());

        let file = std::fs::File::create(&path).expect("create archive");
        let mut encoder = liblzma::write::XzEncoder::new(file, 1);
        use std::io::Write;
        encoder.write_all(&header).expect("write header");
        encoder.write_all(payload).expect("write payload");
        // 数据补齐到 512 边界后写两块全零 EOF。
        let padding = (512 - payload.len() % 512) % 512;
        encoder
            .write_all(&vec![0u8; padding])
            .expect("write padding");
        encoder.write_all(&[0u8; 1024]).expect("write EOF blocks");
        encoder.finish().expect("finish xz");
        path
    }

    /// 声明超大 size 的头（无真实数据）——总量上限在读取前按头大小记账即
    /// bail，用最小构造验证炸弹拒绝。
    fn archive_with_lying_header(name: &str, declared_size: u64) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pinvou3-import-liar-{}-{}.tar.xz",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        let size_field = format!("{:011o}\0", declared_size);
        header[124..124 + size_field.len()].copy_from_slice(size_field.as_bytes());
        header[156] = b'0';
        header[148..156].fill(b' ');
        let sum: u32 = header.iter().map(|&b| b as u32).sum();
        let chksum = format!("{:06o}\0 ", sum);
        header[148..156].copy_from_slice(chksum.as_bytes());
        let file = std::fs::File::create(&path).expect("create archive");
        let mut encoder = liblzma::write::XzEncoder::new(file, 1);
        use std::io::Write;
        encoder.write_all(&header).expect("write header");
        encoder.write_all(&[0u8; 1024]).expect("write EOF blocks");
        encoder.finish().expect("finish xz");
        path
    }

    #[test]
    fn import_round_trips_export_and_skips_existing_id_with_artifacts() {
        let (store, _lock, _env) = isolated_store();
        let source = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let mut session = store.load(&source.metadata.id).expect("load");
        session.messages = vec![turn_prompt("导入往返"), assistant_text("回复")];
        store.save(&session).expect("seed messages");
        seed_artifact(
            &store,
            &source.metadata.id,
            "summary.md",
            b"# roundtrip payload",
        );

        let archive = std::env::temp_dir().join(format!(
            "pinvou3-import-roundtrip-{}-{}.tar.xz",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        store
            .export_archive(&source.metadata.id, &archive, true)
            .expect("export");

        // 同 id 已存在 → 跳过，不重复导入。
        let summary = store.import_archive(&archive).expect("import");
        assert!(summary.imported.is_empty(), "id already exists");
        assert_eq!(summary.skipped_existing, vec![source.metadata.id.clone()]);

        // 删源后重导 → 装回（记录 + artifacts 实体都要回来）。
        store.delete(&source.metadata.id).expect("delete source");
        let summary = store.import_archive(&archive).expect("re-import");
        assert_eq!(summary.imported, vec![source.metadata.id.clone()]);
        let restored = store.load(&source.metadata.id).expect("restored");
        assert_eq!(restored.messages.len(), 2);
        let restored_artifact = store
            .sessions_dir()
            .join(&source.metadata.id)
            .join("artifacts")
            .join("reports")
            .join("summary.md");
        assert_eq!(
            std::fs::read(&restored_artifact).expect("read restored artifact"),
            b"# roundtrip payload",
            "artifacts must land for a fresh id (regression: dead import path)"
        );
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn import_rejects_malicious_artifact_member_paths() {
        let (store, _lock, _env) = isolated_store();
        for member in [
            "artifacts/../escape.json",
            "artifacts/C:/evil.exe",
            "artifacts/x:stream.txt",
            // 反斜杠归一化（`\`→`/`）后同样落入 `..` 拒绝：POSIX 上合法的
            // 单文件名会被导入当分隔符改名，Windows 上本就是分隔符。
            "artifacts\\..\\escape.json",
        ] {
            // tar::Builder 会清洗 `..` 成员名，恶意样例走裸头构造（解析期
            // 校验在 session.json 缺失检查之前，单坏成员即可触发拒绝）。
            let archive = archive_with_raw_member(member, b"payload");
            let error = store
                .import_archive(&archive)
                .expect_err(&format!("{member} must reject the whole archive"));
            assert!(
                format!("{error:#}").contains("invalid artifact member path"),
                "{member}: {error:#}"
            );
            // fail-closed：拒绝后不得有任何记录落盘。
            assert!(
                !store.sessions_dir().join("escape.json").exists(),
                "traversal member must not write outside the artifacts dir"
            );
        }
    }

    #[test]
    fn import_rejects_non_regular_members() {
        let (store, _lock, _env) = isolated_store();
        let path = std::env::temp_dir().join(format!(
            "pinvou3-import-dirmember-{}-{}.tar.xz",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let file = std::fs::File::create(&path).expect("create archive");
        let encoder = liblzma::write::XzEncoder::new(file, 1);
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "artifacts", std::io::empty())
            .expect("append dir member");
        builder
            .into_inner()
            .expect("finish")
            .finish()
            .expect("finish xz");

        let error = store
            .import_archive(&path)
            .expect_err("dir member must fail");
        assert!(format!("{error:#}").contains("non-regular"), "{error:#}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_enforces_total_cap_by_declared_size() {
        let (store, _lock, _env) = isolated_store();
        // 声明 3 GiB 的头，无数据：总量记账在读取前按头大小即 bail。
        let archive = archive_with_lying_header("bomb.bin", 3 * 1024 * 1024 * 1024);
        let error = store
            .import_archive(&archive)
            .expect_err("declared-size bomb must fail the total cap");
        assert!(
            format!("{error:#}").contains("total import cap"),
            "{error:#}"
        );
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn import_refuses_reserved_and_class_prefixed_ids() {
        let (store, _lock, _env) = isolated_store();
        // aux- 前缀。
        let path = write_json_record(&store, "aux-import1");
        let summary = store.import_archive(&path).expect("import aux record");
        assert!(summary.imported.is_empty());
        assert!(
            summary.failed[0].reason.contains("not importable"),
            "{}",
            summary.failed[0].reason
        );
        // sched- 大小写变体：is_sched_session_id 的别名击穿口径必须拒绝
        // （case-sensitive starts_with 会放行成列表隐身 + 删除被拒的幽灵记录）。
        let path = write_json_record(&store, "SCHED-import1");
        let summary = store.import_archive(&path).expect("import SCHED record");
        assert!(
            summary
                .failed
                .iter()
                .any(|f| f.reason.contains("not importable"))
        );
        // 底座保留名：导入会覆盖 boot-owners 注册表，必须按非法 id 拒绝。
        let path = write_json_record(&store, "session_boot_owners");
        let summary = store
            .import_archive(&path)
            .expect("import reserved-stem record");
        assert!(
            summary.failed.iter().any(|f| f.reason.contains("reserved")),
            "{:?}",
            summary.failed
        );
        // 大小写变体同样拒绝：charset 允许大写，而注册表文件名在大小写
        // 不敏感文件系统（APFS/NTFS）上与 `SESSION_BOOT_OWNERS.json` 是
        // 同一文件——精确匹配会放行变体覆盖注册表（sched-/aux- 同一口径）。
        let path = write_json_record(&store, "SESSION_BOOT_OWNERS");
        let summary = store
            .import_archive(&path)
            .expect("import reserved-stem case variant");
        assert!(
            summary.failed.iter().any(|f| f.reason.contains("reserved")),
            "{:?}",
            summary.failed
        );
    }

    /// 单成员上限按实际字节执行（头可谎报小、流不可谎报短——`take()` 截断
    /// 后按读到的真实长度记账）：512 MiB+1 的零字节成员 xz 压缩后极小，
    /// 构造廉价，读侧按实际长度 bail。
    #[test]
    fn import_enforces_per_member_cap_by_actual_bytes() {
        let (store, _lock, _env) = isolated_store();
        let payload = vec![0u8; MAX_MEMBER_BYTES as usize + 1];
        let archive = archive_with_raw_member("bomb.bin", &payload);
        let error = store
            .import_archive(&archive)
            .expect_err("actual-bytes bomb must fail the member cap");
        assert!(format!("{error:#}").contains("exceeds the"), "{error:#}");
        let _ = std::fs::remove_file(&archive);
    }

    /// 格式 v1 恰有一个 session.json：第二个成员说明归档被拼接/篡改，
    /// 静默 last-wins 会让 manifest 声明的会话无声丢失，必须拒绝。
    #[test]
    fn import_rejects_duplicate_session_json_members() {
        let (store, _lock, _env) = isolated_store();
        let first = write_json_record(&store, "dupsession1");
        let second = write_json_record(&store, "dupsession2");
        let archive = build_archive(&[
            ("session.json", &std::fs::read(&first).expect("read first")),
            (
                "session.json",
                &std::fs::read(&second).expect("read second"),
            ),
        ]);
        let error = store
            .import_archive(&archive)
            .expect_err("duplicate session.json must fail closed");
        assert!(
            format!("{error:#}").contains("multiple session.json"),
            "{error:#}"
        );
        assert!(
            !store.sessions_dir().join("dupsession1.json").exists(),
            "nothing may persist from a rejected archive"
        );
        let _ = std::fs::remove_file(&archive);
    }

    /// 产线侧 serde pin（与 projects.rs 的 RebindWorkspaceReport 同款）：
    /// wire 字段必须是 snake_case——未来给 summary 加
    /// `rename_all = "camelCase"` 会让 SettingsView 读到的 skipped_existing
    /// 变 undefined，toast 静默全零而所有套件保持绿。
    #[test]
    fn session_import_summary_serializes_snake_case_wire_fields() {
        let summary = SessionImportSummary {
            imported: vec!["a".into()],
            skipped_existing: vec!["b".into()],
            failed: vec![SessionImportFailure {
                session_id: "c".into(),
                reason: "boom".into(),
            }],
            artifacts_failed: vec![SessionImportFailure {
                session_id: "d".into(),
                reason: "disk full".into(),
            }],
        };
        let value = serde_json::to_value(&summary).expect("serialize");
        assert_eq!(value["imported"], serde_json::json!(["a"]));
        assert_eq!(value["skipped_existing"], serde_json::json!(["b"]));
        assert_eq!(value["failed"][0]["session_id"], "c");
        assert_eq!(value["artifacts_failed"][0]["session_id"], "d");
        assert_eq!(value["artifacts_failed"][0]["reason"], "disk full");
    }

    /// 记录落盘但 artifacts 失败的部分导入必须出现在
    /// `artifacts_failed`（而非只写 stderr）：GUI 据此区分完整/部分导入。
    /// 预植 `artifacts/reports` 指向 store 外的符号链接（残缺删除残留的
    /// 形态），逐级父目录链接拒绝应拦住成员落盘。
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn import_reports_artifacts_failure_and_refuses_parent_symlinks() {
        let (store, _lock, _env) = isolated_store();

        // store 外目标目录 + sessions/<victim>/artifacts/reports → 外部的
        // 符号链接（记录不存在，导入按全新 id 走）。
        let outside = std::env::temp_dir().join(format!(
            "pinvou3-import-outside-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::fs::create_dir_all(&outside).expect("mkdir outside");
        let victim = format!(
            "victim-{}-symlink",
            crate::platform::paths::tests::unique_suffix()
        );
        let artifacts_dir = store.sessions_dir().join(&victim).join("artifacts");
        std::fs::create_dir_all(&artifacts_dir).expect("mkdir artifacts");
        std::os::unix::fs::symlink(&outside, artifacts_dir.join("reports")).expect("symlink");

        // 归档：记录 + artifacts/reports/evil.txt 成员。
        let record = write_json_record(&store, &victim);
        let record_bytes = std::fs::read(&record).expect("read record");
        let archive = build_archive(&[
            ("session.json", &record_bytes),
            ("artifacts/reports/evil.txt", b"payload".as_slice()),
        ]);

        let summary = store.import_archive(&archive).expect("import completes");
        assert!(
            summary.imported.contains(&victim),
            "record itself lands: {:?}",
            summary.imported
        );
        assert!(
            !summary.artifacts_failed.is_empty(),
            "artifacts failure must surface in the summary: {:?}",
            summary.artifacts_failed
        );
        assert!(
            !outside.join("evil.txt").exists(),
            "symlinked parent must not leak the member outside the store"
        );
        let _ = std::fs::remove_file(&archive);
        let _ = std::fs::remove_file(&record);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// 导出前置校验（M4 修复）：`:` 是 macOS/Linux 合法文件名字符（Windows
    /// 无法创建，故 POSIX-only 测试），exporter 不校验文件名、导入侧
    /// fail-closed——带 `:` 的 artifacts 文件必须让导出先行失败并指名，
    /// 归档永不产出「导出成功、导入必拒」的产物；不含该文件时导出照常。
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn export_refuses_artifact_names_that_cannot_round_trip() {
        let (store, _lock, _env) = isolated_store();
        let source = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        seed_artifact(&store, &source.metadata.id, "ok.md", b"fine");
        let archive = std::env::temp_dir().join(format!(
            "pinvou3-export-prescan-{}-{}.tar.xz",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        store
            .export_archive(&source.metadata.id, &archive, true)
            .expect("clean artifacts export fine");
        let _ = std::fs::remove_file(&archive);

        seed_artifact(&store, &source.metadata.id, "weird:name.txt", b"colon");
        let error = store
            .export_archive(&source.metadata.id, &archive, true)
            .expect_err("':'-named artifact must fail the export");
        assert!(
            format!("{error:#}").contains("cannot be restored"),
            "{error:#}"
        );
        assert!(
            format!("{error:#}").contains("weird:name.txt"),
            "the offending file must be named: {error:#}"
        );
        // 不带 artifacts 的导出不经过扫描，照常成功。
        let summary_only = std::env::temp_dir().join(format!(
            "pinvou3-export-prescan2-{}-{}.tar.xz",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        store
            .export_archive(&source.metadata.id, &summary_only, false)
            .expect("record-only export unaffected");
        let _ = std::fs::remove_file(&archive);
        let _ = std::fs::remove_file(&summary_only);
    }

    #[test]
    fn import_rejects_future_schema_versions() {
        let (store, _lock, _env) = isolated_store();
        let path = write_json_record(&store, "importschema1");
        let mut session: deepseek_tui::session_manager::SavedSession =
            serde_json::from_slice(&std::fs::read(&path).expect("read json")).expect("parse");
        session.schema_version = session_schema_version_ceiling() + 1;
        std::fs::write(&path, serde_json::to_vec(&session).expect("to_json")).expect("write");

        let summary = store.import_archive(&path).expect("import future schema");
        assert!(summary.imported.is_empty());
        assert!(
            summary.failed[0].reason.contains("newer than supported"),
            "{}",
            summary.failed[0].reason
        );
        // id 未被占用（不留下永远打不开的记录）。
        assert!(!store.sessions_dir().join("importschema1.json").exists());
    }

    #[test]
    fn validated_artifact_target_rejects_traversal_forms() {
        let base = std::path::Path::new("/tmp/whatever/artifacts");
        for bad in [
            "../escape",
            "/absolute",
            "a//b",
            "a/./b",
            "C:/evil",
            "C:evil",
            "a/b:c",
            "",
            "a/",
        ] {
            assert!(
                validated_artifact_target(base, bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
        let ok = validated_artifact_target(base, "reports/summary.md").expect("valid path");
        assert_eq!(ok, base.join("reports").join("summary.md"));
    }
}
