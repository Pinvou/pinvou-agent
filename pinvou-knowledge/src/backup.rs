use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use age::secrecy::ExposeSecret;
use age::x25519::{Identity, Recipient};
use age::{Decryptor, Encryptor};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const BACKUP_FORMAT: u32 = 1;
const DATABASE_NAME: &str = "knowledge.db";
const DOCUMENTS_NAME: &str = "documents";
const MANIFEST_NAME: &str = "manifest.json";
const TLS_DIR: &str = "tls";
const CA_FILES: &[&str] = &["ca.pem", "ca-key.pem"];
const MAX_BACKUP_ENTRIES: usize = 20_100;
const MAX_BACKUP_UNPACKED_BYTES: u64 = 128 * 1024 * 1024 * 1024;
const MAX_BACKUP_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_BACKUP_DATABASE_BYTES: u64 = 32 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackupManifest {
    pub format: u32,
    pub created_at: i64,
    pub server_id: String,
    pub database_sha256: String,
    pub document_count: usize,
    pub document_bytes: u64,
    pub documents: Vec<BackupFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackupFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreMode {
    SameHost,
    ContentOnly,
}

pub fn recover_interrupted_restore(data_dir: &Path) -> Result<(), String> {
    let rollback = restore_rollback_path(data_dir);
    if data_dir.exists() {
        // 恢复在 staging 已换入 data_dir、清理 rollback 前崩溃时，data_dir 已是
        // 恢复后的完整数据，残留的 rollback 只是上一轮换入前的旧数据；不清理会让
        // replace_data_dir 永远以「恢复回滚目录已存在」拒绝后续恢复。
        if rollback.exists() {
            fs::remove_dir_all(&rollback)
                .map_err(|error| format!("无法清理中断恢复遗留的旧数据：{error}"))?;
        }
        return Ok(());
    }
    if rollback.exists() {
        fs::rename(&rollback, data_dir)
            .map_err(|error| format!("无法恢复中断前的共享知识库数据：{error}"))?;
    }
    Ok(())
}

pub fn generate_identity() -> (String, String) {
    let identity = Identity::generate();
    let recipient = identity.to_public().to_string();
    (identity.to_string().expose_secret().to_string(), recipient)
}

pub fn create_encrypted_backup(
    data_dir: &Path,
    output: &Path,
    recipients: &[String],
) -> Result<BackupManifest, String> {
    let database = data_dir.join(DATABASE_NAME);
    let documents_dir = data_dir.join(DOCUMENTS_NAME);
    if !database.is_file() {
        return Err("共享知识库数据库不存在".to_string());
    }
    if recipients.is_empty() || recipients.len() > 4 {
        return Err("备份接收密钥数量无效".to_string());
    }
    let recipients = recipients
        .iter()
        .map(|value| Recipient::from_str(value).map_err(|_| "备份接收密钥无效".to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| "备份目标路径无效".to_string())?;
    if !parent.is_dir() {
        return Err("备份目标目录不存在".to_string());
    }
    ensure_path_outside_data_dir(data_dir, parent, "备份文件不能保存在共享知识库数据目录中")?;
    if output.exists() {
        return Err("备份文件已存在，请选择其他名称".to_string());
    }

    // Copying only knowledge.db while WAL mode is active can silently omit committed
    // pages that still live in knowledge.db-wal. First request a non-blocking
    // checkpoint, then use SQLite itself to materialize one transactional snapshot.
    // VACUUM INTO reads the complete logical database even when a concurrent reader
    // prevents the checkpoint from draining the WAL completely.
    // 快照体积与数据目录相当，必须落在数据目录父目录（与恢复暂存区同一策略），
    // 避免系统 /tmp（常为 tmpfs 或小分区）被大库撑爆。
    let snapshot_parent = data_dir
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| "共享知识库数据目录无效".to_string())?;
    let snapshot_dir = tempfile::Builder::new()
        .prefix(".pinvou-knowledge-backup-snapshot-")
        .tempdir_in(snapshot_parent)
        .map_err(|error| format!("无法创建数据库备份快照：{error}"))?;
    let snapshot_database = snapshot_dir.path().join(DATABASE_NAME);
    create_database_snapshot(&database, &snapshot_database)?;
    let snapshot_connection = Connection::open_with_flags(
        &snapshot_database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("无法读取数据库备份快照：{error}"))?;
    crate::store::configure_connection(&snapshot_connection)
        .map_err(|error| format!("无法配置数据库备份快照：{error}"))?;
    let server_id = snapshot_connection
        .query_row("SELECT value FROM meta WHERE key='server_id'", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| format!("无法读取共享知识库身份：{error}"))?;
    let snapshot_documents = snapshot_dir.path().join(DOCUMENTS_NAME);
    copy_snapshot_documents(&snapshot_connection, &documents_dir, &snapshot_documents)?;
    drop(snapshot_connection);

    let mut documents = collect_documents(&snapshot_documents)?;
    documents.sort_by(|left, right| left.path.cmp(&right.path));
    let manifest = BackupManifest {
        format: BACKUP_FORMAT,
        created_at: chrono::Utc::now().timestamp(),
        server_id,
        database_sha256: hash_file(&snapshot_database)?,
        document_count: documents.len(),
        document_bytes: documents.iter().map(|file| file.size).sum(),
        documents,
    };

    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        output
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("backup"),
        std::process::id()
    ));
    let result = (|| {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("无法创建备份：{error}"))?;
        let encryptor = Encryptor::with_recipients(
            recipients
                .iter()
                .map(|recipient| recipient as &dyn age::Recipient),
        )
        .map_err(|error| format!("无法初始化备份加密：{error}"))?;
        let encrypted = encryptor
            .wrap_output(file)
            .map_err(|error| format!("无法写入加密备份：{error}"))?;
        let compressed = GzEncoder::new(encrypted, Compression::default());
        let mut archive = tar::Builder::new(compressed);
        let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest_bytes.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        archive
            .append_data(&mut header, MANIFEST_NAME, manifest_bytes.as_slice())
            .map_err(|error| format!("无法写入备份清单：{error}"))?;
        archive
            .append_path_with_name(&snapshot_database, DATABASE_NAME)
            .map_err(|error| format!("无法备份数据库：{error}"))?;
        if snapshot_documents.is_dir() {
            archive
                .append_dir_all(DOCUMENTS_NAME, &snapshot_documents)
                .map_err(|error| format!("无法备份源文档：{error}"))?;
        }
        let compressed = archive
            .into_inner()
            .map_err(|error| format!("无法完成备份归档：{error}"))?;
        let encrypted = compressed
            .finish()
            .map_err(|error| format!("无法完成备份压缩：{error}"))?;
        let mut file = encrypted
            .finish()
            .map_err(|error| format!("无法完成备份加密：{error}"))?;
        file.flush().map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        fs::hard_link(&temporary, output).map_err(|error| {
            format!("无法保存备份；目标文件可能已存在，请选择其他名称：{error}")
        })?;
        fs::remove_file(&temporary).map_err(|error| format!("无法完成备份：{error}"))?;
        Ok::<_, String>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(manifest)
}

pub fn restore_encrypted_backup(
    data_dir: &Path,
    input: &Path,
    identity: &str,
    mode: RestoreMode,
) -> Result<BackupManifest, String> {
    let identity =
        Identity::from_str(identity.trim()).map_err(|_| "恢复码或本机备份密钥无效".to_string())?;
    let parent = data_dir
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| "共享知识库数据目录无效".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let input_parent = input
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| "备份文件路径无效".to_string())?;
    ensure_path_outside_data_dir(data_dir, input_parent, "不能从共享知识库数据目录内恢复备份")?;
    let staging = tempfile::Builder::new()
        .prefix(".pinvou-knowledge-restore-")
        .tempdir_in(parent)
        .map_err(|error| format!("无法创建恢复暂存区：{error}"))?;
    decrypt_archive(input, &identity, &staging)?;
    let manifest = verify_snapshot(staging.path())?;

    let staged_database = staging.path().join(DATABASE_NAME);
    match mode {
        RestoreMode::SameHost => {
            preserve_current_host_state(data_dir, &staged_database)?;
            preserve_current_tls_identity(data_dir, staging.path())?;
        }
        RestoreMode::ContentOnly => clear_host_state(&staged_database)?,
    }
    replace_data_dir(data_dir, staging)?;
    Ok(manifest)
}

fn preserve_current_tls_identity(data_dir: &Path, staging: &Path) -> Result<(), String> {
    let source = data_dir.join(TLS_DIR);
    let existing = CA_FILES
        .iter()
        .map(|name| source.join(name).is_file())
        .collect::<Vec<_>>();
    if existing.iter().all(|value| !value) {
        return Ok(());
    }
    if existing.iter().any(|value| !value) {
        return Err("当前共享知识库的加密身份不完整，已拒绝恢复".to_string());
    }
    let destination = staging.join(TLS_DIR);
    fs::create_dir_all(&destination).map_err(|error| error.to_string())?;
    for name in CA_FILES {
        fs::copy(source.join(name), destination.join(name))
            .map_err(|error| format!("无法保留共享知识库加密身份：{error}"))?;
    }
    Ok(())
}

fn decrypt_archive(input: &Path, identity: &Identity, staging: &TempDir) -> Result<(), String> {
    let file = File::open(input).map_err(|error| format!("无法打开备份：{error}"))?;
    let decryptor =
        Decryptor::new(BufReader::new(file)).map_err(|_| "备份格式无效或已损坏".to_string())?;
    let decrypted = decryptor
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(|_| "无法解密备份，请检查恢复码".to_string())?;
    let compressed = GzDecoder::new(decrypted);
    let mut archive = tar::Archive::new(compressed);
    let mut entries_seen = 0usize;
    let mut unpacked_bytes = 0u64;
    for entry in archive.entries().map_err(|error| error.to_string())? {
        let mut entry = entry.map_err(|error| format!("备份归档损坏：{error}"))?;
        let path = entry
            .path()
            .map_err(|error| error.to_string())?
            .into_owned();
        let size = entry.size();
        entries_seen = entries_seen.saturating_add(1);
        unpacked_bytes = unpacked_bytes
            .checked_add(size)
            .ok_or_else(|| "备份展开大小无效".to_string())?;
        if entries_seen > MAX_BACKUP_ENTRIES
            || unpacked_bytes > MAX_BACKUP_UNPACKED_BYTES
            || !safe_archive_entry(&path, entry.header().entry_type(), size)
            || !entry
                .unpack_in(staging.path())
                .map_err(|error| error.to_string())?
        {
            return Err("备份包含不安全的文件路径".to_string());
        }
    }
    Ok(())
}

fn safe_archive_entry(path: &Path, entry_type: tar::EntryType, size: u64) -> bool {
    if path == Path::new(MANIFEST_NAME) {
        return entry_type.is_file() && size <= MAX_BACKUP_MANIFEST_BYTES;
    }
    if path == Path::new(DATABASE_NAME) {
        return entry_type.is_file() && size <= MAX_BACKUP_DATABASE_BYTES;
    }
    let mut components = path.components();
    if components.next()
        != Some(std::path::Component::Normal(std::ffi::OsStr::new(
            DOCUMENTS_NAME,
        )))
        || components.any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return false;
    }
    (entry_type.is_dir() && size == 0)
        || (entry_type.is_file() && size <= crate::MAX_UPLOAD_BYTES as u64)
}

fn verify_snapshot(root: &Path) -> Result<BackupManifest, String> {
    let manifest: BackupManifest = serde_json::from_reader(
        File::open(root.join(MANIFEST_NAME)).map_err(|_| "备份缺少清单".to_string())?,
    )
    .map_err(|_| "备份清单无效".to_string())?;
    if manifest.format != BACKUP_FORMAT {
        return Err("备份版本暂不受支持".to_string());
    }
    let database = root.join(DATABASE_NAME);
    if hash_file(&database)? != manifest.database_sha256 {
        return Err("备份数据库校验失败".to_string());
    }
    let documents = root.join(DOCUMENTS_NAME);
    let mut actual_documents = collect_documents(&documents)?;
    actual_documents.sort_by(|left, right| left.path.cmp(&right.path));
    let mut expected_documents = manifest.documents.clone();
    expected_documents.sort_by(|left, right| left.path.cmp(&right.path));
    if actual_documents != expected_documents
        || manifest.document_count != expected_documents.len()
        || manifest.document_bytes != expected_documents.iter().map(|file| file.size).sum::<u64>()
    {
        return Err("备份源文档清单校验失败".to_string());
    }
    for expected in &expected_documents {
        let relative = crate::managed_relative_path(&expected.path)
            .ok_or_else(|| "备份清单包含不安全路径".to_string())?;
        let path = documents.join(relative);
        let metadata = fs::metadata(&path).map_err(|_| "备份源文档不完整".to_string())?;
        if metadata.len() != expected.size || hash_file(&path)? != expected.sha256 {
            return Err("备份源文档校验失败".to_string());
        }
    }
    verify_database_snapshot(&database, &expected_documents)?;
    Ok(manifest)
}

fn verify_database_snapshot(database: &Path, documents: &[BackupFile]) -> Result<(), String> {
    let connection = Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("无法打开备份数据库：{error}"))?;
    crate::store::configure_connection(&connection)
        .map_err(|error| format!("无法配置备份数据库：{error}"))?;
    let quick_check = connection
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(|error| format!("无法校验备份数据库：{error}"))?;
    if quick_check != "ok" {
        return Err("备份数据库完整性校验失败".to_string());
    }

    let manifest_documents = documents
        .iter()
        .map(|document| (document.path.as_str(), document))
        .collect::<HashMap<_, _>>();
    let mut statement = connection
        .prepare("SELECT storage_path,size,sha256 FROM documents ORDER BY id")
        .map_err(|error| format!("无法读取备份文档记录：{error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| format!("无法读取备份文档记录：{error}"))?;
    for row in rows {
        let (storage_path, size, sha256) =
            row.map_err(|error| format!("无法读取备份文档记录：{error}"))?;
        crate::managed_relative_path(&storage_path)
            .ok_or_else(|| "备份数据库包含不安全的受管源文件路径".to_string())?;
        let expected = manifest_documents
            .get(storage_path.as_str())
            .ok_or_else(|| "备份数据库引用了清单外的受管源文件".to_string())?;
        if size < 0
            || size as u64 != expected.size
            || !sha256.eq_ignore_ascii_case(&expected.sha256)
        {
            return Err("备份数据库中的受管源文件记录与清单不一致".to_string());
        }
    }

    let has_invalid_vector = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM chunks WHERE typeof(vec)!='blob' OR length(vec)=0 OR length(vec)>?1 OR length(vec)%4!=0)",
            [crate::MAX_VECTOR_BLOB_BYTES as i64],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| format!("无法校验备份向量索引：{error}"))?;
    if has_invalid_vector {
        return Err("备份数据库包含无效或过大的向量索引".to_string());
    }
    Ok(())
}

fn preserve_current_host_state(data_dir: &Path, staged_database: &Path) -> Result<(), String> {
    let current = data_dir.join(DATABASE_NAME);
    if !current.is_file() {
        return Err("本机恢复需要现有共享知识库".to_string());
    }
    let connection = Connection::open(staged_database).map_err(|error| error.to_string())?;
    crate::store::configure_connection(&connection).map_err(|error| error.to_string())?;
    connection
        .execute(
            "ATTACH DATABASE ?1 AS current_host",
            params![current.to_string_lossy().as_ref()],
        )
        .map_err(|error| error.to_string())?;
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         DELETE FROM devices;
         INSERT INTO devices SELECT * FROM current_host.devices;
         DELETE FROM shares;
         DELETE FROM join_requests;
         DELETE FROM invites;
         DELETE FROM meta WHERE key IN ('server_id','server_identity','server_name','host_owner_device_id');
         INSERT INTO meta(key,value)
           SELECT key,value FROM current_host.meta
           WHERE key IN ('server_id','server_identity','server_name','host_owner_device_id');
         COMMIT;",
    )
    .map_err(|error| format!("无法保留本机成员和身份：{error}"))?;
    Ok(())
}

fn clear_host_state(database: &Path) -> Result<(), String> {
    let connection = Connection::open(database).map_err(|error| error.to_string())?;
    crate::store::configure_connection(&connection).map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             DELETE FROM shares;
             DELETE FROM join_requests;
             DELETE FROM invites;
             DELETE FROM devices;
             DELETE FROM meta WHERE key IN ('server_id','server_identity','server_name','host_owner_device_id');
             COMMIT;",
        )
        .map_err(|error| format!("无法清理原主机身份：{error}"))?;
    Ok(())
}

fn replace_data_dir(data_dir: &Path, staging: TempDir) -> Result<(), String> {
    let rollback = restore_rollback_path(data_dir);
    if rollback.exists() {
        return Err("恢复回滚目录已存在，请稍后重试".to_string());
    }
    if data_dir.exists() {
        fs::rename(data_dir, &rollback).map_err(|error| format!("无法创建恢复点：{error}"))?;
    }
    let staged_path = staging.keep();
    if let Err(error) = fs::rename(&staged_path, data_dir) {
        let _ = fs::rename(&rollback, data_dir);
        return Err(format!("无法应用恢复数据：{error}"));
    }
    let _ = fs::remove_file(data_dir.join(MANIFEST_NAME));
    if rollback.exists() {
        fs::remove_dir_all(&rollback)
            .map_err(|error| format!("恢复成功，但旧恢复点清理失败：{error}"))?;
    }
    Ok(())
}

fn restore_rollback_path(data_dir: &Path) -> PathBuf {
    data_dir.with_extension("restore-backup")
}

fn collect_documents(root: &Path) -> Result<Vec<BackupFile>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut pending = vec![root.to_path_buf()];
    let mut output = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let file_type = entry.file_type().map_err(|error| error.to_string())?;
            if file_type.is_symlink() {
                return Err("源文档目录包含不受支持的符号链接".to_string());
            }
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                let relative = entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|error| error.to_string())?
                    .to_path_buf();
                let path = relative
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                let metadata = entry.metadata().map_err(|error| error.to_string())?;
                output.push(BackupFile {
                    path,
                    size: metadata.len(),
                    sha256: hash_file(&entry.path())?,
                });
            }
        }
    }
    Ok(output)
}

fn copy_snapshot_documents(
    snapshot: &Connection,
    source_root: &Path,
    snapshot_root: &Path,
) -> Result<(), String> {
    let mut statement = snapshot
        .prepare("SELECT storage_path,size,sha256 FROM documents ORDER BY id")
        .map_err(|error| format!("无法读取备份文档清单：{error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| format!("无法读取备份文档清单：{error}"))?;
    for row in rows {
        let (storage_path, expected_size, expected_sha256) =
            row.map_err(|error| format!("无法读取备份文档：{error}"))?;
        let relative = crate::managed_relative_path(&storage_path)
            .ok_or_else(|| "备份清单包含不安全路径".to_string())?;
        let source = source_root.join(relative);
        let metadata = fs::symlink_metadata(&source)
            .map_err(|error| format!("备份源文档不存在({}): {error}", source.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(format!("备份源文档不是普通文件({})", source.display()));
        }
        let destination = snapshot_root.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("无法创建文档快照目录：{error}"))?;
        }
        fs::copy(&source, &destination)
            .map_err(|error| format!("无法复制备份源文档({}): {error}", source.display()))?;
        let actual_size = destination
            .metadata()
            .map_err(|error| format!("无法检查文档快照({}): {error}", destination.display()))?
            .len();
        let actual_sha256 = hash_file(&destination)?;
        if expected_size < 0
            || actual_size != expected_size as u64
            || !actual_sha256.eq_ignore_ascii_case(&expected_sha256)
        {
            return Err(format!(
                "源文档在备份期间发生变化({})，请稍后重试",
                storage_path
            ));
        }
    }
    Ok(())
}

fn ensure_path_outside_data_dir(
    data_dir: &Path,
    candidate_parent: &Path,
    message: &str,
) -> Result<(), String> {
    if !data_dir.exists() {
        return Ok(());
    }
    let data_dir = fs::canonicalize(data_dir).map_err(|error| error.to_string())?;
    let candidate_parent = fs::canonicalize(candidate_parent).map_err(|error| error.to_string())?;
    if candidate_parent.starts_with(data_dir) {
        return Err(message.to_string());
    }
    Ok(())
}

fn create_database_snapshot(database: &Path, snapshot: &Path) -> Result<(), String> {
    let connection =
        Connection::open(database).map_err(|error| format!("无法打开共享知识库数据库：{error}"))?;
    crate::store::configure_connection(&connection)
        .map_err(|error| format!("无法配置共享知识库数据库：{error}"))?;
    let _checkpoint = connection
        .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|error| format!("无法检查共享知识库 WAL：{error}"))?;
    connection
        .execute("VACUUM INTO ?1", [snapshot.to_string_lossy().into_owned()])
        .map_err(|error| format!("无法创建数据库一致性快照：{error}"))?;
    drop(connection);

    let snapshot_connection = Connection::open_with_flags(
        snapshot,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("无法验证数据库备份快照：{error}"))?;
    crate::store::configure_connection(&snapshot_connection)
        .map_err(|error| format!("无法配置数据库备份快照：{error}"))?;
    let integrity = snapshot_connection
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(|error| format!("无法验证数据库备份快照：{error}"))?;
    if integrity != "ok" {
        return Err(format!("数据库备份快照校验失败：{integrity}"));
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_data(root: &Path, server_id: &str, device_id: &str) {
        fs::create_dir_all(root.join(DOCUMENTS_NAME)).unwrap();
        fs::write(
            root.join(DOCUMENTS_NAME).join("sample.txt"),
            b"hello backup",
        )
        .unwrap();
        let connection = Connection::open(root.join(DATABASE_NAME)).unwrap();
        connection.execute_batch(crate::store::SCHEMA).unwrap();
        connection
            .execute(
                "INSERT INTO meta(key,value) VALUES('server_id',?1)",
                [server_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO meta(key,value) VALUES('server_identity','identity')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO meta(key,value) VALUES('server_name','Shared')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO meta(key,value) VALUES('host_owner_device_id',?1)",
                [device_id],
            )
            .unwrap();
        connection.execute("INSERT INTO devices(id,name,scope,token_hash,created_at,revoked) VALUES(?1,'Owner','owner','hash',1,0)", [device_id]).unwrap();
        connection
            .execute(
                "INSERT INTO collections(id,name,status,created_at,updated_at) VALUES(1,'Backup','ready',1,1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO documents(id,collection_id,name,ext,storage_path,size,sha256,status,n_chunks,created_at,updated_at) VALUES(1,1,'sample.txt','txt','sample.txt',12,?1,'ready',0,1,1)",
                [hash_file(&root.join(DOCUMENTS_NAME).join("sample.txt")).unwrap()],
            )
            .unwrap();
    }

    fn write_snapshot_manifest(root: &Path, server_id: &str) {
        let document = root.join(DOCUMENTS_NAME).join("sample.txt");
        let manifest = BackupManifest {
            format: BACKUP_FORMAT,
            created_at: 1,
            server_id: server_id.to_string(),
            database_sha256: hash_file(&root.join(DATABASE_NAME)).unwrap(),
            document_count: 1,
            document_bytes: document.metadata().unwrap().len(),
            documents: vec![BackupFile {
                path: "sample.txt".to_string(),
                size: document.metadata().unwrap().len(),
                sha256: hash_file(&document).unwrap(),
            }],
        };
        fs::write(
            root.join(MANIFEST_NAME),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn encrypted_backup_restores_same_host_and_content_only_modes() {
        let source = tempfile::tempdir().unwrap();
        seed_data(source.path(), "source-server", "source-owner");
        let original_tls = crate::tls::ensure_tls_identity(source.path()).unwrap();
        let (local_identity, local_recipient) = generate_identity();
        let (recovery_identity, recovery_recipient) = generate_identity();
        let backup_root = tempfile::tempdir().unwrap();
        let backup = backup_root.path().join("shared.pinbak");
        let manifest = create_encrypted_backup(
            source.path(),
            &backup,
            &[local_recipient, recovery_recipient],
        )
        .unwrap();
        assert_eq!(manifest.document_count, 1);
        assert!(!fs::read(&backup)
            .unwrap()
            .windows(5)
            .any(|chunk| chunk == b"hello"));

        fs::write(
            source.path().join(DOCUMENTS_NAME).join("sample.txt"),
            b"changed",
        )
        .unwrap();
        let current = Connection::open(source.path().join(DATABASE_NAME)).unwrap();
        current
            .execute(
                "UPDATE meta SET value='current-server' WHERE key='server_id'",
                [],
            )
            .unwrap();
        drop(current);
        restore_encrypted_backup(
            source.path(),
            &backup,
            &local_identity,
            RestoreMode::SameHost,
        )
        .unwrap();
        let same_host_tls = crate::tls::ensure_tls_identity(source.path()).unwrap();
        assert_eq!(same_host_tls.ca_pem, original_tls.ca_pem);
        assert_eq!(
            fs::read(source.path().join(DOCUMENTS_NAME).join("sample.txt")).unwrap(),
            b"hello backup"
        );
        let restored = Connection::open(source.path().join(DATABASE_NAME)).unwrap();
        assert_eq!(
            restored
                .query_row("SELECT value FROM meta WHERE key='server_id'", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "current-server"
        );
        assert_eq!(
            restored
                .query_row(
                    "SELECT value FROM meta WHERE key='host_owner_device_id'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "source-owner"
        );
        drop(restored);

        let migrated = tempfile::tempdir().unwrap();
        let target = migrated.path().join("data");
        restore_encrypted_backup(
            &target,
            &backup,
            &recovery_identity,
            RestoreMode::ContentOnly,
        )
        .unwrap();
        let migrated_tls = crate::tls::ensure_tls_identity(&target).unwrap();
        assert_ne!(migrated_tls.ca_pem, original_tls.ca_pem);
        let migrated_db = Connection::open(target.join(DATABASE_NAME)).unwrap();
        assert_eq!(
            migrated_db
                .query_row("SELECT COUNT(*) FROM devices", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            migrated_db
                .query_row(
                    "SELECT COUNT(*) FROM meta WHERE key='server_id'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn encrypted_backup_includes_commits_still_pinned_in_the_wal() {
        let source = tempfile::tempdir().unwrap();
        seed_data(source.path(), "source-server", "source-owner");
        let database = source.path().join(DATABASE_NAME);

        // Keep an older read snapshot open so PASSIVE checkpoint cannot drain every
        // WAL frame. A raw copy of knowledge.db would therefore miss this marker.
        let reader = Connection::open(&database).unwrap();
        reader
            .execute_batch("PRAGMA wal_autocheckpoint=0; BEGIN;")
            .unwrap();
        assert_eq!(
            reader
                .query_row("SELECT value FROM meta WHERE key='server_id'", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "source-server"
        );
        let writer = Connection::open(&database).unwrap();
        writer
            .execute_batch("PRAGMA wal_autocheckpoint=0;")
            .unwrap();
        writer
            .execute(
                "INSERT INTO meta(key,value) VALUES('wal_snapshot_marker','committed')",
                [],
            )
            .unwrap();
        assert!(database.with_extension("db-wal").is_file());

        let (identity, recipient) = generate_identity();
        let backup_root = tempfile::tempdir().unwrap();
        let backup = backup_root.path().join("wal.pinbak");
        create_encrypted_backup(source.path(), &backup, &[recipient]).unwrap();

        let extracted = tempfile::tempdir().unwrap();
        let identity = Identity::from_str(&identity).unwrap();
        decrypt_archive(&backup, &identity, &extracted).unwrap();
        let snapshot = Connection::open(extracted.path().join(DATABASE_NAME)).unwrap();
        assert_eq!(
            snapshot
                .query_row(
                    "SELECT value FROM meta WHERE key='wal_snapshot_marker'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "committed"
        );

        reader.execute_batch("ROLLBACK;").unwrap();
        drop(writer);
    }

    #[test]
    fn backup_never_overwrites_or_writes_inside_the_data_directory() {
        let source = tempfile::tempdir().unwrap();
        seed_data(source.path(), "source-server", "source-owner");
        let (_, recipient) = generate_identity();
        let backup_root = tempfile::tempdir().unwrap();
        let existing = backup_root.path().join("existing.pinbak");
        fs::write(&existing, b"keep me").unwrap();

        assert!(create_encrypted_backup(
            source.path(),
            &existing,
            std::slice::from_ref(&recipient)
        )
        .unwrap_err()
        .contains("已存在"));
        assert_eq!(fs::read(&existing).unwrap(), b"keep me");

        let nested = source.path().join(DOCUMENTS_NAME).join("nested.pinbak");
        assert!(
            create_encrypted_backup(source.path(), &nested, &[recipient])
                .unwrap_err()
                .contains("数据目录")
        );
        assert!(!nested.exists());
    }

    #[test]
    fn restore_archive_rejects_links_escape_paths_and_oversized_entries() {
        assert!(safe_archive_entry(
            Path::new("documents/sample.txt"),
            tar::EntryType::Regular,
            12,
        ));
        assert!(!safe_archive_entry(
            Path::new("documents/sample.txt"),
            tar::EntryType::Symlink,
            0,
        ));
        assert!(!safe_archive_entry(
            Path::new("documents/../secret"),
            tar::EntryType::Regular,
            12,
        ));
        assert!(!safe_archive_entry(
            Path::new("documents-elsewhere/secret"),
            tar::EntryType::Regular,
            12,
        ));
        assert!(!safe_archive_entry(
            Path::new("documents/large.bin"),
            tar::EntryType::Regular,
            crate::MAX_UPLOAD_BYTES as u64 + 1,
        ));
    }

    #[test]
    fn snapshot_rejects_self_consistent_manifest_with_unsafe_database_path() {
        let snapshot = tempfile::tempdir().unwrap();
        seed_data(snapshot.path(), "source-server", "source-owner");
        let connection = Connection::open(snapshot.path().join(DATABASE_NAME)).unwrap();
        connection
            .execute(
                "UPDATE documents SET storage_path='../../../../etc/passwd' WHERE id=1",
                [],
            )
            .unwrap();
        drop(connection);
        write_snapshot_manifest(snapshot.path(), "source-server");

        let error = verify_snapshot(snapshot.path()).unwrap_err();

        assert!(error.contains("不安全的受管源文件路径"));
    }

    #[test]
    fn snapshot_rejects_oversized_or_malformed_vector_blobs() {
        for vector_size in [crate::MAX_VECTOR_BLOB_BYTES + 4, 3] {
            let snapshot = tempfile::tempdir().unwrap();
            seed_data(snapshot.path(), "source-server", "source-owner");
            let connection = Connection::open(snapshot.path().join(DATABASE_NAME)).unwrap();
            connection
                .execute(
                    "INSERT INTO chunks(document_id,collection_id,ord,text,vec) VALUES(1,1,0,'chunk',zeroblob(?1))",
                    [vector_size as i64],
                )
                .unwrap();
            drop(connection);
            write_snapshot_manifest(snapshot.path(), "source-server");

            let error = verify_snapshot(snapshot.path()).unwrap_err();

            assert!(error.contains("无效或过大的向量索引"));
        }
    }

    #[test]
    fn backup_rejects_source_bytes_that_no_longer_match_the_database_snapshot() {
        let source = tempfile::tempdir().unwrap();
        seed_data(source.path(), "source-server", "source-owner");
        fs::write(
            source.path().join(DOCUMENTS_NAME).join("sample.txt"),
            b"changed after database commit",
        )
        .unwrap();
        let (_, recipient) = generate_identity();
        let output_root = tempfile::tempdir().unwrap();
        let output = output_root.path().join("inconsistent.pinbak");

        let error = create_encrypted_backup(source.path(), &output, &[recipient]).unwrap_err();

        assert!(error.contains("源文档在备份期间发生变化"));
        assert!(!output.exists());
    }

    #[test]
    fn interrupted_restore_recovers_the_previous_data_directory_on_next_boot() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("knowledge-data");
        let rollback = restore_rollback_path(&data);
        fs::create_dir_all(&rollback).unwrap();
        fs::write(rollback.join("sentinel"), b"previous").unwrap();

        recover_interrupted_restore(&data).unwrap();

        assert_eq!(fs::read(data.join("sentinel")).unwrap(), b"previous");
        assert!(!rollback.exists());
    }

    #[test]
    fn interrupted_restore_cleanup_after_successful_swap_drops_the_stale_rollback() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("knowledge-data");
        let rollback = restore_rollback_path(&data);
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("sentinel"), b"restored").unwrap();
        fs::create_dir_all(&rollback).unwrap();
        fs::write(rollback.join("sentinel"), b"previous").unwrap();

        recover_interrupted_restore(&data).unwrap();

        // staging 已换入后崩溃：data_dir 是恢复后的数据，rollback 只是旧数据残留。
        assert_eq!(fs::read(data.join("sentinel")).unwrap(), b"restored");
        assert!(!rollback.exists());
    }
}
