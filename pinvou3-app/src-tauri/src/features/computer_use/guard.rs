//! Consent guard: the settings toggle, session grants, the stop flag, T3
//! confirmation.
//!
//! The engine currently auto-approves all tool calls, so consent gating must
//! be built into the tool itself — this module is the single source of truth.
//! The integration layer (Tauri commands) injects the user's decisions through
//! the public API: `set_enabled` / `grant_session` / `revoke_session` /
//! `stop_all` / `mint_confirmation`. Grants live only in memory and are never
//! persisted; a session grant lives until it is explicitly revoked (revoke /
//! stop / master-switch off / session end — session end is the engine
//! reclaiming the tool: its `Drop` calls `revoke_session`), no idle expiry —
//! no mainstream product puts an idle clock on session-level grants.
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::backend::BackendRegistry;

/// The validity window of a T3 confirmation: an unanswered pending must
/// expire on its own — after expiry it reads as nonexistent and can no longer
/// mint an approval token.
pub const CONFIRM_TTL: Duration = Duration::from_secs(5 * 60);
/// Bounded-wait cap for the cross-session physical input lock. Input actions
/// hold the lock from screening through injection (up to a full backend call
/// cap, see backend.rs's `BACKEND_CALL_TIMEOUT`); waiting unboundedly would
/// silently wedge other sessions (review finding) — on timeout, fail
/// explicitly and let the model wait and retry.
pub const PHYSICAL_INPUT_LOCK_TIMEOUT: Duration = Duration::from_secs(20);

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
    // Sends.
    "send",
    "发送",
    "傳送",
    "送信",
    // Irreversible deletion, including common drag/delete destinations.
    "delete",
    "trash",
    "erase",
    "discard",
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
    // Form/order submission.
    "submit",
    "提交",
    "提出",
    // Terms/consent acceptance.
    "accept",
    "agree",
    "同意",
    "接受",
];

/// Whether a label hits the T3 consequential denylist.
pub fn matches_t3_denylist(label: &str) -> bool {
    let lower = label.to_lowercase();
    T3_DENYLIST.iter().any(|term| lower.contains(term))
}

/// Whether the element role is a password/secure text field (a T3 signal;
/// corresponds to Operator's takeover scenario).
pub fn is_secure_role(role: &str) -> bool {
    let lower = role.to_lowercase();
    lower.contains("password") || lower.contains("secure")
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

/// A session grant records only "whether this session holds a grant": the
/// grant lives until explicitly revoked (revoke / stop / master-switch off /
/// session end — session end is the engine reclaiming the tool (the tool's
/// `Drop` revokes)), no idle expiry — no mainstream product puts an idle
/// clock on session-level grants (Claude Code's "allow for this session" is
/// the same position).

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
    /// approval token is bound to exactly this summary.
    pub action_summary: String,
    pub element_label: String,
    pub created_at: Instant,
}

/// A minted approval token: bound to the session and the action summary;
/// expires if not spent within [`CONFIRM_TTL`].
#[derive(Debug, Clone)]
struct ApprovedToken {
    session_id: String,
    action_summary: String,
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
    /// The set of session ids holding a session grant (a grant lives until
    /// explicitly revoked, see the lifetime note at the top of this module).
    sessions: Mutex<HashSet<String>>,
    /// The physical mouse/keyboard is a globally exclusive resource, but the
    /// backend has one worker per session — this process-level lock
    /// serializes input injection **across sessions** (review finding: two
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

    /// Grants this session input control (a session grant). The grant lives
    /// until explicitly revoked (revoke / stop / master-switch off / session
    /// end — session end is the engine reclaiming the tool (the tool's `Drop`
    /// revokes)), no idle expiry — no mainstream product puts an idle clock on
    /// session-level grants (Claude Code's "allow for this session" is the
    /// same position).
    pub fn grant_session(&self, session_id: &str) {
        self.sessions.lock().insert(session_id.to_string());
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

    /// Whether the session currently holds a valid grant (not cleared by
    /// revoke/emergency stop). A read-only projection for status commands;
    /// gating decisions still go by [`Self::begin_input_action`].
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
    pub fn reset_stop(&self) {
        self.stop.store(false, Ordering::SeqCst);
    }

    /// Revokes all session grants and clears all consent state (pending
    /// confirmations, minted tokens) but does **not** raise the stop flag —
    /// distinct from [`Self::stop_all`]'s emergency-stop semantics: turning
    /// the master switch off is not an emergency stop, and no stop state
    /// should remain after re-enabling (the `computer_use_set_enabled(false)`
    /// call). Review finding: previously, turning the switch off did not
    /// clear grants and tokens, so after re-enabling the old grant and old
    /// approval tokens remained valid — consent state from the off period
    /// must not survive a re-enable.
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

    /// Gate for input-class actions: switch on, not stopped, and the session
    /// holds a grant (grants live until explicitly revoked, no idle expiry).
    /// [`Self::verify_input_action`] must be checked once more before
    /// injection.
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
    /// screenshot (review finding: a revoke inside that window did not take
    /// effect); must be called before injecting.
    pub fn verify_input_action(&self, session_id: &str) -> Result<(), GuardRejection> {
        self.check_readonly()?;
        if !self.sessions.lock().contains(session_id) {
            return Err(GuardRejection::GrantRequired);
        }
        Ok(())
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
            .try_lock_for(PHYSICAL_INPUT_LOCK_TIMEOUT)
            .ok_or(GuardRejection::InputBusy)
    }

    /// Registers a T3 confirmation waiting for the user's decision and
    /// returns the confirm_id. At most one pending per session: a new request
    /// replaces the session's existing pending (newest wins, like an ordinary
    /// dialog), so this method cannot fail. Also sweeps expired pendings.
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
        // confirm_id's minted token is unspent, retract it too (review
        // finding: deny used to clear only the pending, so the token from
        // "approved → changed my mind" lived out its TTL and the deny button
        // silently did nothing inside the race window). Returns false when
        // the token does not exist (already spent / expired / already cleared
        // by revoke), the same as an unknown id.
        consent.approved_tokens.remove(confirm_id).is_some()
    }

    /// Mints an approval token. Callable only by the `computer_use_confirm`
    /// Tauri command — the model must never be able to mint one via a tool
    /// call. Mints only for a pending that exists and is unexpired, returning
    /// `true`; returns `false` when the pending does not exist / has expired /
    /// was already decided — a silent no-op would let the frontend show a
    /// failure as success (review finding). The token inherits the pending's
    /// session and action summary (compared item by item at spend time).
    /// Tokens have no stock cap: a session has at most one pending at a time,
    /// and minting removes the pending, so the token stock is naturally
    /// bounded by the interaction cadence; expiry is backstopped by the TTL
    /// sweep before spending.
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

    /// Spends an approval token (single-use). The tool calls it before
    /// executing an action carrying a `confirm_id`; the token must exactly
    /// match **this** session and the action summary (review finding: a bare
    /// string token could be spent on any action/session). On a match,
    /// execution proceeds — no second screening (mainstream model: the API
    /// confirmation is just a per-action confirmation id; once the client
    /// acknowledges, execute); on a mismatch the token is kept (under exact
    /// binding, the only combination that can pass is the user-approved
    /// original action replay — a wrong attempt should not burn the user's
    /// confirmation).
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

    fn enabled_shared() -> ComputerUseShared {
        let shared = ComputerUseShared::new();
        shared.set_enabled(true);
        shared
    }

    /// Test convenience wrapper: the standard three-argument pending / spend.
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
    fn has_active_grant_reflects_grant_and_revoke() {
        let shared = enabled_shared();
        assert!(!shared.has_active_grant("s1"));
        shared.grant_session("s1");
        assert!(shared.has_active_grant("s1"));
        shared.revoke_session("s1");
        assert!(!shared.has_active_grant("s1"));
    }

    /// Session grants have no idle expiry: once granted, a grant lives until
    /// explicitly revoked — repeated gating/re-checks, granting other
    /// sessions, or re-granting this session never clears it (the old
    /// grant_session also swept "idle-expired" sessions; that mechanism was
    /// removed entirely, and this test pins that no implicit invalidation
    /// path exists).
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
            // Sends.
            "Send message",
            "发送",
            "傳送",
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

    /// Review-fix regression: a "denial" after minting is a change of heart —
    /// the same confirm_id's unspent token must be retracted along with it,
    /// not live out its TTL (deny used to clear only the pending; after
    /// minting, deny silently did nothing, and the frontend also returned
    /// false on this wrong basis).
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

    /// Review-fix regression: turning the master switch off revokes all
    /// session grants and all consent state (pending confirmations, minted
    /// tokens) but does **not** raise the stop flag (distinct from stop_all
    /// semantics) — after re-enabling, the old grant must not resurrect, and
    /// in a disable→enable cycle the old minted tokens must not be replayed
    /// confirmation-free (third-round review finding).
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

    /// Regression (round-10 review m12): a MINTED approval token past
    /// [`CONFIRM_TTL`] must be refused at spend time — previously only the
    /// pending side of the TTL had test coverage, so the spend-path expiry
    /// branch (the only defense against a token minted then spent minutes
    /// later) was untested.
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
