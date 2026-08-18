//! 子智能体对话记录的读取。
//!
//! 底座把每个子智能体的完整对话落在工作区
//! `.codewhale/state/subagent-transcripts/<sha256(agent_id)>.jsonl`：首行是
//! `{"kind":"subagent_transcript_header","agent_id":...}`，其后每行
//! `{"kind":"message","index":N,"message":{...}}`。列表枚举时读取固定大小的
//! 表头识别身份，详情则按 agent_id 摘要直接定位；只为旧记录保留表头扫描回退。
//!
//! 这里只做**只读呈现**：Codex 式右侧面板按 agent 点开看它干了什么。消息体
//! 原样交给前端（role + content blocks），渲染取舍留在界面层。

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SubagentTranscriptSummary {
    pub agent_id: String,
    /// 模型调用 `agent` 时可选的稳定名称。未显式起名时底座会回填 agent_id，
    /// 前端据此忽略占位值并继续走任务标题/类型兜底。
    pub session_name: Option<String>,
    pub role: Option<String>,
    /// 底座 canonical Fleet 类型（worker/scout/planner/reviewer/builder/verifier/custom）。
    /// 普通对话通常不传 role/profile，界面靠它避免全部显示成“通用执行者”。
    pub agent_type: Option<String>,
    /// 直接父代理 ID；None 表示由主对话直接派出。用于把多级代理恢复成树，
    /// 避免 UI 把所有后代伪装成同级成员。
    pub parent_run_id: Option<String>,
    /// 底座记录的派生深度（主对话直派为 1）；无 ledger 的遗留记录为 None。
    pub spawn_depth: Option<u32>,
    pub status: Option<String>,
    pub error: Option<String>,
    pub done: bool,
    pub failed: bool,
    /// 子智能体按入口约定以 `[BLOCKED]` 开头收尾：正常返回但明说没干成。
    /// 底座只认"自然停机+有总结=成功"，界面若把这种返回画成绿色完成，
    /// 用户就会把"三个调研全部受阻"读成"调研全部完成"（真机事故）。
    pub blocked: bool,
    /// 任务一句话目标（ledger spec.objective）：同角色派多个子智能体时靠它
    /// 区分，不用逼用户读 agent_xxxxxxxx。
    pub objective: Option<String>,
    /// ledger 登记时间（毫秒）：清单按派出顺序排序；无 ledger 的遗留行为 None。
    pub created_at_ms: Option<u64>,
    /// transcript 是否已落盘。false = 排队/刚启动：面板要显示"排队中"，
    /// 点开详情要解释"还没有记录"，不能装作空任务。
    pub has_transcript: bool,
}

/// transcript 的增量读取结果。`next_offset` 与 `revision` 必须成对回传；
/// 文件被截断、替换或游标附近内容变化时，后端从头重读并标记 `reset`，前端
/// 据此替换旧消息。正常追加只返回新行，避免面板每 1.5 秒整文件读取、解析和
/// 跨 IPC 传输。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SubagentTranscriptChunk {
    pub messages: Vec<serde_json::Value>,
    pub next_offset: u64,
    pub revision: String,
    pub reset: bool,
}

fn transcripts_dir(workspace: &Path) -> PathBuf {
    workspace
        .join(".codewhale")
        .join("state")
        .join("subagent-transcripts")
}

fn transcript_path(workspace: &Path, agent_id: &str) -> PathBuf {
    let digest = Sha256::digest(agent_id.as_bytes());
    transcripts_dir(workspace).join(format!(
        "{}.jsonl",
        crate::platform::encoding::hex_lower(&digest)
    ))
}

fn header_agent_id(first_line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(first_line).ok()?;
    if value.get("kind").and_then(|k| k.as_str()) != Some("subagent_transcript_header") {
        return None;
    }
    value
        .get("agent_id")
        .and_then(|id| id.as_str())
        .map(str::to_string)
}

/// 入口提示要求子任务"无法完成时最终回复以 `[BLOCKED]` 开头"。
pub const BLOCKED_MARKER: &str = "[BLOCKED]";

/// 只读文件首行认表头身份。清单每 2s 轮询一次，整读正文会随记录长度
/// 线性变贵（复核 P2）；表头行是固定的一小行。
fn read_header_agent_id(path: &Path) -> Option<String> {
    use std::io::BufRead as _;
    let file = std::fs::File::open(path).ok()?;
    let mut first = String::new();
    std::io::BufReader::new(file).read_line(&mut first).ok()?;
    header_agent_id(&first)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TranscriptStamp {
    len: u64,
    modified: Option<SystemTime>,
}

const BLOCKED_CACHE_LIMIT: usize = 1024;
static BLOCKED_CACHE: OnceLock<Mutex<HashMap<PathBuf, (TranscriptStamp, bool)>>> = OnceLock::new();

/// 整读正文判受阻。成功终态不会再追加内容，因此按文件长度与修改时间缓存；
/// 列表轮询期间只付一次正文解析成本。
fn transcript_is_blocked(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    let stamp = TranscriptStamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    };
    let cache = BLOCKED_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let guard = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((cached_stamp, blocked)) = guard.get(path) {
            if *cached_stamp == stamp {
                return *blocked;
            }
        }
    }
    let Ok(body) = std::fs::read_to_string(path) else {
        return false;
    };
    let blocked =
        last_assistant_reply_is_blocked(body.lines().skip(1).filter(|l| !l.trim().is_empty()));
    let mut guard = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.len() >= BLOCKED_CACHE_LIMIT && !guard.contains_key(path) {
        guard.clear();
    }
    guard.insert(path.to_path_buf(), (stamp, blocked));
    blocked
}

/// 最后一条带正文的 assistant 消息是否以受阻标记开头。
///
/// 逐行独立解析（坏行跳过），只看 text block 拼接后的开头——这是一个
/// 展示层启发式约定，不参与任何执行决策。
fn last_assistant_reply_is_blocked<'a>(lines: impl Iterator<Item = &'a str>) -> bool {
    let mut last_text: Option<String> = None;
    for line in lines {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("kind").and_then(|k| k.as_str()) != Some("message") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        if message.get("role").and_then(|r| r.as_str()) != Some("assistant") {
            continue;
        }
        let Some(blocks) = message.get("content").and_then(|c| c.as_array()) else {
            continue;
        };
        let text: String = blocks
            .iter()
            .filter(|block| block.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
            .collect();
        if !text.trim().is_empty() {
            last_text = Some(text);
        }
    }
    last_text.is_some_and(|text| text.trim_start().starts_with(BLOCKED_MARKER))
}

/// 非终态 worker 的投影裁决：只有「引擎在（纪元 Some）且该记录在本纪元内
/// 有过活动」才算仍在跑，否则如实投影为 interrupted。
///
/// 单看"父会话引擎是否存在"不够：底座重启加载只把**内存**状态翻成
/// Interrupted、落盘记录仍是 running（`subagent/mod.rs` 的 load 路径不回写）。
/// 用户重启后在父会话再发一条消息、引擎重建，上一进程的僵尸 worker 就会
/// 重新显示"工作中"并被永久轮询。比较用 `max(created, updated)`：本纪元
/// 新派的 worker created ≥ 纪元；被 agents/followup 复活的旧 worker
/// updated 会被顶上去，不误杀。
fn projected_worker_status(
    status: deepseek_tui::tools::subagent::AgentWorkerStatus,
    created_at_ms: u64,
    updated_at_ms: u64,
    engine_epoch_ms: Option<u64>,
) -> deepseek_tui::tools::subagent::AgentWorkerStatus {
    if status.is_terminal() {
        return status;
    }
    let alive_in_epoch =
        engine_epoch_ms.is_some_and(|epoch| created_at_ms.max(updated_at_ms) >= epoch);
    if alive_in_epoch {
        status
    } else {
        deepseek_tui::tools::subagent::AgentWorkerStatus::Interrupted
    }
}

/// 枚举工作区里的全部子智能体记录。目录/ledger 缺失返回 Ok(空表)——运行还
/// 没派发过子任务；ledger 损坏或权限错误如实上抛（复核 P2：吞成空表会把
/// 故障伪装成"没有子智能体"，前端的"读取失败重试中"永远收不到）。
///
/// `engine_epoch_ms`：父会话引擎的纪元时间戳，None = 引擎没起。非终态
/// worker 的存活判定见 [`projected_worker_status`]。
pub fn list(
    workspace: &Path,
    engine_epoch_ms: Option<u64>,
) -> Result<Vec<SubagentTranscriptSummary>, String> {
    // 附表：agent_id -> transcript 文件路径（只读表头行认身份，不整读正文）。
    // NotFound = 还没派发过，正常空表；其余 I/O 错误（权限/非目录）如实
    // 上抛，不得伪装成"没有子智能体"（复核边缘）。
    let mut transcripts: HashMap<String, PathBuf> = HashMap::new();
    match std::fs::read_dir(transcripts_dir(workspace)) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("读取 transcript 目录失败: {err}")),
        Ok(entries) => {
            for e in entries.flatten() {
                let path = e.path();
                if path.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                    continue;
                }
                let Some(agent_id) = read_header_agent_id(&path) else {
                    continue;
                };
                transcripts.insert(agent_id, path);
            }
        }
    }
    // 主表 = worker ledger：底座先登记（Starting/Queued）、之后才建
    // transcript 文件。若以 transcript 为主表，排队/刚启动的子智能体在
    // 清单里整段不可见（复核 P1）。
    let mut out: Vec<SubagentTranscriptSummary> = Vec::new();
    let records = deepseek_tui::tools::subagent::read_persisted_agent_worker_records(workspace)
        .map_err(|err| format!("读取 worker ledger 失败: {err:#}"))?;
    for record in records {
        let agent_id = record.spec.worker_id.clone();
        let transcript = transcripts.remove(&agent_id);
        let status = projected_worker_status(
            record.status,
            record.created_at_ms,
            record.updated_at_ms,
            engine_epoch_ms,
        );
        let done = status.is_terminal();
        let failed = done && status != deepseek_tui::tools::subagent::AgentWorkerStatus::Completed;
        // 只有"成功完成"需要甄别受阻，此时才整读一次正文；失败/进行中的
        // 展示语义已经准确，清单轮询不为它们付整读成本。
        let blocked = done && !failed && transcript.as_deref().is_some_and(transcript_is_blocked);
        let objective = record.spec.objective.trim();
        let session_name = record.spec.session_name.clone();
        let agent_type = record.spec.agent_type.as_str().to_string();
        // 新记录在 spec 与 record 顶层都带父链；旧版只写过顶层，兼容读取。
        let parent_run_id = record
            .spec
            .parent_run_id
            .clone()
            .or_else(|| record.parent_run_id.clone());
        let spawn_depth = record.spec.spawn_depth;
        out.push(SubagentTranscriptSummary {
            agent_id,
            session_name,
            role: record.spec.role.clone(),
            agent_type: Some(agent_type),
            parent_run_id,
            spawn_depth: Some(spawn_depth),
            status: Some(
                deepseek_tui::tools::subagent::agent_worker_status_name(status).to_string(),
            ),
            error: record.error.clone(),
            done,
            failed,
            blocked,
            objective: (!objective.is_empty()).then(|| objective.to_string()),
            created_at_ms: Some(record.created_at_ms),
            has_transcript: transcript.is_some(),
        });
    }
    // 附表剩余：有 transcript、没 ledger（老运行或 ledger 条数上限裁剪）。
    // 语义与旧版一致：身份来自表头，状态未知，不冒充成功。
    for (agent_id, _path) in transcripts {
        out.push(SubagentTranscriptSummary {
            agent_id,
            session_name: None,
            role: None,
            agent_type: None,
            parent_run_id: None,
            spawn_depth: None,
            status: None,
            error: None,
            done: false,
            failed: false,
            blocked: false,
            objective: None,
            created_at_ms: None,
            has_transcript: true,
        });
    }
    // 按派出顺序排；遗留行（无登记时间）排最后。
    out.sort_by(|a, b| {
        let ka = a.created_at_ms.unwrap_or(u64::MAX);
        let kb = b.created_at_ms.unwrap_or(u64::MAX);
        ka.cmp(&kb).then_with(|| a.agent_id.cmp(&b.agent_id))
    });
    Ok(out)
}

fn locate_transcript_path(workspace: &Path, agent_id: &str) -> Result<PathBuf, String> {
    let direct = transcript_path(workspace, agent_id);
    if direct.is_file() {
        if read_header_agent_id(&direct).as_deref() == Some(agent_id) {
            return Ok(direct);
        }
        return Err(format!(
            "子智能体记录表头与文件名不一致: {}",
            direct.display()
        ));
    }

    let entries = match std::fs::read_dir(transcripts_dir(workspace)) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("该运行还没有子智能体记录: {agent_id}"));
        }
        Err(err) => return Err(format!("读取 transcript 目录失败: {err}")),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|x| x.to_str()) != Some("jsonl") {
            continue;
        }
        // 兼容旧记录：非标准文件名仍按表头定位。单个损坏文件不妨碍寻找
        // 其它 agent；标准摘要路径的读取错误已在上面明确上报。
        if read_header_agent_id(&path).as_deref() == Some(agent_id) {
            return Ok(path);
        }
    }
    Err(format!("找不到子智能体的对话记录: {agent_id}"))
}

const CURSOR_REVISION_WINDOW: u64 = 128;

/// 游标校验只读游标前固定窗口，而不是重新哈希全部历史。创建时间用于识别
/// 原子替换，尾窗用于识别原地截断/改写；追加时二者都稳定。
fn cursor_revision(
    file: &mut std::fs::File,
    offset: u64,
    created: Option<SystemTime>,
) -> Result<String, String> {
    use std::io::{Read as _, Seek as _, SeekFrom};

    let start = offset.saturating_sub(CURSOR_REVISION_WINDOW);
    let mut tail = vec![0_u8; (offset - start) as usize];
    file.seek(SeekFrom::Start(start))
        .map_err(|err| format!("定位子智能体记录游标失败: {err}"))?;
    file.read_exact(&mut tail)
        .map_err(|err| format!("校验子智能体记录游标失败: {err}"))?;

    let mut digest = Sha256::new();
    digest.update(b"pinvou3-subagent-transcript-cursor-v1");
    digest.update(offset.to_le_bytes());
    match created.and_then(|time| time.duration_since(UNIX_EPOCH).ok()) {
        Some(duration) => {
            digest.update([1]);
            digest.update(duration.as_nanos().to_le_bytes());
        }
        None => digest.update([0]),
    }
    digest.update(&tail);
    Ok(crate::platform::encoding::hex_lower(&digest.finalize()))
}

fn validated_header_end(file: &mut std::fs::File, agent_id: &str) -> Result<u64, String> {
    use std::io::{BufRead as _, Seek as _, SeekFrom};

    file.seek(SeekFrom::Start(0))
        .map_err(|err| format!("定位子智能体记录表头失败: {err}"))?;
    let mut first = String::new();
    let read = std::io::BufReader::new(&mut *file)
        .read_line(&mut first)
        .map_err(|err| format!("读取子智能体记录表头失败: {err}"))?;
    if read == 0 {
        return Err("子智能体记录为空".to_string());
    }
    if header_agent_id(&first).as_deref() != Some(agent_id) {
        return Err("子智能体记录表头身份不匹配".to_string());
    }
    Ok(read as u64)
}

fn read_messages_from(
    file: &mut std::fs::File,
    offset: u64,
) -> Result<(Vec<serde_json::Value>, u64), String> {
    use std::io::{BufRead as _, Seek as _, SeekFrom};

    file.seek(SeekFrom::Start(offset))
        .map_err(|err| format!("定位子智能体记录正文失败: {err}"))?;
    let mut reader = std::io::BufReader::new(&mut *file);
    let mut messages = Vec::new();
    let mut next_offset = offset;
    loop {
        let mut line = Vec::new();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|err| format!("读取子智能体记录正文失败: {err}"))?;
        if read == 0 {
            break;
        }
        let terminated = line.last() == Some(&b'\n');
        let parsed = serde_json::from_slice::<serde_json::Value>(&line);
        // 写入中的半行留到下一轮；带换行的坏行则跳过，不能卡死游标。
        if !terminated && parsed.is_err() {
            break;
        }
        next_offset = next_offset.saturating_add(read as u64);
        let Ok(mut value) = parsed else {
            continue;
        };
        if value.get("kind").and_then(|kind| kind.as_str()) != Some("message") {
            continue;
        }
        if let Some(message) = value.get_mut("message") {
            messages.push(serde_json::Value::take(message));
        }
    }
    Ok((messages, next_offset))
}

/// 按字节游标增量读取某个子智能体的消息。正常记录按 agent_id 摘要直接
/// 定位；旧版非标准文件名才枚举表头。游标无效时返回从头恢复的 `reset`
/// chunk，找不到该 agent 时返回 Err。
pub fn read_chunk(
    workspace: &Path,
    agent_id: &str,
    offset: Option<u64>,
    revision: Option<&str>,
) -> Result<SubagentTranscriptChunk, String> {
    let path = locate_transcript_path(workspace, agent_id)?;
    let mut file = std::fs::File::open(&path)
        .map_err(|err| format!("读取子智能体记录 {} 失败: {err}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|err| format!("读取子智能体记录元数据失败: {err}"))?;
    let created = metadata.created().ok();
    let header_end = validated_header_end(&mut file, agent_id)?;
    let requested = offset.unwrap_or(0);

    let cursor_valid = if requested == 0 {
        false
    } else if requested < header_end || requested > metadata.len() {
        false
    } else if let Some(expected) = revision {
        cursor_revision(&mut file, requested, created).is_ok_and(|actual| actual == expected)
    } else {
        false
    };
    let reset = requested != 0 && !cursor_valid;
    let start = if cursor_valid { requested } else { header_end };
    let (messages, next_offset) = read_messages_from(&mut file, start)?;
    let revision = cursor_revision(&mut file, next_offset, created)?;
    Ok(SubagentTranscriptChunk {
        messages,
        next_offset,
        revision,
        reset,
    })
}

#[cfg(test)]
mod tests {
    use super::projected_worker_status;
    use deepseek_tui::tools::subagent::AgentWorkerStatus;
    use std::sync::atomic::{AtomicU64, Ordering};

    static FIXTURE_SEQ: AtomicU64 = AtomicU64::new(0);

    /// 重启后父会话重建引擎：上一进程遗留的 running 记录（活动时间早于
    /// 本纪元）必须继续判 interrupted，不得因"引擎存在"而复活成工作中。
    #[test]
    fn stale_running_workers_stay_interrupted_after_engine_respawn() {
        let epoch = 1_000_000;
        // 引擎没起：非终态一律中断
        assert_eq!(
            projected_worker_status(AgentWorkerStatus::Running, 500, 900, None),
            AgentWorkerStatus::Interrupted
        );
        // 引擎重建，但记录的活动都在纪元之前：僵尸，保持中断
        assert_eq!(
            projected_worker_status(AgentWorkerStatus::Running, 500, 900, Some(epoch)),
            AgentWorkerStatus::Interrupted
        );
        // 本纪元新派的 worker：created ≥ 纪元 → 如实显示运行态
        assert_eq!(
            projected_worker_status(
                AgentWorkerStatus::Running,
                epoch + 5,
                epoch + 9,
                Some(epoch)
            ),
            AgentWorkerStatus::Running
        );
        // 旧 worker 被 agents/followup 复活：updated 顶上纪元 → 不误杀
        assert_eq!(
            projected_worker_status(AgentWorkerStatus::RunningTool, 500, epoch + 1, Some(epoch)),
            AgentWorkerStatus::RunningTool
        );
        // 终态永远原样：不受纪元影响
        assert_eq!(
            projected_worker_status(AgentWorkerStatus::Completed, 500, 900, Some(epoch)),
            AgentWorkerStatus::Completed
        );
        assert_eq!(
            projected_worker_status(AgentWorkerStatus::Failed, 500, 900, None),
            AgentWorkerStatus::Failed
        );
    }

    use super::*;

    fn fixture_workspace(files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-transcripts-{}-{:?}-{}",
            std::process::id(),
            std::thread::current().id(),
            FIXTURE_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let dir = transcripts_dir(&root);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, body) in files {
            std::fs::write(dir.join(name), body).unwrap();
        }
        root
    }

    const GOOD: &str = concat!(
        r#"{"kind":"subagent_transcript_header","schema_version":1,"agent_id":"agent_a"}"#,
        "\n",
        r#"{"kind":"message","index":0,"message":{"role":"user","content":[{"type":"text","text":"任务"}]}}"#,
        "\n",
        r#"{"kind":"message","index":1,"message":{"role":"assistant","content":[{"type":"text","text":"结果"}]}}"#,
        "\n",
    );

    fn write_worker_state(workspace: &Path, status: &str, error: Option<&str>) {
        let state = serde_json::json!({
            "schema_version": 1,
            "agents": [],
            "workers": [{
                "spec": {
                    "worker_id": "agent_a",
                    "run_id": "workflow-run",
                    "session_name": "build-report",
                    "objective": "执行任务",
                    "role": "builder",
                    "agent_type": "general",
                    "model": "test-model",
                    "workspace": workspace,
                    "context_mode": "isolated",
                    "fork_context": false,
                    "tool_profile": "inherited",
                    "max_steps": 4,
                    "spawn_depth": 1,
                    "max_spawn_depth": 1
                },
                "status": status,
                "created_at_ms": 1,
                "updated_at_ms": 2,
                "error": error
            }]
        });
        std::fs::write(
            workspace
                .join(".codewhale")
                .join("state")
                .join("subagents.v1.json"),
            serde_json::to_vec_pretty(&state).expect("serialize worker fixture"),
        )
        .expect("write worker fixture");
    }

    #[test]
    fn lists_agents_from_headers_and_counts_messages() {
        let ws = fixture_workspace(&[("aaaa.jsonl", GOOD)]);
        let listed = list(&ws, None).expect("list");
        assert_eq!(
            listed,
            vec![SubagentTranscriptSummary {
                agent_id: "agent_a".to_string(),
                session_name: None,
                role: None,
                agent_type: None,
                parent_run_id: None,
                spawn_depth: None,
                status: None,
                error: None,
                done: false,
                failed: false,
                blocked: false,
                objective: None,
                created_at_ms: None,
                has_transcript: true,
            }],
            "文件名是摘要，agent 身份必须来自表头；没有 worker ledger 时不能冒充成功"
        );
    }

    /// 受阻返回（以 [BLOCKED] 开头的最终回复）不能画成绿色成功。
    /// 真机事故：三个调研子任务全部"成功返回一段无法联网的说明"，
    /// 界面全绿，用户把"全部受阻"读成"全部完成"。
    #[test]
    fn blocked_final_reply_is_flagged_on_completed_workers() {
        let blocked_body = concat!(
            r#"{"kind":"subagent_transcript_header","agent_id":"agent_a"}"#,
            "\n",
            r#"{"kind":"message","index":0,"message":{"role":"user","content":[{"type":"text","text":"任务"}]}}"#,
            "\n",
            r#"{"kind":"message","index":1,"message":{"role":"assistant","content":[{"type":"text","text":"[BLOCKED] 无法访问目标站点：无联网工具"}]}}"#,
            "\n",
        );
        let ws = fixture_workspace(&[("aaaa.jsonl", blocked_body)]);
        write_worker_state(&ws, "completed", None);

        let listed = list(&ws, None).expect("list");
        assert_eq!(listed.len(), 1);
        assert!(listed[0].done && !listed[0].failed, "底座视其为成功完成");
        assert!(listed[0].blocked, "界面必须知道这是受阻，不是真完成");

        // 正常完成不误报；失败的也不用再标受阻。
        let ok = fixture_workspace(&[("aaaa.jsonl", GOOD)]);
        write_worker_state(&ok, "completed", None);
        assert!(!list(&ok, None).expect("list")[0].blocked);

        let failed = fixture_workspace(&[("aaaa.jsonl", blocked_body)]);
        write_worker_state(&failed, "failed", Some("boom"));
        assert!(
            !list(&failed, None).expect("list")[0].blocked,
            "失败态不需要受阻标记"
        );
    }

    #[test]
    fn restored_transcript_uses_persisted_worker_failure_instead_of_claiming_success() {
        let ws = fixture_workspace(&[("aaaa.jsonl", GOOD)]);
        write_worker_state(&ws, "failed", Some("boom"));

        let listed = list(&ws, None).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].role.as_deref(), Some("builder"));
        assert_eq!(listed[0].status.as_deref(), Some("failed"));
        assert_eq!(listed[0].error.as_deref(), Some("boom"));
        assert!(listed[0].done);
        assert!(listed[0].failed);
    }

    #[test]
    fn nonterminal_worker_is_interrupted_after_restart_but_not_while_run_is_live() {
        let ws = fixture_workspace(&[("aaaa.jsonl", GOOD)]);
        write_worker_state(&ws, "running", None);

        let restored = list(&ws, None).expect("list");
        assert_eq!(restored[0].status.as_deref(), Some("interrupted"));
        assert!(restored[0].done);
        assert!(restored[0].failed);

        let live = list(&ws, Some(0)).expect("list");
        assert_eq!(live[0].status.as_deref(), Some("running"));
        assert!(!live[0].done);
        assert!(!live[0].failed);
    }

    #[test]
    fn reads_messages_in_order_for_the_requested_agent() {
        let other = concat!(
            r#"{"kind":"subagent_transcript_header","agent_id":"agent_b"}"#,
            "\n",
            r#"{"kind":"message","index":0,"message":{"role":"user","content":[]}}"#,
            "\n",
        );
        let ws = fixture_workspace(&[("bbbb.jsonl", other)]);
        std::fs::write(transcript_path(&ws, "agent_a"), GOOD).unwrap();
        let messages = read_chunk(&ws, "agent_a", None, None)
            .map(|c| c.messages)
            .expect("read agent_a");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"][0]["text"], "结果");
    }

    #[test]
    fn incremental_read_returns_only_appended_messages() {
        use std::io::Write as _;

        let ws = fixture_workspace(&[]);
        let path = transcript_path(&ws, "agent_a");
        std::fs::write(&path, GOOD).unwrap();

        let first = read_chunk(&ws, "agent_a", None, None).expect("initial chunk");
        assert_eq!(first.messages.len(), 2);
        assert!(!first.reset);

        let unchanged = read_chunk(
            &ws,
            "agent_a",
            Some(first.next_offset),
            Some(&first.revision),
        )
        .expect("unchanged chunk");
        assert!(unchanged.messages.is_empty(), "未追加时不得重复传历史消息");
        assert_eq!(unchanged.next_offset, first.next_offset);
        assert_eq!(unchanged.revision, first.revision);

        writeln!(
            std::fs::OpenOptions::new().append(true).open(&path).unwrap(),
            r#"{{"kind":"message","index":2,"message":{{"role":"assistant","content":[{{"type":"text","text":"新增"}}]}}}}"#
        )
        .unwrap();
        let appended = read_chunk(
            &ws,
            "agent_a",
            Some(first.next_offset),
            Some(&first.revision),
        )
        .expect("appended chunk");
        assert!(!appended.reset);
        assert_eq!(appended.messages.len(), 1);
        assert_eq!(appended.messages[0]["content"][0]["text"], "新增");
        assert!(appended.next_offset > first.next_offset);
    }

    #[test]
    fn incremental_read_resets_after_truncate_or_rewrite() {
        let ws = fixture_workspace(&[]);
        let path = transcript_path(&ws, "agent_a");
        std::fs::write(&path, GOOD).unwrap();
        let first = read_chunk(&ws, "agent_a", None, None).expect("initial chunk");

        let replacement = concat!(
            r#"{"kind":"subagent_transcript_header","agent_id":"agent_a"}"#,
            "\n",
            r#"{"kind":"message","index":0,"message":{"role":"assistant","content":[{"type":"text","text":"替换"}]}}"#,
            "\n",
        );
        std::fs::write(&path, replacement).unwrap();
        let reset = read_chunk(
            &ws,
            "agent_a",
            Some(first.next_offset),
            Some(&first.revision),
        )
        .expect("reset chunk");
        assert!(reset.reset, "旧游标不得用于被截断/改写后的文件");
        assert_eq!(reset.messages.len(), 1);
        assert_eq!(reset.messages[0]["content"][0]["text"], "替换");
    }

    #[test]
    fn incremental_read_waits_for_a_complete_json_line() {
        use std::io::Write as _;

        let ws = fixture_workspace(&[]);
        let path = transcript_path(&ws, "agent_a");
        std::fs::write(&path, GOOD).unwrap();
        let first = read_chunk(&ws, "agent_a", None, None).expect("initial chunk");

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(
            br#"{"kind":"message","index":2,"message":{"role":"assistant","content":[{"type":"text","text":"pending"#,
        )
        .unwrap();
        file.flush().unwrap();
        let partial = read_chunk(
            &ws,
            "agent_a",
            Some(first.next_offset),
            Some(&first.revision),
        )
        .expect("partial chunk");
        assert!(partial.messages.is_empty());
        assert_eq!(partial.next_offset, first.next_offset, "半行不能推进游标");

        file.write_all(b"\"}]}}\n").unwrap();
        file.flush().unwrap();
        let completed = read_chunk(
            &ws,
            "agent_a",
            Some(first.next_offset),
            Some(&first.revision),
        )
        .expect("completed chunk");
        assert_eq!(completed.messages.len(), 1);
        assert_eq!(completed.messages[0]["content"][0]["text"], "pending");
    }

    #[test]
    fn blocked_cache_invalidates_when_terminal_transcript_changes() {
        let ws = fixture_workspace(&[("aaaa.jsonl", GOOD)]);
        write_worker_state(&ws, "completed", None);
        assert!(!list(&ws, None).expect("first list")[0].blocked);

        let blocked_body = concat!(
            r#"{"kind":"subagent_transcript_header","agent_id":"agent_a"}"#,
            "\n",
            r#"{"kind":"message","index":0,"message":{"role":"assistant","content":[{"type":"text","text":"[BLOCKED] 权限不足"}]}}"#,
            "\n",
        );
        std::fs::write(transcripts_dir(&ws).join("aaaa.jsonl"), blocked_body).unwrap();
        assert!(
            list(&ws, None).expect("list after rewrite")[0].blocked,
            "文件签名变化后不得复用旧缓存"
        );
    }

    #[test]
    fn missing_agent_and_missing_dir_are_reported_not_panicked() {
        let ws = fixture_workspace(&[("aaaa.jsonl", GOOD)]);
        assert!(read_chunk(&ws, "agent_zzz", None, None)
            .map(|c| c.messages)
            .is_err());

        let empty = std::env::temp_dir().join(format!(
            "pinvou3-transcripts-empty-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&empty);
        assert!(
            list(&empty, None)
                .expect("目录缺失是全新状态，不是错误")
                .is_empty(),
            "目录缺失=还没派发过子任务，不是错误"
        );
        assert!(read_chunk(&empty, "agent_a", None, None)
            .map(|c| c.messages)
            .is_err());
    }

    /// 底座先在 ledger 登记（Starting/Queued）、后建 transcript：清单必须
    /// 以 ledger 为主表，否则排队中的子智能体整段不可见（复核 P1）。
    #[test]
    fn queued_worker_without_transcript_is_listed_with_objective() {
        let ws = fixture_workspace(&[]);
        write_worker_state(&ws, "starting", None);

        let listed = list(&ws, Some(0)).expect("list");
        assert_eq!(listed.len(), 1, "没有 transcript 也必须出现在清单里");
        assert_eq!(listed[0].agent_id, "agent_a");
        assert_eq!(listed[0].session_name.as_deref(), Some("build-report"));
        assert_eq!(listed[0].agent_type.as_deref(), Some("worker"));
        assert_eq!(listed[0].parent_run_id, None);
        assert_eq!(listed[0].spawn_depth, Some(1));
        assert_eq!(listed[0].status.as_deref(), Some("starting"));
        assert_eq!(listed[0].objective.as_deref(), Some("执行任务"));
        assert!(!listed[0].has_transcript, "面板据此显示排队中而不是空任务");
        assert!(!listed[0].done);
        assert!(!listed[0].blocked);
    }

    #[test]
    fn failed_worker_without_transcript_keeps_ledger_error() {
        let ws = fixture_workspace(&[]);
        write_worker_state(&ws, "failed", Some("model route unavailable"));

        let listed = list(&ws, None).expect("list");
        assert_eq!(listed.len(), 1);
        assert!(listed[0].done && listed[0].failed);
        assert!(!listed[0].has_transcript);
        assert_eq!(
            listed[0].error.as_deref(),
            Some("model route unavailable"),
            "详情必须能直接展示 ledger 错误，不依赖不存在的 transcript"
        );
    }

    fn write_two_worker_state(workspace: &Path) {
        let spec = |id: &str, objective: &str, parent_run_id: Option<&str>, spawn_depth: u32| {
            serde_json::json!({
                "worker_id": id, "run_id": "workflow-run", "objective": objective,
                "parent_run_id": parent_run_id,
                "session_name": format!("lane-{id}"),
                "role": "builder", "agent_type": "general", "model": "test-model",
                "workspace": workspace, "context_mode": "isolated", "fork_context": false,
                "tool_profile": "inherited", "max_steps": 4, "spawn_depth": spawn_depth,
                "max_spawn_depth": 3
            })
        };
        let state = serde_json::json!({
            "schema_version": 1,
            "agents": [],
            "workers": [
                { "spec": spec("agent_late", "晚登记", Some("agent_early"), 2), "status": "running",
                  "created_at_ms": 50, "updated_at_ms": 60, "error": null },
                { "spec": spec("agent_early", "早登记", None, 1), "status": "running",
                  "created_at_ms": 10, "updated_at_ms": 20, "error": null }
            ]
        });
        std::fs::write(
            workspace
                .join(".codewhale")
                .join("state")
                .join("subagents.v1.json"),
            serde_json::to_vec_pretty(&state).expect("serialize two-worker fixture"),
        )
        .expect("write two-worker fixture");
    }

    /// 清单按 ledger 登记时间排（派出顺序）；无 ledger 的遗留行排最后。
    #[test]
    fn rows_sort_by_ledger_creation_time_with_legacy_rows_last() {
        // GOOD 表头是 agent_a，但 ledger 里没有它 → 遗留行。
        let ws = fixture_workspace(&[("aaaa.jsonl", GOOD)]);
        write_two_worker_state(&ws);
        let rows = list(&ws, Some(0)).expect("list");
        let ids: Vec<String> = rows.iter().map(|s| s.agent_id.clone()).collect();
        assert_eq!(ids, vec!["agent_early", "agent_late", "agent_a"]);
        assert_eq!(rows[0].parent_run_id, None);
        assert_eq!(rows[0].spawn_depth, Some(1));
        assert_eq!(rows[1].parent_run_id.as_deref(), Some("agent_early"));
        assert_eq!(rows[1].spawn_depth, Some(2));
    }

    /// 权限/非目录等 I/O 错误不得伪装成"还没有记录"（复核边缘）。
    /// 跨平台可测的形态：transcript 目录位置被一个普通文件占住（NotADirectory）。
    #[test]
    fn transcript_dir_io_errors_surface_instead_of_empty() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-transcripts-notdir-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let dir = transcripts_dir(&root);
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        std::fs::write(&dir, "not a directory").unwrap();
        assert!(
            list(&root, None).is_err(),
            "非 NotFound 的 I/O 错误必须上抛"
        );
        assert!(read_chunk(&root, "agent_a", None, None)
            .map(|c| c.messages)
            .unwrap_err()
            .contains("读取 transcript 目录失败"));
    }

    /// ledger 损坏必须如实报错——吞成空表会把故障伪装成"没有子智能体"，
    /// 前端的"读取失败重试中"永远不会出现（复核 P2）。
    #[test]
    fn corrupted_ledger_surfaces_an_error_instead_of_empty_list() {
        let ws = fixture_workspace(&[("aaaa.jsonl", GOOD)]);
        std::fs::write(
            ws.join(".codewhale")
                .join("state")
                .join("subagents.v1.json"),
            "{ not json",
        )
        .unwrap();
        assert!(list(&ws, None).is_err(), "损坏 ledger 不得伪装成空清单");
    }

    /// 单条损坏记录只丢那一行，别让整个面板打不开。
    #[test]
    fn broken_lines_are_skipped_not_fatal() {
        let broken = concat!(
            r#"{"kind":"subagent_transcript_header","agent_id":"agent_c"}"#,
            "\n",
            "{ not json\n",
            r#"{"kind":"message","index":1,"message":{"role":"assistant","content":[]}}"#,
            "\n",
        );
        let ws = fixture_workspace(&[("cccc.jsonl", broken)]);
        let messages = read_chunk(&ws, "agent_c", None, None)
            .map(|c| c.messages)
            .expect("read");
        assert_eq!(messages.len(), 1, "坏行跳过，好行保留");
    }
}
