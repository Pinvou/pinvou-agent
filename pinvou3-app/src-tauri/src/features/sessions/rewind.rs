//! 代码模式「改动随对话回退」的对话侧：把 transcript 截断到第 N 个用户 turn 末尾。
//!
//! 设计文档 `docs/code-mode-改动随对话回退-设计.md` §4：
//! - 定位口径与 `code_checkpoints::count_user_turns`、底座 `EditLastTurn` 同一谓词
//!   [`deepseek_tui::is_user_turn_prompt`]：tool_result 与运行时内部信封同样以
//!   `role = "user"` 落盘，按 role 定位会切在工具往返中间。
//! - 被截段落写入 sidecar `_rewound_turns.json`（纯数据备份，UI 不暴露 redo）。
//! - 本方法是 [`super::transcript::looks_like_truncating_overwrite`] 守卫的显式
//!   放行路径：守卫只拦 `update_messages` / `compare_and_swap_messages` 两个通用
//!   入口，回退走这里的专用方法、不经过它们，既有守卫对其他调用方的保护不变。
//! - revision/CAS：截断后 `transcript_revision` 自然变化，持有旧 revision 的
//!   `compare_and_swap_messages` 会自然 CAS 失败——这是期望行为（远程控制编辑
//!   不得覆盖回退结果），无需额外处理。

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

/// `_rewound_turns.json` 的专用互斥锁：truncate/restore/purge 三方对整个
/// sidecar map 的 read-modify-write 都经它串行化（替代「调用方已持
/// scheduled_mutation」的错误假设——`SessionStore::delete` 路径并不持该锁，
/// 删除会话 X 与会话 Y 的回退存在真实的覆盖竞态，评审 M8）。叶锁：持它期间
/// 不再取其它锁，与 scheduled_mutation 无环。
static REWIND_BACKUP_LOCK: Mutex<()> = Mutex::new(());

/// sidecar 文件名（与 `_session_models.json` 等并列在 sessions 根下）。
const REWOUND_TURNS_FILE: &str = "_rewound_turns.json";
/// 每个会话保留的回退备份条数上限（与 checkpoint LRU 上限一致），超出裁掉最老，
/// 防止反复回退把 sidecar 写到无限大。
const MAX_REWIND_BACKUPS_PER_SESSION: usize = 20;

/// 底座 compaction 摘要标记（`CodeWhale/crates/tui/src/core/engine/context.rs` 的
/// `COMPACTION_SUMMARY_MARKER`，`pub(super)` 不可直接引用，app 侧用同一字面量匹配）。
/// 漂移后果：底座改文案后这里不再命中，回退后不再提示「对话含压缩摘要残留」——
/// 只是不提示，不影响回退正确性，可接受。
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
    /// 截断前 transcript 的 revision（恢复线索 + 调试锚点）。
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
    /// 被截掉的消息条数。
    pub removed_messages: usize,
    /// 截断后的新 transcript revision。
    pub new_revision: String,
    /// 持久化的 system_prompt 是否含 compaction 摘要残留（截断不触碰
    /// system_prompt；前端据此提示「回退到的位置之前发生过上下文压缩」）。
    pub had_compaction: bool,
}

fn rewound_turns_path() -> PathBuf {
    crate::platform::paths::sessions_root().join(REWOUND_TURNS_FILE)
}

fn load_rewound_turns_map() -> Result<HashMap<String, Vec<RewoundTurnsRecord>>> {
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
            let quarantine =
                path.with_extension(format!("corrupt-{}", Utc::now().format("%Y%m%d%H%M%S")));
            eprintln!(
                "[sessions] 回退备份损坏，隔离为 {} 后按空继续: {parse_error:#}",
                quarantine.display()
            );
            if let Err(error) = std::fs::rename(&path, &quarantine) {
                eprintln!("[sessions] 隔离损坏的回退备份失败: {error:#}");
            }
            Ok(HashMap::new())
        }
    }
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
    deepseek_tui::utils::write_atomic(&path, &payload)
        .with_context(|| format!("写入回退备份失败: {}", path.display()))
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

        // 与 code_checkpoints::count_user_turns 同一谓词，保证「第 N 轮」在快照、
        // 截断、UI 三处口径一致。
        let turn_prompt_indices: Vec<usize> = session
            .messages
            .iter()
            .enumerate()
            .filter(|(_, message)| deepseek_tui::is_user_turn_prompt(message))
            .map(|(index, _)| index)
            .collect();
        let total_turns = turn_prompt_indices.len() as u32;
        if keep_turns >= total_turns {
            bail!("会话当前只有 {total_turns} 轮，无法回退到第 {keep_turns} 轮末尾（无可截内容）");
        }
        // turn_prompt_indices[keep_turns] 即第 N+1 个用户 turn prompt 的下标。
        let cut = turn_prompt_indices[keep_turns as usize];
        let original_revision = transcript_revision(&session.messages)?;
        let removed_messages: Vec<Message> = session.messages.split_off(cut);
        // 截断后的新 revision（undo 复核条件 + 返回值；同一份数据只算一次）。
        let truncated_revision = transcript_revision(&session.messages)?;

        // 先备份后落盘：备份失败时磁盘上的 transcript 尚未被修改。sidecar 的
        // read-modify-write 经 REWIND_BACKUP_LOCK 与 purge/restore 串行（评审 M8）。
        let removed_count = removed_messages.len();
        let record = RewoundTurnsRecord {
            rewound_at: Utc::now().to_rfc3339(),
            original_revision,
            kept_turns: keep_turns,
            pre_restore_checkpoint_id,
            truncated_revision: truncated_revision.clone(),
            removed_messages,
        };
        {
            let _backup_guard = REWIND_BACKUP_LOCK.lock();
            let mut backups = load_rewound_turns_map()?;
            let records = backups.entry(id.to_string()).or_default();
            records.push(record);
            if records.len() > MAX_REWIND_BACKUPS_PER_SESSION {
                let overflow = records.len() - MAX_REWIND_BACKUPS_PER_SESSION;
                records.drain(..overflow);
            }
            persist_rewound_turns_map(&backups).context("回退备份写入失败，已中止截断")?;
        }

        session.metadata.message_count = session.messages.len();
        session.metadata.updated_at = Utc::now();
        let had_compaction = system_prompt_has_compaction_summary(session.system_prompt.as_ref());
        // 显式放行路径：直接持久化截断结果，不走 update_messages /
        // compare_and_swap_messages，因此不触发 looks_like_truncating_overwrite；
        // 该守卫对其它调用方的保护保持不变。
        self.persist_then_reconcile(&session, "turn rewind truncation")?;
        Ok(TruncateToTurnOutcome {
            rewound_turns: total_turns - keep_turns,
            removed_messages: removed_count,
            new_revision: truncated_revision,
            had_compaction,
        })
    }

    /// 最新一条回退备份记录（`undo_last_rewind` 的可反悔判定与恢复数据源）。
    pub fn latest_rewound_turns_record(&self, id: &str) -> Result<Option<RewoundTurnsRecord>> {
        validate_session_id(id)?;
        Ok(load_rewound_turns_map()?
            .get(id)
            .and_then(|records| records.last().cloned()))
    }

    /// 该会话的全部回退备份记录（陈旧 Turn 快照和解的数据源：作废步骤失败/
    /// 崩溃时，按各记录的 kept_turns + rewound_at 重放作废——备份先于截断
    /// 落盘，记录存在即截断已生效）。
    pub fn rewound_turns_records(&self, id: &str) -> Result<Vec<RewoundTurnsRecord>> {
        validate_session_id(id)?;
        Ok(load_rewound_turns_map()?
            .get(id)
            .cloned()
            .unwrap_or_default())
    }

    /// 回退反悔：把最新备份记录的被截消息追加回 transcript 尾部并持久化，随后从
    /// sidecar 删除该条记录，返回恢复的消息条数。
    ///
    /// 与 `truncate_to_user_turn` 同级的显式专用路径：不经过 update_messages /
    /// compare_and_swap_messages 的 `looks_like_truncating_overwrite` 守卫入口，
    /// 守卫对其它调用方的保护不变。revision/CAS 语义与截断一致：恢复后
    /// `transcript_revision` 自然变化，持旧 revision 的 CAS 自然失败（期望行为，
    /// 远程控制编辑不得覆盖反悔结果），无需额外处理。
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
        let _backup_guard = REWIND_BACKUP_LOCK.lock();
        let mut backups = load_rewound_turns_map()?;
        let record = backups
            .get(id)
            .and_then(|records| records.last().cloned())
            .with_context(|| format!("会话 '{id}' 没有可反悔的回退记录"))?;
        let mut session = self
            .manager
            .load_session_snapshot(id)
            .with_context(|| format!("load_session({id}) for rewind undo"))?;
        let current_turns = session
            .messages
            .iter()
            .filter(|message| deepseek_tui::is_user_turn_prompt(message))
            .count() as u32;
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
        // 记录删除失败只留孤儿数据；反悔条件②（turn 数已不等于 kept_turns）会
        // 自然挡住重复恢复，不算失败，如实记日志。
        if let Some(records) = backups.get_mut(id) {
            records.pop();
        }
        if let Err(error) = persist_rewound_turns_map(&backups) {
            eprintln!("[sessions] remove consumed rewind backup for {id} failed: {error:#}");
        }
        Ok(restored)
    }

    /// 删除/保留策略清理会话时同步清掉其回退备份（best-effort：失败只留孤儿数据，
    /// 不影响主流程）。read-modify-write 经 REWIND_BACKUP_LOCK 与 truncate/restore
    /// 串行（评审 M8：delete 路径不持 scheduled_mutation，专用锁替代此前的错误假设）。
    pub(crate) fn purge_rewound_turns_backups(ids: &[String]) {
        if ids.is_empty() || !rewound_turns_path().exists() {
            return;
        }
        let _backup_guard = REWIND_BACKUP_LOCK.lock();
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
