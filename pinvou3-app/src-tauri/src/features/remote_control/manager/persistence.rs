//! Persistent configuration, the durable RPC idempotency ledger, pending
//! revocation replay, and user-custom Relay settings.
//!
//! These helpers are stateless and touch only the private JSON files under the
//! pinvou3 home directory. They never lock `Inner`; the facade reads the result
//! and updates `Inner` under its own guard.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{
    ActiveEndpoint, PendingRevocation, PendingRevocationLedger, RpcLedger, WebAccessConfig,
    WebAccessInfo, platform,
};
use crate::platform::paths;

pub(super) fn pairing_info(endpoint: &ActiveEndpoint) -> WebAccessInfo {
    WebAccessInfo {
        endpoint_id: endpoint.config.endpoint_id.clone(),
        url: endpoint.url.clone(),
        qr_data_url: crate::features::connectors::connector_cli::make_qr(&endpoint.url),
        status: endpoint.status,
    }
}

pub(super) fn fresh_config(allow_host_workspace: bool) -> WebAccessConfig {
    WebAccessConfig {
        relay_url: remote_relay_ws_url(),
        endpoint_id: format!("ep_{}", crate::features::remote_control::short_token(24)),
        access_token: crate::features::remote_control::short_token(48),
        desktop_secret: crate::features::remote_control::short_token(48),
        allow_host_workspace,
    }
}

/// ws(s) 地址统一校验：长度上限、`ws://` 或 `wss://` 前缀、不含空白；
/// 失败文案由调用方保留原样。
fn validate_ws_url(url: &str, invalid_message: &str) -> Result<(), String> {
    if url.len() > 2_048
        || !(url.starts_with("ws://") || url.starts_with("wss://"))
        || url.chars().any(char::is_whitespace)
    {
        return Err(invalid_message.to_string());
    }
    Ok(())
}

/// http(s) 页面基址统一校验，与 `validate_ws_url` 同一基线。
fn validate_http_base_url(url: &str, invalid_message: &str) -> Result<(), String> {
    if url.len() > 2_048
        || !(url.starts_with("http://") || url.starts_with("https://"))
        || url.chars().any(char::is_whitespace)
    {
        return Err(invalid_message.to_string());
    }
    Ok(())
}

pub(super) fn validate_relay_settings(settings: &RelaySettings) -> Result<(), String> {
    validate_ws_url(
        &settings.relay_url,
        "relay address does not normalize to a valid ws(s) URL",
    )?;
    validate_http_base_url(
        &settings.public_base_url,
        "relay address does not normalize to a valid http(s) base",
    )
}

pub(super) fn validate_config(config: &WebAccessConfig) -> Result<(), String> {
    validate_ws_url(&config.relay_url, "Web access relay_url is invalid")?;
    if config.endpoint_id.len() < 8
        || config.endpoint_id.len() > 128
        || !config
            .endpoint_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("Web access endpoint_id is invalid".to_string());
    }
    if !(24..=1_024).contains(&config.access_token.len())
        || !(24..=1_024).contains(&config.desktop_secret.len())
    {
        return Err("Web access credentials are invalid".to_string());
    }
    Ok(())
}

pub(super) fn config_path() -> PathBuf {
    paths::pinvou3_home().join("web-access.json")
}

pub(super) fn process_lock_path() -> PathBuf {
    paths::pinvou3_home().join("web-access.lock")
}

pub(super) fn pending_revocations_path() -> PathBuf {
    paths::pinvou3_home().join("web-access-pending-revocations.json")
}

/// Acquire the process-ownership lock (single desktop owner for Web access).
///
/// Uses `fd_lock` (already in the dependency graph via codewhale-config)
/// instead of fs2; both wrap the same OS primitive (flock on Unix,
/// LockFileEx on Windows). On success the backing allocation is kept alive
/// for the guard's `'static` lifetime: the ownership lock lives until process
/// exit anyway, and dropping the guard (or the process) releases it. On the
/// contended path the allocation is reclaimed so a failed attempt does not
/// strand an unlocked fd for the rest of the process (fs2 closed its file
/// there as well).
pub(super) fn acquire_process_lock(
    path: &Path,
) -> Result<fd_lock::RwLockWriteGuard<'static, File>, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid Web access lock path: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    platform::configure_private_open_options(&mut options);
    let file = options
        .open(path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    platform::enforce_private_permissions(&file, path)
        .map_err(|error| format!("set private permissions on {}: {error}", path.display()))?;
    // SAFETY: `lock` is an owned `Box::into_raw` allocation with no other
    // references. The `&mut` below lives only for the `try_write` call unless
    // a guard is returned; a returned guard carries it for `'static`, which
    // is intended — the process-ownership lock is held until process exit and
    // the allocation is deliberately never freed on that path.
    let lock = Box::into_raw(Box::new(fd_lock::RwLock::new(file)));
    // SAFETY: lock is the sole owning pointer handed out by Box::into_raw with no
    // other references; the &mut lives only for the try_write call, a successfully
    // returned guard carries that borrow (deliberately held until process exit,
    // with the allocation never freed), and the failure path reclaims it via
    // Box::from_raw below.
    let guard = match unsafe { &mut *lock }.try_write() {
        Ok(guard) => guard,
        Err(error) => {
            // SAFETY: `try_write` failed, so no guard borrows the lock and the
            // `&mut` above has expired. Re-taking the Box is sound; dropping
            // it closes the fd instead of stranding it for the process
            // lifetime.
            unsafe { drop(Box::from_raw(lock)) };
            return Err(format!(
                "Web access is already owned by another desktop process ({}): {error}",
                path.display()
            ));
        }
    };
    Ok(guard)
}

pub(super) fn rpc_ledger_path() -> PathBuf {
    paths::pinvou3_home().join("web-access-rpc-ledger.json")
}

pub(super) fn load_config() -> Result<Option<WebAccessConfig>, String> {
    load_config_from(&config_path())
}

pub(super) fn load_config_from(path: &Path) -> Result<Option<WebAccessConfig>, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    // 读取即修复,且先于任何内容读取:损坏 + 放宽权限的组合也不能把一个
    // 世界可读的凭据文件留在盘上(备份恢复或外部工具可能把它改成组/全员
    // 可读)。Round-6 复审:修复必须挂在**第一个**句柄上、先于
    // `read_to_string`——非 UTF-8 的损坏内容会让读取腿先行报错返回,挂在
    // 读取之后的修复永远不会执行,放宽权限的损坏文件就一直裸留在盘上。
    // 与写入路径的 0600 基线保持一致;修复失败按读取错误上抛,同时免去
    // 第二次 open 的 TOCTOU(两次打开之间路径可被换掉)。Windows/fallback
    // 平台该调用是显式 no-op。
    platform::enforce_private_permissions(&file, path).map_err(|error| {
        // Round-8 复审 minor:修复自身失败(fchmod 需要 owner 权限,
        // 备份恢复可能留下他者属主的世界可读密文件)时,不能把它一直
        // 留在众所周知的路径上。隔离改名只需要父目录写权限,下一轮
        // start 会用 0600 重建;隔离也失败(目录只读/Windows 文件锁)
        // 则保持原错误——那与目录不可写同级,后续写入同样会失败。
        match crate::platform::filesystem::quarantine_corrupt_file(path) {
            Ok(quarantined) => eprintln!(
                "[web-access] could not restrict permissions on {} ({error}); \
                     quarantined to {} and the next start creates a fresh 0600 file",
                path.display(),
                quarantined.display()
            ),
            Err(quarantine_error) => eprintln!(
                "[web-access] could not restrict permissions on {} ({error}) and the \
                     quarantine rename also failed ({quarantine_error}); the loose file \
                     stays in place",
                path.display()
            ),
        }
        format!("restrict {}: {error}", path.display())
    })?;
    let mut text = String::new();
    file.take((super::MAX_WEB_ACCESS_CONFIG_BYTES + 1) as u64)
        .read_to_string(&mut text)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    if text.len() > super::MAX_WEB_ACCESS_CONFIG_BYTES {
        return Err(format!(
            "Web access config is larger than {} KiB: {}",
            super::MAX_WEB_ACCESS_CONFIG_BYTES / 1024,
            path.display()
        ));
    }
    let config: WebAccessConfig = serde_json::from_str(&text)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    validate_config(&config)?;
    Ok(Some(config))
}

pub(super) fn persist_config(config: &WebAccessConfig) -> Result<(), String> {
    validate_config(config)?;
    atomic_write_private_json(&config_path(), config, "Web access config")
}

pub(super) fn remove_config() -> Result<(), String> {
    remove_private_file(&config_path())
}

// ── 用户自定义 Relay 地址 ─────────────────────────────────────────────
// 没有应用内设置入口：web-relay.json 由用户手工维护，格式为「WebSocket
// 地址 + 页面基址」一对；生效优先级：运行时 env 覆盖 > PINVOU_REMOTE_* env
// > 本设置 > 内置默认。

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelaySettings {
    pub relay_url: String,
    pub public_base_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelaySettingsInfo {
    pub relay_url: String,
    pub public_base_url: String,
    pub custom: bool,
    pub default_relay_url: String,
    pub default_public_base_url: String,
}

pub(super) fn relay_settings_path() -> PathBuf {
    paths::pinvou3_home().join("web-relay.json")
}

/// 读取失败（不存在/损坏/非法）一律回落默认 relay：设置文件不是凭据，宁可
/// 降级也不把「启用 Web 访问」整个卡死；文件损坏时由用户手工修复或删除。
pub(super) fn load_relay_settings() -> Option<RelaySettings> {
    let file = File::open(relay_settings_path()).ok()?;
    let mut text = String::new();
    file.take((super::MAX_WEB_ACCESS_CONFIG_BYTES + 1) as u64)
        .read_to_string(&mut text)
        .ok()?;
    if text.len() > super::MAX_WEB_ACCESS_CONFIG_BYTES {
        return None;
    }
    let settings: RelaySettings = serde_json::from_str(&text).ok()?;
    validate_relay_settings(&settings).ok()?;
    Some(settings)
}

pub(super) fn pending_revocation_key(pending: &PendingRevocation) -> String {
    format!("{}\n{}", pending.relay_url, pending.endpoint_id)
}

pub(super) fn validate_pending_revocation(pending: &PendingRevocation) -> Result<(), String> {
    validate_ws_url(
        &pending.relay_url,
        "pending Web revocation has an invalid relay_url",
    )?;
    if pending.endpoint_id.len() < 4
        || pending.endpoint_id.len() > 128
        || !pending
            .endpoint_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("pending Web revocation has an invalid endpoint_id".to_string());
    }
    if pending.desktop_secret.len() < 24 || pending.desktop_secret.len() > 1_024 {
        return Err("pending Web revocation has an invalid desktop secret".to_string());
    }
    Ok(())
}

pub(super) fn validate_pending_revocation_ledger(
    ledger: &PendingRevocationLedger,
) -> Result<(), String> {
    if ledger.version != super::PENDING_REVOCATIONS_VERSION {
        return Err(format!(
            "unsupported pending Web revocation version {}",
            ledger.version
        ));
    }
    if ledger.entries.len() > super::MAX_PENDING_REVOCATIONS {
        return Err(format!(
            "pending Web revocation count exceeds {}",
            super::MAX_PENDING_REVOCATIONS
        ));
    }
    let mut keys = HashSet::new();
    for pending in &ledger.entries {
        validate_pending_revocation(pending)?;
        if !keys.insert(pending_revocation_key(pending)) {
            return Err(format!(
                "duplicate pending Web revocation for {}",
                pending.endpoint_id
            ));
        }
    }
    Ok(())
}

/// replay 启动路径专用:Corrupt 的原件已在读取时隔离,按空账本继续——与
/// queue/acknowledge 的「恢复路径不被坏文件卡死」口径一致,enable/resume/
/// rotate 不再为一个已经清理掉的文件一次性失败(原文 parse 错误直接抛给
/// 用户,重试虽能自愈但首因已不存在)。Unavailable 仍是瞬时 IO,照旧上抛。
pub(super) fn load_pending_revocations_for_replay() -> Result<PendingRevocationLedger, String> {
    match load_pending_revocations_classified(&pending_revocations_path()) {
        RevocationLedgerLoad::Ledger(ledger) => Ok(ledger),
        RevocationLedgerLoad::Corrupt(error) => {
            eprintln!("[web-access] pending-revocation ledger is corrupt, starting fresh: {error}");
            Ok(PendingRevocationLedger::default())
        }
        RevocationLedgerLoad::Unavailable(error) => Err(error),
    }
}

/// 严格读取(Corrupt/Unavailable 都报错):生产路径全部走分类版或
/// replay 包装;本函数只服务测试对真实磁盘文件的直接断言。
#[cfg(test)]
pub(super) fn load_pending_revocations_from(
    path: &Path,
) -> Result<PendingRevocationLedger, String> {
    match load_pending_revocations_classified(path) {
        RevocationLedgerLoad::Ledger(ledger) => Ok(ledger),
        RevocationLedgerLoad::Corrupt(error) | RevocationLedgerLoad::Unavailable(error) => {
            Err(error)
        }
    }
}

/// Pending-revocation 账本读取的三分类:Corrupt 表示无法解析或超限的内容
/// (没有任何条目可救;原件已在读取时隔离留证),重试无意义;可解析但未过
/// 整体校验的账本在读取时已逐条抢救为 Ledger,不走这里。Unavailable 是瞬时
/// IO,重试有意义。queue/acknowledge 路径靠它决定「挪开坏账本继续」还是
/// 「上抛等重试」——停用/轮换是恢复路径,不能被一个坏文件永久卡死。
enum RevocationLedgerLoad {
    Ledger(PendingRevocationLedger),
    Corrupt(String),
    Unavailable(String),
}

/// 逐条抢救可解析但未过整体校验的账本:version 漂移、条目超限、单条损坏、
/// 重复键都只否定各自的条目,不否定其余仍然合法的待吊销记录——整体丢弃会
/// 让旧 endpoint 在 relay 侧保持可加入。每条沿用与正常加载相同的
/// `validate_pending_revocation` 严格度,重复键保留首个,超出容量即止。
fn salvage_pending_revocations(ledger: &PendingRevocationLedger) -> PendingRevocationLedger {
    let mut salvaged = PendingRevocationLedger::default();
    let mut keys = HashSet::new();
    for pending in &ledger.entries {
        if salvaged.entries.len() >= super::MAX_PENDING_REVOCATIONS {
            break;
        }
        if validate_pending_revocation(pending).is_err() {
            continue;
        }
        if !keys.insert(pending_revocation_key(pending)) {
            continue;
        }
        salvaged.entries.push_back(pending.clone());
    }
    salvaged
}

fn quarantine_pending_ledger(path: &Path, data: &[u8]) {
    match crate::platform::filesystem::quarantine_corrupt_file(path) {
        Ok(quarantined) => {
            eprintln!(
                "[web-access] quarantined corrupt pending-revocation ledger to {}",
                quarantined.display()
            );
        }
        Err(rename_error) => {
            // rename 失败(Windows AV/备份锁等)不能默许证据被后续的
            // fresh-start 写入原地覆盖/删除。先整文件复制(oversize 场景
            // 内存中的 `data` 只有截断前缀,完整证据在盘上);复制也失败
            // 再退回内存字节;两者皆败才降级为仅日志。
            match crate::platform::filesystem::quarantine_corrupt_file_copy(path) {
                Ok(quarantined) => eprintln!(
                    "[web-access] rename failed ({rename_error}); copied corrupt pending-revocation ledger to {}",
                    quarantined.display()
                ),
                Err(copy_error) => {
                    match crate::platform::filesystem::quarantine_corrupt_file_bytes(path, data) {
                        Ok(quarantined) => eprintln!(
                            "[web-access] rename failed ({rename_error}, copy failed: {copy_error}); \
                             wrote in-memory bytes of corrupt pending-revocation ledger to {}",
                            quarantined.display()
                        ),
                        Err(error) => eprintln!(
                            "[web-access] failed to quarantine corrupt pending-revocation ledger {}: {error}",
                            path.display()
                        ),
                    }
                }
            }
        }
    }
}

fn load_pending_revocations_classified(path: &Path) -> RevocationLedgerLoad {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return RevocationLedgerLoad::Ledger(PendingRevocationLedger::default());
        }
        Err(error) => {
            return RevocationLedgerLoad::Unavailable(format!("read {}: {error}", path.display()));
        }
    };
    // 读取即修复,且先于任何内容读取(Round-7 复审补齐,与 load_config_from
    // 同形):该文件持有 desktop_secret,损坏 + 放宽权限的组合也不能把世界
    // 可读的密文件留在盘上。修复挂在**第一个**句柄上——读取腿的真实 I/O
    // 错误(EISDIR/EACCES 等)会先行返回,挂在读取之后的修复永远不会执行,
    // 放宽权限的损坏文件就一直裸留在盘上;同时免去第二次 open 的 TOCTOU
    // (两次打开之间路径可被换掉,修复的可能不是读到的那个 inode)。
    // (Windows/fallback 平台该调用是显式 no-op。)
    if let Err(error) = platform::enforce_private_permissions(&file, path) {
        // Round-8 复审 minor:修复自身失败时同样把放宽权限的密文件隔离
        // 挪离众所周知的路径(与 load_config_from 同因同策);隔离失败则
        // 保持 Unavailable 原错误。
        match crate::platform::filesystem::quarantine_corrupt_file(path) {
            Ok(quarantined) => eprintln!(
                "[web-access] could not restrict permissions on {} ({error}); \
                 quarantined to {} and the next write creates a fresh 0600 ledger",
                path.display(),
                quarantined.display()
            ),
            Err(quarantine_error) => eprintln!(
                "[web-access] could not restrict permissions on {} ({error}) and the \
                 quarantine rename also failed ({quarantine_error}); the loose file \
                 stays in place",
                path.display()
            ),
        }
        return RevocationLedgerLoad::Unavailable(format!("restrict {}: {error}", path.display()));
    }
    // 与 config 读取相同的 take() 有界读取:先按上限截断再检查,损坏/超大
    // 文件不能触发无界分配。
    let mut data = Vec::new();
    if let Err(error) = file
        .take((super::MAX_PENDING_REVOCATIONS_FILE_BYTES + 1) as u64)
        .read_to_end(&mut data)
    {
        return RevocationLedgerLoad::Unavailable(format!("read {}: {error}", path.display()));
    }
    if data.len() > super::MAX_PENDING_REVOCATIONS_FILE_BYTES {
        quarantine_pending_ledger(path, &data);
        return RevocationLedgerLoad::Corrupt(format!(
            "pending Web revocation file is larger than {} KiB: {}",
            super::MAX_PENDING_REVOCATIONS_FILE_BYTES / 1024,
            path.display()
        ));
    }
    let ledger = match serde_json::from_slice::<PendingRevocationLedger>(&data) {
        Ok(ledger) => ledger,
        Err(error) => {
            // 完全不可解析:没有任何条目可救,隔离原件留证后按损坏处理
            //(queue/acknowledge 会从空账本继续并用合法内容重写)。
            quarantine_pending_ledger(path, &data);
            return RevocationLedgerLoad::Corrupt(format!("parse {}: {error}", path.display()));
        }
    };
    if let Err(error) = validate_pending_revocation_ledger(&ledger) {
        // 可解析但未过整体校验(version 漂移/超限/单条损坏/重复键):逐条
        // 抢救仍合法的条目并隔离原件,而不是整体丢弃——被丢弃的待吊销条目
        // 会让旧 endpoint 在 relay 侧保持可加入。
        let salvaged = salvage_pending_revocations(&ledger);
        eprintln!(
            "[web-access] pending-revocation ledger invalid ({error}); salvaged {} of {} entries",
            salvaged.entries.len(),
            ledger.entries.len()
        );
        quarantine_pending_ledger(path, &data);
        // 抢救出的条目必须立刻落盘:进程崩溃在 worker ack 之前会丢掉这批
        // 待吊销记录(隔离副本只留证据,不会被自动重放),旧 endpoint 将在
        // relay 侧保持可加入——正是本分支要避免的结局。空抢救集按空账本
        // 语义移除已隔离的路径,避免下一次 load 重复走隔离。落盘失败则
        // 降级为内存态(与原行为一致),由后续 queue/ack 的写入自愈。
        if let Err(persist_error) = persist_pending_revocations_to(path, &salvaged) {
            eprintln!(
                "[web-access] failed to persist salvaged pending revocations: {persist_error}"
            );
        }
        return RevocationLedgerLoad::Ledger(salvaged);
    }
    RevocationLedgerLoad::Ledger(ledger)
}

pub(super) fn persist_pending_revocations_to(
    path: &Path,
    ledger: &PendingRevocationLedger,
) -> Result<(), String> {
    validate_pending_revocation_ledger(ledger)?;
    if ledger.entries.is_empty() {
        remove_private_file(path)
    } else {
        atomic_write_private_json(path, ledger, "pending Web revocations")
    }
}

pub(super) fn queue_pending_revocation(config: &WebAccessConfig) -> Result<(), String> {
    queue_pending_revocation_at(&pending_revocations_path(), PendingRevocation::from(config))
}

pub(super) fn queue_pending_revocation_at(
    path: &Path,
    pending: PendingRevocation,
) -> Result<(), String> {
    validate_pending_revocation(&pending)?;
    let mut ledger = match load_pending_revocations_classified(path) {
        RevocationLedgerLoad::Ledger(ledger) => ledger,
        RevocationLedgerLoad::Corrupt(error) => {
            // 停用/轮换是恢复路径:Corrupt 只剩完全不可解析/超限两类(可解
            // 析但未过验证的账本已在读取时逐条抢救并隔离原件),记录后从空
            // 账本继续;随后的持久化会用合法内容重写,旧 endpoint 的吊销
            // 不会被永久卡死。
            eprintln!("[web-access] pending-revocation ledger is corrupt, starting fresh: {error}");
            PendingRevocationLedger::default()
        }
        RevocationLedgerLoad::Unavailable(error) => return Err(error),
    };
    let key = pending_revocation_key(&pending);
    if let Some(existing) = ledger
        .entries
        .iter()
        .find(|entry| pending_revocation_key(entry) == key)
    {
        return if existing == &pending {
            Ok(())
        } else {
            Err(format!(
                "conflicting pending Web revocation credentials for {}",
                pending.endpoint_id
            ))
        };
    }
    if ledger.entries.len() >= super::MAX_PENDING_REVOCATIONS {
        return Err(format!(
            "pending Web revocation queue is full ({})",
            super::MAX_PENDING_REVOCATIONS
        ));
    }
    ledger.entries.push_back(pending);
    persist_pending_revocations_to(path, &ledger)
}

pub(super) fn acknowledge_pending_revocation(pending: &PendingRevocation) -> Result<(), String> {
    acknowledge_pending_revocation_at(&pending_revocations_path(), pending)
}

pub(super) fn acknowledge_pending_revocation_at(
    path: &Path,
    pending: &PendingRevocation,
) -> Result<(), String> {
    let mut ledger = match load_pending_revocations_classified(path) {
        RevocationLedgerLoad::Ledger(ledger) => ledger,
        RevocationLedgerLoad::Corrupt(error) => {
            // 与 queue 同理:吊销回放的确认不能被坏账本卡死;从空账本继续
            // 意味着确认结果(删除条目)覆盖坏文件。
            eprintln!("[web-access] pending-revocation ledger is corrupt, starting fresh: {error}");
            PendingRevocationLedger::default()
        }
        RevocationLedgerLoad::Unavailable(error) => return Err(error),
    };
    ledger.entries.retain(|entry| entry != pending);
    persist_pending_revocations_to(path, &ledger)
}

pub(super) fn eligible_pending_revocations(
    ledger: &PendingRevocationLedger,
    current: Option<&WebAccessConfig>,
) -> Vec<PendingRevocation> {
    ledger
        .entries
        .iter()
        .filter(|pending| {
            current.is_none_or(|config| {
                pending.relay_url != config.relay_url
                    || pending.endpoint_id != config.endpoint_id
                    || pending.desktop_secret != config.desktop_secret
            })
        })
        .cloned()
        .collect()
}

pub(super) fn load_or_initialize_rpc_ledger(endpoint_id: &str) -> Result<RpcLedger, String> {
    load_or_initialize_rpc_ledger_at(&rpc_ledger_path(), endpoint_id)
}

/// Path-injected variant for tests; the initialization branches still persist
/// through the global `persist_rpc_ledger`, so tests only exercise the
/// read/validation paths against injected paths.
pub(super) fn load_or_initialize_rpc_ledger_at(
    path: &Path,
    endpoint_id: &str,
) -> Result<RpcLedger, String> {
    // take() 有界读取:512 条墓碑的账本远小于 1 MiB,超大文件必然损坏,
    // 不能在解析前无界读入内存。
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let ledger = RpcLedger::for_endpoint(endpoint_id);
            persist_rpc_ledger(&ledger)?;
            return Ok(ledger);
        }
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    let mut text = String::new();
    file.take((super::MAX_RPC_LEDGER_FILE_BYTES + 1) as u64)
        .read_to_string(&mut text)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    if text.len() > super::MAX_RPC_LEDGER_FILE_BYTES {
        return Err(format!(
            "Web RPC ledger is larger than {} KiB: {}",
            super::MAX_RPC_LEDGER_FILE_BYTES / 1024,
            path.display()
        ));
    }
    let mut ledger: RpcLedger = serde_json::from_str(&text)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    if ledger.endpoint_id != endpoint_id {
        let ledger = RpcLedger::for_endpoint(endpoint_id);
        persist_rpc_ledger(&ledger)?;
        return Ok(ledger);
    }
    if ledger.version != super::RPC_LEDGER_VERSION {
        return Err(format!(
            "unsupported Web RPC ledger version {} in {}",
            ledger.version,
            path.display()
        ));
    }
    let mut request_ids = HashSet::new();
    for entry in &ledger.entries {
        if entry.request_id.is_empty()
            || entry.request_id.len() > 256
            || entry.fingerprint.len() != 64
            || !entry
                .fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(format!(
                "invalid Web RPC ledger entry in {}",
                path.display()
            ));
        }
        if !request_ids.insert(entry.request_id.as_str()) {
            return Err(format!(
                "duplicate Web RPC request id in {}: {}",
                path.display(),
                entry.request_id
            ));
        }
    }
    if ledger.entries.len() > super::RPC_CACHE_CAPACITY {
        let overflow = ledger.entries.len() - super::RPC_CACHE_CAPACITY;
        ledger.entries.drain(..overflow);
        persist_rpc_ledger(&ledger)?;
    }
    Ok(ledger)
}

pub(super) fn persist_rpc_ledger(ledger: &RpcLedger) -> Result<(), String> {
    if ledger.version != super::RPC_LEDGER_VERSION || ledger.endpoint_id.is_empty() {
        return Err("refusing to persist an invalid Web RPC ledger".to_string());
    }
    atomic_write_private_json(&rpc_ledger_path(), ledger, "Web RPC ledger")
}

pub(super) fn remove_rpc_ledger() -> Result<(), String> {
    remove_private_file(&rpc_ledger_path())
}

pub(super) fn atomic_write_private_json<T: Serialize>(
    path: &Path,
    value: &T,
    label: &str,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid {label} path: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let data =
        serde_json::to_vec_pretty(value).map_err(|error| format!("serialize {label}: {error}"))?;
    // Private temp file + fsync + atomic replace + failure cleanup are provided
    // by the shared platform filesystem helper.
    crate::platform::filesystem::atomic_write_private(path, &data)
        .map_err(|error| format!("write {}: {error}", path.display()))
}

pub(super) fn remove_private_file(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            if let Some(parent) = path.parent() {
                // Best-effort durability for the directory-entry deletion; on
                // platforms where a directory cannot be opened as a file the
                // sync is skipped.
                if let Ok(dir) = std::fs::File::open(parent) {
                    dir.sync_all()
                        .map_err(|error| format!("sync {}: {error}", parent.display()))?;
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove {}: {error}", path.display())),
    }
}

/// 环境变量覆盖的统一入口：trim 后为空或不通过校验一律返回 None（回落存储
/// 值/内置默认），不报错——env 只是覆盖手段，宁可降级也不能因非法覆盖把
/// Web 访问卡死。纯函数便于无环境变量的测试。
fn normalized_env_url(
    raw: Option<String>,
    env_name: &str,
    validate: impl Fn(&str) -> Result<(), String>,
) -> Option<String> {
    let value = raw?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Err(error) = validate(trimmed) {
        // 运维显式设置的覆盖被丢弃时必须留痕:静默回落会让操作者以为在连
        // 自己的 relay,实际已在存储值/内置默认上工作。
        eprintln!("[web-access] ignoring invalid {env_name} override, falling back: {error}");
        return None;
    }
    Some(trimmed.to_string())
}

pub(super) fn remote_public_base_url() -> String {
    if let Some(url) = normalized_env_url(
        std::env::var("PINVOU_REMOTE_PUBLIC_URL").ok(),
        "PINVOU_REMOTE_PUBLIC_URL",
        |url| validate_http_base_url(url, "PINVOU_REMOTE_PUBLIC_URL is invalid"),
    ) {
        return url;
    }
    if let Some(settings) = load_relay_settings() {
        return settings.public_base_url;
    }
    super::DEFAULT_PUBLIC_BASE_URL.to_string()
}

pub(super) fn remote_relay_ws_url() -> String {
    // 与另外两个 env 覆盖同规则(trim 后为空/非法一律回落,不报错):非法
    // 覆盖降级到存储值/内置默认,而不是把 enable 卡死在 validate_config。
    if let Some(url) = normalized_env_url(
        std::env::var("PINVOU_REMOTE_RELAY_WS_URL").ok(),
        "PINVOU_REMOTE_RELAY_WS_URL",
        |url| validate_ws_url(url, "PINVOU_REMOTE_RELAY_WS_URL is invalid"),
    ) {
        return url;
    }
    if let Some(settings) = load_relay_settings() {
        return settings.relay_url;
    }
    super::DEFAULT_RELAY_WS_URL.to_string()
}

/// Development/manual-test override for the active connection only. Unlike
/// `PINVOU_REMOTE_RELAY_WS_URL`, this value is never written to the persistent
/// Web access config, so testing a local relay cannot strand the normal link.
/// 非法（含空白、非 ws(s)、超长、trim 后为空）的覆盖被忽略并回落存储值。
pub(super) fn apply_runtime_relay_override(mut config: WebAccessConfig) -> WebAccessConfig {
    if let Some(relay_url) = normalized_env_url(
        std::env::var("PINVOU_REMOTE_RUNTIME_RELAY_WS_URL").ok(),
        "PINVOU_REMOTE_RUNTIME_RELAY_WS_URL",
        |url| validate_ws_url(url, "PINVOU_REMOTE_RUNTIME_RELAY_WS_URL is invalid"),
    ) {
        config.relay_url = relay_url;
    }
    config
}

pub(super) fn public_url(config: &WebAccessConfig) -> String {
    let mut url = format!(
        "{}/#endpoint={}&token={}",
        remote_public_base_url().trim_end_matches('/'),
        percent_encode(&config.endpoint_id),
        percent_encode(&config.access_token),
    );
    if config.relay_url != super::DEFAULT_RELAY_WS_URL {
        url.push_str("&relay=");
        url.push_str(&percent_encode(&config.relay_url));
    }
    url
}

pub(super) fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Fresh opaque epoch token for the stream journal. Used by `StreamState` in the
/// parent module, so it stays `pub(super)`.
pub(super) fn new_stream_epoch() -> String {
    format!("epoch_{}", crate::features::remote_control::short_token(24))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_url_overrides_are_trimmed_and_fail_safe() {
        // 覆盖只在通过与设置文件相同校验时生效;非法/空值一律回落(None),
        // 不上抛——env 覆盖宁可降级,不能卡死 Web 访问。
        let http = |url: &str| validate_http_base_url(url, "invalid");
        assert_eq!(
            normalized_env_url(
                Some("  https://relay.example.test/pinvou3/remote \n".into()),
                "PINVOU_REMOTE_TEST_URL",
                http
            ),
            Some("https://relay.example.test/pinvou3/remote".into())
        );
        assert_eq!(
            normalized_env_url(
                Some("ftp://not-http".into()),
                "PINVOU_REMOTE_TEST_URL",
                http
            ),
            None
        );
        assert_eq!(
            normalized_env_url(
                Some("https://has space".into()),
                "PINVOU_REMOTE_TEST_URL",
                http
            ),
            None
        );
        assert_eq!(
            normalized_env_url(
                Some(format!("https://{}", "a".repeat(2_049))),
                "PINVOU_REMOTE_TEST_URL",
                http
            ),
            None,
            "the length cap matches the relay settings validation"
        );
        assert_eq!(
            normalized_env_url(Some("   ".into()), "PINVOU_REMOTE_TEST_URL", http),
            None
        );
        assert_eq!(
            normalized_env_url(None, "PINVOU_REMOTE_TEST_URL", http),
            None
        );

        let ws = |url: &str| validate_ws_url(url, "invalid");
        assert_eq!(
            normalized_env_url(
                Some(" ws://127.0.0.1:8787/ws ".into()),
                "PINVOU_REMOTE_TEST_URL",
                ws
            ),
            Some("ws://127.0.0.1:8787/ws".into())
        );
        assert_eq!(
            normalized_env_url(Some("http://not-ws".into()), "PINVOU_REMOTE_TEST_URL", ws),
            None
        );
        assert_eq!(
            normalized_env_url(Some("".into()), "PINVOU_REMOTE_TEST_URL", ws),
            None
        );
    }

    /// Round-6 复审:三个 env 覆盖读取点必须全部经由 `normalized_env_url`
    /// (trim + 校验 + 非法回落),直接回退到裸 `std::env::var` 会让非法
    /// 覆盖静默生效。helper 级测试(上方)看不见接线,按本模块惯例用
    /// source-contract 钉住三个调用点的函数体。
    #[test]
    fn env_override_readers_route_through_the_normalized_helper() {
        let source = include_str!("persistence.rs");
        for (fn_name, env_name) in [
            (
                "fn remote_public_base_url() -> String",
                "PINVOU_REMOTE_PUBLIC_URL",
            ),
            (
                "fn remote_relay_ws_url() -> String",
                "PINVOU_REMOTE_RELAY_WS_URL",
            ),
            (
                "fn apply_runtime_relay_override(mut config: WebAccessConfig)",
                "PINVOU_REMOTE_RUNTIME_RELAY_WS_URL",
            ),
        ] {
            let start = source
                .find(fn_name)
                .unwrap_or_else(|| panic!("{fn_name} must exist"));
            let body = &source[start..];
            let body = &body[..body
                .find("\npub(super) fn ")
                .or_else(|| body.find("\nfn "))
                .unwrap_or_else(|| panic!("{fn_name} body must be terminated by the next fn"))];
            assert!(
                body.contains("normalized_env_url("),
                "{fn_name} must route its env override through normalized_env_url"
            );
            assert!(body.contains(env_name), "{fn_name} must read {env_name}");
        }
    }
}
