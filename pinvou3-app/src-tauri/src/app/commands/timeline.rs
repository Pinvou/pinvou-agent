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
/// (`aux-<parent_id>`)的读取确实发生(诊断入口按 id 直读,不经排除 aux 的侧栏
/// 列表),但并非渲染承载性(前端对该读取 try/catch 退空,辅助面板不投影
/// timelineEvents);规范要点是此读面不得长出 id 类别拒绝。
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
