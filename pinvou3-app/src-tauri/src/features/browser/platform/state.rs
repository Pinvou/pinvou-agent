//! 原生浏览器宿主的纯状态机。
//!
//! 这里不依赖具体 WebView 内核：标签双射、控制权 lease 与请求 tombstone 可以在
//! Windows/macOS/Linux 上使用同一套规则，并由纯单元测试覆盖。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_TERMINAL_REQUESTS: usize = 2048;
const MAX_REQUEST_RECORDS: usize = 4096;
const AGENT_INPUT_WINDOW: Duration = Duration::from_millis(750);
/// A begun operation is authoritative only while its owner is demonstrably
/// alive. Hosted BrowserCore requests have a 25 second outer budget; the
/// additional margin covers scheduling and durable cancellation cleanup.
/// Windows renews this deadline while its upstream MCP call is in flight.
const AGENT_OPERATION_WINDOW: Duration = Duration::from_secs(30);
/// WebKit may deliver the navigation-delegate callback created by the page's
/// trusted-input takeover listener one run-loop turn after the native
/// responder method returns. The operation itself ends immediately; this
/// short callback grace suppresses only that already-dispatched event.
const POST_DISPATCH_CALLBACK_GRACE: Duration = Duration::from_millis(100);

#[derive(Clone, Debug)]
pub(super) struct SurfaceEntry {
    pub(super) label: String,
    pub(super) token: String,
    /// Agent-facing BrowserCore page id. It is process-local, monotonically allocated and never
    /// reused, so closing an earlier tab cannot retarget a stale tool call to a later tab.
    pub(super) page_id: u64,
    pub(super) automation_target: Option<String>,
    /// 仅 Agent create_tab 设置，用于 request tombstone/后续失败的精确补偿。
    /// 普通用户标签与重启恢复标签没有 creation generation。
    pub(super) created_by_request_id: Option<String>,
    /// Agent create_tab 在 target 发现和首航完成前保持 false。此时页面回调不得
    /// 发布事件、改变控制权或派生 popup。
    pub(super) published: Arc<AtomicBool>,
    /// Agent create_tab 成功提交后的 control revision。晚到取消只能在 owner /
    /// revision 仍是这个 generation 时回滚。
    pub(super) created_at_revision: Option<u64>,
}

impl SurfaceEntry {
    pub(super) fn is_published(&self) -> bool {
        self.published.load(Ordering::SeqCst)
    }

    pub(super) fn publish(&self) {
        self.published.store(true, Ordering::SeqCst);
    }

    pub(super) fn unpublish(&self) {
        self.published.store(false, Ordering::SeqCst);
    }
}

/// 宿主权威的 tabToken ↔ WebView label 双射。
///
/// 页面主世界里的 marker 只用于 CDP 初次发现，不能修改这里的归属关系。
#[derive(Default)]
pub(super) struct TabRegistry {
    entries: Vec<SurfaceEntry>,
}

impl TabRegistry {
    pub(super) fn from_entry(entry: SurfaceEntry) -> Self {
        Self {
            entries: vec![entry],
        }
    }

    pub(super) fn insert(&mut self, entry: SurfaceEntry) -> Result<(), String> {
        if self.by_token(&entry.token).is_some() {
            return Err("浏览器标签 token 已被占用".to_string());
        }
        if self.token_for_label(&entry.label).is_some() {
            return Err("浏览器 WebView 已绑定其他标签".to_string());
        }
        if self.token_for_page_id(entry.page_id).is_some() {
            return Err("浏览器 pageId 已绑定其他标签".to_string());
        }
        self.entries.push(entry);
        Ok(())
    }

    pub(super) fn by_token(&self, token: &str) -> Option<&SurfaceEntry> {
        self.entries.iter().find(|entry| entry.token == token)
    }

    pub(super) fn by_token_mut(&mut self, token: &str) -> Option<&mut SurfaceEntry> {
        self.entries.iter_mut().find(|entry| entry.token == token)
    }

    pub(super) fn token_for_label(&self, label: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.label == label)
            .map(|entry| entry.token.as_str())
    }

    pub(super) fn token_for_page_id(&self, page_id: u64) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.page_id == page_id)
            .map(|entry| entry.token.as_str())
    }

    pub(super) fn target_for_token(&self, token: &str) -> Option<&str> {
        self.by_token(token)?.automation_target.as_deref()
    }

    pub(super) fn token_for_target(&self, target: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.automation_target.as_deref() == Some(target))
            .map(|entry| entry.token.as_str())
    }

    pub(super) fn bind_target(&mut self, token: &str, target: &str) -> Result<(), String> {
        if let Some(bound_token) = self.token_for_target(target) {
            if bound_token != token {
                return Err("自动化 target 已绑定其他标签".to_string());
            }
        }
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.token == token)
            .ok_or_else(|| "标签页不存在或不属于当前对话".to_string())?;
        if entry
            .automation_target
            .as_deref()
            .is_some_and(|current| current != target)
        {
            return Err("标签页已绑定其他自动化 target".to_string());
        }
        entry.automation_target = Some(target.to_string());
        Ok(())
    }

    pub(super) fn remove_token(&mut self, token: &str) -> Option<(usize, SurfaceEntry)> {
        let index = self.entries.iter().position(|entry| entry.token == token)?;
        Some((index, self.entries.remove(index)))
    }

    pub(super) fn token_at(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(|entry| entry.token.as_str())
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &SurfaceEntry> {
        self.entries.iter()
    }

    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum NativeControlOwner {
    /// 进程重启后刚恢复的页面尚未发生新的用户或 Agent 操作。它不是 Agent lease，
    /// 也不能被 UI 误报成“用户已接管”；谁先通过宿主提交真实操作，谁取得控制权。
    Unclaimed,
    Agent,
    User,
}

impl NativeControlOwner {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Unclaimed => "unclaimed",
            Self::Agent => "agent",
            Self::User => "user",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlSnapshot {
    pub(crate) revision: u64,
    pub(crate) owner: NativeControlOwner,
}

/// Process incarnation that owns one hosted Agent operation. The PID alone is
/// insufficient because operating systems may reuse it after the wrapper
/// exits; the per-process random nonce makes that reuse fail closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentCallerEpoch {
    caller_pid: u32,
    wrapper_instance_nonce: String,
}

impl AgentCallerEpoch {
    pub(crate) fn new(
        caller_pid: u32,
        wrapper_instance_nonce: impl Into<String>,
    ) -> Result<Self, String> {
        let wrapper_instance_nonce = wrapper_instance_nonce.into();
        if caller_pid == 0
            || wrapper_instance_nonce.len() != 32
            || !wrapper_instance_nonce
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("browser/invalid-caller-epoch".to_string());
        }
        Ok(Self {
            caller_pid,
            wrapper_instance_nonce,
        })
    }

    pub(crate) const fn caller_pid(&self) -> u32 {
        self.caller_pid
    }

    pub(crate) fn wrapper_instance_nonce(&self) -> &str {
        &self.wrapper_instance_nonce
    }
}

/// Exact retained authorization for one popup observed synchronously inside
/// an Agent dispatch. This value never enters page/React state. Its opaque
/// holder id makes release idempotent and prevents a duplicate popup cleanup
/// from consuming the upstream operation's hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RetainedAgentOperation {
    authorization: NativeTabLease,
    caller_epoch: AgentCallerEpoch,
    holder_id: u64,
}

impl RetainedAgentOperation {
    pub(crate) fn authorization(&self) -> &NativeTabLease {
        &self.authorization
    }

    pub(crate) fn caller_epoch(&self) -> &AgentCallerEpoch {
        &self.caller_epoch
    }
}

pub(super) struct WorkspaceControl {
    state: parking_lot::Mutex<ControlState>,
}

struct ControlState {
    snapshot: ControlSnapshot,
    active_lease: Option<String>,
    active_lease_expires_at: Option<Instant>,
    /// 当前已经通过宿主 lease 校验并进入 dispatch 临界区的完整授权。popup 回调
    /// 只能复制这份 Rust 内部授权，不能根据一个短期 bool 猜测 Agent 所有权。
    active_agent_operation: Option<ActiveAgentOperation>,
    agent_input_until: Option<Instant>,
}

#[derive(Debug, Clone)]
struct ActiveAgentOperation {
    lease: NativeTabLease,
    caller_epoch: AgentCallerEpoch,
    expires_at: Instant,
    /// The upstream tool has a distinct hold from retained popups. Keeping it
    /// explicit makes duplicate End and popup cleanup idempotent, and ensures
    /// popup completion cannot shorten a still-running trusted-input window.
    upstream_active: bool,
    popup_holders: HashSet<u64>,
    next_popup_holder_id: u64,
}

impl ControlState {
    fn clear_expired_authorization(&mut self, now: Instant) {
        if self
            .active_lease_expires_at
            .is_some_and(|deadline| deadline <= now)
            || self
                .active_agent_operation
                .as_ref()
                .is_some_and(|operation| operation.expires_at <= now)
        {
            self.active_lease = None;
            self.active_lease_expires_at = None;
            self.active_agent_operation = None;
            self.agent_input_until = None;
        }
    }

    fn active_operation_matches(&self, lease: &NativeTabLease) -> bool {
        self.active_agent_operation
            .as_ref()
            .is_some_and(|operation| operation.lease == *lease)
    }

    fn refresh_agent_operation(&mut self, lease: &NativeTabLease, now: Instant) -> bool {
        self.clear_expired_authorization(now);
        if lease.owner != NativeControlOwner::Agent
            || self.snapshot.owner != NativeControlOwner::Agent
            || self.snapshot.revision != lease.revision
            || self.active_lease.as_deref() != Some(lease.lease.as_str())
            || !self.active_operation_matches(lease)
        {
            return false;
        }
        if let Some(operation) = self.active_agent_operation.as_mut() {
            operation.expires_at = now + AGENT_OPERATION_WINDOW;
        }
        self.active_lease_expires_at = Some(now + AGENT_OPERATION_WINDOW);
        true
    }
}

impl WorkspaceControl {
    pub(super) fn new(revision: u64, owner: NativeControlOwner) -> Self {
        Self {
            state: parking_lot::Mutex::new(ControlState {
                snapshot: ControlSnapshot { revision, owner },
                active_lease: None,
                active_lease_expires_at: None,
                active_agent_operation: None,
                agent_input_until: None,
            }),
        }
    }

    pub(super) fn snapshot(&self) -> ControlSnapshot {
        self.state.lock().snapshot
    }

    pub(super) fn bump(&self, owner: Option<NativeControlOwner>) -> ControlSnapshot {
        let mut state = self.state.lock();
        state.snapshot.revision = state.snapshot.revision.saturating_add(1);
        if let Some(owner) = owner {
            state.snapshot.owner = owner;
        }
        state.active_lease = None;
        state.active_lease_expires_at = None;
        state.active_agent_operation = None;
        state.agent_input_until = None;
        state.snapshot
    }

    /// A normal document navigation is part of an already-begun Agent tool when
    /// the exact active operation is still valid. In that case the navigation
    /// callback must not revoke its own lease before the platform/upstream
    /// dispatch returns. The check and optional revision bump share this lock so
    /// a newly-begun operation cannot appear between them. Real user takeover
    /// calls `bump(User)` first, clears the operation, and therefore still wins.
    pub(super) fn bump_for_navigation_if_no_active_agent_operation(
        &self,
    ) -> Option<ControlSnapshot> {
        let mut state = self.state.lock();
        state.clear_expired_authorization(Instant::now());
        let has_current_agent_operation =
            state
                .active_agent_operation
                .as_ref()
                .is_some_and(|operation| {
                    operation.lease.owner == NativeControlOwner::Agent
                        && state.snapshot.owner == NativeControlOwner::Agent
                        && state.snapshot.revision == operation.lease.revision
                        && state.active_lease.as_deref() == Some(operation.lease.lease.as_str())
                });
        if has_current_agent_operation {
            return None;
        }
        state.snapshot.revision = state.snapshot.revision.saturating_add(1);
        state.active_lease = None;
        state.active_lease_expires_at = None;
        state.active_agent_operation = None;
        state.agent_input_until = None;
        Some(state.snapshot)
    }

    /// 用户停止操作后的自动交还只能提交仍对应同一次接管的 revision。
    /// 任何新的用户动作、标签切换或 Agent 显式 hand-back 都会前进 revision，
    /// 让旧定时器静默失效，避免迟到任务覆盖更新后的控制权。
    pub(super) fn release_user_control_if_unchanged(
        &self,
        expected_revision: u64,
    ) -> Option<ControlSnapshot> {
        let mut state = self.state.lock();
        if state.snapshot.owner != NativeControlOwner::User
            || state.snapshot.revision != expected_revision
        {
            return None;
        }
        state.snapshot.revision = state.snapshot.revision.saturating_add(1);
        state.snapshot.owner = NativeControlOwner::Agent;
        state.active_lease = None;
        state.active_lease_expires_at = None;
        state.active_agent_operation = None;
        state.agent_input_until = None;
        Some(state.snapshot)
    }

    pub(super) fn issue_agent_lease(&self) -> (ControlSnapshot, String) {
        self.issue_agent_lease_if_allowed(true)
            .expect("显式授权路径必须能签发 Agent lease")
    }

    /// 在同一控制锁内判断 User owner 与签发新 lease，消除“先看 owner、再 issue”
    /// 之间用户接管会被覆盖的 TOCTOU。只有 UI 的显式 hand-back 可传 true。
    pub(super) fn issue_agent_lease_if_allowed(
        &self,
        explicit_user_handback: bool,
    ) -> Option<(ControlSnapshot, String)> {
        self.issue_agent_lease_with(explicit_user_handback, |_| Ok(()))
            .expect("空的 Agent activation mutation 不会失败")
            .map(|(snapshot, lease, ())| (snapshot, lease))
    }

    /// owner 复核、宿主 active-tab mutation 与新 lease 签发共用同一临界区。
    /// 用户接管先提交时 closure 完全不执行；Agent 先提交时，随后用户接管会成为
    /// 最后状态且不会被迟到的 issue 覆盖。
    pub(super) fn issue_agent_lease_with<T>(
        &self,
        explicit_user_handback: bool,
        mutation: impl FnOnce(u64) -> Result<T, String>,
    ) -> Result<Option<(ControlSnapshot, String, T)>, String> {
        let mut state = self.state.lock();
        if state.snapshot.owner == NativeControlOwner::User && !explicit_user_handback {
            return Ok(None);
        }
        let committed_revision = state.snapshot.revision.saturating_add(1);
        let output = mutation(committed_revision)?;
        state.snapshot.revision = committed_revision;
        state.snapshot.owner = NativeControlOwner::Agent;
        state.agent_input_until = None;
        let lease = format!(
            "{:016x}{:016x}",
            rand::random::<u64>(),
            rand::random::<u64>()
        );
        state.active_lease = Some(lease.clone());
        state.active_lease_expires_at = Some(Instant::now() + AGENT_OPERATION_WINDOW);
        state.active_agent_operation = None;
        Ok(Some((state.snapshot, lease, output)))
    }

    pub(super) fn assert_agent_lease(&self, revision: u64, lease: &str) -> bool {
        let mut state = self.state.lock();
        state.clear_expired_authorization(Instant::now());
        state.snapshot.owner == NativeControlOwner::Agent
            && state.snapshot.revision == revision
            && state.active_lease.as_deref() == Some(lease)
    }

    /// Agent 对标签注册表做 create/close 等 mutation 时的线性化提交点。校验与
    /// revision 前进在同一把锁内完成；若用户接管先获得锁，本调用只返回 false，
    /// 绝不把 owner 改回 Agent。调用方在成功后也不得再无条件 bump Agent。
    pub(super) fn commit_agent_mutation<T>(
        &self,
        authorization: &NativeTabLease,
        mutation: impl FnOnce() -> Result<T, String>,
    ) -> Result<Option<(ControlSnapshot, T)>, String> {
        let mut state = self.state.lock();
        state.clear_expired_authorization(Instant::now());
        if authorization.owner != NativeControlOwner::Agent
            || state.snapshot.owner != NativeControlOwner::Agent
            || state.snapshot.revision != authorization.revision
            || state.active_lease.as_deref() != Some(authorization.lease.as_str())
            || !state.active_operation_matches(authorization)
        {
            return Ok(None);
        }
        let output = mutation()?;
        state.snapshot.revision = state.snapshot.revision.saturating_add(1);
        state.active_lease = None;
        state.active_lease_expires_at = None;
        state.active_agent_operation = None;
        state.agent_input_until = None;
        Ok(Some((state.snapshot, output)))
    }

    /// 精确回滚一个已提交的 Agent creation generation。create 成功会撤销旧
    /// lease，因此补偿只比较宿主提交时记录的 owner/revision；用户接管或任意后续
    /// mutation 都会让 CAS 返回 None，保留用户正在使用的页面。
    pub(super) fn commit_agent_generation_rollback<T>(
        &self,
        expected_revision: u64,
        mutation: impl FnOnce() -> Result<T, String>,
    ) -> Result<Option<(ControlSnapshot, T)>, String> {
        let mut state = self.state.lock();
        if state.snapshot.owner != NativeControlOwner::Agent
            || state.snapshot.revision != expected_revision
        {
            return Ok(None);
        }
        let output = mutation()?;
        state.snapshot.revision = state.snapshot.revision.saturating_add(1);
        state.active_lease = None;
        state.active_lease_expires_at = None;
        state.active_agent_operation = None;
        state.agent_input_until = None;
        Ok(Some((state.snapshot, output)))
    }

    /// Revoke one acknowledged-lost Agent tab activation without converting
    /// it into a synthetic User takeover. The previous owner/tab are restored
    /// only while the exact committed activation revision is still current;
    /// any real user or later Agent mutation wins and makes this a no-op.
    pub(super) fn rollback_agent_activation<T>(
        &self,
        expected_revision: u64,
        previous_owner: NativeControlOwner,
        mutation: impl FnOnce(u64) -> Result<T, String>,
    ) -> Result<Option<(ControlSnapshot, T)>, String> {
        let mut state = self.state.lock();
        state.clear_expired_authorization(Instant::now());
        if state.snapshot.owner != NativeControlOwner::Agent
            || state.snapshot.revision != expected_revision
        {
            return Ok(None);
        }
        let rollback_revision = expected_revision.saturating_add(1);
        let output = mutation(rollback_revision)?;
        state.snapshot.revision = rollback_revision;
        state.snapshot.owner = previous_owner;
        state.active_lease = None;
        state.active_lease_expires_at = None;
        state.active_agent_operation = None;
        state.agent_input_until = None;
        Ok(Some((state.snapshot, output)))
    }

    /// 标记一次已 begin 的原子 dispatch。所有浏览器工具都会登记完整授权；只有会
    /// 产生 trusted input 的工具额外打开 750ms 输入抑制保险窗。登记与 lease 复核
    /// 在同一控制锁内完成，因此 popup 回调不会把已经被用户接管的旧 lease 当真。
    pub(super) fn begin_agent_operation_for_caller(
        &self,
        lease: &NativeTabLease,
        emits_trusted_input: bool,
        caller_epoch: AgentCallerEpoch,
    ) -> bool {
        let mut state = self.state.lock();
        state.clear_expired_authorization(Instant::now());
        if lease.owner != NativeControlOwner::Agent
            || state.snapshot.owner != NativeControlOwner::Agent
            || state.snapshot.revision != lease.revision
            || state.active_lease.as_deref() != Some(lease.lease.as_str())
        {
            return false;
        }
        let now = Instant::now();
        if let Some(operation) = state.active_agent_operation.as_mut() {
            if operation.lease != *lease
                || operation.caller_epoch != caller_epoch
                || !operation.upstream_active
            {
                return false;
            }
            // A lost Begin ACK may make the wrapper repeat the exact same
            // idempotent control request. It must refresh, not add a holder;
            // the one matching End still closes the operation.
            operation.expires_at = now + AGENT_OPERATION_WINDOW;
            state.active_lease_expires_at = Some(now + AGENT_OPERATION_WINDOW);
            if emits_trusted_input {
                state.agent_input_until = Some(now + AGENT_INPUT_WINDOW);
            }
            return true;
        }
        state.active_agent_operation = Some(ActiveAgentOperation {
            lease: lease.clone(),
            caller_epoch,
            expires_at: now + AGENT_OPERATION_WINDOW,
            upstream_active: true,
            popup_holders: HashSet::new(),
            next_popup_holder_id: 1,
        });
        state.active_lease_expires_at = Some(now + AGENT_OPERATION_WINDOW);
        state.agent_input_until = emits_trusted_input.then(|| now + AGENT_INPUT_WINDOW);
        true
    }

    /// Unit-level platform tests do not have a real wrapper process. Production
    /// callers must use [`Self::begin_agent_operation_for_caller`] so every
    /// retained popup carries a validated process incarnation.
    #[cfg(test)]
    pub(super) fn begin_agent_operation(
        &self,
        lease: &NativeTabLease,
        emits_trusted_input: bool,
    ) -> bool {
        self.begin_agent_operation_for_caller(
            lease,
            emits_trusted_input,
            AgentCallerEpoch::new(1, "00000000000000000000000000000000")
                .expect("test caller epoch is valid"),
        )
    }

    /// Retain the exact begun operation for a popup that was synchronously
    /// observed by the host callback. Binding the replacement task-owned
    /// WebView is asynchronous, so the upstream tool may return before the
    /// popup reaches its final mutation CAS. Retention keeps only that already
    /// authorized operation alive; user takeover and the hard TTL still clear
    /// every holder atomically.
    pub(super) fn retain_agent_operation_for_popup(
        &self,
        session_id: &str,
        source_tab_token: &str,
    ) -> Option<RetainedAgentOperation> {
        let mut state = self.state.lock();
        let now = Instant::now();
        state.clear_expired_authorization(now);
        let snapshot = state.snapshot;
        let active_lease = state.active_lease.clone();
        let operation = state.active_agent_operation.as_mut()?;
        if operation.lease.owner != NativeControlOwner::Agent
            || !operation.upstream_active
            || snapshot.owner != NativeControlOwner::Agent
            || snapshot.revision != operation.lease.revision
            || active_lease.as_deref() != Some(operation.lease.lease.as_str())
            || operation.lease.session_id != session_id
            || operation.lease.tab_token != source_tab_token
        {
            return None;
        }
        let holder_id = operation.next_popup_holder_id;
        operation.next_popup_holder_id = operation.next_popup_holder_id.checked_add(1)?;
        if !operation.popup_holders.insert(holder_id) {
            return None;
        }
        operation.expires_at = now + AGENT_OPERATION_WINDOW;
        let retained = RetainedAgentOperation {
            authorization: operation.lease.clone(),
            caller_epoch: operation.caller_epoch.clone(),
            holder_id,
        };
        state.active_lease_expires_at = Some(now + AGENT_OPERATION_WINDOW);
        Some(retained)
    }

    /// popup 回调只在一个尚未结束、且控制权/lease 仍完全相同的 dispatch 内取得
    /// Agent 授权。返回的是 Rust 内存中的不透明 lease，不会暴露到页面或 React。
    pub(super) fn active_agent_operation(&self) -> Option<NativeTabLease> {
        let mut state = self.state.lock();
        state.clear_expired_authorization(Instant::now());
        let operation = state.active_agent_operation.as_ref()?;
        (state.snapshot.owner == NativeControlOwner::Agent
            && state.snapshot.revision == operation.lease.revision
            && state.active_lease.as_deref() == Some(operation.lease.lease.as_str()))
        .then(|| operation.lease.clone())
    }

    /// Revalidate one exact retained popup holder, including its originating
    /// wrapper incarnation. This is intentionally stronger than validating the
    /// shared lease because sibling popups may coexist under that lease.
    pub(super) fn authorize_retained_agent_operation(
        &self,
        retained: &RetainedAgentOperation,
    ) -> bool {
        let mut state = self.state.lock();
        state.clear_expired_authorization(Instant::now());
        let Some(operation) = state.active_agent_operation.as_ref() else {
            return false;
        };
        state.snapshot.owner == NativeControlOwner::Agent
            && state.snapshot.revision == retained.authorization.revision
            && state.active_lease.as_deref() == Some(retained.authorization.lease.as_str())
            && operation.lease == retained.authorization
            && operation.caller_epoch == retained.caller_epoch
            && operation.popup_holders.contains(&retained.holder_id)
    }

    /// Revalidate the exact operation authorization immediately before a
    /// platform dispatch that does not emit any takeover-observed event. This
    /// keeps stale operations fail-closed without opening a temporal window
    /// that could suppress an unrelated real user pointer/key/wheel event.
    pub(super) fn authorize_agent_dispatch(&self, lease: &NativeTabLease) -> bool {
        let mut state = self.state.lock();
        state.clear_expired_authorization(Instant::now());
        lease.owner == NativeControlOwner::Agent
            && state.snapshot.owner == NativeControlOwner::Agent
            && state.snapshot.revision == lease.revision
            && state.active_lease.as_deref() == Some(lease.lease.as_str())
            && state.active_operation_matches(lease)
    }

    /// Renew only the liveness deadline for one exact begun operation. Unlike
    /// `refresh_agent_input_window`, this never suppresses real user input and
    /// is therefore safe for long read-only/navigation/evaluate calls.
    pub(super) fn refresh_agent_operation(&self, lease: &NativeTabLease) -> bool {
        self.state
            .lock()
            .refresh_agent_operation(lease, Instant::now())
    }

    /// Restart the bounded trusted-input provenance window immediately before
    /// a native platform event is dispatched. The full active operation,
    /// opaque lease, owner, and revision are checked atomically; a stale or
    /// forged caller cannot extend the window after user takeover.
    pub(super) fn refresh_agent_input_window(&self, lease: &NativeTabLease) -> bool {
        let mut state = self.state.lock();
        let now = Instant::now();
        if !state.refresh_agent_operation(lease, now) {
            return false;
        }
        state.agent_input_until = Some(now + AGENT_INPUT_WINDOW);
        true
    }

    pub(super) fn end_agent_operation(&self, lease: &NativeTabLease) {
        let mut state = self.state.lock();
        let ended = state
            .active_agent_operation
            .as_mut()
            .is_some_and(|operation| {
                if operation.lease != *lease || !operation.upstream_active {
                    return false;
                }
                operation.upstream_active = false;
                true
            });
        if ended {
            let now = Instant::now();
            state.agent_input_until = state
                .agent_input_until
                .filter(|deadline| *deadline > now)
                .map(|deadline| deadline.min(now + POST_DISPATCH_CALLBACK_GRACE));
            let has_popup_holders = state
                .active_agent_operation
                .as_ref()
                .is_some_and(|operation| !operation.popup_holders.is_empty());
            if !has_popup_holders {
                state.active_agent_operation = None;
                state.active_lease = None;
                state.active_lease_expires_at = None;
            }
        }
    }

    /// Release one exact popup hold. Duplicate or stale releases are no-ops;
    /// they cannot consume the upstream hold or a holder from another caller
    /// incarnation that happens to reuse the same process id.
    pub(super) fn release_retained_agent_operation(&self, retained: &RetainedAgentOperation) {
        let mut state = self.state.lock();
        let release_result = state
            .active_agent_operation
            .as_mut()
            .filter(|operation| {
                operation.lease == retained.authorization
                    && operation.caller_epoch == retained.caller_epoch
            })
            .map(|operation| {
                let removed = operation.popup_holders.remove(&retained.holder_id);
                (
                    removed,
                    operation.upstream_active,
                    operation.popup_holders.is_empty(),
                )
            });
        if matches!(release_result, Some((true, false, true))) {
            state.active_agent_operation = None;
            state.active_lease = None;
            state.active_lease_expires_at = None;
        }
    }

    /// Cancel the one in-flight BrowserCore operation owned by this session.
    ///
    /// Hosted BrowserCore requests are consumed serially, but their lease is
    /// issued inside the Rust handler rather than being supplied by the MCP
    /// envelope.  A durable cancellation therefore cannot reconstruct the
    /// opaque lease from the request file.  Clearing only an operation whose
    /// authoritative session still matches is the narrow fail-closed action:
    /// it prevents a queued platform callback or popup from borrowing stale
    /// Agent authority without changing the current owner/revision.
    pub(super) fn cancel_agent_operation_for_session(&self, session_id: &str) -> bool {
        let mut state = self.state.lock();
        let matches_session = state
            .active_agent_operation
            .as_ref()
            .is_some_and(|operation| operation.lease.session_id == session_id);
        if matches_session {
            state.active_agent_operation = None;
            state.active_lease = None;
            state.active_lease_expires_at = None;
            state.agent_input_until = None;
        }
        matches_session
    }

    pub(super) fn begin_agent_input(&self, revision: u64, lease: &str) -> bool {
        let mut state = self.state.lock();
        state.clear_expired_authorization(Instant::now());
        if state.snapshot.owner != NativeControlOwner::Agent
            || state.snapshot.revision != revision
            || state.active_lease.as_deref() != Some(lease)
        {
            return false;
        }
        // 防 wrapper 崩溃后永久吞掉用户接管。正常路径会在 dispatch 完成时撤销
        // active operation；确实派发过原生输入时只留下很短的 callback grace。
        // 750ms 是进程异常时的保险丝，不是正常用户接管延迟。
        state.agent_input_until = Some(Instant::now() + AGENT_INPUT_WINDOW);
        true
    }

    pub(super) fn end_agent_input(&self, revision: u64, lease: &str) {
        let mut state = self.state.lock();
        state.clear_expired_authorization(Instant::now());
        if state.snapshot.revision == revision && state.active_lease.as_deref() == Some(lease) {
            state.agent_input_until = None;
        }
    }

    pub(super) fn agent_input_in_progress(&self) -> bool {
        let mut state = self.state.lock();
        let active = state.snapshot.owner == NativeControlOwner::Agent
            && state
                .agent_input_until
                .is_some_and(|deadline| deadline > Instant::now());
        if !active {
            state.agent_input_until = None;
        }
        active
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeTabLease {
    pub(crate) session_id: String,
    pub(crate) tab_token: String,
    pub(crate) target_id: String,
    pub(crate) revision: u64,
    pub(crate) owner: NativeControlOwner,
    /// 不透明的宿主能力令牌。revision/targetId 可被观察到，不能单独充当授权凭据。
    pub(crate) lease: String,
}

impl NativeTabLease {
    /// 从 wrapper 的 `assert_host_lease` payload 构造受校验的宿主 lease。
    pub(crate) fn from_assertion(
        session_id: impl Into<String>,
        tab_token: impl Into<String>,
        target_id: impl Into<String>,
        revision: u64,
        lease: impl Into<String>,
    ) -> Result<Self, String> {
        let session_id = session_id.into();
        let tab_token = tab_token.into();
        let target_id = target_id.into();
        let lease = lease.into();
        if session_id.is_empty()
            || tab_token.len() != 16
            || !tab_token.bytes().all(|byte| byte.is_ascii_hexdigit())
            || target_id.is_empty()
            || target_id.len() > 512
            || lease.len() != 32
            || !lease.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("浏览器宿主 lease payload 无效".to_string());
        }
        Ok(Self {
            session_id,
            tab_token,
            target_id,
            revision,
            owner: NativeControlOwner::Agent,
            lease,
        })
    }

    pub(crate) fn to_json_value(&self) -> Result<Value, String> {
        serde_json::to_value(self).map_err(|error| format!("序列化浏览器宿主 lease 失败: {error}"))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NativeRequestClaim {
    Execute,
    InFlight,
    Replay(Value),
    Canceled,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NativeRequestCancel {
    Tombstoned,
    /// 请求已经进入执行临界区；取消记录必须保留到执行方提交补偿元数据。
    AwaitingCompletion,
    AlreadyCanceled,
    /// 请求已经提交；调用方必须根据结果回滚它创建的 WebView/工作区。
    AlreadyCompleted(Value),
}

#[derive(Debug, Clone)]
enum RequestState {
    Pending,
    Completed(Value),
    /// cancel 在执行完成前到达。不能把它当成已 ACK 的终态，否则执行方稍后
    /// 产生的资源没有可重试补偿记录。
    CancelAwaitingCompletion,
    /// 已有完整补偿记录，但补偿尚未成功。重复 cancel 必须返回同一份记录。
    CancelPendingRollback(Value),
    Canceled,
}

#[derive(Default)]
pub(super) struct RequestLedger {
    records: HashMap<String, RequestState>,
    terminal_order: VecDeque<String>,
}

impl RequestLedger {
    pub(super) fn claim(
        &mut self,
        session_id: &str,
        request_id: &str,
    ) -> Result<NativeRequestClaim, String> {
        validate_request_id(request_id)?;
        let key = request_key(session_id, request_id)?;
        Ok(match self.records.get(&key) {
            Some(RequestState::Pending) => NativeRequestClaim::InFlight,
            Some(RequestState::Completed(value)) => NativeRequestClaim::Replay(value.clone()),
            Some(
                RequestState::CancelAwaitingCompletion
                | RequestState::CancelPendingRollback(_)
                | RequestState::Canceled,
            ) => NativeRequestClaim::Canceled,
            None => {
                if self.records.len() >= MAX_REQUEST_RECORDS {
                    return Err("浏览器请求登记已满，请等待进行中的请求结束".to_string());
                }
                self.records.insert(key, RequestState::Pending);
                NativeRequestClaim::Execute
            }
        })
    }

    /// 返回 true 表示结果可以提交；false 表示 cancel 已先到达，调用方必须回滚资源。
    pub(super) fn complete(
        &mut self,
        session_id: &str,
        request_id: &str,
        value: Value,
    ) -> Result<bool, String> {
        validate_request_id(request_id)?;
        let key = request_key(session_id, request_id)?;
        match self.records.get(&key) {
            Some(RequestState::CancelAwaitingCompletion) => {
                self.records
                    .insert(key, RequestState::CancelPendingRollback(value));
                Ok(false)
            }
            Some(RequestState::CancelPendingRollback(_) | RequestState::Canceled) => Ok(false),
            Some(RequestState::Completed(_)) => Ok(true),
            Some(RequestState::Pending) => {
                self.records
                    .insert(key.clone(), RequestState::Completed(value));
                self.remember_terminal(&key);
                Ok(true)
            }
            None => Err("浏览器请求尚未 claim，不能提交结果".to_string()),
        }
    }

    pub(super) fn cancel(
        &mut self,
        session_id: &str,
        request_id: &str,
    ) -> Result<NativeRequestCancel, String> {
        validate_request_id(request_id)?;
        let key = request_key(session_id, request_id)?;
        let disposition = match self.records.get(&key).cloned() {
            Some(RequestState::Canceled) => NativeRequestCancel::AlreadyCanceled,
            Some(RequestState::CancelAwaitingCompletion) => NativeRequestCancel::AwaitingCompletion,
            Some(RequestState::CancelPendingRollback(value)) => {
                NativeRequestCancel::AlreadyCompleted(value)
            }
            Some(RequestState::Completed(value)) => {
                // cancel 到达得更晚时保留完整结果，直到调用方明确 ACK 补偿成功；
                // 瞬时 close/I/O 失败后的重复 tombstone 仍可取得同一份回滚记录。
                self.records
                    .insert(key, RequestState::CancelPendingRollback(value.clone()));
                NativeRequestCancel::AlreadyCompleted(value)
            }
            Some(RequestState::Pending) => {
                self.records
                    .insert(key, RequestState::CancelAwaitingCompletion);
                NativeRequestCancel::AwaitingCompletion
            }
            None => {
                if self.records.len() >= MAX_REQUEST_RECORDS {
                    return Err("浏览器请求登记已满，请等待进行中的请求结束".to_string());
                }
                self.records.insert(key.clone(), RequestState::Canceled);
                self.remember_terminal(&key);
                NativeRequestCancel::Tombstoned
            }
        };
        Ok(disposition)
    }

    /// 补偿成功（或被更新的 user/control generation 安全取代）后才把取消推进为
    /// 已 ACK 终态。失败时调用方不调用本方法，record 会保留供下一次 tombstone 重试。
    pub(super) fn acknowledge_cancellation(
        &mut self,
        session_id: &str,
        request_id: &str,
    ) -> Result<(), String> {
        validate_request_id(request_id)?;
        let key = request_key(session_id, request_id)?;
        match self.records.get(&key) {
            Some(RequestState::CancelPendingRollback(_)) => {
                self.records.insert(key.clone(), RequestState::Canceled);
                self.remember_terminal(&key);
                Ok(())
            }
            Some(RequestState::Canceled) => Ok(()),
            Some(RequestState::CancelAwaitingCompletion) => {
                Err("浏览器请求仍在执行，不能提前 ACK 取消".to_string())
            }
            Some(RequestState::Pending | RequestState::Completed(_)) => {
                Err("浏览器请求尚未进入可 ACK 的取消状态".to_string())
            }
            None => Err("浏览器取消请求不存在".to_string()),
        }
    }

    fn remember_terminal(&mut self, request_id: &str) {
        self.terminal_order.push_back(request_id.to_string());
        while self.terminal_order.len() > MAX_TERMINAL_REQUESTS {
            let Some(expired) = self.terminal_order.pop_front() else {
                break;
            };
            if matches!(
                self.records.get(&expired),
                Some(RequestState::Completed(_) | RequestState::Canceled)
            ) {
                self.records.remove(&expired);
            }
        }
    }
}

fn request_key(session_id: &str, request_id: &str) -> Result<String, String> {
    if session_id.is_empty() || session_id.len() > 512 {
        return Err("浏览器请求 sessionId 无效".to_string());
    }
    Ok(format!("{}:{session_id}:{request_id}", session_id.len()))
}

fn validate_request_id(request_id: &str) -> Result<(), String> {
    let valid = !request_id.is_empty()
        && request_id.len() <= 128
        && request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if valid {
        Ok(())
    } else {
        Err("浏览器 requestId 无效".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(token: &str, label: &str) -> SurfaceEntry {
        SurfaceEntry {
            token: token.to_string(),
            label: label.to_string(),
            page_id: label.bytes().map(u64::from).sum(),
            automation_target: None,
            created_by_request_id: None,
            published: Arc::new(AtomicBool::new(true)),
            created_at_revision: None,
        }
    }

    #[test]
    fn tab_registry_is_an_authoritative_bijection() {
        let mut registry = TabRegistry::from_entry(entry("tab-a", "view-a"));
        registry.insert(entry("tab-b", "view-b")).unwrap();
        let tab_b_page_id = registry.by_token("tab-b").unwrap().page_id;

        assert_eq!(registry.by_token("tab-a").unwrap().label, "view-a");
        assert_eq!(registry.token_for_label("view-b"), Some("tab-b"));
        assert_eq!(
            registry.token_for_page_id(entry("ignored", "view-b").page_id),
            Some("tab-b")
        );
        assert!(registry.insert(entry("tab-a", "view-c")).is_err());
        assert!(registry.insert(entry("tab-c", "view-b")).is_err());
        let mut reused_page_id = entry("tab-d", "view-d");
        reused_page_id.page_id = entry("ignored", "view-a").page_id;
        assert!(registry.insert(reused_page_id).is_err());

        let (_, removed) = registry.remove_token("tab-a").unwrap();
        registry.insert(entry("tab-d", "view-d")).unwrap();
        assert_eq!(registry.token_for_page_id(tab_b_page_id), Some("tab-b"));
        assert_ne!(registry.by_token("tab-d").unwrap().page_id, tab_b_page_id);

        assert_eq!(removed.label, "view-a");
        assert_eq!(registry.token_for_label("view-a"), None);
    }

    #[test]
    fn automation_target_binding_is_bijective_and_host_owned() {
        let mut registry = TabRegistry::from_entry(entry("tab-a", "view-a"));
        registry.insert(entry("tab-b", "view-b")).unwrap();
        registry.bind_target("tab-a", "target-a").unwrap();

        assert_eq!(registry.target_for_token("tab-a"), Some("target-a"));
        assert_eq!(registry.token_for_target("target-a"), Some("tab-a"));
        assert!(registry.bind_target("tab-b", "target-a").is_err());
        assert!(registry.bind_target("tab-a", "target-b").is_err());
    }

    #[test]
    fn revision_and_owner_invalidate_an_agent_lease() {
        let control = WorkspaceControl::new(7, NativeControlOwner::Agent);
        let (snapshot, lease) = control.issue_agent_lease();
        assert_eq!(snapshot.revision, 8);
        assert_eq!(snapshot.owner, NativeControlOwner::Agent);
        assert!(control.assert_agent_lease(8, &lease));

        let takeover = control.bump(Some(NativeControlOwner::User));
        assert_eq!(takeover.revision, 9);
        assert_eq!(takeover.owner, NativeControlOwner::User);
        assert!(!control.assert_agent_lease(8, &lease));
    }

    #[test]
    fn restored_unclaimed_workspace_is_claimed_by_first_real_actor() {
        let control = WorkspaceControl::new(1, NativeControlOwner::Unclaimed);
        let (snapshot, lease) = control
            .issue_agent_lease_if_allowed(false)
            .expect("未发生用户操作的恢复页应允许 Agent 首次认领");
        assert_eq!(snapshot.owner, NativeControlOwner::Agent);
        assert!(control.assert_agent_lease(snapshot.revision, &lease));

        let user_first = WorkspaceControl::new(1, NativeControlOwner::Unclaimed);
        user_first.bump(Some(NativeControlOwner::User));
        assert!(user_first.issue_agent_lease_if_allowed(false).is_none());
        assert_eq!(user_first.snapshot().owner, NativeControlOwner::User);
    }

    #[test]
    fn user_control_auto_release_is_revision_guarded() {
        let control = WorkspaceControl::new(4, NativeControlOwner::Agent);
        let first_takeover = control.bump(Some(NativeControlOwner::User));
        assert!(control
            .release_user_control_if_unchanged(first_takeover.revision.saturating_sub(1))
            .is_none());

        let renewed_takeover = control.bump(Some(NativeControlOwner::User));
        assert!(control
            .release_user_control_if_unchanged(first_takeover.revision)
            .is_none());
        let released = control
            .release_user_control_if_unchanged(renewed_takeover.revision)
            .expect("最后一次用户动作的空闲窗口结束后应自动交还");
        assert_eq!(released.owner, NativeControlOwner::Agent);
        assert_eq!(released.revision, renewed_takeover.revision + 1);
        assert!(control
            .release_user_control_if_unchanged(renewed_takeover.revision)
            .is_none());
    }

    #[test]
    fn mutation_cas_never_runs_after_user_takeover() {
        let control = WorkspaceControl::new(3, NativeControlOwner::Agent);
        let (snapshot, lease) = control.issue_agent_lease();
        let authorization = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            snapshot.revision,
            lease,
        )
        .unwrap();
        assert!(control.begin_agent_operation(&authorization, false));
        control.bump(Some(NativeControlOwner::User));
        let ran = Arc::new(AtomicBool::new(false));
        let mutation_ran = Arc::clone(&ran);
        let result = control
            .commit_agent_mutation(&authorization, move || {
                mutation_ran.store(true, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
        assert!(result.is_none());
        assert!(!ran.load(Ordering::SeqCst));
        assert_eq!(control.snapshot().owner, NativeControlOwner::User);
    }

    #[test]
    fn creation_generation_rollback_fails_closed_after_takeover() {
        let control = WorkspaceControl::new(10, NativeControlOwner::Agent);
        control.bump(Some(NativeControlOwner::User));
        let ran = Arc::new(AtomicBool::new(false));
        let rollback_ran = Arc::clone(&ran);
        let result = control
            .commit_agent_generation_rollback(10, move || {
                rollback_ran.store(true, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
        assert!(result.is_none());
        assert!(!ran.load(Ordering::SeqCst));
    }

    #[test]
    fn activation_rollback_restores_previous_owner_only_for_exact_generation() {
        let control = WorkspaceControl::new(3, NativeControlOwner::Unclaimed);
        let (activated, _) = control.issue_agent_lease();
        let ran = Arc::new(AtomicBool::new(false));
        let rollback_ran = Arc::clone(&ran);
        let rolled_back = control
            .rollback_agent_activation(
                activated.revision,
                NativeControlOwner::Unclaimed,
                move |_| {
                    rollback_ran.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )
            .unwrap()
            .expect("unchanged activation generation should roll back");
        assert!(ran.load(Ordering::SeqCst));
        assert_eq!(rolled_back.0.owner, NativeControlOwner::Unclaimed);
        assert_eq!(rolled_back.0.revision, activated.revision + 1);

        let (next, _) = control.issue_agent_lease();
        control.bump(Some(NativeControlOwner::User));
        assert!(control
            .rollback_agent_activation(next.revision, NativeControlOwner::Unclaimed, |_| Ok(()))
            .unwrap()
            .is_none());
        assert_eq!(control.snapshot().owner, NativeControlOwner::User);
    }

    #[test]
    fn agent_input_window_is_bounded_and_explicitly_closed() {
        let control = WorkspaceControl::new(7, NativeControlOwner::Agent);
        let (snapshot, lease) = control.issue_agent_lease();
        assert!(!control.begin_agent_input(snapshot.revision - 1, &lease));
        assert!(!control.begin_agent_input(snapshot.revision, "forged"));
        assert!(control.begin_agent_input(snapshot.revision, &lease));
        assert!(control.agent_input_in_progress());
        control.end_agent_input(snapshot.revision, &lease);
        assert!(!control.agent_input_in_progress());
    }

    #[test]
    fn begun_dispatch_exposes_full_popup_authorization_until_end_or_takeover() {
        let control = WorkspaceControl::new(7, NativeControlOwner::Agent);
        let (snapshot, opaque_lease) = control.issue_agent_lease();
        let authorization = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            snapshot.revision,
            opaque_lease,
        )
        .unwrap();

        // Non-input tools are still atomic dispatches and may legitimately call window.open.
        assert!(control.begin_agent_operation(&authorization, false));
        assert_eq!(
            control.active_agent_operation(),
            Some(authorization.clone())
        );
        control.end_agent_operation(&authorization);
        assert!(control.active_agent_operation().is_none());
        assert!(
            !control.begin_agent_operation(&authorization, true),
            "end must consume the dispatch lease so a delayed begin cannot reopen it"
        );

        let (next_snapshot, next_opaque_lease) = control.issue_agent_lease();
        let next_authorization = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            next_snapshot.revision,
            next_opaque_lease,
        )
        .unwrap();
        assert!(control.begin_agent_operation(&next_authorization, true));
        assert!(control.agent_input_in_progress());
        control.bump(Some(NativeControlOwner::User));
        assert!(control.active_agent_operation().is_none());
        assert!(!control.agent_input_in_progress());
    }

    #[test]
    fn retained_popup_authorization_outlives_parent_end_but_not_takeover() {
        let control = WorkspaceControl::new(7, NativeControlOwner::Agent);
        let (snapshot, opaque_lease) = control.issue_agent_lease();
        let authorization = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            snapshot.revision,
            opaque_lease,
        )
        .unwrap();

        let caller_epoch = AgentCallerEpoch::new(41, "0123456789abcdef0123456789abcdef").unwrap();
        assert!(control.begin_agent_operation_for_caller(
            &authorization,
            true,
            caller_epoch.clone()
        ));
        let popup = control
            .retain_agent_operation_for_popup("session-a", "0123456789abcdef")
            .expect("the synchronous popup callback retains the exact begun operation");
        assert_eq!(popup.authorization(), &authorization);
        assert_eq!(popup.caller_epoch(), &caller_epoch);
        control.end_agent_operation(&authorization);
        assert_eq!(
            control.active_agent_operation(),
            Some(popup.authorization().clone())
        );
        assert!(
            control.state.lock().agent_input_until.unwrap()
                <= Instant::now() + POST_DISPATCH_CALLBACK_GRACE,
            "a retained popup must not prolong the parent's 750ms trusted-input window"
        );
        assert!(control
            .commit_agent_mutation(popup.authorization(), || Ok(()))
            .unwrap()
            .is_some());
        control.release_retained_agent_operation(&popup);
        assert!(control.active_agent_operation().is_none());

        let (next_snapshot, next_opaque_lease) = control.issue_agent_lease();
        let next = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            next_snapshot.revision,
            next_opaque_lease,
        )
        .unwrap();
        assert!(control.begin_agent_operation_for_caller(&next, false, caller_epoch));
        let retained = control
            .retain_agent_operation_for_popup("session-a", "0123456789abcdef")
            .unwrap();
        control.end_agent_operation(&next);
        control.bump(Some(NativeControlOwner::User));
        assert!(control
            .commit_agent_mutation(retained.authorization(), || Ok(()))
            .unwrap()
            .is_none());
        control.release_retained_agent_operation(&retained);
    }

    #[test]
    fn popup_holders_preserve_epoch_and_release_independently_from_upstream() {
        let control = WorkspaceControl::new(7, NativeControlOwner::Agent);
        let (snapshot, opaque_lease) = control.issue_agent_lease();
        let authorization = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            snapshot.revision,
            opaque_lease,
        )
        .unwrap();
        let epoch_a = AgentCallerEpoch::new(41, "0123456789abcdef0123456789abcdef").unwrap();
        let epoch_b = AgentCallerEpoch::new(41, "fedcba9876543210fedcba9876543210").unwrap();

        assert!(control.begin_agent_operation_for_caller(&authorization, true, epoch_a.clone()));
        assert!(
            !control.begin_agent_operation_for_caller(&authorization, true, epoch_b),
            "a recycled pid from another wrapper incarnation cannot refresh the operation"
        );
        let first = control
            .retain_agent_operation_for_popup("session-a", "0123456789abcdef")
            .unwrap();
        let second = control
            .retain_agent_operation_for_popup("session-a", "0123456789abcdef")
            .unwrap();
        assert_eq!(first.caller_epoch(), &epoch_a);
        assert_eq!(second.caller_epoch(), &epoch_a);
        assert!(control.authorize_retained_agent_operation(&first));
        assert!(control.authorize_retained_agent_operation(&second));

        control.release_retained_agent_operation(&first);
        control.release_retained_agent_operation(&first);
        assert!(!control.authorize_retained_agent_operation(&first));
        assert!(control.authorize_retained_agent_operation(&second));
        assert_eq!(
            control.active_agent_operation(),
            Some(authorization.clone()),
            "duplicate popup cleanup must not consume the upstream or sibling holder"
        );
        assert!(
            control.state.lock().agent_input_until.unwrap()
                > Instant::now() + POST_DISPATCH_CALLBACK_GRACE,
            "popup completion must not shorten a still-running trusted-input window"
        );

        control.end_agent_operation(&authorization);
        assert!(
            control
                .retain_agent_operation_for_popup("session-a", "0123456789abcdef")
                .is_none(),
            "a delayed popup callback cannot retain after upstream End"
        );
        assert_eq!(control.active_agent_operation(), Some(authorization));
        control.release_retained_agent_operation(&second);
        assert!(!control.authorize_retained_agent_operation(&second));
        assert!(control.active_agent_operation().is_none());
    }

    #[test]
    fn caller_epoch_requires_pid_and_full_random_nonce() {
        assert!(AgentCallerEpoch::new(0, "0123456789abcdef0123456789abcdef").is_err());
        assert!(AgentCallerEpoch::new(41, "0123456789abcdef").is_err());
        assert!(AgentCallerEpoch::new(41, "zzzz456789abcdef0123456789abcdef").is_err());
        assert!(AgentCallerEpoch::new(41, "ABCDEF6789abcdef0123456789abcdef").is_err());
        let epoch = AgentCallerEpoch::new(41, "abcdef6789abcdef0123456789abcdef").unwrap();
        assert_eq!(epoch.caller_pid(), 41);
        assert_eq!(
            epoch.wrapper_instance_nonce(),
            "abcdef6789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn navigation_preserves_an_active_agent_dispatch_but_advances_after_it_ends() {
        let control = WorkspaceControl::new(7, NativeControlOwner::Agent);
        let (snapshot, opaque_lease) = control.issue_agent_lease();
        let authorization = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            snapshot.revision,
            opaque_lease,
        )
        .unwrap();

        assert!(control.begin_agent_operation(&authorization, true));
        assert!(control
            .bump_for_navigation_if_no_active_agent_operation()
            .is_none());
        assert_eq!(control.snapshot(), snapshot);
        assert_eq!(
            control.active_agent_operation(),
            Some(authorization.clone())
        );

        control.end_agent_operation(&authorization);
        let advanced = control
            .bump_for_navigation_if_no_active_agent_operation()
            .expect("navigation outside an active Agent dispatch must advance revision");
        assert_eq!(advanced.revision, snapshot.revision + 1);
        assert_eq!(advanced.owner, NativeControlOwner::Agent);
        assert!(!control.assert_agent_lease(snapshot.revision, &authorization.lease));
    }

    #[test]
    fn hosted_cancellation_revokes_only_the_matching_sessions_active_operation() {
        let control = WorkspaceControl::new(7, NativeControlOwner::Agent);
        let (snapshot, opaque_lease) = control.issue_agent_lease();
        let authorization = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            snapshot.revision,
            opaque_lease,
        )
        .unwrap();

        assert!(control.begin_agent_operation(&authorization, true));
        assert!(!control.cancel_agent_operation_for_session("session-b"));
        assert_eq!(
            control.active_agent_operation(),
            Some(authorization.clone())
        );
        assert!(control.cancel_agent_operation_for_session("session-a"));
        assert!(control.active_agent_operation().is_none());
        assert!(!control.agent_input_in_progress());
        assert!(!control.authorize_agent_dispatch(&authorization));
    }

    #[test]
    fn expired_operation_cannot_authorize_dispatch_popup_or_navigation() {
        let control = WorkspaceControl::new(7, NativeControlOwner::Agent);
        let (snapshot, opaque_lease) = control.issue_agent_lease();
        let authorization = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            snapshot.revision,
            opaque_lease,
        )
        .unwrap();

        assert!(control.begin_agent_operation(&authorization, true));
        {
            let mut state = control.state.lock();
            state.active_agent_operation.as_mut().unwrap().expires_at =
                Instant::now() - Duration::from_millis(1);
        }
        assert!(control.active_agent_operation().is_none());
        assert!(!control.authorize_agent_dispatch(&authorization));
        assert!(!control.refresh_agent_operation(&authorization));
        assert!(!control.refresh_agent_input_window(&authorization));
        assert!(!control.agent_input_in_progress());
        assert!(
            !control.begin_agent_operation(&authorization, false),
            "an expired authorization cannot open a new operation"
        );
        let mutation_ran = Arc::new(AtomicBool::new(false));
        let mutation_flag = Arc::clone(&mutation_ran);
        assert!(control
            .commit_agent_mutation(&authorization, move || {
                mutation_flag.store(true, Ordering::SeqCst);
                Ok(())
            })
            .unwrap()
            .is_none());
        assert!(!mutation_ran.load(Ordering::SeqCst));

        let advanced = control
            .bump_for_navigation_if_no_active_agent_operation()
            .expect("an expired operation must not suppress a navigation revision bump");
        assert_eq!(advanced.revision, snapshot.revision + 1);
    }

    #[test]
    fn generic_operation_heartbeat_does_not_suppress_real_user_input() {
        let control = WorkspaceControl::new(7, NativeControlOwner::Agent);
        let (snapshot, opaque_lease) = control.issue_agent_lease();
        let authorization = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            snapshot.revision,
            opaque_lease,
        )
        .unwrap();

        assert!(control.begin_agent_operation(&authorization, false));
        assert!(control.refresh_agent_operation(&authorization));
        assert_eq!(
            control.active_agent_operation(),
            Some(authorization.clone())
        );
        assert!(!control.agent_input_in_progress());
        control.end_agent_operation(&authorization);
        assert!(control.active_agent_operation().is_none());
    }

    #[test]
    fn native_input_refresh_is_strict_and_end_keeps_only_callback_grace() {
        let control = WorkspaceControl::new(7, NativeControlOwner::Agent);
        let (snapshot, opaque_lease) = control.issue_agent_lease();
        let authorization = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            snapshot.revision,
            opaque_lease,
        )
        .unwrap();

        assert!(control.begin_agent_operation(&authorization, true));
        control.state.lock().agent_input_until = Some(Instant::now() - Duration::from_millis(1));
        assert!(!control.agent_input_in_progress());

        let mut forged_target = authorization.clone();
        forged_target.target_id = "target-b".to_string();
        assert!(!control.refresh_agent_input_window(&forged_target));
        let mut forged_owner = authorization.clone();
        forged_owner.owner = NativeControlOwner::User;
        assert!(!control.refresh_agent_input_window(&forged_owner));
        let mut forged_opaque_lease = authorization.clone();
        forged_opaque_lease.lease = "fedcba98765432100123456789abcdef".to_string();
        assert!(!control.refresh_agent_input_window(&forged_opaque_lease));

        assert!(control.refresh_agent_input_window(&authorization));
        assert!(control.agent_input_in_progress());
        control.end_agent_operation(&authorization);
        assert!(control.active_agent_operation().is_none());
        assert!(
            !control.refresh_agent_input_window(&authorization),
            "a heartbeat arriving after end must never reopen the suppression window"
        );
        // Only the already-dispatched native event's asynchronous WebKit
        // delegate callback is covered after the active operation ends.
        assert!(control.agent_input_in_progress());
        control.state.lock().agent_input_until = Some(Instant::now() - Duration::from_millis(1));
        assert!(!control.agent_input_in_progress());

        // A delayed operation A cannot borrow a newer operation B's active
        // authorization, even when both share this workspace's opaque lease.
        let (snapshot_b, opaque_lease_b) = control.issue_agent_lease();
        let operation_b = NativeTabLease::from_assertion(
            "session-a",
            "fedcba9876543210",
            "target-b",
            snapshot_b.revision,
            opaque_lease_b,
        )
        .unwrap();
        assert!(control.begin_agent_operation(&operation_b, true));
        assert!(!control.refresh_agent_input_window(&authorization));
        assert!(control.refresh_agent_input_window(&operation_b));
        control.end_agent_operation(&operation_b);

        // Explicit UI takeover always wins immediately over callback grace.
        control.bump(Some(NativeControlOwner::User));
        assert!(!control.agent_input_in_progress());
        assert!(
            !control.refresh_agent_input_window(&operation_b),
            "user takeover must permanently reject the old operation heartbeat"
        );
    }

    #[test]
    fn non_signalling_native_dispatch_revalidates_without_suppressing_user_input() {
        let control = WorkspaceControl::new(7, NativeControlOwner::Agent);
        let (snapshot, opaque_lease) = control.issue_agent_lease();
        let authorization = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            snapshot.revision,
            opaque_lease,
        )
        .unwrap();

        assert!(control.begin_agent_operation(&authorization, false));
        assert!(control.authorize_agent_dispatch(&authorization));
        assert!(!control.agent_input_in_progress());

        let mut forged = authorization.clone();
        forged.tab_token = "fedcba9876543210".to_string();
        assert!(!control.authorize_agent_dispatch(&forged));
        assert!(!control.agent_input_in_progress());

        control.bump(Some(NativeControlOwner::User));
        assert!(!control.authorize_agent_dispatch(&authorization));
    }

    #[test]
    fn opaque_lease_assertion_round_trips_wrapper_schema() {
        let lease = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            9,
            "0123456789abcdeffedcba9876543210",
        )
        .unwrap();
        let value = lease.to_json_value().unwrap();
        assert_eq!(value["sessionId"], "session-a");
        assert_eq!(value["tabToken"], "0123456789abcdef");
        assert_eq!(value["targetId"], "target-a");
        assert_eq!(value["revision"], 9);
        assert_eq!(value["owner"], "agent");
        assert_eq!(value["lease"], "0123456789abcdeffedcba9876543210");
        assert!(NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            9,
            "forged"
        )
        .is_err());
    }

    #[test]
    fn request_ledger_replays_completed_requests_without_reexecution() {
        let mut ledger = RequestLedger::default();
        assert_eq!(
            ledger.claim("session-a", "request-1").unwrap(),
            NativeRequestClaim::Execute
        );
        assert_eq!(
            ledger.claim("session-a", "request-1").unwrap(),
            NativeRequestClaim::InFlight
        );
        assert!(ledger
            .complete("session-a", "request-1", json!({ "tabToken": "tab-a" }))
            .unwrap());
        assert_eq!(
            ledger.claim("session-a", "request-1").unwrap(),
            NativeRequestClaim::Replay(json!({ "tabToken": "tab-a" }))
        );
        assert_eq!(
            ledger.claim("session-b", "request-1").unwrap(),
            NativeRequestClaim::Execute
        );
    }

    #[test]
    fn cancel_tombstone_wins_over_a_late_completion() {
        let mut ledger = RequestLedger::default();
        assert_eq!(
            ledger.cancel("session-a", "request-2").unwrap(),
            NativeRequestCancel::Tombstoned
        );
        assert_eq!(
            ledger.claim("session-a", "request-2").unwrap(),
            NativeRequestClaim::Canceled
        );
        assert!(!ledger
            .complete("session-a", "request-2", json!({}))
            .unwrap());
    }

    #[test]
    fn cancel_after_commit_returns_the_result_needed_for_rollback() {
        let mut ledger = RequestLedger::default();
        assert_eq!(
            ledger.claim("session-a", "request-3").unwrap(),
            NativeRequestClaim::Execute
        );
        let result = json!({ "tabToken": "tab-c" });
        assert!(ledger
            .complete("session-a", "request-3", result.clone())
            .unwrap());
        assert_eq!(
            ledger.cancel("session-a", "request-3").unwrap(),
            NativeRequestCancel::AlreadyCompleted(result)
        );
        // 补偿未 ACK 前重复 tombstone 必须拿回同一份 record，不能退化成裸 Canceled。
        assert_eq!(
            ledger.cancel("session-a", "request-3").unwrap(),
            NativeRequestCancel::AlreadyCompleted(json!({ "tabToken": "tab-c" }))
        );
        assert_eq!(
            ledger.claim("session-a", "request-3").unwrap(),
            NativeRequestClaim::Canceled
        );
        ledger
            .acknowledge_cancellation("session-a", "request-3")
            .unwrap();
        assert_eq!(
            ledger.cancel("session-a", "request-3").unwrap(),
            NativeRequestCancel::AlreadyCanceled
        );
    }

    #[test]
    fn cancellation_while_pending_retains_late_completion_until_ack() {
        let mut ledger = RequestLedger::default();
        assert_eq!(
            ledger.claim("session-a", "request-4").unwrap(),
            NativeRequestClaim::Execute
        );
        assert_eq!(
            ledger.cancel("session-a", "request-4").unwrap(),
            NativeRequestCancel::AwaitingCompletion
        );
        assert!(ledger
            .acknowledge_cancellation("session-a", "request-4")
            .is_err());

        let record = json!({ "rollback": { "kind": "prepared_session" } });
        assert!(!ledger
            .complete("session-a", "request-4", record.clone())
            .unwrap());
        assert_eq!(
            ledger.cancel("session-a", "request-4").unwrap(),
            NativeRequestCancel::AlreadyCompleted(record)
        );
        ledger
            .acknowledge_cancellation("session-a", "request-4")
            .unwrap();
    }
}
