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

/// The T3 consequential-action denylist. Terms are matched as substrings of
/// [`fold_for_matching`] output, so every entry must itself already be in
/// folded form: lowercase, and free of whitespace (the fold strips it, so a
/// label split as `支 付` or `Place  Order` still matches).
/// `t3_denylist_terms_are_prefolded` pins both properties.
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
    "placeorder",
    "ordernow",
    "购买",
    "購買",
    "支付",
    "付款",
    "转账",
    "转帐",
    "结算",
    "轉賬",
    "轉帳",
    "結算",
    "购入",
    "購入",
    // Stem, not 支払い: `"支払う".contains("支払い")` is false, so the
    // inflected form on a real 支払う button used to screen Clear. 決済 is
    // the standard Japanese checkout verb and 振込/振替 the standard bank
    // transfer terms; none were covered by 送金 alone.
    "支払",
    "決済",
    "振込",
    "振替",
    "送金",
    "注文",
    "下单",
    "下單",
    "充值",
    "儲值",
    "提现",
    "提現",
    "汇款",
    "匯款",
    "捐款",
    "捐赠",
    "捐贈",
    "投资",
    "投資",
    "订阅",
    "訂閱",
    "subscribe",
    "withdraw",
    "remit",
    "donate",
    // Both spellings: the fold strips whitespace but keeps the hyphen, so
    // "Top up" folds to "topup" while "Top-up" keeps its hyphen.
    "top-up",
    "topup",
    // Sends.
    "send",
    "发送",
    "發送",
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
    // The standard Japanese "initialize / factory reset" button label; the
    // 格式化/フォーマット pair does not cover it.
    "初期化",
    // Form/order submission.
    "submit",
    "提交",
    "提出",
    // Terms/consent acceptance.
    "accept",
    "agree",
    "同意",
    "接受",
    "承诺",
    "承諾",
];

/// The longest [`T3_DENYLIST`] term, in folded characters. The streaming
/// matcher in `platform::helpers` carries this much context across its window
/// boundaries, so a term straddling two windows is still found; a longer term
/// would be silently unmatchable, which `t3_denylist_terms_are_prefolded`
/// pins.
pub const T3_MATCH_WINDOW_CHARS: usize = 32;

/// Whether `c` is invisible to the user and therefore must not separate two
/// halves of a denylist term.
///
/// Covers C0/C1 controls and whitespace plus the format and default-ignorable
/// characters a hostile label can splice into a word while rendering
/// identically: soft hyphen, the zero-width space/joiner family, the bidi
/// overrides and isolates, the invisible-operator block, the byte-order mark,
/// the combining grapheme joiner, the Hangul fillers, the variation selectors
/// (both planes), the interlinear annotation controls, the invisible musical
/// beam controls, and the Tag block.
///
/// This is an enumeration of `Default_Ignorable_Code_Point` rather than a
/// property lookup: the ranges are stable, and pulling a full property table
/// in for one predicate is not worth the dependency. `zero_width_evasions`
/// pins the list against the demonstrated families.
fn is_invisible_for_matching(c: char) -> bool {
    c.is_control()
        || c.is_whitespace()
        || matches!(c,
            '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'..='\u{1160}'
            | '\u{17B4}'..='\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2065}'
            | '\u{2066}'..='\u{2069}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}')
}

/// Maps a character to the Latin letter it is visually indistinguishable
/// from.
///
/// This covers only the **cross-script** homoglyphs, which have no Unicode
/// compatibility decomposition and therefore survive the NFKC pass in
/// [`fold_for_matching`]: the Cyrillic and Greek letters that share a glyph
/// with Latin (`Pаy` with a Cyrillic `а`, `ԁelete` with a Komi de). The
/// compatibility families — fullwidth ASCII, the Math Alphanumeric block,
/// circled and parenthesized letters, halfwidth katakana — are NFKC's job and
/// are deliberately absent here.
///
/// It is a best-effort subset, not a complete confusable table: the full
/// relation is UTS#39's, and a determined attacker can still find a glyph pair
/// this misses. It raises the cost of the demonstrated single-substitution
/// evasions; it does not make them impossible.
fn fold_confusable(c: char) -> char {
    match c {
        // Cyrillic look-alikes (lowercase and uppercase folded to lowercase
        // Latin; the caller lowercases afterwards either way).
        'а' | 'А' => 'a',
        'ԁ' => 'd',
        'в' | 'В' => 'b',
        'с' | 'С' => 'c',
        'е' | 'Е' | 'ё' | 'Ё' => 'e',
        'н' | 'Н' => 'h',
        'і' | 'І' => 'i',
        'ј' | 'Ј' => 'j',
        'к' | 'К' => 'k',
        'м' | 'М' => 'm',
        'о' | 'О' => 'o',
        'р' | 'Р' => 'p',
        'ѕ' | 'Ѕ' => 's',
        'т' | 'Т' => 't',
        'у' | 'У' => 'y',
        'х' | 'Х' => 'x',
        // Greek look-alikes.
        'α' | 'Α' => 'a',
        'Β' => 'b',
        'ε' | 'Ε' => 'e',
        'Η' => 'h',
        'ι' | 'Ι' => 'i',
        'κ' | 'Κ' => 'k',
        'Μ' => 'm',
        'Ν' => 'n',
        'ο' | 'Ο' => 'o',
        'ρ' | 'Ρ' => 'p',
        'τ' | 'Τ' => 't',
        'υ' | 'Υ' => 'y',
        'χ' | 'Χ' => 'x',
        'Ζ' => 'z',
        // Other single-script look-alikes with no compatibility mapping.
        'ɑ' => 'a',
        'օ' => 'o',
        other => other,
    }
}

/// Folds text for denylist matching: drops everything invisible (controls,
/// whitespace, zero-width, bidi and default-ignorable formatting), applies
/// NFKC, maps the remaining cross-script confusables to Latin, then
/// lowercases.
///
/// Each step closes a demonstrated evasion of the plain `to_lowercase()` match
/// this replaces. A hostile `aria-label` only had to carry a zero-width space
/// (`De<U+200B>lete`), a soft hyphen, fullwidth letters, a single Cyrillic `а`
/// (`Pаy`) or an inserted space (`支 付`) to screen Clear while rendering
/// identically to the user. Dropping the invisible characters rather than
/// folding them to a space also closes the inverse hole, where space-folding a
/// C0 character split a term the matcher would have found.
///
/// NFKC runs **after** the invisible filter so a spliced default-ignorable
/// cannot block a compatibility composition, and it is what collapses the
/// whole-block substitutions a per-character table would have to enumerate
/// one family at a time: `𝐃𝐞𝐥𝐞𝐭𝐞` (Math Alphanumeric), `Ｄｅｌｅｔｅ`
/// (fullwidth), `Ⓓⓔⓛⓔⓣⓔ` (circled), `ﾌｫｰﾏｯﾄ` (halfwidth katakana) and the
/// CJK compatibility ideographs all fold to their ordinary forms.
///
/// This is not a complete confusable defence and is not meant to be read as
/// one: combining marks are left in place (they are *visible*, so they change
/// what the user sees rather than hiding from them), and the cross-script
/// table in [`fold_confusable`] is a subset of UTS#39's relation. The
/// guarantee is that the demonstrated evasion families cost more than one
/// code point, not that no glyph substitution can succeed.
pub fn fold_for_matching(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization as _;

    text.chars()
        .filter(|c| !is_invisible_for_matching(*c))
        .nfkc()
        .map(fold_confusable)
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether a label hits the T3 consequential denylist.
pub fn matches_t3_denylist(label: &str) -> bool {
    matches_t3_denylist_folded(&fold_for_matching(label))
}

/// The denylist match over text that is already [`fold_for_matching`] output.
/// Exposed for the streaming matcher, which folds in bounded chunks so an
/// arbitrarily long attacker-controlled label never has to be materialized.
pub(crate) fn matches_t3_denylist_folded(folded: &str) -> bool {
    T3_DENYLIST.iter().any(|term| folded.contains(term))
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
    /// A confirmation dialog is unanswered. Every input action from **every**
    /// session is rejected while any pending confirmation exists: otherwise
    /// the model could click the dialog's own approve control — an ordinary
    /// clickable element whose label screens Clear — and mint the approval
    /// itself (round-17 self-approval finding). Observing stays allowed;
    /// deny/stop/disable/expiry all clear the pending and unblock.
    ///
    /// The block is process-wide rather than per-session because the dialog
    /// is a process-global window and physical input is a process-global
    /// device: a per-session block left a second granted session free to
    /// click the first session's "Allow this once" and mint its approval,
    /// which is the same self-approval hole one indirection further out.
    ConfirmationPending,
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
            Self::ConfirmationPending => {
                "an action is waiting for the user's confirmation in the app. No input actions are accepted until that dialog is answered (approved, denied, or stopped); wait for the user, or stop if the request is abandoned."
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
    /// The full `confirm_required` event payload exactly as minted
    /// (identity + structured fields + optional preview), served through
    /// `computer_use_get_status` so every window reconciles from server
    /// truth: dialogs resolved in one window collapse in the others, and a
    /// window that (re)loads mid-request reconstructs the dialog. The
    /// payload was already broadcast to every window at mint time, so
    /// re-serving it adds no exposure.
    pub payload: serde_json::Value,
}

/// A minted approval token: bound to the session, the action summary, the
/// action content hash **and** the element label the dialog showed the user;
/// expires if not spent within [`CONFIRM_TTL`].
#[derive(Debug, Clone)]
struct ApprovedToken {
    session_id: String,
    action_summary: String,
    action_binding: u64,
    /// The screened target the user actually saw and approved. The action
    /// parameters alone do not identify a target: `left_mouse_down` and a
    /// coordinate-less `left_click` act wherever the cursor happens to be,
    /// and even a coordinate-carrying click lands on whatever occupies that
    /// point. Carrying the label lets the spend path re-screen and refuse a
    /// token aimed at a different consequential control than the one on the
    /// dialog.
    element_label: String,
    minted_at: Instant,
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
    /// The set of session ids holding a session grant. A session grant
    /// records only "whether this session holds a grant": the grant is bound
    /// to the tool instance's lifetime — it ends on revoke / stop /
    /// master-switch off, or when the engine reclaims the tool (the tool's
    /// `Drop` revokes), which includes the engine pool's idle reaper
    /// silently reaping an idle engine (the grant then lapses without any
    /// user action and the next input attempt reads `GrantRequired`).
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
    /// Sessions with an unanswered grant request (advisory UI truth, set
    /// when the tool emits `grant_required`; every lifecycle transition
    /// that ends the request clears it). Served through
    /// `computer_use_get_status` alongside the pending confirmation.
    grant_requests: Mutex<HashSet<String>>,
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
            grant_requests: Mutex::new(HashSet::new()),
            backends: BackendRegistry::default(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// The settings toggle (called by the integration layer's settings
    /// command). The consent sweeps are part of the flip itself, not a
    /// caller duty: consent state from the off period must not survive a
    /// re-enable and consent state from the stopped period must not survive
    /// the resume, so `set_enabled(false)` performs the full
    /// [`Self::revoke_all_sessions`] sweep and `set_enabled(true)`
    /// performs the full [`Self::reset_stop`] sweep — no future caller can
    /// flip the flag without the sweep (previously the invariant was
    /// enforced only by the command remembering to pair the calls). The
    /// flip preserves the swept methods' semantics: disabling does not
    /// raise the stop flag and enabling restores no grants. What the guard
    /// does not own stays with the caller — the backend OS-grant
    /// termination on disable.
    pub fn set_enabled(&self, enabled: bool) {
        // Store first, sweep second (the order the command previously
        // used): a grant racing the disable slips in only before the store
        // and is then removed by the sweep; after the store,
        // `grant_session` refuses under the sessions lock.
        self.enabled.store(enabled, Ordering::SeqCst);
        if enabled {
            self.reset_stop();
        } else {
            self.revoke_all_sessions();
            // Turning the master switch off is not an emergency stop, and it
            // leaves nothing for a stop to protect: the sweep above already
            // dropped every grant, pending and token. Leaving the latch raised
            // made `computer_use_get_status` keep reporting `stopped: true`
            // for a feature that is simply off, which is what drove the
            // settings row to tell a user who had just switched the toggle OFF
            // to "turn it off and back on". The flag is cleared here, in the
            // one place that owns it, so the backend answer and the UI agree
            // without the frontend having to guess.
            self.stop.store(false, Ordering::SeqCst);
        }
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
        self.grant_requests.lock().remove(session_id);
        GrantOutcome::Granted
    }

    /// Revoke a single session: besides the grant row, also drop that
    /// session's pending confirmations and minted approval tokens (all under
    /// the single consent lock). After the user withdraws control, this
    /// session's consent artifacts for previously blocked actions must not
    /// survive; other sessions' artifacts are untouched.
    pub fn revoke_session(&self, session_id: &str) {
        self.sessions.lock().remove(session_id);
        self.grant_requests.lock().remove(session_id);
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
        self.grant_requests.lock().clear();
        let mut consent = self.consent.lock();
        consent.pending.clear();
        consent.approved_tokens.clear();
    }

    /// Clears the stop flag when the user re-enables (restores no grants).
    /// Also sweeps any consent state an in-flight run created between the
    /// stop and the re-enable: a pending minted during the stopped window
    /// would otherwise become mintable the moment the stop is lifted —
    /// consent state from the stopped period must not survive the resume,
    /// same rule as `revoke_all_sessions` for the off period. Also invoked
    /// by [`Self::set_enabled`] on re-enable, so the sweep cannot be
    /// skipped by a flag-only caller.
    pub fn reset_stop(&self) {
        self.stop.store(false, Ordering::SeqCst);
        self.grant_requests.lock().clear();
        let mut consent = self.consent.lock();
        consent.pending.clear();
        consent.approved_tokens.clear();
    }

    /// Revokes all session grants and clears all consent state (pending
    /// confirmations, minted tokens) but does not touch the stop flag —
    /// distinct from [`Self::stop_all`]'s emergency-stop semantics: turning
    /// the master switch off is not an emergency stop. Consent state from the
    /// off period must not survive a re-enable: the old grant and old approval
    /// tokens would otherwise remain valid. Also invoked by
    /// [`Self::set_enabled`] on disable, so the sweep cannot be skipped by a
    /// flag-only caller; [`Self::set_enabled`] additionally lowers the stop
    /// latch there, so no stop state remains for a feature that is off.
    pub fn revoke_all_sessions(&self) {
        self.sessions.lock().clear();
        self.grant_requests.lock().clear();
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

    /// Gate for input-class actions: switch on, not stopped, the session
    /// holds a grant (grant lifetime: see the note at the top of this
    /// module), and no confirmation dialog anywhere in the process is
    /// unanswered (an unanswered dialog must not be clickable by the model
    /// itself — the approve control is an ordinary clickable element, so
    /// letting input through while a pending exists would let a session
    /// approve a consequential action). [`Self::verify_input_action`] must be
    /// checked once more before injection.
    pub fn begin_input_action(&self, session_id: &str) -> Result<(), GuardRejection> {
        self.check_readonly()?;
        if !self.sessions.lock().contains(session_id) {
            return Err(GuardRejection::GrantRequired);
        }
        if self.has_outstanding_pending() {
            return Err(GuardRejection::ConfirmationPending);
        }
        Ok(())
    }

    /// Whether **any** session has an unanswered (unexpired) pending
    /// confirmation. Expired pendings are swept under the same lock so a
    /// TTL'd-out dialog cannot block input until some other path happens to
    /// clear it.
    ///
    /// Deliberately not scoped to the calling session: the consent dialog is
    /// a process-global window and physical input is a process-global device,
    /// so a per-session block let a second granted session click the first
    /// session's approve control (see [`GuardRejection::ConfirmationPending`]).
    fn has_outstanding_pending(&self) -> bool {
        let mut consent = self.consent.lock();
        let now = Instant::now();
        consent
            .pending
            .retain(|_, entry| now.duration_since(entry.created_at) <= CONFIRM_TTL);
        !consent.pending.is_empty()
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
    /// dialog). `action_binding` is the tool layer's content hash of the
    /// blocked action and is carried into the minted token. Also sweeps
    /// expired pendings.
    ///
    /// Returns `None` when the master switch is off or the stop is latched:
    /// `mint_confirmation` refuses in that state, so a pending registered
    /// there could never be approved — an in-flight run racing a
    /// disable/stop must not leave an unapprovable dialog on screen for the
    /// TTL (its caller reports the refusal as part of the same stable error
    /// prefix, so the audit shape is unchanged).
    pub fn new_pending_confirmation(
        &self,
        session_id: &str,
        action_summary: impl Into<String>,
        element_label: impl Into<String>,
        action_binding: u64,
    ) -> Option<String> {
        let confirm_id = format!("cu-{:016x}", rand::random::<u64>());
        let now = Instant::now();
        let mut consent = self.consent.lock();
        if !self.is_enabled() || self.is_stopped() {
            return None;
        }
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
                // Attached right after the mint via `set_pending_payload`
                // (the payload embeds the mint-returned confirm_id).
                payload: serde_json::Value::Null,
            },
        );
        Some(confirm_id)
    }

    /// Server truth for the consent UI: the newest unexpired pending for
    /// this session with its full event payload, or `None` when nothing is
    /// pending (including after a decision, expiry, stop, revoke or
    /// disable — the sweeps all remove the entry). Expired entries are
    /// swept here so a dialog cannot be reconstructed after its TTL.
    pub fn pending_payload_for_session(&self, session_id: &str) -> Option<serde_json::Value> {
        let mut consent = self.consent.lock();
        let now = Instant::now();
        consent
            .pending
            .retain(|_, entry| now.duration_since(entry.created_at) <= CONFIRM_TTL);
        consent
            .pending
            .values()
            .find(|entry| entry.session_id == session_id)
            .map(|entry| entry.payload.clone())
    }

    /// Attaches the event payload to a just-minted pending (the payload
    /// embeds the mint-returned confirm_id, so it can only be built after
    /// the mint). No-op when the pending no longer exists (replaced by a
    /// newer request or swept): a payload without a live pending is never
    /// served.
    pub fn set_pending_payload(&self, confirm_id: &str, payload: serde_json::Value) {
        let mut consent = self.consent.lock();
        if let Some(entry) = consent.pending.get_mut(confirm_id) {
            entry.payload = payload;
        }
    }

    /// Marks a session as having an unanswered grant request (called right
    /// before the tool emits `grant_required`).
    pub fn mark_grant_requested(&self, session_id: &str) {
        self.grant_requests.lock().insert(session_id.to_string());
    }

    /// Server truth for the consent UI: whether this session's grant
    /// request is still unanswered (granted/revoke/stop/disable all clear
    /// the marker).
    pub fn grant_request_pending(&self, session_id: &str) -> bool {
        self.grant_requests.lock().contains(session_id)
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

    /// Test helper: deny the session's newest outstanding pending (what a
    /// user clicking Deny on the dialog does), so a test can proceed past an
    /// intercepted action to its next leg.
    #[cfg(test)]
    pub(crate) fn deny_newest_pending_for_tests(&self, session_id: &str) -> bool {
        let mut consent = self.consent.lock();
        let now = Instant::now();
        consent
            .pending
            .retain(|_, entry| now.duration_since(entry.created_at) <= CONFIRM_TTL);
        let Some(id) = consent
            .pending
            .iter()
            .find(|(_, entry)| entry.session_id == session_id)
            .map(|(id, _)| id.clone())
        else {
            return false;
        };
        consent.pending.remove(&id).is_some()
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
        let Some(entry) = consent.pending.get(confirm_id) else {
            return false;
        };
        let expired = entry.created_at.elapsed() > CONFIRM_TTL;
        // The session must still hold its grant: a pending created by an
        // in-flight run whose grant was concurrently revoked must not mint a
        // token for a grant-less session. (The tool's last-moment verify
        // rejects the action anyway — this only stops the dead token from
        // existing at all, and sweeping the pending here collapses the
        // dialog instead of serving it until the TTL.) Checked under the
        // consent lock with a nested `sessions` read — the reverse nesting
        // nowhere exists (grant_session nests sessions → grant_requests;
        // the sweeps take the locks sequentially), so no ordering cycle is
        // introduced.
        let granted = self.sessions.lock().contains(entry.session_id.as_str());
        if expired || !granted {
            consent.pending.remove(confirm_id);
            return false;
        }
        let Some(entry) = consent.pending.remove(confirm_id) else {
            unreachable!("entry was read under the same unexpired consent lock")
        };
        let now = Instant::now();
        let tokens = &mut consent.approved_tokens;
        tokens.retain(|_, token| now.duration_since(token.minted_at) <= CONFIRM_TTL);
        tokens.insert(
            confirm_id.to_string(),
            ApprovedToken {
                session_id: entry.session_id,
                action_summary: entry.action_summary,
                action_binding: entry.action_binding,
                element_label: entry.element_label,
                minted_at: now,
            },
        );
        true
    }

    /// Validates an approval token **without consuming it** and returns the
    /// element label the user approved.
    ///
    /// The token must exactly match this session, the action summary and the
    /// action content hash — the user approves a summary for readability, but
    /// the token is bound to the full action content, so a token approved for
    /// one `type N characters` cannot be spent on a different same-length
    /// text. `None` on any mismatch, and the token is kept: under exact
    /// binding the only combination that can pass is a replay of the
    /// user-approved action, so a wrong attempt should not burn the
    /// confirmation.
    ///
    /// Split from the consume step so the caller can re-screen the current
    /// target against the returned label before spending: the action's own
    /// parameters do not pin a target, so validating and consuming in one
    /// step let an approval granted for one control be spent on another (see
    /// [`ApprovedToken::element_label`]).
    pub fn peek_confirmation(
        &self,
        confirm_id: &str,
        session_id: &str,
        action_summary: &str,
        action_binding: u64,
    ) -> Option<String> {
        // Defense in depth (mirrors the mint side): a token minted while
        // enabled must not be spendable after a stop/disable landed — the
        // spend path still dies at verify_input_action, but consuming the
        // user's approval there would be the wrong direction. The check lives
        // under the consent lock (the mint side always did), so a disable
        // landing between the check and the spend cannot consume the approval.
        let now = Instant::now();
        let mut consent = self.consent.lock();
        if !self.is_enabled() || self.is_stopped() {
            return None;
        }
        let token = consent.approved_tokens.get(confirm_id)?;
        if now.duration_since(token.minted_at) > CONFIRM_TTL {
            consent.approved_tokens.remove(confirm_id);
            return None;
        }
        if token.session_id != session_id
            || token.action_summary != action_summary
            || token.action_binding != action_binding
        {
            return None;
        }
        Some(token.element_label.clone())
    }

    /// Consumes a token previously validated by [`Self::peek_confirmation`],
    /// making it single-use.
    ///
    /// Called once the re-screen has agreed the target is still the one the
    /// user approved, so a spend refused for naming a **different** target
    /// does not burn the approval and a corrected retry can still use it.
    /// Later refusals still consume it — a stop or revoke landing during
    /// screening, a missing input capability, the backend itself failing. The
    /// split exists to make the target check non-destructive, not to defer the
    /// spend all the way to the injection call.
    pub fn consume_confirmation(&self, confirm_id: &str) {
        self.consent.lock().approved_tokens.remove(confirm_id);
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
        shared
            .new_pending_confirmation(session, summary, "Buy now", 0)
            .expect("pending minted while the feature is enabled")
    }

    /// The spend outcome, as a value the assertions can compare. Production
    /// splits validate-then-consume so the tool can re-screen the target in
    /// between; these tests exercise the session/summary/binding/TTL and
    /// single-use semantics, which are unchanged by that split.
    #[derive(Debug, PartialEq, Eq)]
    enum SpendOutcome {
        Granted,
        Unknown,
    }

    fn take(shared: &ComputerUseShared, id: &str, session: &str, summary: &str) -> SpendOutcome {
        take_bound(shared, id, session, summary, 0)
    }

    fn take_bound(
        shared: &ComputerUseShared,
        id: &str,
        session: &str,
        summary: &str,
        binding: u64,
    ) -> SpendOutcome {
        match shared.peek_confirmation(id, session, summary, binding) {
            Some(_) => {
                shared.consume_confirmation(id);
                SpendOutcome::Granted
            }
            None => SpendOutcome::Unknown,
        }
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
        // The terms this PR added, each with the inflected/variant spelling
        // that motivated it: an entry that only matches its own dictionary
        // form buys nothing on a real button.
        for label in [
            "支払う",
            "お支払いへ進む",
            "決済する",
            "振込を実行",
            "口座振替",
            "初期化する",
            "提现到银行卡",
            "提現",
            "汇款",
            "匯款",
            "订阅方案",
            "捐赠",
            "承诺并继续",
            "转帐",
            "购入",
            "發送訊息",
            "Withdraw funds",
            "Remit payment",
        ] {
            assert!(matches_t3_denylist(label), "should match: {label}");
        }
        assert!(is_secure_role("Password Text"));
        assert!(is_secure_role("AXSecureTextField"));
        assert!(!is_secure_role("button"));
        // "insecure" contains "secure" as a substring but is the opposite
        // signal — it must not trip the secure-role screen.
        assert!(!is_secure_role("AXInsecureTextField"));
        assert!(!is_secure_role("insecure text field"));
    }

    /// Every [`T3_DENYLIST`] entry must survive [`fold_for_matching`]
    /// unchanged, and must fit inside the streaming matcher's carry window.
    ///
    /// Both invariants are silent when broken, which is why they are pinned
    /// rather than reasoned about. Matching runs `folded.contains(term)`
    /// against folded text, so a term carrying a space, an uppercase letter,
    /// a fullwidth or compatibility form — anything the fold would rewrite —
    /// is not merely weaker, it is **unmatchable forever**: the fold has
    /// already removed from the haystack the very bytes the needle still
    /// carries. `"place order"` was exactly that bug before this list moved to
    /// folded matching. Likewise a term longer than
    /// [`T3_MATCH_WINDOW_CHARS`] would be missed whenever it straddles two
    /// streaming windows — an intermittent failure keyed on the label's
    /// length, which is the hardest possible shape to notice in the field.
    #[test]
    fn t3_denylist_terms_are_prefolded() {
        for term in T3_DENYLIST {
            assert_eq!(
                &fold_for_matching(term),
                term,
                "denylist term is not in folded form, so it can never match: {term:?}"
            );
            let folded_len = term.chars().count();
            assert!(
                folded_len <= T3_MATCH_WINDOW_CHARS,
                "denylist term is longer than the {T3_MATCH_WINDOW_CHARS}-char match \
                 window and would be missed across a chunk boundary: {term:?} ({folded_len})"
            );
            // `screen_element` also matches the *display* name as a fallback
            // for a backend that forgot the raw verdict, and `sanitize_name`
            // rewrites `"` to `'` on its way to the display copy. Folding does
            // not, so a term containing either quote would match the raw name
            // but not the display one — the fallback would silently stop being
            // a subset of the real verdict.
            assert!(
                !term.contains('"') && !term.contains('\''),
                "a denylist term must not contain a quote: sanitize_name rewrites \
                 them, so the display-name fallback would disagree: {term:?}"
            );
        }
    }

    /// The fold must see through the substitution families that were
    /// demonstrated as evasions, including the whole-block compatibility
    /// forms NFKC collapses.
    ///
    /// This pins the *families*, not a closed set: the doc on
    /// [`fold_for_matching`] is explicit that a complete confusable defence
    /// is not claimed. What must not regress is that each of these costs an
    /// attacker more than swapping one code point.
    #[test]
    fn zero_width_evasions() {
        for label in [
            // Invisible splices: each renders as plain "Delete".
            "De\u{200B}lete",
            "D\u{00AD}elete",
            "De\u{FE0F}lete",
            "De\u{FE00}lete",
            "De\u{034F}lete",
            "De\u{3164}lete",
            "De\u{115F}lete",
            "De\u{2065}lete",
            "De\u{E0041}lete",
            "De\u{1D173}lete",
            // Whole-block compatibility substitutions (NFKC).
            "\u{1D403}\u{1D41E}\u{1D425}\u{1D41E}\u{1D42D}\u{1D41E}", // math bold
            "\u{1D673}\u{1D68E}\u{1D695}\u{1D68E}\u{1D69D}\u{1D68E}", // math monospace
            "Ⓓⓔⓛⓔⓣⓔ",                                                 // circled
            "Ｄｅｌｅｔｅ",                                           // fullwidth
            "ﾌｫｰﾏｯﾄ",                                                 // halfwidth katakana
            // Cross-script homoglyphs.
            "Pаy",      // Cyrillic а
            "ԁelete",   // Cyrillic Komi de
            "Pɑy",      // Latin alpha
            "dօnate",   // Armenian o
            "τransfer", // Greek tau
            // Splitting and padding.
            "支 付",
            "D\u{0001}elete",
        ] {
            assert!(
                matches_t3_denylist(label),
                "fold must see through this evasion: {label:?}"
            );
        }
    }

    #[test]
    fn confirmation_tokens_are_single_use_and_bound_to_session_and_action() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        let summary = "left click x1 at Some((100, 200))";
        let id = new_pending(&shared, "s1", summary);
        let pending = shared.pending_confirmation(&id);
        assert!(pending.as_ref().is_some_and(|p| p.session_id == "s1"));
        // Cannot be spent before minting.
        assert_eq!(take(&shared, &id, "s1", summary), SpendOutcome::Unknown);
        assert!(
            shared.mint_confirmation(&id),
            "mint must report success for a live pending"
        );
        // The pending is cleared after minting.
        assert!(shared.pending_confirmation(&id).is_none());
        // Only the correct session + action summary can spend it.
        assert_eq!(
            take(&shared, &id, "s-other", summary),
            SpendOutcome::Unknown,
            "token minted for s1 must not be spent by another session"
        );
        assert_eq!(
            take(&shared, &id, "s1", "type 5 characters"),
            SpendOutcome::Unknown,
            "token must be bound to the action it approved"
        );
        // A mismatched wrong attempt does not destroy the token (under exact
        // binding, the only thing that can pass is the user-approved original
        // action).
        assert_eq!(take(&shared, &id, "s1", summary), SpendOutcome::Granted);
        // Single-use: the second spend fails.
        assert_eq!(take(&shared, &id, "s1", summary), SpendOutcome::Unknown);
    }

    /// deny consumes the pending; a denial records no server-side state — a
    /// retry of the same action mints a **new** pending (a new confirm_id) as
    /// usual and goes through the confirm flow again (mainstream model: a
    /// denial is only model-visible context).
    #[test]
    fn deny_confirmation_consumes_the_pending_and_retry_mints_a_new_one() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        let id = new_pending(&shared, "s1", "left click");
        assert!(shared.deny_confirmation(&id));
        // deny clears the pending: it can no longer mint (mint returns false,
        // no longer a silent no-op).
        assert!(shared.pending_confirmation(&id).is_none());
        assert!(!shared.mint_confirmation(&id));
        // Retry the same id: the token is invalid.
        assert_eq!(
            take(&shared, &id, "s1", "left click"),
            SpendOutcome::Unknown,
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
            SpendOutcome::Granted
        );
        // deny on an unknown id fails.
        assert!(!shared.deny_confirmation("cu-unknown"));
        assert_eq!(
            take(&shared, "cu-unknown", "s1", "x"),
            SpendOutcome::Unknown,
            "unknown id must stay Unknown"
        );
    }

    /// A "denial" after minting is a change of heart — the same confirm_id's
    /// unspent token must be retracted along with it, not live out its TTL.
    #[test]
    fn deny_after_mint_retracts_the_unspent_token() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        let id = new_pending(&shared, "s1", "left click x1 at Some((5, 6))");
        assert!(shared.mint_confirmation(&id));
        // Change of heart: retract the unspent token.
        assert!(shared.deny_confirmation(&id));
        assert_eq!(
            take(&shared, &id, "s1", "left click x1 at Some((5, 6))"),
            SpendOutcome::Unknown,
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
        // Panic-safe restore (same drop-guard pattern as tool/tests.rs): an
        // assert failure here must not leak the 5 ms override into unrelated
        // tests in this binary.
        struct RestoreTimeout;
        impl Drop for RestoreTimeout {
            fn drop(&mut self) {
                set_physical_input_lock_timeout_for_tests(Duration::ZERO);
            }
        }
        let _restore = RestoreTimeout;
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
        shared.grant_session("s2");
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
            SpendOutcome::Unknown,
            "a disabled cycle must wipe minted approval tokens"
        );
        // Distinct from stop_all semantics: the stop flag is not raised and
        // observe actions still work.
        assert!(!shared.is_stopped());
        assert!(shared.check_readonly().is_ok());
    }

    /// The off-period invariant is self-enforcing: the sweeps live inside
    /// `set_enabled`, so a caller that only flips the flag still gets them
    /// (previously the invariant depended on the command pairing
    /// `set_enabled(false)` with `revoke_all_sessions` and
    /// `set_enabled(true)` with `reset_stop`). A grant, a pending and a
    /// minted token from before the disable are gone after the disable AND
    /// stay gone after a bare re-enable.
    #[test]
    fn set_enabled_itself_sweeps_consent_across_a_disable_enable_cycle() {
        let shared = ComputerUseShared::new();
        shared.set_enabled(true);
        assert_eq!(shared.grant_session("s1"), GrantOutcome::Granted);
        shared.grant_session("s2");
        let pending_id = new_pending(&shared, "s1", "left click");
        let token_id = new_pending(&shared, "s2", "type 3 characters");
        assert!(shared.mint_confirmation(&token_id));

        // Disable: the guard sweeps without any explicit revoke call.
        shared.set_enabled(false);
        assert!(!shared.has_active_grant("s1"));
        assert!(shared.pending_confirmation(&pending_id).is_none());
        assert_eq!(
            take(&shared, &token_id, "s2", "type 3 characters"),
            SpendOutcome::Unknown,
            "a flag-only disable must wipe minted approval tokens"
        );

        // Bare re-enable: the off-period artifacts must not resurrect.
        shared.set_enabled(true);
        assert!(
            !shared.has_active_grant("s1"),
            "a grant from before the off period must not survive re-enable"
        );
        assert!(shared.pending_confirmation(&pending_id).is_none());
        assert_eq!(
            take(&shared, &token_id, "s2", "type 3 characters"),
            SpendOutcome::Unknown
        );

        // The enable-side sweep (reset_stop semantics) rides on the same
        // flip: consent created in the stopped window does not survive
        // re-enable even when the caller never calls reset_stop.
        shared.stop_all();
        // A pending can no longer be registered in the stopped window at all
        // (the insert-side gate), so the enable-side sweep is defense in
        // depth for artifacts that raced in before the stop landed; the
        // model-facing consequence of a stopped-window ask is the refusal,
        // not a dead dialog.
        assert!(
            shared
                .new_pending_confirmation("s3", "left click", "Buy now", 0)
                .is_none(),
            "no pending may be registered while the stop latch is raised"
        );
        shared.set_enabled(true);
        // Re-enable restores the ability to ask (a fresh id), while nothing
        // from the stopped window exists.
        let fresh = shared
            .new_pending_confirmation("s3", "left click", "Buy now", 0)
            .expect("pending registered after re-enable");
        assert!(shared.pending_confirmation(&fresh).is_some());
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
            SpendOutcome::Unknown
        );
    }

    /// A MINTED approval token past [`CONFIRM_TTL`] must be refused at spend
    /// time — the spend-path expiry branch is the only defense against a
    /// token minted then spent minutes later.
    #[test]
    fn minted_token_expires_at_spend() {
        let shared = enabled_shared();
        shared.grant_session("s1");
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
        assert_eq!(take(&shared, &id, "s1", summary), SpendOutcome::Unknown);
        assert_eq!(take(&shared, &id, "s1", summary), SpendOutcome::Unknown);
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
        shared.grant_session("s1");
        let summary = "left click x1 at Some((100, 200))";
        let id = new_pending(&shared, "s1", summary);
        assert!(shared.mint_confirmation(&id));
        shared.revoke_session("s1");
        // Re-granting (the user changes their mind and grants again) must
        // not resurrect the wiped token.
        shared.grant_session("s1");
        assert_eq!(
            take(&shared, &id, "s1", summary),
            SpendOutcome::Unknown,
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
        shared.grant_session("s2");
        let summary = "left click x1 at Some((100, 200))";
        let other_id = new_pending(&shared, "s2", summary);
        assert!(shared.mint_confirmation(&other_id));
        shared.revoke_session("s1");
        assert_eq!(
            take(&shared, &other_id, "s2", summary),
            SpendOutcome::Granted,
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
            shared.grant_session(&session);
            let id = new_pending(&shared, &session, "left click");
            assert!(shared.mint_confirmation(&id));
            ids.push((session, id));
        }
        for (session, id) in &ids {
            assert_eq!(
                take(&shared, id, session, "left click"),
                SpendOutcome::Granted
            );
            assert_eq!(
                take(&shared, id, session, "left click"),
                SpendOutcome::Unknown,
                "each token is single-use"
            );
        }
    }

    /// Stop race: an in-flight run can no longer register a pending AFTER
    /// stop_all (the insert-side gate refuses while the stop latch is
    /// raised), and a pending that raced in before the stop must not mint
    /// while stopped — nor after a later re-enable (reset_stop sweeps
    /// consent created before the stop landed).
    #[test]
    fn mint_refuses_consent_created_while_stopped() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        let raced = new_pending(&shared, "s1", "left click");
        shared.stop_all();
        assert!(
            shared
                .new_pending_confirmation("s1", "left click", "Buy now", 0)
                .is_none(),
            "no pending may be registered while the stop latch is raised"
        );
        assert!(
            !shared.mint_confirmation(&raced),
            "mint must refuse while the stop latch is raised"
        );
        // Re-enabling sweeps consent created before the stop: the stale
        // dialog stays dead even though the stop flag is now cleared.
        shared.reset_stop();
        assert!(
            !shared.mint_confirmation(&raced),
            "a pending from before the stop must not mint after resume"
        );
        assert!(shared.pending_confirmation(&raced).is_none());
    }

    /// Disabling the master switch lowers the emergency-stop latch.
    ///
    /// `computer_use_get_status` reports `is_stopped()` verbatim, and the
    /// settings row renders its "stopped — turn it off and back on to resume"
    /// hint from that flag. Leaving the latch raised for a feature that is
    /// simply off therefore produced a status the UI cannot render sensibly —
    /// the recovery hint next to an already-off toggle — and no amount of
    /// frontend-local patching survives the next status read. The flag is
    /// owned here, so it is cleared here.
    #[test]
    fn disabling_lowers_the_stop_latch() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        shared.stop_all();
        assert!(shared.is_stopped(), "stop_all raises the latch");

        shared.set_enabled(false);
        assert!(
            !shared.is_stopped(),
            "a disabled feature must not keep reporting an emergency stop"
        );
        assert!(!shared.is_enabled());
        assert!(
            !shared.has_active_grant("s1"),
            "disable still revokes grants"
        );

        // Re-enabling is unchanged: no stop, and no consent state carried over.
        shared.set_enabled(true);
        assert!(!shared.is_stopped());
        assert!(!shared.has_active_grant("s1"));
    }

    /// Stop race, mint side: a token minted before the stop is wiped by
    /// stop_all and cannot be resurrected by a re-enable.
    #[test]
    fn stop_then_resume_leaves_no_mintable_token() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        let id = new_pending(&shared, "s1", "left click");
        assert!(shared.mint_confirmation(&id));
        shared.stop_all();
        shared.reset_stop();
        assert_eq!(
            take(&shared, &id, "s1", "left click"),
            SpendOutcome::Unknown,
            "no token may survive stop → resume"
        );
    }

    /// Content binding: the token is bound to the action content hash in
    /// addition to the summary — a same-summary, different-content spend is
    /// rejected and keeps the token.
    #[test]
    fn token_binding_covers_the_action_content() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        let id = shared
            .new_pending_confirmation("s1", "type 3 characters", "field", 42)
            .expect("pending registered");
        assert!(shared.mint_confirmation(&id));
        assert_eq!(
            take_bound(&shared, &id, "s1", "type 3 characters", 43),
            SpendOutcome::Unknown,
            "a different content hash must not spend the token"
        );
        assert_eq!(
            take_bound(&shared, &id, "s1", "type 3 characters", 42),
            SpendOutcome::Granted,
            "the exact approved content spends it; the failed attempt kept the token"
        );
        // Mint refuses while disabled, too (toggle-off race symmetry).
        let shared2 = enabled_shared();
        let id2 = shared2
            .new_pending_confirmation("s2", "left click", "Buy now", 7)
            .expect("pending registered");
        shared2.set_enabled(false);
        assert!(
            !shared2.mint_confirmation(&id2),
            "mint must refuse while the master switch is off"
        );
    }

    /// A pending whose session grant was revoked concurrently must not mint
    /// an approval token for a grant-less session (the dead pending is swept
    /// at mint, so the dialog collapses instead of hanging unapprovable
    /// until the TTL; the tool's last-moment verify would reject the action
    /// anyway — this only stops the dead token from existing at all).
    #[test]
    fn mint_refuses_after_the_sessions_grant_was_revoked() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        let id = shared
            .new_pending_confirmation("s1", "left click", "Buy now", 0)
            .expect("pending registered");
        shared.revoke_session("s1");
        assert!(!shared.mint_confirmation(&id));
        assert!(
            shared.pending_confirmation(&id).is_none(),
            "the dead pending must not stay served for the TTL"
        );
    }

    /// No pending may be registered while the feature is disabled or the
    /// stop is latched: mint refuses in those states, so a pending
    /// registered there could never be approved — the in-flight caller gets
    /// the refusal and no unapprovable dialog is served for the TTL.
    #[test]
    fn no_pending_is_registered_while_disabled_or_stopped() {
        let shared = enabled_shared();
        shared.set_enabled(false);
        assert!(
            shared
                .new_pending_confirmation("s1", "left click", "Buy now", 0)
                .is_none()
        );
        shared.set_enabled(true);
        shared.stop_all();
        assert!(
            shared
                .new_pending_confirmation("s1", "left click", "Buy now", 0)
                .is_none()
        );
    }

    /// While a confirmation dialog is unanswered, every input action from
    /// the session is rejected: the dialog's own approve control is an
    /// ordinary clickable element whose label screens Clear, so letting
    /// input through would let the model mint the approval itself by
    /// clicking it (round-17 self-approval finding). Observing is not an
    /// input action and stays allowed; every decision path (deny, mint,
    /// stop, expiry) unblocks.
    #[test]
    fn input_is_rejected_while_a_confirmation_pends_and_unblocked_by_every_decision() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        assert!(shared.begin_input_action("s1").is_ok());

        let id = new_pending(&shared, "s1", "left click");
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::ConfirmationPending),
            "an unanswered dialog must not be clickable by the model"
        );
        assert!(
            GuardRejection::ConfirmationPending
                .message()
                .contains("waiting for the user's confirmation"),
            "the message must tell the model to wait for the decision"
        );
        // Observing is unaffected: only input-class actions are gated.
        assert_eq!(shared.check_readonly(), Ok(()));

        // Deny clears the pending: the retry path (which re-raises a fresh
        // dialog) works again.
        assert!(shared.deny_confirmation(&id));
        assert!(shared.begin_input_action("s1").is_ok());

        // Approve path: mint consumes the pending, so the confirmed retry
        // with the confirm_id goes through.
        let id = new_pending(&shared, "s1", "left click");
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::ConfirmationPending)
        );
        assert!(shared.mint_confirmation(&id));
        assert!(shared.begin_input_action("s1").is_ok());
        assert_eq!(
            take(&shared, &id, "s1", "left click"),
            SpendOutcome::Granted
        );

        // A stop sweeps the pending and unblocks.
        let _id = new_pending(&shared, "s1", "left click");
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::ConfirmationPending)
        );
        shared.stop_all();
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::Stopped),
            "the latched stop gates before the (cleared) pending would"
        );
        // stop_all also revoked the grant alongside the pending; re-enabling
        // resets the latch, and a fresh grant unblocks input.
        shared.set_enabled(true);
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::GrantRequired),
            "stop_all revoked the grant with the pending"
        );
        assert_eq!(shared.grant_session("s1"), GrantOutcome::Granted);
        assert!(
            shared.begin_input_action("s1").is_ok(),
            "stop swept the pending; re-grant unblocks input"
        );
    }

    /// A pending left to expire must not block input until some unrelated
    /// path sweeps it: the block check itself removes TTL'd-out pendings.
    #[test]
    fn expired_pendings_stop_blocking_input() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        let id = new_pending(&shared, "s1", "left click");
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::ConfirmationPending)
        );
        force_pending_expired(&shared, &id);
        assert!(
            shared.begin_input_action("s1").is_ok(),
            "an expired dialog must not block input"
        );
        assert!(
            shared.pending_confirmation(&id).is_none(),
            "the expiry sweep must have removed the pending"
        );
    }

    /// Test helper: age a pending past [`CONFIRM_TTL`] (Instant cannot be
    /// faked without an injection point, so tests shift the timestamp).
    fn force_pending_expired(shared: &ComputerUseShared, id: &str) {
        let mut consent = shared.consent.lock();
        if let Some(entry) = consent.pending.get_mut(id) {
            entry.created_at = Instant::now() - CONFIRM_TTL - Duration::from_secs(1);
        }
    }

    /// A pending confirmation blocks input from **every** session, not only
    /// the one that raised it.
    ///
    /// The dialog is a process-global window and its approve control is an
    /// ordinary clickable element whose label screens Clear, so a per-session
    /// block left a second granted session free to click "Allow this once"
    /// and mint the first session's approval — the same self-approval hole
    /// the per-session block was added to close, one indirection further out.
    /// Observation stays allowed throughout, and deciding the dialog unblocks
    /// both sessions.
    #[test]
    fn another_sessions_pending_blocks_input_everywhere() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        shared.grant_session("s2");
        let id = new_pending(&shared, "s2", "left click");
        assert_eq!(
            shared.begin_input_action("s1"),
            Err(GuardRejection::ConfirmationPending),
            "a bystander session must not be able to click the dialog"
        );
        assert_eq!(
            shared.begin_input_action("s2"),
            Err(GuardRejection::ConfirmationPending)
        );
        assert!(
            shared.check_readonly().is_ok(),
            "observation stays allowed while a dialog pends"
        );
        assert!(shared.deny_confirmation(&id));
        assert!(shared.begin_input_action("s1").is_ok());
        assert!(shared.begin_input_action("s2").is_ok());
    }

    /// Server truth for the consent UI: `pending_payload_for_session` serves
    /// the newest unexpired payload, a replaced pending replaces the served
    /// payload, and a decided (denied) pending stops being served — this is
    /// what collapses phantom dialogs in the other windows.
    #[test]
    fn pending_payload_serves_newest_and_collapses_on_decision() {
        let shared = enabled_shared();
        assert!(shared.pending_payload_for_session("s1").is_none());
        let id1 = shared
            .new_pending_confirmation("s1", "left click", "Buy now", 0)
            .expect("pending registered");
        shared.set_pending_payload(&id1, serde_json::json!({ "confirm_id": id1 }));
        let served = shared
            .pending_payload_for_session("s1")
            .expect("payload served");
        assert_eq!(served["confirm_id"], id1);
        // Newest wins: the replacement's payload is the one served.
        let id2 = shared
            .new_pending_confirmation("s1", "type 3 characters", "Buy now", 0)
            .expect("pending registered");
        shared.set_pending_payload(&id2, serde_json::json!({ "confirm_id": id2 }));
        let served = shared
            .pending_payload_for_session("s1")
            .expect("payload served");
        assert_eq!(served["confirm_id"], id2);
        // A payload without a live pending is never served (set is a no-op).
        shared.set_pending_payload(&id1, serde_json::json!({ "confirm_id": "stale" }));
        let served = shared
            .pending_payload_for_session("s1")
            .expect("payload served");
        assert_eq!(served["confirm_id"], id2);
        // Deciding the pending collapses it.
        assert!(shared.deny_confirmation(&id2));
        assert!(shared.pending_payload_for_session("s1").is_none());
    }

    /// The grant-request marker follows the request lifecycle: set at emit,
    /// cleared by grant, and re-set requests clear on revoke/stop too.
    #[test]
    fn grant_request_marker_follows_lifecycle() {
        let shared = enabled_shared();
        shared.mark_grant_requested("s1");
        assert!(shared.grant_request_pending("s1"));
        shared.grant_session("s1");
        assert!(
            !shared.grant_request_pending("s1"),
            "granting answers the request"
        );
        // A revoke also clears any lingering marker.
        shared.mark_grant_requested("s1");
        shared.revoke_session("s1");
        assert!(!shared.grant_request_pending("s1"));
        // Stop wipes every session's marker.
        shared.mark_grant_requested("s1");
        shared.mark_grant_requested("s2");
        shared.stop_all();
        assert!(!shared.grant_request_pending("s1"));
        assert!(!shared.grant_request_pending("s2"));
    }

    /// Defense in depth: a token minted while enabled must not be spendable
    /// after the master switch goes off (the spend would die at
    /// verify_input_action anyway, but consuming the user's approval there
    /// would be the wrong direction).
    #[test]
    fn take_confirmation_refuses_while_disabled_or_stopped() {
        let shared = enabled_shared();
        shared.grant_session("s1");
        let id = new_pending(&shared, "s1", "left click");
        assert!(shared.mint_confirmation(&id));
        shared.set_enabled(false);
        assert_eq!(
            take(&shared, &id, "s1", "left click"),
            SpendOutcome::Unknown
        );
        shared.set_enabled(true);
        shared.stop_all();
        assert_eq!(
            take(&shared, &id, "s1", "left click"),
            SpendOutcome::Unknown
        );
    }
}
