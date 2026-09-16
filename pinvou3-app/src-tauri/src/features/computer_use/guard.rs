//! Consent guard: the settings toggle, session grants, the stop flag, T3
//! confirmation.
//!
//! The engine currently auto-approves all tool calls, so consent gating must
//! be built into the tool itself — this module is the single source of truth.
//! The integration layer (Tauri commands) injects the user's decisions through
//! the public API: `set_enabled` / `grant_session` / `revoke_session` /
//! `stop_all` / `mint_confirmation`. Grants live only in memory and are never
//! persisted; a session grant is bound to the tool instance's lifetime: it
//! ends on revoke / stop / master-switch off, or when the engine reclaims the
//! tool (its `Drop` calls `revoke_session`) — which includes the engine
//! pool's idle reaper silently reaping an idle engine. A grant lost to idle
//! reclaim is simply gone: the next input attempt reads `GrantRequired` and
//! the user must grant again (a fresh tool instance starts grant-less).
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

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::backend::BackendRegistry;

/// The validity window of a T3 confirmation: an unanswered pending must
/// expire on its own — after expiry it reads as nonexistent and can no longer
/// mint an approval token.
pub const CONFIRM_TTL: Duration = Duration::from_secs(5 * 60);
/// Bounded-wait cap for the cross-session physical input lock, in seconds.
/// An input action holds the lock from screening through injection; a
/// waiting session blocks at most this long and then fails closed with
/// [`GuardRejection::InputBusy`], so the model can wait and retry — far
/// shorter than a full backend call (see backend.rs's
/// `BACKEND_CALL_TIMEOUT`), and an unbounded wait would silently wedge the
/// other sessions.
pub const PHYSICAL_INPUT_LOCK_TIMEOUT: Duration = Duration::from_secs(20);

/// Test override for [`PHYSICAL_INPUT_LOCK_TIMEOUT`], in milliseconds: 0
/// means "use the default" (see
/// [`set_physical_input_lock_timeout_for_tests`]).
static PHYSICAL_INPUT_LOCK_TIMEOUT_MS: AtomicU64 = AtomicU64::new(0);

/// Test-only: overrides the bounded-wait cap of
/// [`ComputerUseShared::lock_physical_input`]; pass `Duration::ZERO` to
/// restore [`PHYSICAL_INPUT_LOCK_TIMEOUT`].
#[cfg(test)]
pub(crate) fn set_physical_input_lock_timeout_for_tests(d: Duration) {
    PHYSICAL_INPUT_LOCK_TIMEOUT_MS.store(d.as_millis() as u64, Ordering::SeqCst);
}

fn physical_input_lock_timeout() -> Duration {
    match PHYSICAL_INPUT_LOCK_TIMEOUT_MS.load(Ordering::SeqCst) {
        0 => PHYSICAL_INPUT_LOCK_TIMEOUT,
        ms => Duration::from_millis(ms),
    }
}

/// The T3 consequential-action denylist (case-insensitive substring match;
/// Chinese/English/Japanese/Traditional Chinese).
///
/// Only **consequence category** terms are listed — the five categories
/// mainstream products confirm on (the same position as Google computer-use's
/// LEGAL_TERMS_AND_AGREEMENTS / USER_CONSENT_MANAGEMENT etc.): financial
/// (buy/pay/checkout/transfer), send, irreversible deletion (including the
/// drag destinations recycle bin/trash), form/order submission, and terms &
/// consent acceptance. Generic affirmatives and generic action words
/// (OK/Yes/Continue/Confirm/Run/Execute/Install/Remove/Empty/Bin and their
/// CJK equivalents) are **in no mainstream category list** and have all been
/// removed; under substring matching their false-positive surface disappears
/// with them (e.g. "bin" matching "combine").
pub const T3_DENYLIST: &[&str] = &[
    // Financial.
    "buy",
    "pay",
    "purchase",
    "checkout",
    "transfer",
    "place order",
    "order now",
    "购买",
    "購買",
    "支付",
    "付款",
    "转账",
    "结算",
    "轉賬",
    "轉帳",
    "結算",
    "購入",
    "支払い",
    "送金",
    "注文",
    "下单",
    "下單",
    "充值",
    "儲值",
    "捐款",
    "捐贈",
    "投资",
    "投資",
    "訂閱",
    "subscribe",
    "donate",
    "top-up",
    // Sends.
    "send",
    "发送",
    "傳送",
    "送出",
    "送信",
    // Irreversible deletion, including common drag/delete destinations.
    // "format" covers the English verb the CJK entries (格式化/フォーマット)
    // already carry; substring cost is an extra confirm on labels like
    // "Format document" — the safe direction.
    "delete",
    "trash",
    "erase",
    "discard",
    "format",
    "删除",
    "清空",
    "回收站",
    "回收筒",
    "废纸篓",
    "刪除",
    "資源回收筒",
    "廢紙簍",
    "ゴミ箱",
    "削除",
    "格式化",
    "フォーマット",
    // Form/order submission.
    "submit",
    "提交",
    "提出",
    // Terms/consent acceptance.
    "accept",
    "agree",
    "同意",
    "接受",
    "承諾",
];

/// Whether a label hits the T3 consequential denylist.
pub fn matches_t3_denylist(label: &str) -> bool {
    let lower = label.to_lowercase();
    T3_DENYLIST.iter().any(|term| lower.contains(term))
}

/// Whether the element role is a password/secure text field (a T3 signal;
/// corresponds to Operator's takeover scenario). Roles that merely contain
/// "insecure" (e.g. AXInsecureTextField) must not trip the "secure"
/// substring.
pub fn is_secure_role(role: &str) -> bool {
    let lower = role.to_lowercase();
    lower.contains("password") || (lower.contains("secure") && !lower.contains("insecure"))
}

/// Guard rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardRejection {
    /// The computer use master switch is off in settings.
    Disabled,
    /// The stop flag is raised (panic stop / stop_all).
    Stopped,
    /// An input-class action lacks a valid session grant.
    GrantRequired,
    /// The cross-session physical input lock is held by another session and
    /// the bounded wait timed out (see
    /// [`PHYSICAL_INPUT_LOCK_TIMEOUT`]).
    InputBusy,
}

impl GuardRejection {
    /// The error text shown to the model (the model can replan from it).
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

/// The result of [`ComputerUseShared::grant_session`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantOutcome {
    /// The session now holds a grant.
    Granted,
    /// Refused: the master switch is off. A grant must not silently sleep in
    /// the set until a re-enable resurrects control the user never
    /// re-approved.
    Disabled,
    /// Refused: the emergency stop is latched.
    Stopped,
}

/// A session grant records only "whether this session holds a grant": the
/// grant is bound to the tool instance's lifetime — it ends on revoke / stop
/// / master-switch off, or when the engine reclaims the tool (the tool's
/// `Drop` revokes), which includes the engine pool's idle reaper silently
/// reaping an idle engine (the grant then lapses without any user action and
/// the next input attempt reads `GrantRequired`).

/// A T3 confirmation (pending) waiting for the user's decision. The approval
/// token is minted by the `computer_use_confirm` Tauri command via
/// [`ComputerUseShared::mint_confirmation`]; unanswered past
/// [`CONFIRM_TTL`] it expires. At most one pending per session at a time — a
/// new request replaces the old one directly (like an ordinary dialog; newest
/// wins).
#[derive(Debug, Clone)]
pub struct PendingConfirmation {
    pub session_id: String,
    /// The plain human-readable parameter summary of the blocked action
    /// (e.g. `left click x1 at Some((5, 6))`, `type 3 characters`); the
    /// approval token is bound to exactly this summary **and** to
    /// `action_binding` — a summary-only binding would let a token minted
    /// for one `type N characters` be spent on a different same-length text.
    pub action_summary: String,
    /// A content hash of the full blocked action (every parameter, not just
    /// the shape the summary renders). Computed by the tool layer when the
    /// pending is created and re-checked at spend time, so an approved action
    /// cannot be silently swapped for a same-summary different-content one.
    pub action_binding: u64,
    pub element_label: String,
    pub created_at: Instant,
}

/// A minted approval token: bound to the session, the action summary and the
/// action content hash; expires if not spent within [`CONFIRM_TTL`].
#[derive(Debug, Clone)]
struct ApprovedToken {
    session_id: String,
    action_summary: String,
    action_binding: u64,
    minted_at: Instant,
}

/// The result of spending an approval token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmationCheck {
    /// The token is valid and matches both the session and the action
    /// summary; it has been consumed (single-use).
    Granted,
    /// No such token (unknown / already spent / expired / session or action
    /// mismatch).
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

/// Consent state shared across sessions (an Arc, injected into the tool at
/// factory construction).
pub struct ComputerUseShared {
    enabled: AtomicBool,
    stop: AtomicBool,
    /// The set of session ids holding a session grant (grant lifetime: see
    /// the note at the top of this module — bound to the tool instance, so
    /// engine idle reclaim also ends it).
    sessions: Mutex<HashSet<String>>,
    /// The physical mouse/keyboard is a globally exclusive resource, but the
    /// backend has one worker per session — this process-level lock
    /// serializes input injection **across sessions** (without it, two
    /// concurrent sessions could each hold a valid grant and interleave
    /// typing/clicks). Input-class actions hold it for the whole screening +
    /// execution.
    physical_input_lock: Mutex<()>,
    /// The two consent maps under a single mutex (see [`ConsentMaps`]).
    consent: Mutex<ConsentMaps>,
    /// Session → backend handle registry (registered at construction,
    /// unregistered on drop). When a grant is revoked, on global stop, or
    /// when the master switch goes off, the command layer goes through it to
    /// have the backend close its persistent OS-level grant (e.g. a Wayland
    /// portal session) — the app-side source of truth for grant semantics
    /// lives in this module; the OS-side termination action is forwarded to
    /// the backend here.
    pub backends: BackendRegistry,
}

impl Default for ComputerUseShared {
    fn default() -> Self {
        Self::new()
    }
}

impl ComputerUseShared {
    /// `enabled` defaults to false — the tool rejects everything until the
    /// setting is turned on.
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            sessions: Mutex::new(HashSet::new()),
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

    /// The settings toggle (called by the integration layer's settings
    /// command).
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }

    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Grants this session input control (a session grant), refusing with
    /// [`GrantOutcome::Disabled`] / [`GrantOutcome::Stopped`] while the
    /// master switch is off / the emergency stop is latched. The switch/stop
    /// re-check runs under the `sessions` lock, in the same critical section
    /// as the insert: a grant cannot slip into the set after a
    /// `revoke_all_sessions` sweep and survive the off period to resurrect on
    /// re-enable, and it cannot queue up while stopped to wake with the stop
    /// reset. Grant lifetime: bound to the tool instance — it ends on revoke
    /// / stop / master-switch off, or when the engine reclaims the tool (its
    /// `Drop` revokes), including the engine pool's idle reaper silently
    /// reaping an idle engine.
    pub fn grant_session(&self, session_id: &str) -> GrantOutcome {
        let mut sessions = self.sessions.lock();
        if !self.is_enabled() {
            return GrantOutcome::Disabled;
        }
        if self.is_stopped() {
            return GrantOutcome::Stopped;
        }
        sessions.insert(session_id.to_string());
        GrantOutcome::Granted
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

    /// Whether the session currently holds a valid grant. A read-only
    /// projection for status commands; gating decisions still go by
    /// [`Self::begin_input_action`]. The row is cleared by
    /// [`Self::revoke_session`], [`Self::stop_all`] and the master switch
    /// ([`Self::revoke_all_sessions`]).
    pub fn has_active_grant(&self, session_id: &str) -> bool {
        self.sessions.lock().contains(session_id)
    }

    /// Emergency stop: raises the stop flag, revokes all session grants, and
    /// clears all pending/approved confirmations (no consent state should
    /// survive a stop).
    pub fn stop_all(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.sessions.lock().clear();
        let mut consent = self.consent.lock();
        consent.pending.clear();
        consent.approved_tokens.clear();
    }

    /// Clears the stop flag when the user re-enables (restores no grants).
    /// Also sweeps any consent state an in-flight run created between the
    /// stop and the re-enable: a pending minted during the stopped window
    /// would otherwise become mintable the moment the stop is lifted —
    /// consent state from the stopped period must not survive the resume,
    /// same rule as `revoke_all_sessions` for the off period.
    pub fn reset_stop(&self) {
        self.stop.store(false, Ordering::SeqCst);
        let mut consent = self.consent.lock();
        consent.pending.clear();
        consent.approved_tokens.clear();
    }

    /// Revokes all session grants and clears all consent state (pending
    /// confirmations, minted tokens) but does **not** raise the stop flag —
    /// distinct from [`Self::stop_all`]'s emergency-stop semantics: turning
    /// the master switch off is not an emergency stop, and no stop state
    /// should remain after re-enabling (the `computer_use_set_enabled(false)`
    /// call). Consent state from the off period must not survive a re-enable:
    /// the old grant and old approval tokens would otherwise remain valid.
    pub fn revoke_all_sessions(&self) {
        self.sessions.lock().clear();
        let mut consent = self.consent.lock();
        consent.pending.clear();
        consent.approved_tokens.clear();
    }

    /// Gate for observe/passive actions: only needs the master switch on and
    /// no stop.
    pub fn check_readonly(&self) -> Result<(), GuardRejection> {
        if !self.is_enabled() {
            return Err(GuardRejection::Disabled);
        }
        if self.is_stopped() {
            return Err(GuardRejection::Stopped);
        }
        Ok(())
    }

    /// ToolPolicy hook, called by the composition root's `tool_policy`
    /// closure on every `refresh_disallowed_tools` round: appends
    /// `computer_use` to the disallow list while the master switch is off.
    /// The tool is constructed unconditionally, so the disallow list is the
    /// visibility channel while disabled; the consent guard's `Disabled`
    /// rejection is the second, defense-in-depth layer.
    pub fn add_to_disallow_list_when_disabled(&self, tools: &mut Vec<String>) {
        if !self.is_enabled() {
            tools.push(super::TOOL_NAME.to_string());
        }
    }

    /// Gate for input-class actions: switch on, not stopped, and the session
    /// holds a grant (grant lifetime: see the note at the top of this
    /// module). [`Self::verify_input_action`] must be checked once more
    /// before injection.
    pub fn begin_input_action(&self, session_id: &str) -> Result<(), GuardRejection> {
        self.check_readonly()?;
        if !self.sessions.lock().contains(session_id) {
            return Err(GuardRejection::GrantRequired);
        }
        Ok(())
    }

    /// Read-only re-check: whether the grant is still valid (switch, stop
    /// flag, grant present). Between `begin_input_action` and the actual
    /// injection there can be time-consuming steps such as an automatic
    /// screenshot, and a revoke inside that window must take effect; must be
    /// called before injecting — and before any consent surface, so a
    /// stop/revoke landing mid-run cannot still pop a confirmation dialog.
    pub fn verify_input_action(&self, session_id: &str) -> Result<(), GuardRejection> {
        self.begin_input_action(session_id)
    }

    /// Serializes physical input injection across sessions (see
    /// [`ComputerUseShared::physical_input_lock`]). Acquisition is a **bounded
    /// wait** (`try_lock_for`, see [`PHYSICAL_INPUT_LOCK_TIMEOUT`]): when the
    /// lock is held by another session, it times out returning an explicit
    /// [`GuardRejection::InputBusy`] instead of waiting unboundedly. The lock
    /// semantics (held for the whole screening-to-injection of an Input
    /// action) and the release path (guard drop) are unchanged.
    pub fn lock_physical_input(&self) -> Result<parking_lot::MutexGuard<'_, ()>, GuardRejection> {
        self.physical_input_lock
            .try_lock_for(physical_input_lock_timeout())
            .ok_or(GuardRejection::InputBusy)
    }

    /// Registers a T3 confirmation waiting for the user's decision and
    /// returns the confirm_id. At most one pending per session: a new request
    /// replaces the session's existing pending (newest wins, like an ordinary
    /// dialog), so this method cannot fail. `action_binding` is the tool
    /// layer's content hash of the blocked action and is carried into the
    /// minted token. Also sweeps expired pendings.
    pub fn new_pending_confirmation(
        &self,
        session_id: &str,
        action_summary: impl Into<String>,
        element_label: impl Into<String>,
        action_binding: u64,
    ) -> String {
        let confirm_id = format!("cu-{:016x}", rand::random::<u64>());
        let now = Instant::now();
        let mut consent = self.consent.lock();
        let pending = &mut consent.pending;
        pending.retain(|_, entry| now.duration_since(entry.created_at) <= CONFIRM_TTL);
        // At most one pending per session: the newest request wins (a normal
        // dialog replaces the previous one instead of queueing behind it).
        // Under swarm mode every subagent shares the parent session, so two
        // subagents asking at once collide here BY DESIGN: the first asker's
        // confirm_id stops minting and its action fails with
        // t3-confirmation-required; it must re-ask (a fresh request) after
        // the user dealt with the newer dialog.
        pending.retain(|_, entry| entry.session_id != session_id);
        pending.insert(
            confirm_id.clone(),
            PendingConfirmation {
                session_id: session_id.to_string(),
                action_summary: action_summary.into(),
                action_binding,
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

    /// The user explicitly "denies" a blocked T3 action in the frontend:
    /// consumes the pending. A denial records no server-side state — a retry
    /// of the same action goes through screening as usual and mints a
    /// **new** pending, re-emitting the confirm event (mainstream model: a
    /// denial is only model-visible context, not stored punitive state).
    /// Returns false for an unknown id.
    pub fn deny_confirmation(&self, confirm_id: &str) -> bool {
        let mut consent = self.consent.lock();
        if consent.pending.remove(confirm_id).is_some() {
            return true;
        }
        // A "denial" after approval is a change of heart: if the same
        // confirm_id's minted token is unspent, retract it too — deny clears
        // the pending first, and without this the token from "approved →
        // changed my mind" would live out its TTL while the deny button
        // silently did nothing inside the race window. Returns false when
        // the token does not exist (already spent / expired / already cleared
        // by revoke), the same as an unknown id.
        consent.approved_tokens.remove(confirm_id).is_some()
    }

    /// Mints an approval token. Callable only by the `computer_use_confirm`
    /// Tauri command — the model must never be able to mint one via a tool
    /// call. Mints only for a pending that exists and is unexpired, returning
    /// `true`; returns `false` when the pending does not exist / has expired /
    /// was already decided — a silent no-op would let the frontend show a
    /// failure as success. The token inherits the pending's session and
    /// action summary (compared item by item at spend time). Tokens have no
    /// stock cap: a session has at most one pending at a time, and minting
    /// removes the pending, so the token stock is naturally bounded by the
    /// interaction cadence; expiry is backstopped by the TTL sweep before
    /// spending.
    pub fn mint_confirmation(&self, confirm_id: &str) -> bool {
        // pending removal + token insertion in one lock: a mint cannot
        // interleave with a revoke/clear and leave a token that outlives the
        // disable. The switch/stop re-check also lives inside the lock: a
        // stop/toggle-off that lands while a confirmation dialog is on screen
        // must not leave a mintable token behind — the pending itself is
        // cleared by stop_all/revoke_all_sessions, but an in-flight run can
        // re-create a pending after that sweep, so the mint is the last line
        // of defense and the token can never outlive the stop. The false
        // return maps to the command layer's "unknown or expired" error,
        // which the frontend treats as close-the-stale-dialog.
        let mut consent = self.consent.lock();
        if !self.is_enabled() || self.is_stopped() {
            return false;
        }
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
                action_binding: entry.action_binding,
                minted_at: now,
            },
        );
        true
    }

    /// Spends an approval token (single-use). The tool calls it before
    /// executing an action carrying a `confirm_id`; the token must exactly
    /// match **this** session, the action summary **and** the action content
    /// hash — the user approves a summary for readability but the token is
    /// bound to the full action content, so a token approved for one
    /// `type N characters` cannot be spent on a different same-length text.
    /// On a match, execution proceeds — no second screening (mainstream
    /// model: the API confirmation is just a per-action confirmation id; once
    /// the client acknowledges, execute); on a mismatch the token is kept
    /// (under exact binding, the only combination that can pass is the
    /// user-approved original action replay — a wrong attempt should not burn
    /// the user's confirmation).
    pub fn take_confirmation(
        &self,
        confirm_id: &str,
        session_id: &str,
        action_summary: &str,
        action_binding: u64,
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
        if token.session_id != session_id
            || token.action_summary != action_summary
            || token.action_binding != action_binding
        {
            return ConfirmationCheck::Unknown;
        }
        consent.approved_tokens.remove(confirm_id);
        ConfirmationCheck::Granted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled_shared() -> ComputerUseShared {
        let shared = ComputerUseShared::new();
        shared.set_enabled(true);
        shared
    }

    /// Test convenience wrapper: the standard three-argument pending / spend.
    /// Tests that don't exercise the content binding pass a zero binding on
    /// both sides (mint copies it verbatim, spend compares it verbatim).
    fn new_pending(shared: &ComputerUseShared, session: &str, summary: &str) -> String {
        shared.new_pending_confirmation(session, summary, "Buy now", 0)
    }

    fn take(
        shared: &ComputerUseShared,
        id: &str,
        session: &str,
        summary: &str,
    ) -> ConfirmationCheck {
        shared.take_confirmation(id, session, summary, 0)
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
        assert_eq!(shared.grant_session("s1"), GrantOutcome::Granted);
        assert!(shared.begin_input_action("s1").is_ok());
        shared.revoke_session("s1");
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::GrantRequired)
        );
    }

    /// Grant TOCTOU: the switch/stop check lives inside `grant_session`,
    /// under the `sessions` lock in the same critical section as the insert —
    /// a grant can never slip in after a `revoke_all_sessions` sweep and
    /// survive the off period, nor queue up while stopped.
    #[test]
    fn grant_is_refused_while_disabled_or_stopped() {
        let shared = ComputerUseShared::new();
        assert_eq!(
            shared.grant_session("s1"),
            GrantOutcome::Disabled,
            "granting while the master switch is off must be refused"
        );
        assert!(!shared.has_active_grant("s1"));
        shared.set_enabled(true);
        assert_eq!(shared.grant_session("s1"), GrantOutcome::Granted);
        // Off period: revoking all sessions (master-switch-off semantics)
        // also clears the grant, and a grant attempt inside the off period is
        // refused rather than queued.
        shared.revoke_all_sessions();
        shared.set_enabled(false);
        assert_eq!(shared.grant_session("s1"), GrantOutcome::Disabled);
        shared.set_enabled(true);
        assert!(
            !shared.has_active_grant("s1"),
            "a grant refused during the off period must not resurrect"
        );
        // Stopped: same refusal.
        shared.grant_session("s2");
        shared.stop_all();
        assert_eq!(
            shared.grant_session("s2"),
            GrantOutcome::Stopped,
            "granting while the stop flag is latched must be refused"
        );
        shared.reset_stop();
        assert!(
            !shared.has_active_grant("s2"),
            "stop_all revoked the grant and the refused re-grant must not resurrect it"
        );
    }

    #[test]
    fn has_active_grant_reflects_grant_and_revoke() {
        let shared = enabled_shared();
        assert!(!shared.has_active_grant("s1"));
        shared.grant_session("s1");
        assert!(shared.has_active_grant("s1"));
        shared.revoke_session("s1");
        assert!(!shared.has_active_grant("s1"));
    }

    /// Session grants have no in-guard idle clock: once granted, a grant
    /// lives until the tool instance ends (revoke / stop / master-switch off
    /// / engine reclaim) — repeated gating/re-checks, granting other
    /// sessions, or re-granting this session never clears it.
    #[test]
    fn grant_stays_live_until_revoked() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        // Repeated gating/re-checks never invalidate.
        for _ in 0..10 {
            assert!(shared.begin_input_action("s1").is_ok());
            assert!(shared.verify_input_action("s1").is_ok());
            assert!(shared.has_active_grant("s1"));
        }
        // Granting/re-granting other sessions does not clear s1 (no idle
        // sweep).
        shared.grant_session("s2");
        shared.grant_session("s1");
        assert!(shared.has_active_grant("s1"));
        assert!(shared.begin_input_action("s1").is_ok());
        // The only invalidation path is an explicit revoke.
        shared.revoke_session("s1");
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::GrantRequired)
        );
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
        // stop_all already revoked the grant: even with the stop flag
        // cleared, a re-grant is required.
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::GrantRequired)
        );
    }

    /// The list contains only consequence-category terms (financial/send/
    /// irreversible deletion/submission/terms-consent; Chinese/English/
    /// Japanese/Traditional Chinese), case-insensitive; generic affirmatives
    /// and generic action words have all been removed — no mainstream
    /// category list contains them, so substring false positives
    /// ("bin"→"combine") disappear too.
    #[test]
    fn t3_denylist_matches_only_consequence_categories() {
        for label in [
            // Financial.
            "Buy now",
            "PAY",
            "Complete Purchase",
            "Checkout now",
            "Wire Transfer",
            "Place order",
            "立即购买",
            "确认支付",
            "付款",
            "转账",
            "轉賬",
            "轉帳",
            "結算",
            "カートに追加して購入",
            "お支払い",
            "口座に送金",
            "注文を確定",
            "購買",
            "下单",
            "下單",
            "确认充值",
            "儲值",
            "捐款",
            "投資",
            "訂閱方案",
            "Subscribe to plan",
            "Donate",
            "Top-up wallet",
            // Sends.
            "Send message",
            "发送",
            "傳送",
            "送出訂單",
            "メッセージを送信",
            // Irreversible deletion (including drag destinations).
            "Delete file",
            "Move to Trash",
            "Empty Trash",
            "Erase disk",
            "彻底删除",
            "移到废纸篓",
            "拖入回收站",
            "清空列表",
            "ゴミ箱に移動",
            "刪除檔案",
            "資源回收筒",
            "ファイルを削除",
            "格式化硬盘",
            "フォーマット実行",
            // Submission.
            "submit form",
            "提交订单",
            "フォームを提出",
            // Terms/consent acceptance.
            "Accept all",
            "I agree",
            "同意条款",
        ] {
            assert!(matches_t3_denylist(label), "should match: {label}");
        }
        for label in [
            // Generic affirmatives/action words: in no mainstream category
            // list; always let through.
            "OK",
            "Yes",
            "Continue",
            "Confirm",
            "Yes, continue",
            "Run script",
            "Execute command",
            "Install updates",
            "Remove file",
            "Combine files",
            "继续操作",
            "运行脚本",
            "执行命令",
            "安裝更新",
            // Common non-T3 controls.
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
        // "insecure" contains "secure" as a substring but is the opposite
        // signal — it must not trip the secure-role screen.
        assert!(!is_secure_role("AXInsecureTextField"));
        assert!(!is_secure_role("insecure text field"));
    }

    #[test]
    fn confirmation_tokens_are_single_use_and_bound_to_session_and_action() {
        let shared = enabled_shared();
        let summary = "left click x1 at Some((100, 200))";
        let id = new_pending(&shared, "s1", summary);
        let pending = shared.pending_confirmation(&id);
        assert!(pending.as_ref().is_some_and(|p| p.session_id == "s1"));
        // Cannot be spent before minting.
        assert_eq!(
            take(&shared, &id, "s1", summary),
            ConfirmationCheck::Unknown
        );
        assert!(
            shared.mint_confirmation(&id),
            "mint must report success for a live pending"
        );
        // The pending is cleared after minting.
        assert!(shared.pending_confirmation(&id).is_none());
        // Only the correct session + action summary can spend it.
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
        // A mismatched wrong attempt does not destroy the token (under exact
        // binding, the only thing that can pass is the user-approved original
        // action).
        assert_eq!(
            take(&shared, &id, "s1", summary),
            ConfirmationCheck::Granted
        );
        // Single-use: the second spend fails.
        assert_eq!(
            take(&shared, &id, "s1", summary),
            ConfirmationCheck::Unknown
        );
    }

    /// deny consumes the pending; a denial records no server-side state — a
    /// retry of the same action mints a **new** pending (a new confirm_id) as
    /// usual and goes through the confirm flow again (mainstream model: a
    /// denial is only model-visible context).
    #[test]
    fn deny_confirmation_consumes_the_pending_and_retry_mints_a_new_one() {
        let shared = enabled_shared();
        let id = new_pending(&shared, "s1", "left click");
        assert!(shared.deny_confirmation(&id));
        // deny clears the pending: it can no longer mint (mint returns false,
        // no longer a silent no-op).
        assert!(shared.pending_confirmation(&id).is_none());
        assert!(!shared.mint_confirmation(&id));
        // Retry the same id: the token is invalid.
        assert_eq!(
            take(&shared, &id, "s1", "left click"),
            ConfirmationCheck::Unknown,
            "a denied id is simply unknown afterwards"
        );
        // Retry the same action: a new pending is minted (a new id) and the
        // confirm flow proceeds as usual — no denial memory.
        let retry = new_pending(&shared, "s1", "left click");
        assert_ne!(retry, id, "a retry after denial must mint a new confirm_id");
        assert!(shared.pending_confirmation(&retry).is_some());
        assert!(shared.mint_confirmation(&retry));
        assert_eq!(
            take(&shared, &retry, "s1", "left click"),
            ConfirmationCheck::Granted
        );
        // deny on an unknown id fails.
        assert!(!shared.deny_confirmation("cu-unknown"));
        assert_eq!(
            take(&shared, "cu-unknown", "s1", "x"),
            ConfirmationCheck::Unknown,
            "unknown id must stay Unknown"
        );
    }

    /// A "denial" after minting is a change of heart — the same confirm_id's
    /// unspent token must be retracted along with it, not live out its TTL.
    #[test]
    fn deny_after_mint_retracts_the_unspent_token() {
        let shared = enabled_shared();
        let id = new_pending(&shared, "s1", "left click x1 at Some((5, 6))");
        assert!(shared.mint_confirmation(&id));
        // Change of heart: retract the unspent token.
        assert!(shared.deny_confirmation(&id));
        assert_eq!(
            take(&shared, &id, "s1", "left click x1 at Some((5, 6))"),
            ConfirmationCheck::Unknown,
            "a retracted token must not grant anything"
        );
        // Denying an already-spent (or unknown) id still reports false — the
        // same as "unknown/already decided".
        assert!(!shared.deny_confirmation(&id));
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
        // Read-only: repeated verify has no side effects.
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

    /// With the bounded wait shrunk to ~0 via the test override, a lock held
    /// by another session fails closed with `InputBusy` instead of blocking
    /// for the full 20-second default.
    #[test]
    fn physical_input_lock_times_out_with_the_test_override() {
        set_physical_input_lock_timeout_for_tests(Duration::from_millis(5));
        let shared = enabled_shared();
        let _holder = shared
            .physical_input_lock
            .try_lock()
            .expect("free lock must be acquired");
        assert!(
            matches!(shared.lock_physical_input(), Err(GuardRejection::InputBusy)),
            "a held lock must fail closed once the bounded wait elapses"
        );
        // Restore the default so later tests in this process are unaffected.
        set_physical_input_lock_timeout_for_tests(Duration::ZERO);
    }

    /// ToolPolicy contract: `computer_use` is in the disallow list while the
    /// master switch is off and stays visible while it is on.
    #[test]
    fn disallow_list_hides_computer_use_only_while_disabled() {
        let shared = ComputerUseShared::new();
        let mut tools: Vec<String> = vec!["other_tool".to_string()];
        shared.add_to_disallow_list_when_disabled(&mut tools);
        assert_eq!(
            tools
                .iter()
                .filter(|name| *name == super::super::types::TOOL_NAME)
                .count(),
            1,
            "disabled: computer_use must be in the disallow list"
        );
        shared.set_enabled(true);
        tools.retain(|name| name != super::super::types::TOOL_NAME);
        shared.add_to_disallow_list_when_disabled(&mut tools);
        assert!(
            !tools
                .iter()
                .any(|name| name == super::super::types::TOOL_NAME),
            "enabled: computer_use must stay visible"
        );
    }

    /// Turning the master switch off revokes all session grants and all
    /// consent state (pending confirmations, minted tokens) but does **not**
    /// raise the stop flag (distinct from stop_all semantics) — after
    /// re-enabling, the old grant must not resurrect, and in a
    /// disable→enable cycle the old minted tokens must not be replayed
    /// confirmation-free.
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
        // Minted tokens are cleared too: after re-enabling + re-granting,
        // old tokens must not allow a confirmation-free replay.
        assert_eq!(
            take(&shared, &s2_pending, "s2", "left click 3"),
            ConfirmationCheck::Unknown,
            "a disabled cycle must wipe minted approval tokens"
        );
        // Distinct from stop_all semantics: the stop flag is not raised and
        // observe actions still work.
        assert!(!shared.is_stopped());
        assert!(shared.check_readonly().is_ok());
    }

    #[test]
    fn pending_confirmation_expires_after_ttl() {
        let shared = enabled_shared();
        let id = new_pending(&shared, "s1", "left_click (100,200)");
        // Manually wind created_at back past the TTL (waiting a real 5
        // minutes is too slow).
        {
            let mut consent = shared.consent.lock();
            if let Some(entry) = consent.pending.get_mut(&id) {
                entry.created_at = Instant::now() - CONFIRM_TTL - Duration::from_secs(1);
            }
        }
        // Expired means nonexistent.
        assert!(shared.pending_confirmation(&id).is_none());
        // An expired pending no longer mints: the token is unavailable and
        // mint reports the failure explicitly.
        assert!(!shared.mint_confirmation(&id));
        assert_eq!(
            take(&shared, &id, "s1", "left_click (100,200)"),
            ConfirmationCheck::Unknown
        );
    }

    /// A MINTED approval token past [`CONFIRM_TTL`] must be refused at spend
    /// time — the spend-path expiry branch is the only defense against a
    /// token minted then spent minutes later.
    #[test]
    fn minted_token_expires_at_spend() {
        let shared = enabled_shared();
        let summary = "left click x1 at Some((100, 200))";
        let id = new_pending(&shared, "s1", summary);
        assert!(shared.mint_confirmation(&id), "fresh pending must mint");
        // Manually wind minted_at back past the TTL (waiting a real 5 minutes
        // is too slow).
        {
            let mut consent = shared.consent.lock();
            if let Some(token) = consent.approved_tokens.get_mut(&id) {
                token.minted_at = Instant::now() - CONFIRM_TTL - Duration::from_secs(1);
            }
        }
        // An expired token reports Unknown at spend and is removed (a replay
        // is Unknown too).
        assert_eq!(
            take(&shared, &id, "s1", summary),
            ConfirmationCheck::Unknown
        );
        assert_eq!(
            take(&shared, &id, "s1", summary),
            ConfirmationCheck::Unknown
        );
    }

    /// The English "format" entry must stay: it is the reason the CJK
    /// entries (格式化/フォーマット) exist, and its absence let an English
    /// "Format disk" screen Clear while the CJK label confirmed.
    #[test]
    fn t3_denylist_covers_english_format() {
        assert!(matches_t3_denylist("Format Disk"));
        assert!(matches_t3_denylist("格式化磁盘"));
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

    /// Stop race: a pending that an in-flight run re-creates AFTER stop_all
    /// must not mint a token while the stop latch is raised — and must still
    /// be unusable after a later re-enable (reset_stop sweeps consent created
    /// during the stopped window).
    #[test]
    fn mint_refuses_consent_created_while_stopped() {
        let shared = enabled_shared();
        shared.stop_all();
        let id = new_pending(&shared, "s1", "left click");
        assert!(
            !shared.mint_confirmation(&id),
            "mint must refuse while the stop latch is raised"
        );
        // Re-enabling sweeps the stopped-window pending: the stale dialog
        // stays dead even though the stop flag is now cleared.
        shared.reset_stop();
        assert!(
            !shared.mint_confirmation(&id),
            "a pending created during the stop window must not mint after resume"
        );
        assert!(shared.pending_confirmation(&id).is_none());
    }

    /// Stop race, mint side: a token minted before the stop is wiped by
    /// stop_all and cannot be resurrected by a re-enable.
    #[test]
    fn stop_then_resume_leaves_no_mintable_token() {
        let shared = enabled_shared();
        let id = new_pending(&shared, "s1", "left click");
        assert!(shared.mint_confirmation(&id));
        shared.stop_all();
        shared.reset_stop();
        assert_eq!(
            take(&shared, &id, "s1", "left click"),
            ConfirmationCheck::Unknown,
            "no token may survive stop → resume"
        );
    }

    /// Content binding: the token is bound to the action content hash in
    /// addition to the summary — a same-summary, different-content spend is
    /// rejected and keeps the token.
    #[test]
    fn token_binding_covers_the_action_content() {
        let shared = enabled_shared();
        let id = shared.new_pending_confirmation("s1", "type 3 characters", "field", 42);
        assert!(shared.mint_confirmation(&id));
        assert_eq!(
            shared.take_confirmation(&id, "s1", "type 3 characters", 43),
            ConfirmationCheck::Unknown,
            "a different content hash must not spend the token"
        );
        assert_eq!(
            shared.take_confirmation(&id, "s1", "type 3 characters", 42),
            ConfirmationCheck::Granted,
            "the exact approved content spends it; the failed attempt kept the token"
        );
        // Mint refuses while disabled, too (toggle-off race symmetry).
        let shared2 = enabled_shared();
        let id2 = shared2.new_pending_confirmation("s2", "left click", "Buy now", 7);
        shared2.set_enabled(false);
        assert!(
            !shared2.mint_confirmation(&id2),
            "mint must refuse while the master switch is off"
        );
    }
}
