//! 同意守卫：设置开关、会话授权、停止旗标、速率限制、动作预算、T3 确认。
//!
//! 引擎当前自动批准所有工具调用，因此同意门控必须内建于工具自身——本模块是
//! 唯一的事实来源。集成层（Tauri 命令）通过公开 API 注入用户决定：
//! `set_enabled` / `grant_session` / `revoke_session` / `stop_all` /
//! `mint_confirmation`。授权只活于内存，永不落盘。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// 输入类会话授权的空闲超时：10 分钟无输入动作即失效，需重新授权。
pub const GRANT_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// 输入类动作滑动窗口速率上限：60 次/分钟。
pub const INPUT_RATE_LIMIT_PER_MINUTE: u32 = 60;
/// 单次授权的输入类动作预算：用尽后强制重新授权（防失控循环）。
pub const INPUT_ACTION_BUDGET: u64 = 500;
/// T3 确认的有效期：前端「拒绝」只是本地关闭对话框（无 deny 命令），未答复的
/// pending 必须自行过期——过期后读取视为不存在，也不能再为它铸造批准令牌。
pub const CONFIRM_TTL: Duration = Duration::from_secs(5 * 60);

/// T3 后果性动作名单（大小写不敏感子串匹配；中英文）。
/// 命中即不执行，要求用户显式确认。
pub const T3_DENYLIST: &[&str] = &[
    "buy",
    "pay",
    "purchase",
    "send",
    "delete",
    "transfer",
    "submit",
    "购买",
    "支付",
    "付款",
    "发送",
    "删除",
    "转账",
    "提交",
];

/// 标签是否命中 T3 后果性名单。
pub fn matches_t3_denylist(label: &str) -> bool {
    let lower = label.to_lowercase();
    T3_DENYLIST.iter().any(|term| lower.contains(term))
}

/// 元素角色是否是密码/安全文本字段（T3 信号；对应 Operator 的 takeover 场景）。
pub fn is_secure_role(role: &str) -> bool {
    let lower = role.to_lowercase();
    lower.contains("password") || lower.contains("secure")
}

/// 守卫拒绝原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardRejection {
    /// 设置里 computer use 总开关关闭。
    Disabled,
    /// 停止旗标已置位（panic stop / stop_all）。
    Stopped,
    /// 输入类动作缺少有效会话授权（或已空闲过期/预算用尽）。
    GrantRequired,
    /// 超过 60 次/分钟速率上限。
    RateLimited,
    /// 会话动作预算用尽，强制重新授权。
    BudgetExhausted,
}

impl GuardRejection {
    /// 给模型看的错误文本（模型可据此 replan）。
    pub fn message(self) -> String {
        match self {
            Self::Disabled => {
                "computer use is disabled in settings. Ask the user to enable it before using this tool."
                    .to_string()
            }
            Self::Stopped => {
                "computer use was stopped by the user. Do not attempt further actions; ask the user how to proceed."
                    .to_string()
            }
            Self::GrantRequired => {
                "the user has not granted control of mouse and keyboard for this session. Ask the user to grant control (the app shows a grant prompt) before retrying."
                    .to_string()
            }
            Self::RateLimited => {
                "input action rate limit exceeded (60/minute). Wait before issuing more input actions."
                    .to_string()
            }
            Self::BudgetExhausted => {
                "the session input-action budget is exhausted. Ask the user to re-grant control to continue."
                    .to_string()
            }
        }
    }
}

struct SessionConsent {
    granted_at: Instant,
    last_activity: Instant,
    input_actions: u64,
    recent_inputs: VecDeque<Instant>,
}

impl SessionConsent {
    fn new(now: Instant) -> Self {
        Self {
            granted_at: now,
            last_activity: now,
            input_actions: 0,
            recent_inputs: VecDeque::new(),
        }
    }
}

/// 等待用户决定的 T3 确认（pending）。批准令牌由 `computer_use_confirm`
/// Tauri 命令通过 [`ComputerUseShared::mint_confirmation`] 铸造；超过
/// [`CONFIRM_TTL`] 未答复即过期（前端 Deny 只关对话框，后端靠 TTL 失效）。
#[derive(Debug, Clone)]
pub struct PendingConfirmation {
    pub session_id: String,
    pub action_summary: String,
    pub element_label: String,
    pub created_at: Instant,
}

/// 跨会话共享的同意状态（Arc 由工厂构造注入工具）。
pub struct ComputerUseShared {
    enabled: AtomicBool,
    stop: AtomicBool,
    sessions: Mutex<HashMap<String, SessionConsent>>,
    pending_confirmations: Mutex<HashMap<String, PendingConfirmation>>,
    approved_tokens: Mutex<HashSet<String>>,
}

impl Default for ComputerUseShared {
    fn default() -> Self {
        Self::new()
    }
}

impl ComputerUseShared {
    /// `enabled` 默认 false——工具在设置开启前一律拒绝。
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            sessions: Mutex::new(HashMap::new()),
            pending_confirmations: Mutex::new(HashMap::new()),
            approved_tokens: Mutex::new(HashSet::new()),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// 设置开关（由集成层的设置命令调用）。
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }

    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// 授予本会话输入控制权（一次性会话授权）。
    pub fn grant_session(&self, session_id: &str) {
        let now = Instant::now();
        self.sessions
            .lock()
            .insert(session_id.to_string(), SessionConsent::new(now));
    }

    pub fn revoke_session(&self, session_id: &str) {
        self.sessions.lock().remove(session_id);
    }

    /// 会话当前是否持有有效授权（未空闲过期）。只读投影，供状态命令使用；
    /// 门控判定仍以 [`Self::begin_input_action`] 为准。
    pub fn has_active_grant(&self, session_id: &str) -> bool {
        self.sessions.lock().get(session_id).is_some_and(|consent| {
            consent.last_activity.elapsed() <= GRANT_IDLE_TIMEOUT
        })
    }

    /// 紧急停止：置停止旗标并吊销全部会话授权。
    pub fn stop_all(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.sessions.lock().clear();
    }

    /// 用户重新开启后清除停止旗标（不恢复任何授权）。
    pub fn reset_stop(&self) {
        self.stop.store(false, Ordering::SeqCst);
    }

    /// 观察/被动类动作门控：只需总开关开启且未停止。
    pub fn check_readonly(&self) -> Result<(), GuardRejection> {
        if !self.is_enabled() {
            return Err(GuardRejection::Disabled);
        }
        if self.is_stopped() {
            return Err(GuardRejection::Stopped);
        }
        Ok(())
    }

    /// 输入类动作门控 + 记账。通过即消耗一次预算并刷新活动时间。
    /// 在每次输入注入前调用；注入前还应再查一次 [`Self::is_stopped`]。
    pub fn begin_input_action(&self, session_id: &str) -> Result<(), GuardRejection> {
        self.check_readonly()?;
        let now = Instant::now();
        let mut sessions = self.sessions.lock();
        let Some(consent) = sessions.get_mut(session_id) else {
            return Err(GuardRejection::GrantRequired);
        };
        if now.duration_since(consent.last_activity) > GRANT_IDLE_TIMEOUT {
            sessions.remove(session_id);
            return Err(GuardRejection::GrantRequired);
        }
        if consent.input_actions >= INPUT_ACTION_BUDGET {
            sessions.remove(session_id);
            return Err(GuardRejection::BudgetExhausted);
        }
        let window_start = now.checked_sub(Duration::from_secs(60)).unwrap_or(now);
        while consent
            .recent_inputs
            .front()
            .is_some_and(|t| *t < window_start)
        {
            consent.recent_inputs.pop_front();
        }
        if consent.recent_inputs.len() >= INPUT_RATE_LIMIT_PER_MINUTE as usize {
            return Err(GuardRejection::RateLimited);
        }
        consent.recent_inputs.push_back(now);
        consent.input_actions += 1;
        consent.last_activity = now;
        Ok(())
    }

    /// 注册一个等待用户决定的 T3 确认，返回 confirm_id。
    pub fn new_pending_confirmation(
        &self,
        session_id: &str,
        action_summary: impl Into<String>,
        element_label: impl Into<String>,
    ) -> String {
        let confirm_id = format!("cu-{:016x}", rand::random::<u64>());
        self.pending_confirmations.lock().insert(
            confirm_id.clone(),
            PendingConfirmation {
                session_id: session_id.to_string(),
                action_summary: action_summary.into(),
                element_label: element_label.into(),
                created_at: Instant::now(),
            },
        );
        confirm_id
    }

    pub fn pending_confirmation(&self, confirm_id: &str) -> Option<PendingConfirmation> {
        let mut pending = self.pending_confirmations.lock();
        let entry = pending.get(confirm_id)?;
        if entry.created_at.elapsed() > CONFIRM_TTL {
            pending.remove(confirm_id);
            return None;
        }
        Some(entry.clone())
    }

    /// 铸造批准令牌。只能由 `computer_use_confirm` Tauri 命令调用——绝不能让
    /// 模型经工具调用自己铸造。只为存在且未过期的 pending 铸币（未知/过期 id
    /// 一律不铸，调用方命令已用 [`Self::pending_confirmation`] 拦过一层）。
    pub fn mint_confirmation(&self, confirm_id: &str) {
        let removed = self.pending_confirmations.lock().remove(confirm_id);
        let valid = removed.is_some_and(|entry| entry.created_at.elapsed() <= CONFIRM_TTL);
        if valid {
            self.approved_tokens.lock().insert(confirm_id.to_string());
        }
    }

    /// 消费批准令牌（单次有效）。工具在执行带 `confirm_id` 的动作前调用。
    pub fn take_confirmation(&self, confirm_id: &str) -> bool {
        self.approved_tokens.lock().remove(confirm_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    fn enabled_shared() -> ComputerUseShared {
        let shared = ComputerUseShared::new();
        shared.set_enabled(true);
        shared
    }

    #[test]
    fn disabled_by_default_and_rejects_everything() {
        let shared = ComputerUseShared::new();
        assert_eq!(shared.check_readonly(), Err(GuardRejection::Disabled));
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::Disabled)
        );
        assert_eq!(
            GuardRejection::Disabled.message(),
            "computer use is disabled in settings. Ask the user to enable it before using this tool."
        );
    }

    #[test]
    fn grant_allows_input_and_revoke_blocks_again() {
        let shared = enabled_shared();
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::GrantRequired)
        );
        shared.grant_session("s1");
        assert!(shared.begin_input_action("s1").is_ok());
        shared.revoke_session("s1");
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::GrantRequired)
        );
    }

    #[test]
    fn has_active_grant_reflects_grant_revoke_and_idle_expiry() {
        let shared = enabled_shared();
        assert!(!shared.has_active_grant("s1"));
        shared.grant_session("s1");
        assert!(shared.has_active_grant("s1"));
        {
            let mut sessions = shared.sessions.lock();
            if let Some(consent) = sessions.get_mut("s1") {
                consent.last_activity = Instant::now() - GRANT_IDLE_TIMEOUT - Duration::from_secs(1);
            }
        }
        assert!(!shared.has_active_grant("s1"));
        shared.grant_session("s1");
        assert!(shared.has_active_grant("s1"));
        shared.revoke_session("s1");
        assert!(!shared.has_active_grant("s1"));
    }

    #[test]
    fn grant_expires_after_idle_timeout() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        // 手工把 last_activity 拨回超时之前。
        {
            let mut sessions = shared.sessions.lock();
            let consent = sessions.get_mut("s1");
            if let Some(consent) = consent {
                consent.last_activity = Instant::now() - GRANT_IDLE_TIMEOUT - Duration::from_secs(1);
            }
        }
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::GrantRequired)
        );
        // 过期即吊销：需要重新 grant。
        shared.grant_session("s1");
        assert!(shared.begin_input_action("s1").is_ok());
    }

    #[test]
    fn rate_limit_trips_at_60_per_minute() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        for _ in 0..INPUT_RATE_LIMIT_PER_MINUTE {
            assert!(shared.begin_input_action("s1").is_ok());
        }
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::RateLimited)
        );
    }

    #[test]
    fn budget_exhaustion_forces_regrant() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        // 直接把计数拨到预算上限（500 次真实调用太慢）。
        {
            let mut sessions = shared.sessions.lock();
            if let Some(consent) = sessions.get_mut("s1") {
                consent.input_actions = INPUT_ACTION_BUDGET;
            }
        }
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::BudgetExhausted)
        );
        // 预算用尽吊销授权：重新 grant 后恢复。
        shared.grant_session("s1");
        assert!(shared.begin_input_action("s1").is_ok());
    }

    #[test]
    fn stop_flag_blocks_and_stop_all_revokes() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        shared.stop_all();
        assert!(shared.is_stopped());
        assert_eq!(shared.check_readonly(), Err(GuardRejection::Stopped));
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::Stopped)
        );
        shared.reset_stop();
        // stop_all 已吊销授权：即使清除停止旗标仍需重新 grant。
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::GrantRequired)
        );
    }

    #[test]
    fn t3_denylist_matches_case_insensitively_in_both_languages() {
        for label in [
            "Buy now",
            "PAY",
            "Complete Purchase",
            "Send message",
            "Delete file",
            "Wire Transfer",
            "submit form",
            "立即购买",
            "确认支付",
            "付款",
            "发送",
            "彻底删除",
            "转账",
            "提交订单",
        ] {
            assert!(matches_t3_denylist(label), "should match: {label}");
        }
        for label in ["Open", "Save as", "显示更多", "取消", "Settings"] {
            assert!(!matches_t3_denylist(label), "should not match: {label}");
        }
        assert!(is_secure_role("Password Text"));
        assert!(is_secure_role("AXSecureTextField"));
        assert!(!is_secure_role("button"));
    }

    #[test]
    fn confirmation_tokens_are_single_use() {
        let shared = enabled_shared();
        let id = shared.new_pending_confirmation("s1", "left_click (100,200)", "Buy now");
        let pending = shared.pending_confirmation(&id);
        assert!(pending.as_ref().is_some_and(|p| p.session_id == "s1"));
        // 未铸造前不可消费。
        assert!(!shared.take_confirmation(&id));
        shared.mint_confirmation(&id);
        // 铸造后 pending 清除。
        assert!(shared.pending_confirmation(&id).is_none());
        assert!(shared.take_confirmation(&id));
        // 单次使用：第二次消费失败。
        assert!(!shared.take_confirmation(&id));
    }

    #[test]
    fn pending_confirmation_expires_after_ttl() {
        let shared = enabled_shared();
        let id = shared.new_pending_confirmation("s1", "left_click (100,200)", "Buy now");
        // 手工把 created_at 拨回 TTL 之前（等真实 5 分钟太慢）。
        {
            let mut pending = shared.pending_confirmations.lock();
            if let Some(entry) = pending.get_mut(&id) {
                entry.created_at = Instant::now() - CONFIRM_TTL - Duration::from_secs(1);
            }
        }
        // 过期即视为不存在（前端 Deny 只关对话框，后端靠 TTL 自行失效）。
        assert!(shared.pending_confirmation(&id).is_none());
        // 过期 pending 不再铸币：令牌不可用。
        shared.mint_confirmation(&id);
        assert!(!shared.take_confirmation(&id));
    }

    #[test]
    fn activity_refreshes_grant_idle_clock() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        {
            let mut sessions = shared.sessions.lock();
            if let Some(consent) = sessions.get_mut("s1") {
                consent.last_activity = Instant::now() - GRANT_IDLE_TIMEOUT / 2;
            }
        }
        assert!(shared.begin_input_action("s1").is_ok());
        // 动作刷新了 last_activity：过半超时仍未过期。
        sleep(Duration::from_millis(5));
        {
            let sessions = shared.sessions.lock();
            let fresh = sessions
                .get("s1")
                .is_some_and(|c| c.last_activity.elapsed() < GRANT_IDLE_TIMEOUT / 2);
            assert!(fresh);
        }
    }
}
