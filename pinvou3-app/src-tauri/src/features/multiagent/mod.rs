//! 多智能体（会话内主动委派）的领域模块，见 ADR-0006。
//!
//! 多智能体 = 普通会话能力 + 主动委派 + 专家可视化 + 只读/执行权限：
//! - 委派实例（子智能体）的身份、任务摘要与状态由**底座自己的落盘记录**承载
//!   （worker ledger + subagent transcripts），App 零新增持久化——读取投影见
//!   [`transcripts`]；
//! - 专家名册装配见 [`roster`]；工作区 git 初始化（并行子任务 spawn 的前置）
//!   见 [`platform`]；
//! - Workflow 专属运行台账（run.json 状态机、attempt tracker、进程租约、审批
//!   落盘）已随"每图必停/唯一协议"的旧设计整体退役。`workflow` 工具保持
//!   主线原状（底座 subagents_enabled 连带注册，对所有会话可用）：不禁用、
//!   也不在委派提醒里教学或推荐——底座把 read_only 子任务钳成四个本地文件
//!   工具、结构化阶段默认不传递上游结果两处已知限制记录于 ADR-0006。
//!
//! 存量 `~/.pinvou3/agent-runs/<id>/` 目录属于旧形态的遗留数据：读取路径保留
//! 到 transcripts 的兼容分支，删除级联见 [`delete`]。

pub(crate) mod platform;
pub mod roster;
pub mod transcripts;

use crate::platform::paths;

/// 旧形态运行 id（`wf-` 前缀）的形状校验：只用于遗留数据的读取与删除级联，
/// 防路径穿越。
#[must_use]
pub fn is_valid_run_id(run_id: &str) -> bool {
    crate::features::sessions::is_workflow_session_id(run_id)
        && !run_id.is_empty()
        && run_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 删除旧形态运行的遗留目录（工作区 + 产物）。目录不存在视为已删。
pub fn delete(run_id: &str) -> Result<(), String> {
    if !is_valid_run_id(run_id) {
        return Err(format!("非法 run_id: {run_id}"));
    }
    let dir = paths::agent_run_dir(run_id);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("remove run dir: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 旧运行目录的存储根与老 Python 调度器的 `workflows/` 完全分开
    /// （ADR-0004 的边界在遗留数据上仍然成立）。
    #[test]
    fn run_storage_never_overlaps_the_legacy_scheduler_root() {
        let root = paths::agent_runs_root();
        assert!(root.ends_with("agent-runs"));
        assert!(!root.to_string_lossy().contains("workflows"));
    }

    #[test]
    fn run_id_validation_rejects_traversal_and_foreign_ids() {
        assert!(is_valid_run_id("wf-abc-123"));
        assert!(!is_valid_run_id("wf-"));
        assert!(!is_valid_run_id("wf-../etc"));
        assert!(!is_valid_run_id("chat-abc"));
        assert!(!is_valid_run_id(""));
    }
}
