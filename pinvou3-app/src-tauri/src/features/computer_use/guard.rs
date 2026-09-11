//! 同意守卫：设置开关、会话授权、停止旗标、T3 确认。
//!
//! 引擎当前自动批准所有工具调用，因此同意门控必须内建于工具自身——本模块是
//! 唯一的事实来源。集成层（Tauri 命令）通过公开 API 注入用户决定：
//! `set_enabled` / `grant_session` / `revoke_session` / `stop_all` /
//! `mint_confirmation`。授权只活于内存，永不落盘。
//!
//! Confirmation model (mainstream): a blocked action raises one pending
//! per session (a new request replaces the old one, like a normal dialog);
//! approval mints a single-use token bound to the session and the action
//! summary. No budgets, no rate limits — the safety floor is the
//! denylist screening plus explicit user confirmation. An explicit deny
//! only consumes the pending confirmation and records no server-side
//! state: a retry of the same action goes through screening again and
//! mints a fresh pending (decline is model-visible context, not stored
//! state — the mainstream behavior).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::backend::BackendRegistry;

/// 输入类会话授权的空闲超时：10 分钟无输入动作即失效，需重新授权。
pub const GRANT_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// T3 确认的有效期：未答复的 pending 必须自行过期——过期后读取视为不存在，
/// 也不能再为它铸造批准令牌。
pub const CONFIRM_TTL: Duration = Duration::from_secs(5 * 60);
/// 跨会话物理输入锁的有界等待上限。Input 动作从筛查到注入全程持锁（最长
/// 可达一个 backend 调用上限，见 backend.rs 的 `BACKEND_CALL_TIMEOUT`），
/// 无限挂等会让其他会话静默卡死（评审发现）——超时显式报错，由模型自行
/// 等待重试。
pub const PHYSICAL_INPUT_LOCK_TIMEOUT: Duration = Duration::from_secs(20);

/// T3 后果性动作名单（大小写不敏感子串匹配；中英日）。
/// 命中即不执行，要求用户显式确认。误伤（如 "bin" 命中 "combine"）方向
/// 是 fail-closed，代价只是一次额外确认。
pub const T3_DENYLIST: &[&str] = &[
    "buy",
    "pay",
    "purchase",
    "send",
    "delete",
    "transfer",
    "submit",
    // 拖拽/删除的常见目的地（评审发现：把文件拖进回收站/废纸篓零确认完成）。
    "trash",
    "bin",
    // 评审补充覆盖：确认/下单/安装/抹除类语义（此前 "confirm"、"checkout"、
    // "install"、繁体全域缺席——icon 按钮之外最常被命中的词恰恰是 "Confirm"）。
    "confirm",
    "checkout",
    "place order",
    "order now",
    "install",
    "erase",
    "discard",
    "回收站",
    "废纸篓",
    "ゴミ箱",
    "购买",
    "支付",
    "付款",
    "发送",
    "删除",
    "转账",
    "提交",
    "清空",
    "确认",
    "安裝",
    // 繁体（评审发现：繁体界面全域缺席）。
    "刪除",
    "購買",
    "傳送",
    "轉帳",
    "確認",
    "資源回收筒",
    "廢紙簍",
    "購入",
    "支払い",
    "送信",
    "削除",
    "送金",
    "提出",
    "注文",
    // Review follow-up coverage: generic affirmative/action terms that gate
    // consequential dialogs ("OK", "Yes, continue", "Accept", "Run"...).
    "ok",
    "yes",
    "continue",
    "accept",
    "agree",
    "remove",
    "empty",
    "run",
    "execute",
    // CJK equivalents. The matcher is substring-based, so single-character
    // CJK terms (是/好) would over-match unrelated labels; only multi-char
    // terms are safe to list.
    "继续",
    "同意",
    "运行",
    "执行",
    "删除全部",
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
    /// 输入类动作缺少有效会话授权（或已空闲过期）。
    GrantRequired,
    /// 跨会话物理输入锁被其他会话持有，有界等待超时（见
    /// [`PHYSICAL_INPUT_LOCK_TIMEOUT`]）。
    InputBusy,
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
            Self::InputBusy => {
                "another session is performing a physical input action. Wait and retry."
                    .to_string()
            }
        }
    }
}

/// 单次会话授权：只有活动时间——10 分钟空闲即失效（idle expiry 是唯一
/// 的授权寿命机制；无预算、无速率窗口）。
struct SessionConsent {
    last_activity: Instant,
}

impl SessionConsent {
    fn new(now: Instant) -> Self {
        Self { last_activity: now }
    }
}

/// 等待用户决定的 T3 确认（pending）。批准令牌由 `computer_use_confirm`
/// Tauri 命令通过 [`ComputerUseShared::mint_confirmation`] 铸造；超过
/// [`CONFIRM_TTL`] 未答复即过期。每个会话同时至多一个 pending——新请求
/// 直接替换旧请求（与普通对话框一致，最新胜出）。
#[derive(Debug, Clone)]
pub struct PendingConfirmation {
    pub session_id: String,
    /// The plain human-readable parameter summary of the blocked action
    /// (e.g. `left click x1 at Some((5, 6))`, `type 3 characters`); the
    /// approval token is bound to exactly this summary.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmationCheck {
    /// 令牌有效且与会话/动作摘要全部匹配，已消费（单次有效）。
    Granted,
    /// 无此令牌（未知 / 已消费 / 已过期 / 会话或动作不匹配）。
    Unknown,
}

/// The two consent maps (pending confirmations, approved tokens) share one
/// mutex: the wholesale clears in `stop_all` / `revoke_all_sessions` /
/// `revoke_session` and the pending→token transition in `mint_confirmation`
/// are atomic with respect to each other — with independent locks, a mint
/// interleaved with a disable/revoke could leave behind a token that
/// survived the disable.
struct ConsentMaps {
    pending: HashMap<String, PendingConfirmation>,
    approved_tokens: HashMap<String, ApprovedToken>,
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
    /// The two consent maps under a single mutex (see [`ConsentMaps`]).
    consent: Mutex<ConsentMaps>,
    /// 会话 → 后端句柄登记表（构造登记/析构注销）。撤销授权、全局停止或
    /// 总开关关闭时，命令层经此触发后端关闭持久 OS 级授权（如 Wayland
    /// portal 会话）——授权语义的应用侧事实来源在本模块，OS 侧的终止
    /// 动作经这里转交后端。
    pub backends: BackendRegistry,
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
            consent: Mutex::new(ConsentMaps {
                pending: HashMap::new(),
                approved_tokens: HashMap::new(),
            }),
            backends: BackendRegistry::default(),
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

    /// Revoke a single session: besides the grant row, also drop that
    /// session's pending confirmations and minted approval tokens (all under
    /// the single consent lock). After the user withdraws control, this
    /// session's consent artifacts for previously blocked actions must not
    /// survive; other sessions' artifacts are untouched.
    pub fn revoke_session(&self, session_id: &str) {
        self.sessions.lock().remove(session_id);
        let mut consent = self.consent.lock();
        consent
            .pending
            .retain(|_, entry| entry.session_id != session_id);
        consent
            .approved_tokens
            .retain(|_, token| token.session_id != session_id);
    }

    /// 会话当前是否持有有效授权（未空闲过期）。只读投影，供状态命令使用；
    /// 门控判定仍以 [`Self::begin_input_action`] 为准。
    pub fn has_active_grant(&self, session_id: &str) -> bool {
        self.sessions
            .lock()
            .get(session_id)
            .is_some_and(|consent| consent.last_activity.elapsed() <= GRANT_IDLE_TIMEOUT)
    }

    /// 紧急停止：置停止旗标并吊销全部会话授权、清空全部待决/已批确认
    /// （停止之后不应有任何同意状态存活）。
    pub fn stop_all(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.sessions.lock().clear();
        let mut consent = self.consent.lock();
        consent.pending.clear();
        consent.approved_tokens.clear();
    }

    /// 用户重新开启后清除停止旗标（不恢复任何授权）。
    pub fn reset_stop(&self) {
        self.stop.store(false, Ordering::SeqCst);
    }

    /// 吊销全部会话授权并清空全部同意状态（待决确认、已铸令牌），
    /// 但**不置**停止旗标——与 [`Self::stop_all`] 的紧急停止语义区分：总开关
    /// 关闭不是急停，重新开启后不应残留停止状态
    /// （`computer_use_set_enabled(false)` 调用）。评审发现：此前关闭开关不清
    /// 授权与令牌，重开后旧 grant 在 10 分钟空闲窗口内、旧批准令牌在 5 分钟
    /// TTL 内仍然有效——开关关闭期间的同意状态在重开后不应存活。
    pub fn revoke_all_sessions(&self) {
        self.sessions.lock().clear();
        let mut consent = self.consent.lock();
        consent.pending.clear();
        consent.approved_tokens.clear();
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

    /// 输入类动作门控：开关开启、未停止、会话持有未空闲过期的授权。**不**
    /// 刷新活动时间——keepalive 只在动作真正执行成功后记账
    /// ([`Self::keepalive_input_action`]；round-6 评审：被拦/被拒的调用
    /// 不应让用户反复拒绝的会话永久续期)。注入前还应再查一次
    /// [`Self::verify_input_action`]。
    pub fn begin_input_action(&self, session_id: &str) -> Result<(), GuardRejection> {
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

    /// 动作执行成功后的活动记账（唯一的授权续期点）。
    pub fn keepalive_input_action(&self, session_id: &str) {
        if let Some(consent) = self.sessions.lock().get_mut(session_id) {
            consent.last_activity = Instant::now();
        }
    }

    /// 只读复检：授权是否仍然有效（开关、停止旗标、授权存在且未空闲过期）。
    /// 在 `begin_input_action` 与真实注入之间可能隔着自动截图等耗时步骤
    /// （评审发现：该窗口内的 revoke 不生效），注入前必须调用。
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
    /// 获取是**有界等待**（`try_lock_for`，见 [`PHYSICAL_INPUT_LOCK_TIMEOUT`]）：
    /// 锁被其他会话持有时超时返回显式 [`GuardRejection::InputBusy`]，不无限
    /// 挂等。锁语义（Input 动作筛查到注入全程持有）与释放路径（guard drop）
    /// 不变。
    pub fn lock_physical_input(&self) -> Result<parking_lot::MutexGuard<'_, ()>, GuardRejection> {
        self.physical_input_lock
            .try_lock_for(PHYSICAL_INPUT_LOCK_TIMEOUT)
            .ok_or(GuardRejection::InputBusy)
    }

    /// 注册一个等待用户决定的 T3 确认，返回 confirm_id。每个会话至多一个
    /// pending：新请求替换该会话已有的 pending（最新胜出，与普通对话框
    /// 一致），因此本方法不会失败。顺带清扫过期 pending。
    pub fn new_pending_confirmation(
        &self,
        session_id: &str,
        action_summary: impl Into<String>,
        element_label: impl Into<String>,
    ) -> String {
        let confirm_id = format!("cu-{:016x}", rand::random::<u64>());
        let now = Instant::now();
        let mut consent = self.consent.lock();
        let pending = &mut consent.pending;
        pending.retain(|_, entry| now.duration_since(entry.created_at) <= CONFIRM_TTL);
        // At most one pending per session: the newest request wins (a normal
        // dialog replaces the previous one instead of queueing behind it).
        pending.retain(|_, entry| entry.session_id != session_id);
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
        let mut consent = self.consent.lock();
        let entry = consent.pending.get(confirm_id)?;
        if entry.created_at.elapsed() > CONFIRM_TTL {
            consent.pending.remove(confirm_id);
            return None;
        }
        Some(entry.clone())
    }

    /// 用户在前端明确「拒绝」一个被拦截的 T3 动作：消耗 pending。拒绝不
    /// 记录任何服务端状态——同一动作的重试会照常走筛查并铸造**新的**
    /// pending、重发确认事件（主流模型：拒绝只是模型可见的上下文，不是
    /// 存储的惩罚状态）。未知 id 返回 false。
    pub fn deny_confirmation(&self, confirm_id: &str) -> bool {
        self.consent.lock().pending.remove(confirm_id).is_some()
    }

    /// 铸造批准令牌。只能由 `computer_use_confirm` Tauri 命令调用——绝不能让
    /// 模型经工具调用自己铸造。只为存在且未过期的 pending 铸币并返回 `true`；
    /// pending 不存在/已过期/已被决定时返回 `false`——静默 no-op 会让前端把
    /// 失败显示为成功（评审发现）。令牌继承该 pending 的会话与动作摘要
    /// （消费时逐项比对）。令牌无存量上限：一个会话同时只有一个 pending，
    /// 铸币又移除 pending，令牌存量天然受交互节奏约束；过期由消费前的
    /// TTL 清扫兜底。
    pub fn mint_confirmation(&self, confirm_id: &str) -> bool {
        // pending removal + token insertion in one lock: a mint cannot
        // interleave with a revoke/clear and leave a token that outlives the
        // disable.
        let mut consent = self.consent.lock();
        let entry = consent.pending.remove(confirm_id);
        let Some(entry) = entry else {
            return false;
        };
        if entry.created_at.elapsed() > CONFIRM_TTL {
            return false;
        }
        let now = Instant::now();
        let tokens = &mut consent.approved_tokens;
        tokens.retain(|_, token| now.duration_since(token.minted_at) <= CONFIRM_TTL);
        tokens.insert(
            confirm_id.to_string(),
            ApprovedToken {
                session_id: entry.session_id,
                action_summary: entry.action_summary,
                minted_at: now,
            },
        );
        true
    }

    /// 消费批准令牌（单次有效）。工具在执行带 `confirm_id` 的动作前调用；
    /// 令牌必须与**本次**会话和动作摘要完全匹配（评审发现：裸字符串令牌可
    /// 花在任意动作/会话上）。匹配即放行执行——不做二次筛查（主流模型：
    /// API 确认就是一个 per-action 的确认 id，客户端应答后直接执行）；
    /// 不匹配时令牌保留（精确绑定下唯一能通过的组合就是用户批准的那个
    /// 原动作重放，误试不应烧掉用户的确认）。
    pub fn take_confirmation(
        &self,
        confirm_id: &str,
        session_id: &str,
        action_summary: &str,
    ) -> ConfirmationCheck {
        let now = Instant::now();
        let mut consent = self.consent.lock();
        let Some(token) = consent.approved_tokens.get(confirm_id) else {
            return ConfirmationCheck::Unknown;
        };
        if now.duration_since(token.minted_at) > CONFIRM_TTL {
            consent.approved_tokens.remove(confirm_id);
            return ConfirmationCheck::Unknown;
        }
        if token.session_id != session_id || token.action_summary != action_summary {
            return ConfirmationCheck::Unknown;
        }
        consent.approved_tokens.remove(confirm_id);
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

    /// 测试便捷封装：标准三参数的 pending / 消费。
    fn new_pending(shared: &ComputerUseShared, session: &str, summary: &str) -> String {
        shared.new_pending_confirmation(session, summary, "Buy now")
    }

    fn take(
        shared: &ComputerUseShared,
        id: &str,
        session: &str,
        summary: &str,
    ) -> ConfirmationCheck {
        shared.take_confirmation(id, session, summary)
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
            // 日文词项（评审发现：日文界面元素此前完全不在名单内）。
            "カートに追加して購入",
            "お支払い",
            "メッセージを送信",
            "ファイルを削除",
            "口座に送金",
            "フォームを提出",
            "注文を確定",
            "内容の確認",
            // 拖拽/删除目的地与简体「确认」（第三轮评审发现：把文件拖进
            // 回收站可零确认完成；简体「确认」缺席而日文「確認」在列）。
            "Recycle Bin",
            "Move to Trash",
            "Empty Trash",
            "移到废纸篓",
            "拖入回收站",
            "ゴミ箱に移動",
            "清空列表",
            "确认订单",
            // 评审补充覆盖：确认/下单/安装/抹除 + 繁体。
            "Confirm purchase",
            "Checkout now",
            "Place order",
            "Install updates",
            "Erase disk",
            "刪除檔案",
            "購買",
            "資源回收筒",
            // Review follow-up coverage: generic affirmative/action terms
            // (the most common dialog button copy).
            "OK",
            "yes",
            "Continue",
            "Accept all",
            "I agree",
            "Remove file",
            "Empty folder",
            "Run script",
            "Execute command",
            "继续操作",
            "同意条款",
            "运行脚本",
            "执行命令",
            "删除全部历史",
        ] {
            assert!(matches_t3_denylist(label), "should match: {label}");
        }
        for label in [
            "Open",
            "Save as",
            "显示更多",
            "取消",
            "Settings",
            "キャンセル",
        ] {
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
        let id = new_pending(&shared, "s1", summary);
        let pending = shared.pending_confirmation(&id);
        assert!(pending.as_ref().is_some_and(|p| p.session_id == "s1"));
        // 未铸造前不可消费。
        assert_eq!(
            take(&shared, &id, "s1", summary),
            ConfirmationCheck::Unknown
        );
        assert!(
            shared.mint_confirmation(&id),
            "mint must report success for a live pending"
        );
        // 铸造后 pending 清除。
        assert!(shared.pending_confirmation(&id).is_none());
        // 正确的会话 + 动作摘要才能消费。
        assert_eq!(
            take(&shared, &id, "s-other", summary),
            ConfirmationCheck::Unknown,
            "token minted for s1 must not be spent by another session"
        );
        assert_eq!(
            take(&shared, &id, "s1", "type 5 characters"),
            ConfirmationCheck::Unknown,
            "token must be bound to the action it approved"
        );
        // 不匹配的误试不销毁令牌(精确绑定下唯一能通过的只有用户批准的原动作)。
        assert_eq!(
            take(&shared, &id, "s1", summary),
            ConfirmationCheck::Granted
        );
        // 单次使用：第二次消费失败。
        assert_eq!(
            take(&shared, &id, "s1", summary),
            ConfirmationCheck::Unknown
        );
    }

    /// deny 消费 pending；拒绝不记录服务端状态——同一动作的重试照常铸造
    /// **新的** pending（新 confirm_id），重新走确认流程（主流模型：拒绝
    /// 只是模型可见的上下文）。
    #[test]
    fn deny_confirmation_consumes_the_pending_and_retry_mints_a_new_one() {
        let shared = enabled_shared();
        let id = new_pending(&shared, "s1", "left click");
        assert!(shared.deny_confirmation(&id));
        // deny 清除 pending：不能再为它铸币（mint 返回 false，不再静默 no-op）。
        assert!(shared.pending_confirmation(&id).is_none());
        assert!(!shared.mint_confirmation(&id));
        // 重试同一 id：令牌无效。
        assert_eq!(
            take(&shared, &id, "s1", "left click"),
            ConfirmationCheck::Unknown,
            "a denied id is simply unknown afterwards"
        );
        // 重试同一动作：铸造新的 pending（新 id），确认流程照常——无拒绝记忆。
        let retry = new_pending(&shared, "s1", "left click");
        assert_ne!(retry, id, "a retry after denial must mint a new confirm_id");
        assert!(shared.pending_confirmation(&retry).is_some());
        assert!(shared.mint_confirmation(&retry));
        assert_eq!(
            take(&shared, &retry, "s1", "left click"),
            ConfirmationCheck::Granted
        );
        // 未知 id 的 deny 失败。
        assert!(!shared.deny_confirmation("cu-unknown"));
        assert_eq!(
            take(&shared, "cu-unknown", "s1", "x"),
            ConfirmationCheck::Unknown,
            "unknown id must stay Unknown"
        );
    }

    /// One pending per session: a new request REPLACES the session's
    /// existing pending (newest wins, like a normal dialog).
    #[test]
    fn new_pending_confirmation_replaces_the_sessions_previous_pending() {
        let shared = enabled_shared();
        let first = new_pending(&shared, "s1", "left click");
        let second = new_pending(&shared, "s1", "left click 2");
        assert_ne!(first, second);
        assert!(
            shared.pending_confirmation(&first).is_none(),
            "the replaced pending must be gone"
        );
        let latest = shared.pending_confirmation(&second).expect("newest wins");
        assert_eq!(latest.action_summary, "left click 2");
        // Only one pending for s1 exists; another session is unaffected.
        assert_eq!(
            shared
                .consent
                .lock()
                .pending
                .values()
                .filter(|entry| entry.session_id == "s1")
                .count(),
            1
        );
        let other = new_pending(&shared, "s2", "left click");
        assert!(shared.pending_confirmation(&other).is_some());
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
        // 只读：多次 verify 不产生副作用。
        shared.grant_session("s1");
        for _ in 0..10 {
            assert!(shared.verify_input_action("s1").is_ok());
        }
    }

    #[test]
    fn physical_input_lock_serializes_holders() {
        let shared = enabled_shared();
        {
            let _guard = shared
                .lock_physical_input()
                .expect("free lock must be acquired");
            assert!(
                shared.physical_input_lock.try_lock().is_none(),
                "second holder must block while input is in flight"
            );
        }
        assert!(shared.physical_input_lock.try_lock().is_some());
    }

    /// 评审修复回归：总开关关闭吊销全部会话授权与全部同意状态（待决确认、
    /// 已铸令牌），但**不置**停止旗标（与 stop_all 语义区分）——重开后旧
    /// grant 不得在 10 分钟窗口内复活，disable→enable 循环里旧的已铸令牌
    /// 也不得被免确认重放（第三轮评审发现）。
    #[test]
    fn revoke_all_sessions_clears_grants_and_pendings_without_stop_flag() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        let confirm_id = new_pending(&shared, "s1", "left click");
        let token_id = new_pending(&shared, "s1", "left click 2");
        // new_pending_confirmation replaced the first pending, so mint the
        // token from a separate session-bound request order: re-mint via a
        // fresh pending for s2 to keep the s1 replacement semantics intact.
        let s2_pending = new_pending(&shared, "s2", "left click 3");
        assert!(shared.mint_confirmation(&s2_pending));
        shared.revoke_all_sessions();
        assert!(!shared.has_active_grant("s1"));
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::GrantRequired)
        );
        assert!(shared.pending_confirmation(&confirm_id).is_none());
        assert!(shared.pending_confirmation(&token_id).is_none());
        // 已铸令牌一并清除：重新开启+重新授权后不能拿旧令牌免确认重放。
        assert_eq!(
            take(&shared, &s2_pending, "s2", "left click 3"),
            ConfirmationCheck::Unknown,
            "a disabled cycle must wipe minted approval tokens"
        );
        // 与 stop_all 的语义区分：停止旗标不被置位，观察类动作仍可用。
        assert!(!shared.is_stopped());
        assert!(shared.check_readonly().is_ok());
    }

    #[test]
    fn pending_confirmation_expires_after_ttl() {
        let shared = enabled_shared();
        let id = new_pending(&shared, "s1", "left_click (100,200)");
        // 手工把 created_at 拨回 TTL 之前（等真实 5 分钟太慢）。
        {
            let mut consent = shared.consent.lock();
            if let Some(entry) = consent.pending.get_mut(&id) {
                entry.created_at = Instant::now() - CONFIRM_TTL - Duration::from_secs(1);
            }
        }
        // 过期即视为不存在。
        assert!(shared.pending_confirmation(&id).is_none());
        // 过期 pending 不再铸币：令牌不可用，且 mint 显式报告失败。
        assert!(!shared.mint_confirmation(&id));
        assert_eq!(
            take(&shared, &id, "s1", "left_click (100,200)"),
            ConfirmationCheck::Unknown
        );
    }

    /// keepalive 是**唯一**的授权续期点，且只在动作真正执行成功后由工具层
    /// 调用——`begin_input_action` 门控通过本身不再续期（round-6 评审：
    /// 被拦/被拒的调用不应让用户反复拒绝的会话永久续期）。
    #[test]
    fn keepalive_is_the_only_grant_refresh_and_begin_does_not_extend() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        {
            let mut sessions = shared.sessions.lock();
            if let Some(consent) = sessions.get_mut("s1") {
                consent.last_activity = Instant::now() - GRANT_IDLE_TIMEOUT / 2;
            }
        }
        // 门控通过（仍在窗口内），但**不**续期：时钟仍停在过半位置。
        assert!(shared.begin_input_action("s1").is_ok());
        sleep(Duration::from_millis(5));
        {
            let sessions = shared.sessions.lock();
            let stale = sessions
                .get("s1")
                .is_some_and(|c| c.last_activity.elapsed() > GRANT_IDLE_TIMEOUT / 2);
            assert!(stale, "the gate check must not refresh the idle clock");
        }
        // 执行成功后的 keepalive 才续期。
        shared.keepalive_input_action("s1");
        {
            let sessions = shared.sessions.lock();
            let fresh = sessions
                .get("s1")
                .is_some_and(|c| c.last_activity.elapsed() < GRANT_IDLE_TIMEOUT / 2);
            assert!(fresh);
        }
    }

    /// keepalive 不会复活已过期/已吊销的会话（不存在的条目不重建）。
    #[test]
    fn keepalive_does_not_resurrect_unknown_sessions() {
        let shared = enabled_shared();
        shared.keepalive_input_action("never-granted");
        assert_eq!(
            shared.begin_input_action("never-granted"),
            Err(GuardRejection::GrantRequired)
        );
        shared.grant_session("s1");
        shared.revoke_session("s1");
        shared.keepalive_input_action("s1");
        assert!(!shared.has_active_grant("s1"));
    }

    /// Regression: the T3 denylist must not contain duplicate entries
    /// ("支付"/"提交"/"清空"/"確認" each appeared twice).
    #[test]
    fn t3_denylist_has_no_duplicate_entries() {
        let mut seen = std::collections::HashSet::new();
        for term in T3_DENYLIST {
            assert!(seen.insert(*term), "duplicate denylist entry: {term}");
        }
    }

    /// Regression: revoking a session must also wipe that session's minted
    /// approval tokens — re-granting must not resurrect them.
    #[test]
    fn revoke_session_wipes_that_sessions_minted_tokens() {
        let shared = enabled_shared();
        let summary = "left click x1 at Some((100, 200))";
        let id = new_pending(&shared, "s1", summary);
        assert!(shared.mint_confirmation(&id));
        shared.revoke_session("s1");
        // Re-granting (the user changes their mind and grants again) must
        // not resurrect the wiped token.
        shared.grant_session("s1");
        assert_eq!(
            take(&shared, &id, "s1", summary),
            ConfirmationCheck::Unknown,
            "revoking a session must wipe its minted approval tokens"
        );
        // That session's pending confirmations are wiped too.
        let pending_id = new_pending(&shared, "s1", "left click");
        shared.revoke_session("s1");
        assert!(shared.pending_confirmation(&pending_id).is_none());
    }

    /// Regression: revoking one session must not touch another session's
    /// consent artifacts.
    #[test]
    fn revoke_session_spares_other_sessions_tokens() {
        let shared = enabled_shared();
        let summary = "left click x1 at Some((100, 200))";
        let other_id = new_pending(&shared, "s2", summary);
        assert!(shared.mint_confirmation(&other_id));
        shared.revoke_session("s1");
        assert_eq!(
            take(&shared, &other_id, "s2", summary),
            ConfirmationCheck::Granted,
            "revoking s1 must not touch s2's minted token"
        );
    }

    /// Approved tokens have no capacity cap (one pending per session bounds
    /// the mint rate naturally); a handful of live tokens coexist and each
    /// is spent exactly once.
    #[test]
    fn approved_tokens_have_no_cap_and_stay_single_use() {
        let shared = enabled_shared();
        let mut ids = Vec::new();
        for i in 0..12 {
            // A distinct session per pending mirrors real usage (one pending
            // per session at a time).
            let session = format!("s{i}");
            let id = new_pending(&shared, &session, "left click");
            assert!(shared.mint_confirmation(&id));
            ids.push((session, id));
        }
        for (session, id) in &ids {
            assert_eq!(
                take(&shared, id, session, "left click"),
                ConfirmationCheck::Granted
            );
            assert_eq!(
                take(&shared, id, session, "left click"),
                ConfirmationCheck::Unknown,
                "each token is single-use"
            );
        }
    }
}
