//! 同意守卫：设置开关、会话授权、停止旗标、速率限制、动作预算、T3 确认。
//!
//! 引擎当前自动批准所有工具调用，因此同意门控必须内建于工具自身——本模块是
//! 唯一的事实来源。集成层（Tauri 命令）通过公开 API 注入用户决定：
//! `set_enabled` / `grant_session` / `revoke_session` / `stop_all` /
//! `mint_confirmation`。授权只活于内存，永不落盘。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// 输入类会话授权的空闲超时：10 分钟无输入动作即失效，需重新授权。
pub const GRANT_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// 输入类动作滑动窗口速率上限：60 次/分钟。
pub const INPUT_RATE_LIMIT_PER_MINUTE: u32 = 60;
/// 单次授权的输入类动作预算：用尽后强制重新授权（防失控循环）。
pub const INPUT_ACTION_BUDGET: u64 = 500;
/// T3 确认的有效期：未答复的 pending 必须自行过期——过期后读取视为不存在，
/// 也不能再为它铸造批准令牌。
pub const CONFIRM_TTL: Duration = Duration::from_secs(5 * 60);
/// 用户明确「拒绝」后的记忆时长：模型在窗口内重试同一 confirm_id 会得到
/// 「已被用户拒绝」而不是模糊的「令牌无效」，前端也借此抑制重复弹窗。
pub const DENIED_TTL: Duration = Duration::from_secs(60);
/// pending 确认与批准令牌的存量上限：失控循环的模型每被拦截一次就铸造一个
/// pending，无上限会随进程寿命无界增长（评审发现）；超限逐出最旧条目。
pub const MAX_PENDING_CONFIRMATIONS: usize = 100;
pub const MAX_APPROVED_TOKENS: usize = 100;

/// T3 后果性动作名单（大小写不敏感子串匹配；中英文）。
/// 命中即不执行，要求用户显式确认。
pub const T3_DENYLIST: &[&str] = &[
    "buy", "pay", "purchase", "send", "delete", "transfer", "submit", "购买", "支付", "付款",
    "发送", "删除", "转账", "提交",
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
/// [`CONFIRM_TTL`] 未答复即过期。
///
/// `action_summary` 是拦截时该动作的确定性摘要：铸造出的批准令牌与它绑定，
/// 模型拿着令牌重放**另一个**动作会被拒绝（评审发现：令牌原是裸字符串，
/// 可花在任意动作上，包括 type 进密码框）。
#[derive(Debug, Clone)]
pub struct PendingConfirmation {
    pub session_id: String,
    pub action_summary: String,
    pub element_label: String,
    pub created_at: Instant,
}

/// 已铸造的批准令牌：绑定会话与动作摘要，[`CONFIRM_TTL`] 内未消费即过期。
#[derive(Debug, Clone)]
struct ApprovedToken {
    session_id: String,
    action_summary: String,
    minted_at: Instant,
}

/// 消费批准令牌的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationCheck {
    /// 令牌有效且与会话/动作匹配，已消费（单次有效）。
    Granted,
    /// 无此令牌（未知 / 已消费 / 已过期 / 会话或动作不匹配）。
    Unknown,
    /// 用户明确拒绝了该确认（[`DENIED_TTL`] 内）——模型不应重试同一动作。
    Denied,
}

/// 跨会话共享的同意状态（Arc 由工厂构造注入工具）。
pub struct ComputerUseShared {
    enabled: AtomicBool,
    stop: AtomicBool,
    sessions: Mutex<HashMap<String, SessionConsent>>,
    /// 物理鼠标/键盘是全局独占资源，但 backend 是每会话一条 worker——这把
    /// 进程级锁把**跨会话**的输入注入串行化（评审发现：两个并发会话可各持
    /// 有效授权交替打字/点击）。Input 类动作在筛查+执行全程持有。
    physical_input_lock: Mutex<()>,
    pending_confirmations: Mutex<HashMap<String, PendingConfirmation>>,
    approved_tokens: Mutex<HashMap<String, ApprovedToken>>,
    denied_confirmations: Mutex<HashMap<String, Instant>>,
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
            physical_input_lock: Mutex::new(()),
            pending_confirmations: Mutex::new(HashMap::new()),
            approved_tokens: Mutex::new(HashMap::new()),
            denied_confirmations: Mutex::new(HashMap::new()),
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

    /// 授予本会话输入控制权（一次性会话授权）。顺带清扫已空闲过期的僵尸
    /// 会话（评审发现：会话删除后条目永不再用也不回收）。
    pub fn grant_session(&self, session_id: &str) {
        let now = Instant::now();
        let mut sessions = self.sessions.lock();
        sessions
            .retain(|_, consent| now.duration_since(consent.last_activity) <= GRANT_IDLE_TIMEOUT);
        sessions.insert(session_id.to_string(), SessionConsent::new(now));
    }

    pub fn revoke_session(&self, session_id: &str) {
        self.sessions.lock().remove(session_id);
    }

    /// 会话当前是否持有有效授权（未空闲过期）。只读投影，供状态命令使用；
    /// 门控判定仍以 [`Self::begin_input_action`] 为准。
    pub fn has_active_grant(&self, session_id: &str) -> bool {
        self.sessions
            .lock()
            .get(session_id)
            .is_some_and(|consent| consent.last_activity.elapsed() <= GRANT_IDLE_TIMEOUT)
    }

    /// 紧急停止：置停止旗标并吊销全部会话授权、清空全部待决/已批/已拒确认
    /// （停止之后不应有任何同意状态存活）。
    pub fn stop_all(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.sessions.lock().clear();
        self.pending_confirmations.lock().clear();
        self.approved_tokens.lock().clear();
        self.denied_confirmations.lock().clear();
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
    /// 在每次输入注入前调用；注入前还应再查一次 [`Self::verify_input_action`]。
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

    /// 只读复检：授权是否仍然有效（开关、停止旗标、授权存在且未空闲过期）。
    /// 不消耗预算/速率。在 `begin_input_action` 与真实注入之间可能隔着自动
    /// 截图等耗时步骤（评审发现：该窗口内的 revoke 不生效），注入前必须调用。
    pub fn verify_input_action(&self, session_id: &str) -> Result<(), GuardRejection> {
        self.check_readonly()?;
        let now = Instant::now();
        let sessions = self.sessions.lock();
        let Some(consent) = sessions.get(session_id) else {
            return Err(GuardRejection::GrantRequired);
        };
        if now.duration_since(consent.last_activity) > GRANT_IDLE_TIMEOUT {
            return Err(GuardRejection::GrantRequired);
        }
        Ok(())
    }

    /// 跨会话串行化物理输入注入（见 [`ComputerUseShared::physical_input_lock`]）。
    pub fn lock_physical_input(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.physical_input_lock.lock()
    }

    /// 注册一个等待用户决定的 T3 确认，返回 confirm_id。
    /// 顺带清扫过期 pending 并执行存量上限（超限逐出最旧）。
    pub fn new_pending_confirmation(
        &self,
        session_id: &str,
        action_summary: impl Into<String>,
        element_label: impl Into<String>,
    ) -> String {
        let confirm_id = format!("cu-{:016x}", rand::random::<u64>());
        let now = Instant::now();
        let mut pending = self.pending_confirmations.lock();
        pending.retain(|_, entry| now.duration_since(entry.created_at) <= CONFIRM_TTL);
        while pending.len() >= MAX_PENDING_CONFIRMATIONS {
            let oldest = pending
                .iter()
                .min_by_key(|(_, entry)| entry.created_at)
                .map(|(id, _)| id.clone());
            match oldest {
                Some(id) => pending.remove(&id),
                None => break,
            };
        }
        pending.insert(
            confirm_id.clone(),
            PendingConfirmation {
                session_id: session_id.to_string(),
                action_summary: action_summary.into(),
                element_label: element_label.into(),
                created_at: now,
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

    /// 用户在前端明确「拒绝」一个被拦截的 T3 动作：清除 pending 并短期记忆
    /// 该决定（[`DENIED_TTL`]），模型重试同一 confirm_id 会得到明确的拒绝。
    pub fn deny_confirmation(&self, confirm_id: &str) -> bool {
        let removed = self.pending_confirmations.lock().remove(confirm_id);
        let now = Instant::now();
        let mut denied = self.denied_confirmations.lock();
        denied.retain(|_, at| now.duration_since(*at) <= DENIED_TTL);
        denied.insert(confirm_id.to_string(), now);
        removed.is_some()
    }

    /// 铸造批准令牌。只能由 `computer_use_confirm` Tauri 命令调用——绝不能让
    /// 模型经工具调用自己铸造。只为存在且未过期的 pending 铸币；令牌继承该
    /// pending 的会话与动作摘要（消费时逐项比对）。
    pub fn mint_confirmation(&self, confirm_id: &str) {
        let entry = self.pending_confirmations.lock().remove(confirm_id);
        let Some(entry) = entry else {
            return;
        };
        if entry.created_at.elapsed() > CONFIRM_TTL {
            return;
        }
        let now = Instant::now();
        let mut tokens = self.approved_tokens.lock();
        tokens.retain(|_, token| now.duration_since(token.minted_at) <= CONFIRM_TTL);
        while tokens.len() >= MAX_APPROVED_TOKENS {
            let oldest = tokens
                .iter()
                .min_by_key(|(_, token)| token.minted_at)
                .map(|(id, _)| id.clone());
            match oldest {
                Some(id) => tokens.remove(&id),
                None => break,
            };
        }
        tokens.insert(
            confirm_id.to_string(),
            ApprovedToken {
                session_id: entry.session_id,
                action_summary: entry.action_summary,
                minted_at: now,
            },
        );
    }

    /// 消费批准令牌（单次有效）。工具在执行带 `confirm_id` 的动作前调用；
    /// 令牌必须与**本次**会话和动作摘要完全匹配（评审发现：裸字符串令牌可
    /// 花在任意动作/会话上）。
    pub fn take_confirmation(
        &self,
        confirm_id: &str,
        session_id: &str,
        action_summary: &str,
    ) -> ConfirmationCheck {
        let now = Instant::now();
        {
            let mut denied = self.denied_confirmations.lock();
            denied.retain(|_, at| now.duration_since(*at) <= DENIED_TTL);
            if denied.contains_key(confirm_id) {
                return ConfirmationCheck::Denied;
            }
        }
        let mut tokens = self.approved_tokens.lock();
        let Some(token) = tokens.get(confirm_id) else {
            return ConfirmationCheck::Unknown;
        };
        if now.duration_since(token.minted_at) > CONFIRM_TTL {
            tokens.remove(confirm_id);
            return ConfirmationCheck::Unknown;
        }
        if token.session_id != session_id || token.action_summary != action_summary {
            // 会话/动作不匹配：不放行，但**保留**令牌——绑定是精确匹配，唯一
            // 能通过的组合就是用户批准的那个原动作重放，误试不应烧掉用户的
            // 确认。
            return ConfirmationCheck::Unknown;
        }
        tokens.remove(confirm_id);
        ConfirmationCheck::Granted
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
                consent.last_activity =
                    Instant::now() - GRANT_IDLE_TIMEOUT - Duration::from_secs(1);
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
                consent.last_activity =
                    Instant::now() - GRANT_IDLE_TIMEOUT - Duration::from_secs(1);
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
    fn confirmation_tokens_are_single_use_and_bound_to_session_and_action() {
        let shared = enabled_shared();
        let summary = "left click x1 at Some((100, 200))";
        let id = shared.new_pending_confirmation("s1", summary, "Buy now");
        let pending = shared.pending_confirmation(&id);
        assert!(pending.as_ref().is_some_and(|p| p.session_id == "s1"));
        // 未铸造前不可消费。
        assert_eq!(
            shared.take_confirmation(&id, "s1", summary),
            ConfirmationCheck::Unknown
        );
        shared.mint_confirmation(&id);
        // 铸造后 pending 清除。
        assert!(shared.pending_confirmation(&id).is_none());
        // 正确的会话 + 动作摘要才能消费。
        assert_eq!(
            shared.take_confirmation(&id, "s-other", summary),
            ConfirmationCheck::Unknown,
            "token minted for s1 must not be spent by another session"
        );
        assert_eq!(
            shared.take_confirmation(&id, "s1", "type 5 chars"),
            ConfirmationCheck::Unknown,
            "token must be bound to the action it approved"
        );
        // 不匹配的误试不销毁令牌(精确绑定下唯一能通过的只有用户批准的原动作)。
        assert_eq!(
            shared.take_confirmation(&id, "s1", summary),
            ConfirmationCheck::Granted
        );
        // 单次使用：第二次消费失败。
        assert_eq!(
            shared.take_confirmation(&id, "s1", summary),
            ConfirmationCheck::Unknown
        );
    }

    #[test]
    fn deny_marks_confirmation_denied_and_forgets_after_ttl() {
        let shared = enabled_shared();
        let id = shared.new_pending_confirmation("s1", "left click", "Buy now");
        assert!(shared.deny_confirmation(&id));
        // deny 清除 pending：不能再为它铸币。
        assert!(shared.pending_confirmation(&id).is_none());
        shared.mint_confirmation(&id);
        assert_eq!(
            shared.take_confirmation(&id, "s1", "left click"),
            ConfirmationCheck::Denied,
            "denied confirm_id must report Denied, not Unknown"
        );
        // 未知 id 的 deny 只记忆,不算成功清除。
        assert!(!shared.deny_confirmation("cu-unknown"));
        assert_eq!(
            shared.take_confirmation("cu-unknown", "s1", "x"),
            ConfirmationCheck::Denied
        );
    }

    #[test]
    fn pending_and_token_maps_are_capped() {
        let shared = enabled_shared();
        for i in 0..(MAX_PENDING_CONFIRMATIONS + 20) {
            let id = shared.new_pending_confirmation("s1", format!("action {i}"), "Buy now");
            if i < MAX_APPROVED_TOKENS {
                shared.mint_confirmation(&id);
            }
        }
        assert!(shared.pending_confirmations.lock().len() <= MAX_PENDING_CONFIRMATIONS);
        assert!(shared.approved_tokens.lock().len() <= MAX_APPROVED_TOKENS);
    }

    #[test]
    fn verify_input_action_is_read_only_and_catches_revoke() {
        let shared = enabled_shared();
        assert_eq!(
            shared.verify_input_action("s1"),
            Err(GuardRejection::GrantRequired)
        );
        shared.grant_session("s1");
        assert!(shared.verify_input_action("s1").is_ok());
        shared.revoke_session("s1");
        assert_eq!(
            shared.verify_input_action("s1"),
            Err(GuardRejection::GrantRequired)
        );
        // 只读：多次 verify 不消耗预算。
        shared.grant_session("s1");
        for _ in 0..10 {
            assert!(shared.verify_input_action("s1").is_ok());
        }
    }

    #[test]
    fn physical_input_lock_serializes_holders() {
        let shared = enabled_shared();
        {
            let _guard = shared.lock_physical_input();
            assert!(
                shared.physical_input_lock.try_lock().is_none(),
                "second holder must block while input is in flight"
            );
        }
        assert!(shared.physical_input_lock.try_lock().is_some());
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
        // 过期即视为不存在。
        assert!(shared.pending_confirmation(&id).is_none());
        // 过期 pending 不再铸币：令牌不可用。
        shared.mint_confirmation(&id);
        assert_eq!(
            shared.take_confirmation(&id, "s1", "left_click (100,200)"),
            ConfirmationCheck::Unknown
        );
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
