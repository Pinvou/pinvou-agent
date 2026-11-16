//! 代码模式「改动随对话回退」的对话侧：把 transcript 截断到第 N 个用户 turn 末尾。
//!
//! 设计文档 `docs/code-mode-改动随对话回退-设计.md` §4：
//! - 定位口径与 `code_checkpoints::count_user_turns`、底座 `EditLastTurn` 同源，
//!   都读底座 era 列表 [`deepseek_tui::user_turn_prompt_indices`]（该列表的
//!   Editable 分类曾以单消息谓词 [`deepseek_tui::is_user_turn_prompt`] 表述，
//!   现以 era 列表为准）：tool_result 与运行时内部信封同样以
//!   `role = "user"` 落盘，按 role 定位会切在工具往返中间。
//! - 被截段落写入 sidecar `_rewound_turns.json`（纯数据备份，UI 不暴露 redo）。
//! - 本方法是 [`super::transcript::looks_like_truncating_overwrite`] 守卫的显式
//!   放行路径：守卫只拦 `update_messages` 这一通用入口，回退走这里的专用方法、
//!   不经过它，既有守卫对其他调用方的保护不变。
//! - revision：截断后 `transcript_revision` 自然变化，任何持有旧 revision 的
//!   通用写入路径都无法凭旧快照覆盖回退结果（远程控制编辑不得覆盖回退结果），
//!   无需额外处理。

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use deepseek_tui::models::Message;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::SessionStore;
use super::transcript::transcript_revision;
use super::validators::validate_session_id;

/// Dedicated mutex for `_rewound_turns.json`: the read-modify-writes of all
/// three parties (truncate/restore/purge) over the whole sidecar map are
/// serialized through it. Historical semantics correction: the delete path
/// now also holds `scheduled_mutation` (the delete-serialization fix in
/// store.rs); this lock is acyclic relative to it and points the same
/// direction, kept as belt-and-braces — it also covers the boot-time
/// `purge_all_scheduled_side_maps` caller that runs without
/// `scheduled_mutation`. Leaf lock: only the cross-process file lock below
/// is taken while it is held.
///
/// Hold discipline (r11 review M5): truncate and restore scope this guard
/// to their map read-modify-write ONLY — never across the transcript
/// persist. The persist runs the retention sweep, and a sweep that deletes
/// sessions reaches `purge_rewound_turns_backups`, which re-enters this
/// same non-reentrant mutex: a function-wide guard would deadlock the
/// sweeper with itself while holding `scheduled_mutation` (the whole
/// process's session writes hang). Purge additionally degrades on
/// in-process contention (`try_lock` + skip), so a skipped holder can
/// never wedge the delete/retention caller either.
static REWIND_BACKUP_LOCK: Mutex<()> = Mutex::new(());

/// Cross-process exclusion for `_rewound_turns.json` (round-41 review M2).
/// `REWIND_BACKUP_LOCK` is process-local, but the product CLI
/// (`pinvou code checkpoints rewind/undo`) is a second writer process on
/// the same whole-map sidecar: two interleaved load→mutate→persist runs
/// each commit their own snapshot and silently drop the peer's record —
/// and that record is the sole retention of the truncated turns
/// (「备份是被截对话唯一的留存，不能裸截」). The flock on the sibling
/// `.json.lock` file serializes the RMW across processes and is
/// kernel-released when a holder dies — the same discipline as the
/// settings transaction lock. Acquired only while `REWIND_BACKUP_LOCK` is
/// already held, so it stays a leaf lock. The RMW cost is pinned by the
/// byte budgets below: at a full 32 MiB map one round trip is a ~32 MiB
/// parse plus a serialize and an atomic write — tens-to-hundreds of
/// milliseconds under both locks, not the millisecond scale of the
/// pre-budget sidecar — so waiters queue under the bounded
/// [`REWIND_BACKUP_LOCK_WAIT`] rather than sampling a negligible hold.
///
/// The wait is BOUNDED ([`REWIND_BACKUP_LOCK_WAIT`], the same wedged-peer
/// hazard the purge path already degrades around with
/// [`try_lock_rewound_turns_file`]): truncate/restore cannot take that
/// skip twin — their sidecar write is mandatory (a truncate without its
/// backup must not run; a restore must re-read under the lock it commits
/// against) — so instead of the old indefinite block, a peer wedged while
/// holding the flock (e.g. a SIGSTOP'd CLI writer) now surfaces an
/// explicit timeout error after the bound, and the caller's transcript
/// stays untouched.
const REWIND_BACKUP_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

fn lock_rewound_turns_file() -> Result<std::fs::File> {
    let path = rewound_turns_path().with_extension("json.lock");
    let file =
        open_lock_file().with_context(|| format!("打开回退备份锁失败: {}", path.display()))?;
    // Bounded acquire (same shape as prefs::try_lock_settings_file_bounded):
    // normal contention resolves within the 25 ms poll cadence; the deadline
    // only fires for a wedged peer, where an explicit error beats hanging
    // every rewind/undo caller forever.
    let deadline = std::time::Instant::now() + REWIND_BACKUP_LOCK_WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    bail!(
                        "等待回退备份锁超时（{} 秒）：另一写方长期持有（可能已挂起），已中止本次操作: {}",
                        REWIND_BACKUP_LOCK_WAIT.as_secs(),
                        path.display()
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(error).with_context(|| format!("锁定回退备份失败: {}", path.display()));
            }
        }
    }
}

/// Path-compute → create_dir_all → open scaffold shared by the two lock
/// twins above and below (they differ only in bounded-wait vs `try_lock`):
/// the create_dir_all + open half now comes from the shared platform helper
/// [`crate::platform::filesystem::open_lock_file_with_dirs`]. The waiting
/// twin wraps any failure — including the `create_dir_all` step — in its
/// `打开回退备份锁失败` context; the try twin degrades to `None`.
fn open_lock_file() -> std::io::Result<std::fs::File> {
    crate::platform::filesystem::open_lock_file_with_dirs(
        &rewound_turns_path().with_extension("json.lock"),
    )
}

/// Non-blocking twin of [`lock_rewound_turns_file`] for callers that must
/// degrade instead of wait: `None` means the lock is held (or the lock file
/// could not be opened) and the caller skips with a note. Round-42 review:
/// the purge path promised exactly this best-effort semantics but used the
/// blocking acquire — a peer wedged while holding the flock would hang the
/// delete/retention caller forever instead of skipping.
fn try_lock_rewound_turns_file() -> Option<std::fs::File> {
    let file = open_lock_file().ok()?;
    match file.try_lock() {
        Ok(()) => Some(file),
        Err(std::fs::TryLockError::WouldBlock) => None,
        Err(std::fs::TryLockError::Error(_)) => None,
    }
}

/// sidecar 文件名（与 `_session_models.json` 等并列在 sessions 根下）。
const REWOUND_TURNS_FILE: &str = "_rewound_turns.json";
/// 每个会话保留的回退备份条数上限（与 checkpoint LRU 上限一致），超出裁掉最老，
/// 防止反复回退把 sidecar 写到无限大。`pub(super)` 供 tests.rs 的封顶行为测试
/// 直接引用（与 retention 上限的常量驱动测试同一模式），不是对外接口。
pub(super) const MAX_REWIND_BACKUPS_PER_SESSION: usize = 100;

/// 单条回退备份记录的序列化字节上限（r11 review M6）。条数上限不约束单条
/// 大小：`removed_messages` 是被截 transcript 尾部的整体，一条巨型尾巴就能把
/// 单条记录顶到数十 MiB。超限在动任何磁盘状态之前如实拒绝（与「备份写失败
/// 则中止截断」同一失败类，transcript 未落盘）：回退是会话级反悔操作，不是
/// 大批量数据搬运；更大的裁剪请走会话删除/分叉。
pub(crate) const MAX_REWIND_RECORD_BYTES: usize = 8 * 1024 * 1024;

/// `_rewound_turns.json` 全文件字节预算（r11 review M6）。条数上限 × 会话数
/// 仍不约束总量：100 条 × 多 MiB 尾 × 上千会话可以把 sidecar 顶到数百 MiB，
/// 而 retention 清理在存量达到/超过保留上限时每次都会整文件反序列化一遍
/// （purge 路径；未超限的保存会在 purge 入口按空删除集提前返回，不产生
/// 该成本——r12 review 把这句从「每次普通保存」收敛为真实触发条件），
/// 解析成本因此被乘进每条消息保存并在两个锁内串行。全局预算在唯一增长点
/// （truncate 的 RMW）按「最老记录先淘汰」收紧到预算内（刚写入的记录受
/// 保护，备份是被截对话唯一留存），把 sidecar 总量与其每次解析成本都钉死
/// 在常数上。淘汰只发生在 truncate 追加点；restore/purge 只缩小 map。
pub(crate) const MAX_REWIND_TURNS_MAP_BYTES: usize = 32 * 1024 * 1024;

/// 底座 compaction 摘要标记。底座现行标记是 `COMPACTION_SUMMARY_MARKER`
/// （首句 "Another language model started to solve this problem"，随摘要检查点
/// 以 `Role::User` 历史消息进入对话历史并单独存于 `compaction_summary_prompt`，
/// 稳定 system 前缀不携带检查点——底座 `core/engine.rs`
/// `commit_compaction_checkpoint` 与 `compaction.rs`
/// `compaction_checkpoint_message`），定义在底座
/// `CodeWhale/crates/tui/src/history_recognition.rs`；
/// 本字面量是底座 v0.9.6 前的旧格式标记（`LEGACY_COMPACTION_SUMMARY_MARKER`），
/// 现仅命中旧格式会话——现行格式的会话回退后不再触发压缩残留提示，只是不提示，
/// 不影响回退正确性，可接受。
const COMPACTION_SUMMARY_MARKER: &str = "Conversation Summary (Auto-Generated)";

/// 持久化的 system_prompt 是否含 compaction 摘要（截断不会触碰 system_prompt，
/// 压缩摘要残留因此随回退一起保留，前端据此提示）。
fn system_prompt_has_compaction_summary(system_prompt: Option<&String>) -> bool {
    system_prompt.is_some_and(|text| text.contains(COMPACTION_SUMMARY_MARKER))
}

/// 一次回退被截段落的备份记录（追加进 `_rewound_turns.json`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewoundTurnsRecord {
    /// 截断时间（RFC3339）。
    pub rewound_at: String,
    /// 截断前 transcript 的 revision（恢复线索 + 调试锚点）。业务逻辑虽不再读取，
    /// 但旧版 reader（无 serde default）要求该字段：缺失会让整个 sidecar map 反序列化
    /// 失败并被整体隔离（round-7 review P2——同一 `~/.pinvou3` 下新版 app + 旧版 CLI
    /// 并存即触发），因此必须继续随记录落盘，旧值经 read-modify-write 原样保留。
    pub original_revision: String,
    /// 截断后保留的用户 turn 数（即「回退到第 N 轮」的 N）。
    pub kept_turns: u32,
    /// 本次回退在代码侧强制的 PreRestore checkpoint id（undo 的代码恢复目标，
    /// 精确绑定到本次回退——不能用「最新 PreRestore」兜底：其它回退/反悔产生的
    /// 回滚点会被误配，把代码恢复到与本次回退无关的状态）。降级（仅回退对话、
    /// 代码未动）为 None，undo 时只恢复对话。
    #[serde(default)]
    pub pre_restore_checkpoint_id: Option<String>,
    /// 截断后 transcript 的 revision。undo 复核条件：当前 revision 必须精确等于
    /// 它——turn 数相等只是弱代理（回退后编辑第 N 轮的 assistant 回复不改 turn
    /// 数），revision 精确匹配才能保证被截段落接续的尾部未被改动。旧记录无此
    /// 字段（空串），跳过本校验、退回 turn 数弱代理。
    #[serde(default)]
    pub truncated_revision: String,
    /// 被截掉的消息：第 N+1 个用户 turn prompt 起，含其后的 assistant/tool_result。
    pub removed_messages: Vec<Message>,
}

/// `truncate_to_user_turn` 的结果摘要，供编排命令回填返回值。
#[derive(Debug, Clone)]
pub struct TruncateToTurnOutcome {
    /// 被截掉的用户 turn 数（= 截断前 turn 数 - N）。
    pub rewound_turns: u32,
    /// 持久化的 system_prompt 是否含 compaction 摘要残留（截断不触碰
    /// system_prompt；前端据此提示「回退到的位置之前发生过上下文压缩」）。
    pub had_compaction: bool,
}

fn rewound_turns_path() -> PathBuf {
    crate::platform::paths::sessions_root().join(REWOUND_TURNS_FILE)
}

/// Who is responsible for the cross-process flock when a corrupt sidecar
/// needs quarantining. Round-42 review: the unlocked READ paths
/// (`latest_rewound_turns_record`/`rewound_turns_records`) used to rename
/// without the lock, so a rename racing a lock-holding writer could move
/// the writer's freshly rebuilt map (the reader had parsed stale corrupt
/// bytes) into `corrupt-*` — hiding the newest undo records and resetting
/// the next RMW to empty.
#[derive(Clone, Copy)]
enum RewindLoadMode {
    /// The caller already holds `lock_rewound_turns_file()` (truncate /
    /// restore / purge): the rename cannot race a peer, quarantine directly.
    CallerHoldsLock,
    /// Unlocked read path: take the lock only for the quarantine, and
    /// re-read under it first — a writer that rebuilt the file since our
    /// read wins, and the reader returns the FRESH map instead of
    /// quarantining the peer's good bytes. A held lock means a writer is
    /// mid-RMW: leave the file alone entirely (the writer owns its fate).
    TryLockAndRecheck,
}

fn load_rewound_turns_map() -> Result<HashMap<String, Vec<RewoundTurnsRecord>>> {
    load_rewound_turns_map_with(RewindLoadMode::CallerHoldsLock)
}

/// The unlocked-read variant: see [`RewindLoadMode::TryLockAndRecheck`].
fn load_rewound_turns_map_for_read() -> Result<HashMap<String, Vec<RewoundTurnsRecord>>> {
    load_rewound_turns_map_with(RewindLoadMode::TryLockAndRecheck)
}

fn load_rewound_turns_map_with(
    mode: RewindLoadMode,
) -> Result<HashMap<String, Vec<RewoundTurnsRecord>>> {
    let path = rewound_turns_path();
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("读取回退备份失败: {}", path.display()));
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(map) => Ok(map),
        Err(parse_error) => {
            // 损坏的 sidecar 不得全局锁死 rewind/undo：改名隔离（保留现场供人工
            // 恢复）后按空 map 继续——代价是存量备份记录对该会话不再可见（undo
            // 入口消失），胜过整个功能报错卡死。
            match mode {
                RewindLoadMode::CallerHoldsLock => quarantine_and_continue(&path, parse_error),
                RewindLoadMode::TryLockAndRecheck => {
                    match try_lock_rewound_turns_file() {
                        Some(_lock) => {
                            // Re-read under the lock: our corrupt bytes may be
                            // stale — a peer writer may have already rebuilt the
                            // file between our read and this quarantine.
                            match std::fs::read(&path).map_err(anyhow::Error::new).and_then(
                                |fresh| {
                                    serde_json::from_slice::<
                                        HashMap<String, Vec<RewoundTurnsRecord>>,
                                    >(&fresh)
                                    .map_err(anyhow::Error::new)
                                },
                            ) {
                                Ok(fresh_map) => {
                                    eprintln!(
                                        "[sessions] 回退备份曾在读取后被并发写方重建，按新内容继续"
                                    );
                                    Ok(fresh_map)
                                }
                                Err(_still_corrupt) => quarantine_and_continue(&path, parse_error),
                            }
                        }
                        None => {
                            // Round-48 review: `try_lock_rewound_turns_file`
                            // answers None both for "a writer holds the lock"
                            // and for "the lock file itself cannot be opened
                            // here" (read-only sessions dir) — say so instead
                            // of asserting a writer exists. Either way the
                            // degrade is the same: leave the file alone,
                            // answer empty for this read.
                            eprintln!(
                                "[sessions] 回退备份损坏，但跨进程锁不可用（写方持有或本进程无法打开锁文件）——不动文件，按空继续（写方负责隔离或重建）: {parse_error:#}"
                            );
                            Ok(HashMap::new())
                        }
                    }
                }
            }
        }
    }
}

/// Shared tail of both load modes ([`RewindLoadMode::CallerHoldsLock`] and
/// the re-read-under-lock `TryLockAndRecheck` arm, which were byte-identical):
/// isolate the corrupt sidecar under `corrupt-*`（保留现场供人工恢复）and
/// answer an empty map. Both messages are part of the observable behavior
/// and are pinned verbatim here; see the mode enum for who holds the
/// cross-process flock during the rename.
fn quarantine_and_continue(
    path: &std::path::Path,
    parse_error: serde_json::Error,
) -> Result<HashMap<String, Vec<RewoundTurnsRecord>>> {
    // platform's nanosecond unique-name quarantine primitive
    // (`stem.corrupt-<timestamp>.<nanos>`): a same-second second corruption
    // would overwrite the first forensic copy (rename silently replaces the
    // existing target) — same shape as code_checkpoints' corruption
    // isolation.
    match crate::platform::filesystem::quarantine_corrupt_file(path) {
        Ok(quarantine) => {
            eprintln!(
                "[sessions] corrupt rewind backup quarantined as {}; continuing empty: {parse_error:#}",
                quarantine.display()
            );
        }
        Err(error) => {
            eprintln!("[sessions] failed to quarantine the corrupt rewind backup: {error:#}");
        }
    }
    Ok(HashMap::new())
}

fn persist_rewound_turns_map(map: &HashMap<String, Vec<RewoundTurnsRecord>>) -> Result<()> {
    let path = rewound_turns_path();
    if map.values().all(|records| records.is_empty()) {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("清理回退备份失败: {}", path.display()))
            }
        };
    }
    let payload = serde_json::to_vec_pretty(map).context("序列化回退备份失败")?;
    crate::platform::filesystem::atomic_write_private(&path, &payload)
        .with_context(|| format!("写入回退备份失败: {}", path.display()))
}

/// 一条记录按落盘口径（pretty JSON）的序列化长度：预算计量与实际写出同为
/// pretty 口径，淘汰扣减方向不会漂移。注意嵌套记录独立序列化时每行比它在
/// map 里少两层缩进，扣减因此略微低估单条占比——淘汰会略微多淘汰几条，
/// 最终 map 严格不超预算（偏安全方向）；「同一口径」指 pretty 与 pretty，
/// 不是逐字节相等（r14 review 澄清）。
fn rewound_record_serialized_len(record: &RewoundTurnsRecord) -> usize {
    serde_json::to_vec_pretty(record)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
}

/// 全局预算执行（r11 review M6，只在 truncate 追加点调用）：把 map 收紧到
/// `budget` 字节内。淘汰次序：各会话 vec 只追加（push 在尾部），队首即该
/// 会话最老记录，全局取 `rewound_at` 最小者（RFC3339 UTC，字典序 == 时间序）。
/// `protected_id` 的最后一条（刚写入的新备份）永不淘汰——备份是被截对话唯一
/// 留存，预算不得把本次回退的备份挤掉；受单条上限约束它必然 ≤
/// `MAX_REWIND_RECORD_BYTES` < 预算，循环因此必然终止。返回淘汰条数。
fn enforce_rewound_map_budget(
    map: &mut HashMap<String, Vec<RewoundTurnsRecord>>,
    budget: usize,
    protected_id: &str,
) -> usize {
    // 计量失败（serde 只在非有限浮点上失败，实践不可达）时大声跳过而不是
    // 静默按 0 处理——0 会让预算看似恒满足，本轮淘汰静默失效（r12 review）。
    let mut total: usize = match serde_json::to_vec_pretty(&*map) {
        Ok(bytes) => bytes.len(),
        Err(error) => {
            eprintln!("[sessions] rewind map budget skipped (serialize failed): {error:#}");
            return 0;
        }
    };
    let mut evicted = 0usize;
    while total > budget {
        let oldest = map
            .iter()
            .filter(|(id, records)| {
                !records.is_empty() && !(id.as_str() == protected_id && records.len() == 1)
            })
            .min_by(|a, b| a.1[0].rewound_at.cmp(&b.1[0].rewound_at).then(a.0.cmp(b.0)))
            .map(|(id, _)| id.clone());
        let Some(id) = oldest else {
            break;
        };
        let Some(records) = map.get_mut(&id) else {
            break;
        };
        let Some(record) = records.first() else {
            break;
        };
        total -= rewound_record_serialized_len(record).min(total);
        records.remove(0);
        evicted += 1;
        if records.is_empty() {
            map.remove(&id);
        }
    }
    evicted
}

/// 从某会话的记录队列尾部弹出「正是本次反悔消费掉的那条」（r11 review M5）：
/// restore 的两段式持锁把消费动作挪到了 transcript 持久化之后，跨进程的
/// CLI 写方可能在两段之间追加新记录——无匹配就整段不动（消费挂起为孤儿
/// 记录，文档化的最坏情形），不得误删对方的最新备份。匹配是复合的
/// （r12 review）：RFC3339 字符串相等在粗时钟/NTP 步进下可能碰撞，只比
/// `rewound_at` 会把跨进程写方同刻写入的另一条记录误当本次的消费掉；再比
/// `kept_turns` 与 `truncated_revision` 只收紧接受面（误配降级为孤儿，
/// 不会更糟），也不会漏配本次记录——restore 上半段已重新校验当前 turn 数
/// 与 revision 仍与待消费记录一致。
fn pop_matching_rewound_record(
    records: &mut Vec<RewoundTurnsRecord>,
    rewound_at: &str,
    kept_turns: u32,
    truncated_revision: &str,
) -> bool {
    match records.last() {
        Some(last)
            if last.rewound_at == rewound_at
                && last.kept_turns == kept_turns
                && last.truncated_revision == truncated_revision =>
        {
            records.pop();
            true
        }
        _ => false,
    }
}

impl SessionStore {
    /// 截断到第 `keep_turns` 个用户 turn 末尾：第 N+1 个用户 turn prompt 即截断点，
    /// 保留其前的全部消息（含第 N 轮的 assistant/tool_result）。N=0 = 全部截断
    /// （回退到第一轮之前）；N ≥ 当前 turn 数时如实报错（无可截内容，调用方不得
    /// 把「回退到当前状态」当成功）。
    ///
    /// 被截段落在落盘前先备份进 `_rewound_turns.json`；备份写失败则中止截断
    /// （备份是被截对话唯一的留存，不能裸截）。
    ///
    /// `pre_restore_checkpoint_id`：编排命令在恢复代码步骤拿到的本次回退专属
    /// 回滚点（降级回退传 None），随记录落盘，供 undo 精确配对。
    pub fn truncate_to_user_turn(
        &self,
        id: &str,
        keep_turns: u32,
        pre_restore_checkpoint_id: Option<String>,
    ) -> Result<TruncateToTurnOutcome> {
        let _mutation = self.scheduled_mutation.lock();
        if self.is_scheduled_session(id)? {
            bail!("Cannot rewind scheduled-run session '{id}'");
        }
        validate_session_id(id)?;
        let mut session = self
            .manager
            .load_session_snapshot(id)
            .with_context(|| format!("load_session({id}) for turn rewind"))?;

        // 与 code_checkpoints::count_user_turns 同一口径（其内部同为底座 era
        // 形 user_turn_prompt_indices），保证「第 N 轮」在快照、截断、UI 三处
        // 口径一致。era 口径比单消息 is_user_turn_prompt 多计入 post-provenance
        // 历史里粘贴的压缩摘要头 turn——底座文档明确 host turn 编号应统一读
        // 这一个列表（fork 分叉同一口径）。
        let turn_prompt_indices: Vec<usize> =
            deepseek_tui::user_turn_prompt_indices(&session.messages);
        let total_turns = turn_prompt_indices.len() as u32;
        if keep_turns >= total_turns {
            bail!("会话当前只有 {total_turns} 轮，无法回退到第 {keep_turns} 轮末尾（无可截内容）");
        }
        // turn_prompt_indices[keep_turns] 即第 N+1 个用户 turn prompt 的下标。
        let cut = turn_prompt_indices[keep_turns as usize];
        // Compatibility field (see RewoundTurnsRecord::original_revision):
        // recomputed per rewind and written for older readers even though
        // this build never reads it back.
        let original_revision = transcript_revision(&session.messages)?;
        let removed_messages: Vec<Message> = session.messages.split_off(cut);
        // New revision after truncation (undo re-check condition; the same data is counted only once).
        let truncated_revision = transcript_revision(&session.messages)?;

        // 先备份后落盘：备份失败时磁盘上的 transcript 尚未被修改。sidecar 的
        // read-modify-write 经 REWIND_BACKUP_LOCK 与 purge/restore 串行（评审 M8）。
        let record = RewoundTurnsRecord {
            rewound_at: Utc::now().to_rfc3339(),
            original_revision,
            kept_turns: keep_turns,
            pre_restore_checkpoint_id,
            truncated_revision,
            removed_messages,
        };
        // 单条字节上限（r11 review M6）：在动任何磁盘状态之前如实拒绝。此处
        // 快照仅在内存中被 split_off，transcript 尚未落盘，与「备份写失败则
        // 中止截断」同一失败类——编排命令对 truncate 报错的处理路径原样适用。
        let record_len = rewound_record_serialized_len(&record);
        if record_len == usize::MAX {
            // 计量失败（实践不可达）：如实报错，不拿 usize::MAX 折算出荒谬
            // 的 MiB 数字（r12 review）。
            bail!("回退备份序列化失败，已中止回退（transcript 未改动）");
        }
        if record_len > MAX_REWIND_RECORD_BYTES {
            bail!(
                "被截段落的回退备份序列化后约 {} MiB，超过 {} MiB 单条上限，已中止回退（transcript 未改动）；更大的裁剪请删除或分叉会话",
                record_len.div_ceil(1024 * 1024),
                MAX_REWIND_RECORD_BYTES / (1024 * 1024),
            );
        }
        {
            let _backup_guard = REWIND_BACKUP_LOCK.lock();
            let _cross_process = lock_rewound_turns_file()?;
            let mut backups = load_rewound_turns_map()?;
            let records = backups.entry(id.to_string()).or_default();
            records.push(record);
            if records.len() > MAX_REWIND_BACKUPS_PER_SESSION {
                let overflow = records.len() - MAX_REWIND_BACKUPS_PER_SESSION;
                records.drain(..overflow);
            }
            // 全局字节预算（r11 review M6）：执行在唯一增长点，刚写入的记录
            // 受保护，其余按最老先淘汰到预算内。
            enforce_rewound_map_budget(&mut backups, MAX_REWIND_TURNS_MAP_BYTES, id);
            persist_rewound_turns_map(&backups).context("回退备份写入失败，已中止截断")?;
        }

        session.metadata.message_count = session.messages.len();
        session.metadata.updated_at = Utc::now();
        let had_compaction = system_prompt_has_compaction_summary(session.system_prompt.as_ref());
        // 显式放行路径：直接持久化截断结果，不走 update_messages，
        // 因此不触发 looks_like_truncating_overwrite；
        // 该守卫对其它调用方的保护保持不变。
        self.persist_then_reconcile(&session, "turn rewind truncation")?;
        Ok(TruncateToTurnOutcome {
            rewound_turns: total_turns - keep_turns,
            had_compaction,
        })
    }

    /// 最新一条回退备份记录（`undo_last_rewind` 的可反悔判定与恢复数据源）。
    pub fn latest_rewound_turns_record(&self, id: &str) -> Result<Option<RewoundTurnsRecord>> {
        validate_session_id(id)?;
        Ok(load_rewound_turns_map_for_read()?
            .get(id)
            .and_then(|records| records.last().cloned()))
    }

    /// 该会话的全部回退备份记录（陈旧 Turn 快照和解的数据源：作废步骤失败/
    /// 崩溃时，按各记录的 kept_turns + rewound_at 重放作废——备份先于截断
    /// 落盘，记录存在即截断已生效）。
    pub fn rewound_turns_records(&self, id: &str) -> Result<Vec<RewoundTurnsRecord>> {
        validate_session_id(id)?;
        Ok(load_rewound_turns_map_for_read()?
            .get(id)
            .cloned()
            .unwrap_or_default())
    }

    /// 回退反悔：把最新备份记录的被截消息追加回 transcript 尾部并持久化，随后从
    /// sidecar 删除该条记录，返回恢复的消息条数。
    ///
    /// 与 `truncate_to_user_turn` 同级的显式专用路径：不经过 update_messages 的
    /// `looks_like_truncating_overwrite` 守卫入口，
    /// 守卫对其它调用方的保护不变。revision 语义与截断一致：恢复后
    /// `transcript_revision` 自然变化，任何凭旧 revision 的通用写入都无法覆盖
    /// 反悔结果（期望行为，远程控制编辑不得覆盖反悔结果），无需额外处理。
    ///
    /// 可反悔条件（当前 turn 数 == 记录的 kept_turns，且记录带 truncated_revision 时
    /// 当前 revision 精确匹配，即回退后未发过新轮次、尾部未被编辑）在 mutation 锁
    /// 内重新校验；不满足则如实报错、不动磁盘。调用方（编排命令）应在动代码
    /// （restore checkpoint）之前先用 `resolve_rewind_undo_state` 完成同样的预检
    /// （含 revision 与绑定快照核实），把「代码已反悔、对话未反悔」的窗口压到最小。
    pub fn restore_rewound_turns(&self, id: &str) -> Result<usize> {
        let _mutation = self.scheduled_mutation.lock();
        if self.is_scheduled_session(id)? {
            bail!("Cannot undo rewind for scheduled-run session '{id}'");
        }
        validate_session_id(id)?;
        // 第一段（r11 review M5）：只在 map 读取时持有两把锁，随后立即释放，
        // 与 truncate_to_user_turn 的 RMW 作用域同一形状。transcript 持久化
        // 会触发 retention 清理，而清理删除会话时会走到
        // purge_rewound_turns_backups——若把非重入的 REWIND_BACKUP_LOCK 持有
        // 跨过持久化，清理线程会与自身死锁（scheduled_mutation 被一起钉死，
        // 进程内全部会话写挂起）。
        let record = {
            let _backup_guard = REWIND_BACKUP_LOCK.lock();
            let _cross_process = lock_rewound_turns_file()?;
            load_rewound_turns_map()?
                .get(id)
                .and_then(|records| records.last().cloned())
                .with_context(|| format!("会话 '{id}' 没有可反悔的回退记录"))?
        };
        let mut session = self
            .manager
            .load_session_snapshot(id)
            .with_context(|| format!("load_session({id}) for rewind undo"))?;
        let current_turns = deepseek_tui::user_turn_prompt_indices(&session.messages).len() as u32;
        if current_turns != record.kept_turns {
            bail!(
                "回退后已产生新轮次（当前 {current_turns} 轮，回退时为 {} 轮），不可反悔",
                record.kept_turns
            );
        }
        // turn 数相等只是弱代理：回退后编辑第 N 轮的回复不改 turn 数，但被截段落
        // 接续的尾部已变。revision 精确匹配才放行；旧记录无此字段（空串）时退回
        // turn 数弱代理（功能发布前的开发期数据，不为之保留精确校验）。
        if !record.truncated_revision.is_empty() {
            let current_revision = transcript_revision(&session.messages)?;
            if current_revision != record.truncated_revision {
                bail!("回退后对话内容已被编辑，不可反悔");
            }
        }
        let restored = record.removed_messages.len();
        session.messages.extend(record.removed_messages);
        session.metadata.message_count = session.messages.len();
        session.metadata.updated_at = Utc::now();
        // 先持久化 transcript 再删备份记录：本步失败时记录仍在，可重试。
        self.persist_then_reconcile(&session, "rewind undo restore")?;
        // 第二段（r11 review M5）：重新取锁消费记录。map 必须重读而非复用
        // 第一段的副本——两段之间 retention 清理可能已移除本会话，跨进程 CLI
        // 写方也可能整文件重写（其快照里没有我们看到的记录）。消费按复合
        // 身份精确匹配队尾（rewound_at + kept_turns + truncated_revision，
        // 见 pop_matching_rewound_record）：不匹配就整段不动（消费挂起为
        // 孤儿记录，文档化的最坏情形），不得误删对方的最新备份。
        // 消费段的任何失败都只留孤儿数据；反悔条件②（turn 数已不等于
        // kept_turns）会自然挡住重复恢复，不算失败，如实记日志。
        // （r12 review M1：取锁/重读的 `?` 同样不得外抛——transcript 在上面
        // 一行已持久化成功，这里 Err 外抛会把「已恢复」谎报为「恢复失败
        // （可重试；记录未消费）」，而重试必然被反悔条件①挡成不可重试，
        // 与事实双重不符；与持久化失败同一处理：记日志、留孤儿，函数返回
        // 成功。）
        {
            let consume = || -> Result<()> {
                let _backup_guard = REWIND_BACKUP_LOCK.lock();
                let _cross_process = lock_rewound_turns_file()?;
                let mut backups = load_rewound_turns_map()?;
                if let Some(records) = backups.get_mut(id) {
                    // 只借身份字段（removed_messages 已在上方并入 transcript，
                    // 整体借 &record 会撞上部分移动）。
                    pop_matching_rewound_record(
                        records,
                        &record.rewound_at,
                        record.kept_turns,
                        &record.truncated_revision,
                    );
                }
                if let Err(error) = persist_rewound_turns_map(&backups) {
                    eprintln!(
                        "[sessions] remove consumed rewind backup for {id} failed: {error:#}"
                    );
                }
                Ok(())
            };
            if let Err(error) = consume() {
                eprintln!(
                    "[sessions] consume rewind backup for {id} skipped (transcript already restored; orphaned record is the documented worst case): {error:#}"
                );
            }
        }
        Ok(restored)
    }

    /// Purge a session's rewind backups when delete/retention removes the
    /// session (best-effort: a failure only leaves orphaned data and does not
    /// affect the main flow). Serialized through `REWIND_BACKUP_LOCK` like
    /// truncate/restore; the delete caller already holds
    /// `scheduled_mutation`, so this lock is the supplementary line of defense
    /// outside that window (the boot sweep).
    ///
    /// Both locks degrade instead of wait (r11 review M5): truncate/restore
    /// scope their guards to the map RMW, but nothing structurally prevents a
    /// future holder from widening again — and a blocking acquire here would
    /// deadlock the retention sweeper against a same-thread holder
    /// (non-reentrant mutex) while pinning `scheduled_mutation`. Skipping
    /// leaves orphaned records, which is this path's documented worst case
    /// (they are inert: restore re-validates the session and re-consumes them
    /// on the session's next delete).
    pub(crate) fn purge_rewound_turns_backups(ids: &[String]) {
        if ids.is_empty() || !rewound_turns_path().exists() {
            return;
        }
        let _backup_guard = match REWIND_BACKUP_LOCK.try_lock() {
            Some(guard) => guard,
            None => {
                eprintln!(
                    "[sessions] rewind backup lock held in-process, purge skipped (orphaned records are the documented worst case)"
                );
                return;
            }
        };
        // The purge only rewrites when something changes, but the map load
        // itself must not race a concurrent whole-map persist, so the file
        // lock spans the read here too. Purge stays best-effort: with the
        // lock unavailable it skips with a note instead of failing the
        // delete/retention caller (a skipped purge only leaves orphaned
        // records, its documented worst case).
        let _cross_process = match try_lock_rewound_turns_file() {
            Some(guard) => guard,
            None => {
                eprintln!(
                    "[sessions] rewind backup lock held by another writer, purge skipped (orphaned records are the documented worst case)"
                );
                return;
            }
        };
        let result = load_rewound_turns_map().and_then(|mut map| {
            let mut changed = false;
            for id in ids {
                changed |= map.remove(id).is_some();
            }
            if changed {
                persist_rewound_turns_map(&map)?;
            }
            Ok(())
        });
        if let Err(error) = result {
            eprintln!("[sessions] purge rewound-turns backups failed: {error:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 10s wait is the r3 disclosed bound (bounded acquire of the
    /// cross-process flock); the poll-loop behavior is exercised elsewhere,
    /// but nothing pinned the VALUE — pin the literal so a silent
    /// loosening/tightening turns red (r4 review).
    #[test]
    fn rewind_backup_lock_wait_stays_at_10s() {
        assert_eq!(REWIND_BACKUP_LOCK_WAIT, std::time::Duration::from_secs(10));
    }

    /// The backup cap's seed/asserts are cap-relative (they read the
    /// constant), so nothing held the VALUE after the 20→100 raise — pin the
    /// shipped literal (r6 review, same rationale as the lock pin above).
    #[test]
    fn rewind_backup_cap_stays_at_100() {
        assert_eq!(MAX_REWIND_BACKUPS_PER_SESSION, 100);
    }

    /// The two r11 bounds are enforced with the constants at the growth
    /// point; the behavioral tests below run tiny budgets, so nothing would
    /// catch a silent revert of the shipped VALUES — pin both literals.
    #[test]
    fn rewind_record_cap_stays_at_8mib() {
        assert_eq!(MAX_REWIND_RECORD_BYTES, 8 * 1024 * 1024);
    }

    #[test]
    fn rewind_map_budget_stays_at_32mib() {
        assert_eq!(MAX_REWIND_TURNS_MAP_BYTES, 32 * 1024 * 1024);
    }

    /// A padded record whose `removed_messages` carry `pad_bytes` of text.
    fn padded_rewound_record(rewound_at: &str, pad_bytes: usize) -> RewoundTurnsRecord {
        RewoundTurnsRecord {
            rewound_at: rewound_at.to_string(),
            original_revision: String::new(),
            kept_turns: 1,
            pre_restore_checkpoint_id: None,
            truncated_revision: String::new(),
            removed_messages: vec![Message {
                role: "user".into(),
                content: vec![deepseek_tui::models::ContentBlock::Text {
                    text: "x".repeat(pad_bytes),
                    cache_control: None,
                }],
            }],
        }
    }

    /// Budget enforcement (r11 review M6): with a budget that fits nothing
    /// but the protected record, every older record is evicted
    /// oldest-first and exactly the just-written backup survives; with a
    /// generous budget nothing is touched. The mid-budget case pins the
    /// eviction ORDER: oldest-first evicts the two old sessions and stops
    /// with sess-new's front+tail intact, while any newest-first or
    /// arbitrary order leaves a different survivor set.
    #[test]
    fn rewind_map_budget_evicts_oldest_and_protects_the_new_record() {
        let mut map = HashMap::new();
        map.insert(
            "sess-old".to_string(),
            vec![padded_rewound_record("2026-01-01T00:00:00+00:00", 400)],
        );
        map.insert(
            "sess-mid".to_string(),
            vec![padded_rewound_record("2026-06-01T00:00:00+00:00", 400)],
        );
        map.insert(
            "sess-new".to_string(),
            vec![
                padded_rewound_record("2026-12-01T00:00:00+00:00", 400),
                padded_rewound_record("2026-12-02T00:00:00+00:00", 400),
            ],
        );

        // Generous budget: the map stays untouched.
        let mut generous = map.clone();
        assert_eq!(
            enforce_rewound_map_budget(&mut generous, usize::MAX, "sess-new"),
            0
        );
        assert_eq!(
            serde_json::to_vec(&generous).expect("serialize"),
            serde_json::to_vec(&map).expect("serialize"),
            "a generous budget must not evict anything"
        );

        // Mid budget, derived not guessed: all four records serialize to
        // the same byte count (identical shape, same-length timestamps —
        // the probe below uses a same-length placeholder so its serialized
        // size matches exactly), so budget = map_total − 2×record is
        // exactly the post-two-evictions size — oldest-first evicts
        // sess-old then sess-mid and stops with sess-new's front AND
        // protected tail intact. Newest-first would evict sess-new's front
        // first and end with sess-old surviving instead: a different final
        // set.
        let record_bytes =
            rewound_record_serialized_len(&padded_rewound_record("0000-00-00T00:00:00+00:00", 400));
        // The budget counts PRETTY bytes (the persisted format), so the
        // measurement here must be pretty too.
        let map_total = serde_json::to_vec_pretty(&map).expect("serialize").len();
        let mut mid = map.clone();
        let evicted =
            enforce_rewound_map_budget(&mut mid, map_total - 2 * record_bytes, "sess-new");
        assert_eq!(evicted, 2);
        assert!(mid.contains_key("sess-new"), "protected session survives");
        assert_eq!(
            mid["sess-new"]
                .iter()
                .map(|r| r.rewound_at.as_str())
                .collect::<Vec<_>>(),
            ["2026-12-01T00:00:00+00:00", "2026-12-02T00:00:00+00:00"],
            "oldest-first keeps the new session whole"
        );
        assert_eq!(mid.len(), 1, "emptied sessions are removed from the map");

        // Budget below one record: everything evictable goes except the
        // protected just-written record (sess-new's tail). Note sess-new's
        // FRONT (2026-12-01) is evictable while a newer sibling exists —
        // only the tail is the protected backup — so 3 evictions run, and
        // the loop then stops with only the protected record left even
        // though it alone exceeds the budget (the protected record may not
        // be evicted; the per-record cap bounds its size).
        let evicted = enforce_rewound_map_budget(&mut map, 1, "sess-new");
        assert_eq!(evicted, 3);
        let remaining = map.get("sess-new").expect("protected session survives");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].rewound_at, "2026-12-02T00:00:00+00:00");
        assert_eq!(map.len(), 1, "emptied sessions are removed from the map");
    }

    /// The restore consumer must pop exactly the record it consumed
    /// (matched on the full record identity): a peer that appended a newer
    /// record between the two lock windows must keep its backup.
    #[test]
    fn restore_consumption_matches_the_consumed_record_only() {
        let mut records = vec![
            padded_rewound_record("2026-01-01T00:00:00+00:00", 8),
            padded_rewound_record("2026-02-01T00:00:00+00:00", 8),
        ];
        assert!(pop_matching_rewound_record(
            &mut records,
            "2026-02-01T00:00:00+00:00",
            padded_rewound_record("2026-02-01T00:00:00+00:00", 8).kept_turns,
            padded_rewound_record("2026-02-01T00:00:00+00:00", 8)
                .truncated_revision
                .as_str()
        ));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].rewound_at, "2026-01-01T00:00:00+00:00");
        // A mismatched timestamp (peer appended/rewrote meanwhile) must not
        // remove anything.
        assert!(!pop_matching_rewound_record(
            &mut records,
            "2026-09-09T00:00:00+00:00",
            1,
            "",
        ));
        assert_eq!(records.len(), 1);
        let mut empty: Vec<RewoundTurnsRecord> = Vec::new();
        assert!(!pop_matching_rewound_record(&mut empty, "x", 1, ""));
    }

    /// The consumption match is compound (r12 review): RFC3339 strings can
    /// collide across a coarse clock / NTP step, so a peer record written in
    /// the same instant but for a different truncation must NOT be consumed
    /// — it degrades to an orphan, never to eating the peer's only backup.
    #[test]
    fn consumption_requires_full_identity_not_just_the_timestamp() {
        let mut records = vec![padded_rewound_record("2026-03-01T00:00:00+00:00", 8)];
        records[0].kept_turns = 2;
        // Same rewound_at, different kept_turns: the peer's record must
        // survive.
        assert!(!pop_matching_rewound_record(
            &mut records,
            "2026-03-01T00:00:00+00:00",
            1,
            ""
        ));
        assert_eq!(records.len(), 1, "peer record survives a timestamp tie");
        // Same rewound_at and kept_turns but a different truncated_revision
        // (the tail was edited between the two truncations): still no
        // consumption.
        records[0].kept_turns = 1;
        records[0].truncated_revision = "rev-peer".to_string();
        assert!(!pop_matching_rewound_record(
            &mut records,
            "2026-03-01T00:00:00+00:00",
            1,
            "rev-ours"
        ));
        assert_eq!(records.len(), 1);
        // Full identity match consumes.
        assert!(pop_matching_rewound_record(
            &mut records,
            "2026-03-01T00:00:00+00:00",
            1,
            "rev-peer"
        ));
        assert!(records.is_empty());
    }

    /// Wiring pins (r12 review M2): the r11 M6 bounds live at production
    /// call sites that no behavioral test reaches end-to-end (the 8 MiB
    /// fixture a public-path eviction test would need outweighs the covered
    /// risk; the refusal's public path is covered in sessions/tests.rs).
    /// Mutations proved both call sites deletable with the whole suite
    /// green, so the pins below give them the same mutation kill power as
    /// the ACP wrap pins. The slices are scoped between the enclosing
    /// `pub fn` markers — never into this test module — so these assertion
    /// strings cannot self-match.
    #[test]
    fn m6_bounds_stay_wired_at_their_production_call_sites() {
        let source = include_str!("rewind.rs");
        let scoped = |start_marker: &str, end_marker: &str| {
            let start = source
                .find(start_marker)
                .unwrap_or_else(|| panic!("{start_marker} missing from rewind.rs"));
            let end = source[start..]
                .find(end_marker)
                .map(|offset| start + offset)
                .unwrap_or_else(|| panic!("{end_marker} missing from rewind.rs"));
            source[start..end].to_string()
        };
        let truncate_body = scoped(
            "pub fn truncate_to_user_turn",
            "pub fn latest_rewound_turns_record",
        );
        assert!(
            truncate_body.contains("if record_len > MAX_REWIND_RECORD_BYTES {"),
            "truncate must keep the per-record 8 MiB refusal wired (deleting it silently unbounds the sidecar's record size)"
        );
        assert!(
            truncate_body
                .contains("enforce_rewound_map_budget(&mut backups, MAX_REWIND_TURNS_MAP_BYTES"),
            "truncate must keep the 32 MiB map budget enforcement wired (deleting it silently unbounds the sidecar's total size)"
        );
        let restore_body = scoped(
            "pub fn restore_rewound_turns",
            "pub(crate) fn purge_rewound_turns_backups",
        );
        // Whitespace-squeezed containment (rustfmt-tolerant, same lesson as
        // the ACP wrap pins): the matcher must receive the full compound
        // identity — dropping any argument changes the squeezed text.
        let squeezed: String = restore_body
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            squeezed.contains(
                "pop_matching_rewound_record(records,&record.rewound_at,record.kept_turns,&record.truncated_revision"
            ),
            "restore must consume via the compound identity matcher"
        );
        // The phase-2 consume block must not propagate errors out of
        // restore: the transcript is durably committed one line above, so a
        // `?` there would misreport a successful restore as a retryable
        // failure (r12 review M1). Every remaining `?;` in the region must
        // sit inside the `consume` closure (deeper indent than the block
        // body).
        let phase_two = restore_body
            .split("let consume = || -> Result<()> {")
            .nth(1)
            .expect("restore keeps the consume closure");
        let closure_end = phase_two
            .find("if let Err(error) = consume()")
            .expect("consume result must be handled, not propagated");
        for line in phase_two[..closure_end].lines() {
            if line.trim_end().ends_with("?;") {
                assert!(
                    line.starts_with("                "),
                    "phase-2 error handling must stay inside the closure (no `?` may escape restore after the transcript commit): {line:?}"
                );
            }
        }
    }

    /// Purge degrades on in-process contention (r11 review M5): while the
    /// backup mutex is held the purge must SKIP (sidecar untouched) instead
    /// of blocking — a blocking acquire here deadlocks the retention
    /// sweeper against a same-thread holder, which is exactly the shape
    /// restore's function-wide guard used to produce. After release the
    /// same call must succeed.
    #[test]
    fn purge_skips_while_the_in_process_backup_lock_is_held() {
        let (_lock, _env) = crate::platform::test_support::locked_env(&["PINVOU3_HOME"]);
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-rewind-purge-skip-test-{}",
            crate::platform::paths::tests::unique_suffix()
        ));
        // SAFETY: locked_env holds the test ENV_LOCK; writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let path = rewound_turns_path();
        let mut seeded = HashMap::new();
        seeded.insert(
            "sess-x".to_string(),
            vec![padded_rewound_record("2026-01-01T00:00:00+00:00", 8)],
        );
        std::fs::create_dir_all(path.parent().expect("sessions root has a parent"))
            .expect("create sessions root");
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&seeded).expect("serialize seed"),
        )
        .expect("seed sidecar");

        let held = REWIND_BACKUP_LOCK.lock();
        SessionStore::purge_rewound_turns_backups(&["sess-x".to_string()]);
        assert!(
            path.exists(),
            "a contended purge must skip and leave the sidecar untouched"
        );
        let still_there: HashMap<String, Vec<RewoundTurnsRecord>> =
            serde_json::from_slice(&std::fs::read(&path).expect("read sidecar")).expect("parse");
        assert!(
            still_there.contains_key("sess-x"),
            "record survives the skipped purge"
        );
        drop(held);

        SessionStore::purge_rewound_turns_backups(&["sess-x".to_string()]);
        assert!(
            !path.exists(),
            "uncontended purge removes the record (all-empty sidecar is deleted)"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Round-41 review M2: the cross-process rewind lock is a real exclusive
    /// flock at the documented sibling path. `flock(2)` conflicts between two
    /// open file descriptions even within one process, so while the guard is
    /// held a second opener's `try_lock` must fail with `WouldBlock` —
    /// exactly what a second process's blocking acquire would meet — and
    /// dropping the guard must let the peer in.
    #[test]
    fn rewound_turns_lock_is_an_exclusive_cross_process_flock() {
        let (_lock, _env) = crate::platform::test_support::locked_env(&["PINVOU3_HOME"]);
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-rewind-lock-test-{}",
            crate::platform::paths::tests::unique_suffix()
        ));
        // SAFETY: locked_env holds the test ENV_LOCK; writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let guard = lock_rewound_turns_file().expect("first acquire");
        let lock_path = rewound_turns_path().with_extension("json.lock");
        assert!(
            lock_path.exists(),
            "the flock's sibling file is created: {}",
            lock_path.display()
        );

        let second = std::fs::OpenOptions::new()
            .write(true)
            .open(&lock_path)
            .expect("second opener");
        match second.try_lock() {
            // `WouldBlock` is exactly the flock conflict the held guard
            // must produce.
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(error)) => {
                panic!("unexpected lock error: {error}")
            }
            Ok(()) => panic!("the first guard must hold an exclusive flock"),
        }
        drop(guard);
        second.try_lock().expect("released guard lets the peer in");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
