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

use super::backend::BackendRegistry;

/// 输入类会话授权的空闲超时：10 分钟无输入动作即失效，需重新授权。
pub const GRANT_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// 输入类动作滑动窗口速率上限：60 次/分钟。
pub const INPUT_RATE_LIMIT_PER_MINUTE: u32 = 60;
/// 观察类动作滑动窗口速率上限：screenshot/ui_tree/cursor 等共享 60 次/分钟
/// （评审发现：失控的截图循环可以无限制烧盘/烧 token/刷屏）。
pub const OBSERVE_RATE_LIMIT_PER_MINUTE: u32 = 60;
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
/// 单个会话的待决确认子上限：全局上限是跨会话共享资源，无子上限时一个
/// 失控会话可把全局队列填满，其他会话的确认通道被 DoS 掉约一个 TTL
/// （评审发现）。正常交互单会话同时只有一枚用户可见弹窗，10 已远超需要。
pub const MAX_PENDING_PER_SESSION: usize = 10;
/// 跨会话物理输入锁的有界等待上限。Input 动作从筛查到注入全程持锁（最长
/// 可达 190s×n），无限挂等会让其他会话静默卡死（评审发现）——超时显式报错，
/// 由模型自行等待重试。
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
    "支付",
    "傳送",
    "轉帳",
    "提交",
    "確認",
    "清空",
    "資源回收筒",
    "廢紙簍",
    "購入",
    "支払い",
    "送信",
    "削除",
    "送金",
    "提出",
    "注文",
    "確認",
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
    /// 观察类动作超过 60 次/分钟速率上限。
    ObserveRateLimited,
    /// 会话动作预算用尽，强制重新授权。
    BudgetExhausted,
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
            Self::RateLimited => {
                "input action rate limit exceeded (60/minute). Wait before issuing more input actions."
                    .to_string()
            }
            Self::ObserveRateLimited => {
                "observe action rate limit exceeded (60/minute shared by screenshot/ui_tree/cursor). \
                 Wait before issuing more observe actions."
                    .to_string()
            }
            Self::BudgetExhausted => {
                "the session input-action budget is exhausted. Ask the user to re-grant control to continue."
                    .to_string()
            }
            Self::InputBusy => {
                "another session is performing a physical input action. Wait and retry."
                    .to_string()
            }
        }
    }
}

struct SessionConsent {
    last_activity: Instant,
    input_actions: u64,
    recent_inputs: VecDeque<Instant>,
}

/// 观察类动作的滑动窗口记账（复用输入限速的 VecDeque<Instant> 结构），
/// 按会话分桶，全部观察动作共享一个窗口。
type ObserveWindows = HashMap<String, VecDeque<Instant>>;

/// 滑动窗口限速的共用内核：淘汰窗口外旧样本；窗口满返回 false。
fn sliding_window_admit(recent: &mut VecDeque<Instant>, limit: usize, now: Instant) -> bool {
    let window_start = now.checked_sub(Duration::from_secs(60)).unwrap_or(now);
    while recent.front().is_some_and(|t| *t < window_start) {
        recent.pop_front();
    }
    if recent.len() >= limit {
        return false;
    }
    recent.push_back(now);
    true
}

impl SessionConsent {
    fn new(now: Instant) -> Self {
        Self {
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

/// 已铸造的批准令牌：绑定会话、动作摘要与**用户批准时看到的元素标签**，
/// [`CONFIRM_TTL`] 内未消费即过期。
#[derive(Debug, Clone)]
struct ApprovedToken {
    session_id: String,
    action_summary: String,
    element_label: String,
    minted_at: Instant,
}

/// 消费批准令牌的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmationCheck {
    /// 令牌有效且与会话/动作匹配，已消费（单次有效）。携带用户批准时
    /// 看到的元素标签——工具层重筛时用它判定"眼前的目标还是用户批准
    /// 的那个"（评审发现：批准到重试之间目标可能被换内容）。
    Granted { approved_element_label: String },
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
    /// 观察类动作限速窗口（按会话分桶）。
    observe_windows: Mutex<ObserveWindows>,
    /// 物理鼠标/键盘是全局独占资源，但 backend 是每会话一条 worker——这把
    /// 进程级锁把**跨会话**的输入注入串行化（评审发现：两个并发会话可各持
    /// 有效授权交替打字/点击）。Input 类动作在筛查+执行全程持有。
    physical_input_lock: Mutex<()>,
    pending_confirmations: Mutex<HashMap<String, PendingConfirmation>>,
    approved_tokens: Mutex<HashMap<String, ApprovedToken>>,
    denied_confirmations: Mutex<HashMap<String, Instant>>,
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
            observe_windows: Mutex::new(HashMap::new()),
            physical_input_lock: Mutex::new(()),
            pending_confirmations: Mutex::new(HashMap::new()),
            approved_tokens: Mutex::new(HashMap::new()),
            denied_confirmations: Mutex::new(HashMap::new()),
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
        self.observe_windows.lock().clear();
        self.pending_confirmations.lock().clear();
        self.approved_tokens.lock().clear();
        self.denied_confirmations.lock().clear();
    }

    /// 用户重新开启后清除停止旗标（不恢复任何授权）。
    pub fn reset_stop(&self) {
        self.stop.store(false, Ordering::SeqCst);
    }

    /// 吊销全部会话授权并清空全部同意状态（待决确认、已铸令牌、拒绝记忆），
    /// 但**不置**停止旗标——与 [`Self::stop_all`] 的紧急停止语义区分：总开关
    /// 关闭不是急停，重新开启后不应残留停止状态
    /// （`computer_use_set_enabled(false)` 调用）。评审发现：此前关闭开关不清
    /// 授权与令牌，重开后旧 grant 在 10 分钟空闲窗口内、旧批准令牌在 5 分钟
    /// TTL 内仍然有效——开关关闭期间的同意状态在重开后不应存活。
    pub fn revoke_all_sessions(&self) {
        self.sessions.lock().clear();
        self.pending_confirmations.lock().clear();
        self.approved_tokens.lock().clear();
        self.denied_confirmations.lock().clear();
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
        let admitted = sliding_window_admit(
            &mut consent.recent_inputs,
            INPUT_RATE_LIMIT_PER_MINUTE as usize,
            now,
        );
        if !admitted {
            return Err(GuardRejection::RateLimited);
        }
        consent.input_actions += 1;
        consent.last_activity = now;
        Ok(())
    }

    /// 观察类动作门控 + 限速记账（screenshot/ui_tree/cursor 等共享 60 次/
    /// 分钟，按会话分桶；评审发现：观察类此前完全不限速，失控循环可无限
    /// 截图刷盘）。超限显式报错，由模型自行降速。
    pub fn begin_observe_action(&self, session_id: &str) -> Result<(), GuardRejection> {
        self.check_readonly()?;
        let now = Instant::now();
        let mut windows = self.observe_windows.lock();
        // 清扫早已停用会话的僵尸窗口（整个窗口都过期的桶）。
        let window_start = now.checked_sub(Duration::from_secs(60)).unwrap_or(now);
        windows.retain(|_, recent| recent.back().is_some_and(|t| *t >= window_start));
        let recent = windows.entry(session_id.to_string()).or_default();
        if !sliding_window_admit(recent, OBSERVE_RATE_LIMIT_PER_MINUTE as usize, now) {
            return Err(GuardRejection::ObserveRateLimited);
        }
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
    /// 获取改为**有界等待**（`try_lock_for`，见 [`PHYSICAL_INPUT_LOCK_TIMEOUT`]）：
    /// 锁被其他会话持有时超时返回显式 [`GuardRejection::InputBusy`]，不再无限
    /// 挂等（评审发现）。锁语义（Input 动作筛查到注入全程持有）与释放路径
    /// （guard drop）不变。
    pub fn lock_physical_input(&self) -> Result<parking_lot::MutexGuard<'_, ()>, GuardRejection> {
        self.physical_input_lock
            .try_lock_for(PHYSICAL_INPUT_LOCK_TIMEOUT)
            .ok_or(GuardRejection::InputBusy)
    }

    /// 注册一个等待用户决定的 T3 确认，返回 confirm_id；存量已达
    /// [`MAX_PENDING_CONFIRMATIONS`] 时返回 `None`（**拒绝新 pending**）。
    /// 顺带清扫过期 pending。评审发现：此前的「超限逐出最旧」会把用户正在
    /// 等待答复的确认弹窗逐掉——失控模型以 60 次/分钟发起被拦动作，约
    /// 100 秒即可把用户的 pending 挤出，用户点批准得到「不存在」。容量满时
    /// 宁可让发起方拿到显式错误（fail-closed）也不能丢用户正在看的弹窗。
    /// 另有每会话子上限 [`MAX_PENDING_PER_SESSION`]：全局上限是跨会话资源，
    /// 无子上限时单个失控会话 100 秒即可填满全局队列，把**其他**会话的
    /// 确认通道 DoS 掉约一个 TTL（评审发现）。
    pub fn new_pending_confirmation(
        &self,
        session_id: &str,
        action_summary: impl Into<String>,
        element_label: impl Into<String>,
    ) -> Option<String> {
        let confirm_id = format!("cu-{:016x}", rand::random::<u64>());
        let now = Instant::now();
        let mut pending = self.pending_confirmations.lock();
        pending.retain(|_, entry| now.duration_since(entry.created_at) <= CONFIRM_TTL);
        if pending.len() >= MAX_PENDING_CONFIRMATIONS {
            return None;
        }
        if pending
            .values()
            .filter(|entry| entry.session_id == session_id)
            .count()
            >= MAX_PENDING_PER_SESSION
        {
            return None;
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
        Some(confirm_id)
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
    /// 未知 id 返回 false 且**不写** denied 表——模型自造/过期的 id 不应能向
    /// 拒绝记忆投毒，把未来合法的 confirm_id 变成「已被拒绝」（评审发现）。
    pub fn deny_confirmation(&self, confirm_id: &str) -> bool {
        let removed = self.pending_confirmations.lock().remove(confirm_id);
        if removed.is_none() {
            return false;
        }
        let now = Instant::now();
        let mut denied = self.denied_confirmations.lock();
        denied.retain(|_, at| now.duration_since(*at) <= DENIED_TTL);
        denied.insert(confirm_id.to_string(), now);
        true
    }

    /// 铸造批准令牌。只能由 `computer_use_confirm` Tauri 命令调用——绝不能让
    /// 模型经工具调用自己铸造。只为存在且未过期的 pending 铸币并返回 `true`；
    /// pending 不存在/已过期/已被决定时返回 `false`——静默 no-op 会让前端把
    /// 失败显示为成功（评审发现）。令牌继承该 pending 的会话与动作摘要
    /// （消费时逐项比对）。
    pub fn mint_confirmation(&self, confirm_id: &str) -> bool {
        let entry = self.pending_confirmations.lock().remove(confirm_id);
        let Some(entry) = entry else {
            return false;
        };
        if entry.created_at.elapsed() > CONFIRM_TTL {
            return false;
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
                element_label: entry.element_label,
                minted_at: now,
            },
        );
        true
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
        let approved_element_label = token.element_label.clone();
        tokens.remove(confirm_id);
        ConfirmationCheck::Granted {
            approved_element_label,
        }
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
        let id = shared
            .new_pending_confirmation("s1", summary, "Buy now")
            .expect("pending below cap");
        let pending = shared.pending_confirmation(&id);
        assert!(pending.as_ref().is_some_and(|p| p.session_id == "s1"));
        // 未铸造前不可消费。
        assert_eq!(
            shared.take_confirmation(&id, "s1", summary),
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
            ConfirmationCheck::Granted {
                approved_element_label: "Buy now".to_string()
            }
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
        let id = shared
            .new_pending_confirmation("s1", "left click", "Buy now")
            .expect("pending below cap");
        assert!(shared.deny_confirmation(&id));
        // deny 清除 pending：不能再为它铸币（mint 返回 false，不再静默 no-op）。
        assert!(shared.pending_confirmation(&id).is_none());
        assert!(!shared.mint_confirmation(&id));
        assert_eq!(
            shared.take_confirmation(&id, "s1", "left click"),
            ConfirmationCheck::Denied,
            "denied confirm_id must report Denied, not Unknown"
        );
        // 未知 id 的 deny 失败，且**不写** denied 表（评审修正：自造 id 不得
        // 向拒绝记忆投毒，否则未来合法 id 会被误报「已被拒绝」）。
        assert!(!shared.deny_confirmation("cu-unknown"));
        assert_eq!(
            shared.take_confirmation("cu-unknown", "s1", "x"),
            ConfirmationCheck::Unknown,
            "unknown id must stay Unknown, not Denied"
        );
    }

    /// 评审修复回归：观察类动作共享 60 次/分钟限速，超限显式报错。
    #[test]
    fn observe_actions_share_a_rate_limit_window() {
        let shared = enabled_shared();
        for _ in 0..OBSERVE_RATE_LIMIT_PER_MINUTE {
            assert!(shared.begin_observe_action("s1").is_ok());
        }
        assert_eq!(
            shared.begin_observe_action("s1"),
            Err(GuardRejection::ObserveRateLimited)
        );
        assert!(
            GuardRejection::ObserveRateLimited
                .message()
                .contains("observe action rate limit")
        );
        // 其他会话独立分桶，不受 s1 拖累。
        assert!(shared.begin_observe_action("s2").is_ok());
        // 急停清空观察窗口。
        shared.stop_all();
        shared.reset_stop();
        assert!(
            shared.observe_windows.lock().is_empty(),
            "stop_all must clear observe windows"
        );
    }

    /// 评审修复回归：pending 存量达上限时**拒绝新请求**（返回 None）而不是
    /// 逐出最旧——逐出会把用户正在等待的确认弹窗挤掉（第三轮评审发现）。
    /// 另有每会话子上限（评审修复回归）：全局上限是跨会话资源，无子上限时
    /// 单个失控会话即可把全局队列填满，其他会话的确认通道被 DoS 掉约一个
    /// TTL。
    #[test]
    fn pending_map_rejects_new_requests_when_full() {
        let shared = enabled_shared();
        // 每会话子上限：s1 第 MAX_PENDING_PER_SESSION+1 个被拒；s2 预算独立。
        for i in 0..MAX_PENDING_PER_SESSION {
            let id = shared.new_pending_confirmation("s1", format!("action {i}"), "Buy now");
            assert!(id.is_some(), "request {i} must be admitted below the cap");
        }
        assert!(
            shared
                .new_pending_confirmation("s1", "one more", "Buy now")
                .is_none(),
            "a session at its per-session cap must be rejected"
        );
        assert!(
            shared
                .new_pending_confirmation("s2", "other session", "Buy now")
                .is_some(),
            "another session must keep its own confirmation budget"
        );
        // 已有令牌存量上限照旧（铸造路径仍然逐出最旧：丢弃一个未消费的
        // 旧令牌不影响用户正在看的弹窗）。铸造会移除 pending，用独立会话
        // 循环避免被上面的子上限/全局上限卡住。
        for i in 0..(MAX_APPROVED_TOKENS + 20) {
            let id = shared.new_pending_confirmation(&format!("t{i}"), format!("m {i}"), "Buy now");
            if let Some(id) = id {
                let _ = shared.mint_confirmation(&id);
            }
        }
        assert_eq!(
            shared.approved_tokens.lock().len(),
            MAX_APPROVED_TOKENS,
            "token cap is enforced by evicting the oldest"
        );
        // 全局上限：跨多个会话填满（每会话至多 MAX_PENDING_PER_SESSION 个），
        // 之后新请求被显式拒绝。存活 pending：s1×10 + s2×1（t* 已被铸造移除）。
        let mut admitted = MAX_PENDING_PER_SESSION + 1;
        'fill: for s in 3.. {
            let session = format!("s{s}");
            for _ in 0..MAX_PENDING_PER_SESSION {
                match shared.new_pending_confirmation(&session, "fill", "Buy now") {
                    Some(_) => admitted += 1,
                    None => break 'fill,
                }
            }
        }
        assert_eq!(
            admitted, MAX_PENDING_CONFIRMATIONS,
            "global cap must bind exactly across per-session caps"
        );
        assert!(
            shared
                .new_pending_confirmation("s-final", "one more", "Buy now")
                .is_none(),
            "a full queue must reject new pending requests"
        );
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
    /// 已铸令牌、拒绝记忆），但**不置**停止旗标（与 stop_all 语义区分）——
    /// 重开后旧 grant 不得在 10 分钟窗口内复活，disable→enable 循环里旧的
    /// 已铸令牌也不得被免确认重放（第三轮评审发现）。
    #[test]
    fn revoke_all_sessions_clears_grants_and_pendings_without_stop_flag() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        let confirm_id = shared
            .new_pending_confirmation("s1", "left click", "Buy now")
            .expect("pending below cap");
        let token_id = shared
            .new_pending_confirmation("s1", "left click 2", "Buy now")
            .expect("pending below cap");
        assert!(shared.mint_confirmation(&token_id));
        shared.revoke_all_sessions();
        assert!(!shared.has_active_grant("s1"));
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::GrantRequired)
        );
        assert!(shared.pending_confirmation(&confirm_id).is_none());
        // 已铸令牌一并清除：重新开启+重新授权后不能拿旧令牌免确认重放。
        assert_eq!(
            shared.take_confirmation(&token_id, "s1", "left click 2"),
            ConfirmationCheck::Unknown,
            "a disabled cycle must wipe minted approval tokens"
        );
        // 与 stop_all 的语义区分：停止旗标不被置位，观察类动作仍可用。
        assert!(!shared.is_stopped());
        assert!(shared.begin_observe_action("s1").is_ok());
    }

    #[test]
    fn pending_confirmation_expires_after_ttl() {
        let shared = enabled_shared();
        let id = shared
            .new_pending_confirmation("s1", "left_click (100,200)", "Buy now")
            .expect("pending below cap");
        // 手工把 created_at 拨回 TTL 之前（等真实 5 分钟太慢）。
        {
            let mut pending = shared.pending_confirmations.lock();
            if let Some(entry) = pending.get_mut(&id) {
                entry.created_at = Instant::now() - CONFIRM_TTL - Duration::from_secs(1);
            }
        }
        // 过期即视为不存在。
        assert!(shared.pending_confirmation(&id).is_none());
        // 过期 pending 不再铸币：令牌不可用，且 mint 显式报告失败。
        assert!(!shared.mint_confirmation(&id));
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
