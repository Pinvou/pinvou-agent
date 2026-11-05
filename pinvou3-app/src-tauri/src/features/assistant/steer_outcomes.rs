//! steer 终局分发：引擎事件（SteerCommitted/SteerDropped）的进程内扇出点。
//!
//! forwarder 在事件处调用 [`resolve_steer_outcome`]；消费方（跨会话消息
//! 投递的 in-flight 终局落定）由启动装配经 [`register_steer_resolution_hook`]
//! 注入。依赖方向保持 `消费方 → assistant`（消费者依赖本模块），assistant
//! 不反向依赖任何消费方 feature——架构守卫的 feature 环检查以此为界。
//! 进程内单钩子即可：当前唯一消费者是 features::messaging；若未来出现
//! 第二消费者，把 fn 指针升级为 Vec 广播（注册处已有去重防线）。

use std::sync::OnceLock;

type SteerResolutionHook = fn(&str, bool);

static STEER_RESOLUTION_HOOK: OnceLock<SteerResolutionHook> = OnceLock::new();

/// 注册 steer 终局消费者。仅接受首个注册者；重复注册视为装配错误
/// （fail-loud 而不是静默覆盖），启动装配只应调用一次。
pub fn register_steer_resolution_hook(hook: SteerResolutionHook) -> Result<(), ()> {
    STEER_RESOLUTION_HOOK.set(hook).map_err(|_| ())
}

/// 扇出 steer 终局：committed=true = 引擎已把它写入转录；false = 引擎
/// 丢弃（turn 终止/撤销/代际失效）。无注册者时静默（当前会话的 steer
/// 事件没有跨会话消费者）。
pub fn resolve_steer_outcome(steer_id: &str, committed: bool) {
    if let Some(hook) = STEER_RESOLUTION_HOOK.get() {
        hook(steer_id, committed);
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_steer_outcome;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 注册门：首个注册者生效，重复注册被拒（覆盖会静默换消费者——
    /// 装配错误必须可见）。
    #[test]
    fn hook_registration_accepts_first_and_rejects_rest() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn consumer(_steer_id: &str, _committed: bool) {
            CALLS.fetch_add(1, Ordering::SeqCst);
        }
        fn second(_steer_id: &str, _committed: bool) {
            unreachable!("the second hook must never be installed");
        }
        assert!(
            super::register_steer_resolution_hook(consumer).is_ok(),
            "first registration wins"
        );
        assert!(
            super::register_steer_resolution_hook(second).is_err(),
            "duplicate registration must be rejected"
        );
        resolve_steer_outcome("steer-1@0", true);
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        // 注册后同一进程内无法再单测"无钩子静默"分支（OnceLock 全局
        // 生效，先注册者常驻）——该分支由 get()==None 的短路保证。
        resolve_steer_outcome("steer-2@0", false);
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
    }
}
