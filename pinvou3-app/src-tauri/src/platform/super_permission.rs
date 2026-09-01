//! 超级权限开关：Linux 通过 pkexec 写入 `/etc/sudoers.d/pinvou3` 让当前用户跑 sudo 免密。
//! Windows 不支持该 Linux sudoers 机制，相关开关降级为不可用。
//!
//! 设计要点：
//! - **源真相是文件系统**：`/etc/sudoers.d/pinvou3` 存在 = 开；不存在 = 关。不在 settings.json 里冗余存。
//! - **写/删都走 pkexec**：用户拨开关 → 系统密码框 → root 写文件，零终端命令。
//! - **不动 careful hook**：CodeWhale shell.rs 的 5 条硬拦（`rm -rf /` 等）跨所有模式默认开启，
//!   sudo 也过不去。本开关只是把「sudo 卡密码」变成「sudo 直接跑」。
//!
//! pkexec 退出码约定：
//! - 0  成功
//! - 126 用户取消授权（dismissed dialog）
//! - 127 未授权或 pkexec 不可用
//!
//! 维护：卸载 .deb 时由 `prerm` 删 `/etc/sudoers.d/pinvou3`，避免遗留授权。

/// 进程级切换互斥锁：超级权限的完整切换序列（`is_enabled()` 读盘判定 →
/// pkexec 写/删 sudoers → engine 规则集重算广播）必须在持锁下整体执行。
///
/// 不串行化时,两个并发 toggle 会交错 pkexec 写盘与 sudo 状态快照,后完成的
/// `refresh_permission_rulesets` 可能基于过期的 sudo 快照重建并广播规则集,
/// 让运行中的 engine 带着错误的 sudo 面直到下次重建/重启。切换是低频用户
/// 操作,跨 pkexec 慢调用持锁可接受;锁只被切换序列持有,不阻塞其他命令。
pub(crate) static TOGGLE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn is_enabled() -> bool {
    crate::platform::os::super_permission_is_enabled()
}

/// 静态 system prompt 的 §7 占位段。**不含开关状态**。
///
/// 状态是动态的:静态 prompt 在 engine spawn 时只渲染一次,之后切开关无法热刷
/// (`refresh_all_instructions` 是 no-op —— 见 `engine_pool.rs`)。把状态写进
/// 静态段会导致"UI 显示已开启,但会话 prompt 还停在关闭态"的 desync(实测 case)。
/// 真正的开/关指令由 [`turn_reminder`] 每 turn 注入(`build_send_message_op`),
/// 始终实时。这里只留一句指引,把状态判断交给 per-turn reminder。
pub fn instruction_block() -> &'static str {
    "\n## 7. 超级权限(sudo)\n\n当前是否开启、以及对应该怎么做,见每轮对话顶部的 `<system-reminder>`(实时,以那里为准)。"
}

/// 每 turn 注入 `<system-reminder>` 的超级权限状态指令。
///
/// `is_enabled()` 每次实时读 `/etc/sudoers.d/pinvou3`,所以用户切换开关后**下一
/// turn 即生效**,不需要重启会话或 GUI —— 这是绕开 `refresh_all_instructions`
/// no-op 的关键(静态 prompt 刷不动,就每 turn 重新注入)。
///
/// **开启**:直接 sudo 一步到位,别先试裸命令(否则模型对 `/etc` 写只会裸 touch 然后放弃)。
/// **Off**: sudo is denied (the execpolicy deny rule rejects it immediately instead of blocking until timeout); guide the user to enable the toggle.
pub fn turn_reminder() -> &'static str {
    crate::platform::os::super_permission_turn_reminder()
}

pub fn enable() -> Result<(), String> {
    crate::platform::os::enable_super_permission()
}

pub fn disable() -> Result<(), String> {
    crate::platform::os::disable_super_permission()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    /// TOGGLE_LOCK 必须真正互斥：并发 toggle 的临界区（读盘判定 → 写 sudoers →
    /// 规则集广播）不允许重叠。直接对进程级锁本身做验证（不触 pkexec/磁盘）：
    /// 多个任务各自持锁进入临界区,进程内计数器峰值必须停在 1。
    #[tokio::test]
    async fn toggle_lock_serializes_concurrent_critical_sections() {
        let in_critical = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..4 {
            let in_critical = Arc::clone(&in_critical);
            let max_concurrent = Arc::clone(&max_concurrent);
            handles.push(tokio::spawn(async move {
                let _guard = TOGGLE_LOCK.lock().await;
                let observed = in_critical.fetch_add(1, Ordering::SeqCst) + 1;
                max_concurrent.fetch_max(observed, Ordering::SeqCst);
                // 主动让出：若锁失效,其他任务会趁隙闯入临界区,峰值升到 2+。
                tokio::task::yield_now().await;
                in_critical.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for handle in handles {
            handle.await.expect("toggle lock probe task panicked");
        }
        assert_eq!(in_critical.load(Ordering::SeqCst), 0);
        assert_eq!(
            max_concurrent.load(Ordering::SeqCst),
            1,
            "critical sections overlapped: TOGGLE_LOCK is not mutually exclusive"
        );
    }
}
