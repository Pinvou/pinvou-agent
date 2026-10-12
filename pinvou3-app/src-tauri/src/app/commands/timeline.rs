// ───────────────────────────────────────────────────────────────────────────
// Session timing 诊断读取接口(2026-07 新增)
//
// timing_events.jsonl 此前已有耗时数据,但没有读取接口；token usage 则只发给前端
// 内存态进度条、不落盘。这两个命令把 sidecar 接通为内部诊断基础
// (模型耗时/失败轮次/上下文消耗),不是顶级历史产品入口。
// ───────────────────────────────────────────────────────────────────────────

/// 读取 session 的全部 timeline 事件(user_start / assistant_done 对 + usage),
/// 按 timestamp 升序。空 session / 无 timing 文件返回空数组(诊断面板按空态渲染)。
///
/// 读面登记(ADR-0024):本命令只做字符集校验、**不做 id 类别拒绝**——辅助会话
/// (`aux-<parent_id>`)的读取确实发生(辅助面板的水合/发送按 id 重读,桌面
/// `chat:done` 亦做权威时间线重读;两条路径都不经排除 aux 的侧栏列表),但并非
/// 渲染承载性(水合路径前端 try/catch 退空、`chat:done` 重读失败保留本地投影,
/// 且辅助面板不投影 timelineEvents);规范要点是此读面不得长出 id 类别拒绝。
#[tauri::command]
pub async fn get_session_timeline(
    session_id: String,
) -> Result<Vec<crate::features::assistant::timing::TimelineEvent>, String> {
    // 防路径穿越:timing reader 内部走 sessions_root().join(session_id),
    // 必须先校验 session_id 字符集(只允许 [A-Za-z0-9_-]),否则可构造 ../ 越界。
    crate::features::sessions::validate_session_id(&session_id).map_err(|e| format!("{e:#}"))?;
    tokio::task::spawn_blocking(move || {
        crate::features::assistant::timing::read_timeline(&session_id)
    })
    .await
    .map_err(|error| format!("读取 session timeline 任务失败: {error}"))?
    .map_err(|error| format!("读取 session timeline 失败: {error}"))
}

#[cfg(test)]
// The fixture holds the process-wide ENV_LOCK (std Mutex) across the await of
// the command under test to keep PINVOU3_HOME stable; cargo test runs test
// threads in parallel, and the lock is only contended by other env-writing
// tests, so it cannot deadlock.
#[allow(clippy::await_holding_lock)]
mod tests {
    use crate::features::assistant::timing::{finish_turn, start_turn};

    /// ADR-0024 read-surface register, command-layer pin (round-2 m3): the
    /// registered surface is THIS command fn, not only the timing reader
    /// beneath it — a derived `aux-<parent>` id must pass the command's
    /// charset validation and read back without any id-class rejection. The
    /// store/timing leg is separately pinned in
    /// `features/assistant/timing.rs` tests.
    #[tokio::test]
    async fn get_session_timeline_reads_aux_ids() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-timeline-cmd-aux-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let sid = "aux-l5cz0m8xq2k1b";
        start_turn(sid);
        finish_turn(sid, "Completed", None);

        let timeline = super::get_session_timeline(sid.to_string())
            .await
            .expect("command-layer aux timeline read");
        assert_eq!(timeline.len(), 2, "aux timing events must stay readable");
        assert_eq!(timeline[0].event, "user_start");
        assert_eq!(timeline[1].event, "assistant_done");

        let _ = std::fs::remove_dir_all(tmp);
    }
}
