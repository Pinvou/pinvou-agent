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
/// 底座保留的 sessions 根文件名（`session_boot_owners.json` 注册表）。
/// 与底座 `validated_session_id`（session_manager.rs `SESSION_BOOT_OWNERS_STEM`）
/// 同一拒绝：以它为 id 的记录会把注册表文件整个覆盖掉，静默清空
/// boot-ownership 声明。
const RESERVED_SESSION_ID_STEM: &str = "session_boot_owners";

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
            // 上限在分配之前生效：take 限制读入字节，稀疏文件（truncate 出
            // 的巨文件）不会先整读进内存再被拒。
            let mut bytes = Vec::new();
            std::fs::File::open(path)
                .context("read session record")?
                .take(MAX_MEMBER_BYTES + 1)
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
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        crate::platform::filesystem::atomic_write_private(&target, bytes)
            .with_context(|| format!("write artifact {}", target.display()))?;
    }
    Ok(())
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
    use super::{SessionStore, validated_artifact_target};
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
