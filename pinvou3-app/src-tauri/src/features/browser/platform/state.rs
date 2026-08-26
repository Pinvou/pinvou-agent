//! Pure state machine for the native browser host.
//!
//! This module is independent of a specific WebView engine. Tab bijections,
//! control leases, and request tombstones use the same rules on Windows, macOS,
//! and Linux and are covered by pure unit tests.

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

const MAX_NAVIGATION_URL_GENERATIONS: usize = 128;
const NAVIGATION_COMMIT_WINDOW: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
struct ActiveNavigationGeneration {
    generation: u64,
    request_id: String,
    target_url: String,
    target_observed: bool,
    latest_observed_url: Option<String>,
    latest_started_url: Option<String>,
    cross_document_in_flight: bool,
    expires_at: Instant,
    canceled: bool,
}

#[derive(Clone, Debug)]
struct ObservedNavigationUrl {
    url: String,
    generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum NavigationCommitDecision {
    Current { request_id: Option<String> },
    Stale,
}

#[derive(Debug, Default)]
pub(super) struct UserNavigationState {
    latest_generation: u64,
    /// Monotonic identity for every transition that can change how an
    /// anonymous native navigation callback must be interpreted. Linux keeps
    /// this across its marker/rebind transaction so a completed intervening
    /// navigation cannot be mistaken for an idle, unchanged page.
    admission_epoch: u128,
    active: Option<ActiveNavigationGeneration>,
    observed_urls: VecDeque<ObservedNavigationUrl>,
}

impl UserNavigationState {
    fn advance_admission_epoch(&mut self) {
        self.admission_epoch = self
            .admission_epoch
            .checked_add(1)
            .expect("navigation admission epoch exhausted");
    }

    fn next_generation(&mut self) -> u64 {
        self.latest_generation = self.latest_generation.checked_add(1).unwrap_or_else(|| {
            self.observed_urls.clear();
            1
        });
        self.latest_generation
    }

    fn remember_url(&mut self, url: &str, generation: u64) {
        self.observed_urls.push_back(ObservedNavigationUrl {
            url: url.to_string(),
            generation,
        });
        while self.observed_urls.len() > MAX_NAVIGATION_URL_GENERATIONS {
            self.observed_urls.pop_front();
        }
    }

    fn generation_for_url(&self, url: &str) -> Option<u64> {
        self.observed_urls
            .iter()
            .rev()
            .find(|observed| observed.url == url)
            .map(|observed| observed.generation)
    }

    fn begin_user(
        &mut self,
        request_id: &str,
        target_url: &str,
        cross_document: bool,
    ) -> Result<(), String> {
        self.expire_stale_navigation();
        if self.active.as_ref().is_some_and(|active| {
            active.target_url == target_url
                || active.latest_observed_url.as_deref() == Some(target_url)
                || active.latest_started_url.as_deref() == Some(target_url)
                || self.generation_for_url(target_url) == Some(active.generation)
        }) {
            // A late Started/Finished pair carries no native request identity.
            // Replacing an in-flight generation or its canceled tombstone with
            // the same URL would let callbacks from the first request settle
            // the second requestId.
            return Err(
                "browser/navigation-same-url-in-flight: current URL is still loading; wait for completion and retry"
                    .to_string(),
            );
        }
        let generation = self.next_generation();
        self.active = Some(ActiveNavigationGeneration {
            generation,
            request_id: request_id.to_string(),
            target_url: target_url.to_string(),
            target_observed: false,
            latest_observed_url: None,
            latest_started_url: None,
            cross_document_in_flight: cross_document,
            expires_at: Instant::now() + NAVIGATION_COMMIT_WINDOW,
            canceled: false,
        });
        self.advance_admission_epoch();
        Ok(())
    }

    fn begin_external(&mut self, cross_document: bool) {
        self.expire_stale_navigation();
        let generation = self.next_generation();
        self.active = Some(ActiveNavigationGeneration {
            generation,
            request_id: String::new(),
            target_url: String::new(),
            target_observed: true,
            latest_observed_url: None,
            latest_started_url: None,
            cross_document_in_flight: cross_document,
            expires_at: Instant::now() + NAVIGATION_COMMIT_WINDOW,
            canceled: false,
        });
        self.advance_admission_epoch();
    }

    fn fail_user(&mut self, request_id: &str) {
        self.expire_stale_navigation();
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if active.request_id == request_id {
            // Keep the generation as a tombstone. A callback already queued by
            // the native engine must not revive the superseded document after
            // navigate() reported failure.
            active.canceled = true;
            active.request_id.clear();
            self.advance_admission_epoch();
        }
    }

    fn cancel_active(&mut self) {
        self.expire_stale_navigation();
        if let Some(active) = self.active.as_mut() {
            let changed = !active.canceled
                || !active.request_id.is_empty()
                || active.cross_document_in_flight;
            active.canceled = true;
            active.request_id.clear();
            active.cross_document_in_flight = false;
            if changed {
                self.advance_admission_epoch();
            }
        }
    }

    pub(super) fn observe_requested_target(&mut self, target_url: &str) -> bool {
        self.expire_stale_navigation();
        let Some(active) = self.active.as_mut() else {
            return false;
        };
        if active.canceled
            || active.generation != self.latest_generation
            || active.request_id.is_empty()
            || active.target_url != target_url
        {
            return false;
        }
        active.target_observed = true;
        active.expires_at = Instant::now() + NAVIGATION_COMMIT_WINDOW;
        self.advance_admission_epoch();
        true
    }

    pub(super) fn observe_started(&mut self, started_url: &str) {
        self.expire_stale_navigation();
        if self.active.is_none() {
            self.begin_external(true);
        }
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if active.generation != self.latest_generation || active.canceled {
            return;
        }
        if !active.request_id.is_empty() && !active.target_observed {
            if active.target_url != started_url {
                return;
            }
            active.target_observed = true;
        }
        let generation = active.generation;
        active.latest_observed_url = Some(started_url.to_string());
        active.latest_started_url = Some(started_url.to_string());
        active.cross_document_in_flight = true;
        active.expires_at = Instant::now() + NAVIGATION_COMMIT_WINDOW;
        self.remember_url(started_url, generation);
        self.advance_admission_epoch();
    }

    /// Record a same-document URL mutation made by the document that already
    /// crossed the top-level Started seam. The per-WebView nonce and the live
    /// top-level URL are validated by the host before this method is called.
    /// This advances only the current generation's Finished candidate; it does
    /// not publish an address or close the cross-document gate.
    pub(super) fn observe_same_document_during_load(&mut self, live_url: &str) -> bool {
        self.expire_stale_navigation();
        let Some(active) = self.active.as_mut() else {
            return false;
        };
        if active.generation != self.latest_generation
            || active.canceled
            || !active.cross_document_in_flight
            || active.latest_started_url.is_none()
            || (!active.request_id.is_empty() && !active.target_observed)
        {
            return false;
        }
        let generation = active.generation;
        active.latest_observed_url = Some(live_url.to_string());
        active.latest_started_url = Some(live_url.to_string());
        active.expires_at = Instant::now() + NAVIGATION_COMMIT_WINDOW;
        self.remember_url(live_url, generation);
        self.advance_admission_epoch();
        true
    }

    pub(super) fn current_request_id_for_blocked_target(
        &mut self,
        target_url: &str,
    ) -> Option<String> {
        self.expire_stale_navigation();
        self.active.as_ref().and_then(|active| {
            (!active.canceled && !active.request_id.is_empty() && active.target_url == target_url)
                .then(|| active.request_id.clone())
        })
    }

    pub(super) fn finish(&mut self, committed_url: &str) -> NavigationCommitDecision {
        self.expire_stale_navigation();
        let Some(active) = self.active.as_ref() else {
            return NavigationCommitDecision::Stale;
        };
        if active.generation != self.latest_generation
            || active.canceled
            || !active.cross_document_in_flight
            || active.latest_observed_url.as_deref() != Some(committed_url)
            || active.latest_started_url.as_deref() != Some(committed_url)
            || self.generation_for_url(committed_url) != Some(active.generation)
        {
            return NavigationCommitDecision::Stale;
        }
        if !active.request_id.is_empty() && !active.target_observed {
            return NavigationCommitDecision::Stale;
        }
        let request_id = (!active.request_id.is_empty()).then(|| active.request_id.clone());
        self.active = None;
        self.advance_admission_epoch();
        NavigationCommitDecision::Current { request_id }
    }

    pub(super) fn finish_same_document(&mut self, committed_url: &str) -> NavigationCommitDecision {
        self.expire_stale_navigation();
        let Some(active) = self.active.as_ref() else {
            // An unsolicited page history/hash transition is still an
            // observable top-level URL change. Invalidate any Linux binding
            // transaction that captured the prior document while idle.
            self.advance_admission_epoch();
            return NavigationCommitDecision::Current { request_id: None };
        };
        if active.generation != self.latest_generation
            || active.canceled
            || active.cross_document_in_flight
            || (!active.request_id.is_empty() && active.target_url != committed_url)
        {
            return NavigationCommitDecision::Stale;
        }
        let request_id = (!active.request_id.is_empty()).then(|| active.request_id.clone());
        self.active = None;
        self.advance_admission_epoch();
        NavigationCommitDecision::Current { request_id }
    }

    fn expire_stale_navigation(&mut self) {
        if self
            .active
            .as_ref()
            .is_some_and(|active| Instant::now() >= active.expires_at)
        {
            // Expiry is not an explicit cancellation tombstone. Retire the
            // generation so its Finished callback is rejected (there is no
            // active generation), while a later top-level Started callback can
            // establish a fresh external generation and converge host state.
            self.active = None;
            self.advance_admission_epoch();
        }
    }

    pub(super) fn navigation_in_flight(&mut self) -> bool {
        self.expire_stale_navigation();
        self.active
            .as_ref()
            .is_some_and(|active| !active.canceled && active.cross_document_in_flight)
    }

    /// Admission is stricter than the visible cross-document loading gate:
    /// same-document and history generations also own otherwise anonymous
    /// native callbacks until they commit, fail, are canceled, or time out.
    pub(super) fn navigation_admission_busy(&mut self) -> bool {
        self.expire_stale_navigation();
        self.active.as_ref().is_some_and(|active| !active.canceled)
    }

    pub(super) fn navigation_admission_epoch(&mut self) -> u128 {
        self.expire_stale_navigation();
        self.admission_epoch
    }
}

#[derive(Clone, Debug)]
pub(super) struct SurfaceEntry {
    pub(super) label: String,
    pub(super) token: String,
    /// Agent-facing BrowserCore page id. It is process-local, monotonically allocated and never
    /// reused, so closing an earlier tab cannot retarget a stale tool call to a later tab.
    pub(super) page_id: u64,
    pub(super) automation_target: Option<String>,
    /// Set only by Agent create_tab for exact compensation of request tombstones
    /// or later failures. Ordinary user tabs and restart-restored tabs have no
    /// creation generation.
    pub(super) created_by_request_id: Option<String>,
    /// Remains false until Agent create_tab completes target discovery and
    /// initial navigation. Page callbacks cannot publish events, change control,
    /// or derive popups while false.
    pub(super) published: Arc<AtomicBool>,
    /// Control revision after Agent create_tab commits. A late cancellation can
    /// roll back only while owner/revision still match this generation.
    pub(super) created_at_revision: Option<u64>,
    /// The host-owned last valid top-level URL. WebKitGTK can transiently
    /// report an empty or relative URL while a document is being replaced;
    /// keeping the last validated value prevents one engine callback from
    /// dropping the tab or poisoning the complete restore manifest.
    pub(super) last_known_url: Arc<parking_lot::RwLock<String>>,
    /// Document title paired with the URL observed by the native WebView when
    /// the title callback fired. Pairing prevents a title from the previous
    /// document leaking into a newly committed page.
    pub(super) last_known_title: Arc<parking_lot::RwLock<Option<(String, String)>>>,
    /// One generation-owned state machine correlates redirects, Started and
    /// Finished callbacks and the cross-document gate. Keeping them together
    /// prevents an obsolete callback from opening or closing a newer gate.
    pub(super) user_navigation: Arc<parking_lot::Mutex<UserNavigationState>>,
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

    pub(super) fn last_known_url(&self) -> String {
        self.last_known_url.read().clone()
    }

    pub(super) fn remember_url(&self, url: impl Into<String>) -> bool {
        let url = url.into();
        let mut current = self.last_known_url.write();
        if *current == url {
            return false;
        }
        *current = url;
        true
    }

    pub(super) fn remember_title(&self, url: impl Into<String>, title: impl Into<String>) {
        *self.last_known_title.write() = Some((url.into(), title.into()));
    }

    pub(super) fn title_for_url(&self, url: &str) -> Option<String> {
        self.last_known_title
            .read()
            .as_ref()
            .filter(|(title_url, _)| title_url == url)
            .map(|(_, title)| title.clone())
    }

    pub(super) fn navigation_in_flight(&self) -> bool {
        self.user_navigation.lock().navigation_in_flight()
    }

    pub(super) fn navigation_admission_busy(&self) -> bool {
        self.user_navigation.lock().navigation_admission_busy()
    }

    pub(super) fn begin_user_navigation(
        &self,
        request_id: &str,
        target_url: impl Into<String>,
        cross_document: bool,
    ) -> Result<(), String> {
        validate_request_id(request_id)?;
        let target_url = target_url.into();
        self.user_navigation
            .lock()
            .begin_user(request_id, &target_url, cross_document)
    }

    /// Supersede every older callback before a non-address-bar navigation
    /// (reload/history/Agent staging) is dispatched.
    pub(super) fn begin_external_navigation(&self, cross_document: bool) {
        self.user_navigation.lock().begin_external(cross_document);
    }

    pub(super) fn fail_user_navigation(&self, request_id: &str) {
        self.user_navigation.lock().fail_user(request_id);
    }

    pub(super) fn cancel_active_navigation(&self) {
        self.user_navigation.lock().cancel_active();
    }

    pub(super) fn observe_requested_navigation_target(&self, target_url: &str) -> bool {
        self.user_navigation
            .lock()
            .observe_requested_target(target_url)
    }

    pub(super) fn current_request_id_for_blocked_target(&self, target_url: &str) -> Option<String> {
        self.user_navigation
            .lock()
            .current_request_id_for_blocked_target(target_url)
    }

    pub(super) fn finish_navigation(&self, committed_url: &str) -> NavigationCommitDecision {
        self.user_navigation.lock().finish(committed_url)
    }

    pub(super) fn finish_same_document_navigation(
        &self,
        committed_url: &str,
    ) -> NavigationCommitDecision {
        self.user_navigation
            .lock()
            .finish_same_document(committed_url)
    }
}

/// Authoritative host bijection between tabToken and WebView label.
///
/// A page-main-world marker is only for initial CDP discovery and cannot change
/// ownership recorded here.
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
            return Err("Browser tab token is already in use".to_string());
        }
        if self.token_for_label(&entry.label).is_some() {
            return Err("Browser WebView is already bound to another tab".to_string());
        }
        if self.token_for_page_id(entry.page_id).is_some() {
            return Err("Browser pageId is already bound to another tab".to_string());
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
                return Err("Automation target is already bound to another tab".to_string());
            }
        }
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.token == token)
            .ok_or_else(|| {
                "Tab does not exist or does not belong to this conversation".to_string()
            })?;
        if entry
            .automation_target
            .as_deref()
            .is_some_and(|current| current != target)
        {
            return Err("Tab is already bound to another automation target".to_string());
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
    /// A freshly restored page has no new user or Agent action after process
    /// restart. It is not an Agent lease and UI must not report user takeover.
    /// The first real operation committed through the host gains control.
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
    /// Complete authorization that passed host lease validation and entered the
    /// dispatch critical section. Popup callbacks may copy only this internal
    /// Rust authorization and cannot infer Agent ownership from a short-lived bool.
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
            || !self
                .active_agent_operation
                .as_ref()
                .is_some_and(|operation| operation.upstream_active)
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

    /// Automatic handback after user inactivity can commit only the revision for
    /// the same takeover. Any new user action, tab switch, or explicit Agent
    /// handback advances revision and silently invalidates the old timer, so a
    /// late task cannot overwrite updated control.
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
            .expect("explicit authorization path must issue an Agent lease")
    }

    /// Check User ownership and issue the new lease under the same control lock,
    /// eliminating takeover-overwrite TOCTOU between checking owner and issuing.
    /// Only explicit UI handback may pass true.
    pub(super) fn issue_agent_lease_if_allowed(
        &self,
        explicit_user_handback: bool,
    ) -> Option<(ControlSnapshot, String)> {
        self.issue_agent_lease_with(explicit_user_handback, |_| Ok(()))
            .expect("empty Agent activation mutation cannot fail")
            .map(|(snapshot, lease, ())| (snapshot, lease))
    }

    /// Owner validation, host active-tab mutation, and new lease issuance share
    /// one critical section. If user takeover commits first, the closure never
    /// runs. If the Agent commits first, subsequent user takeover is final and
    /// cannot be overwritten by late issuance.
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

    /// Linearization point for Agent mutations such as registry create/close.
    /// Validation and revision advancement occur under one lock. If user takeover
    /// obtains it first, this returns false and never resets owner to Agent. The
    /// caller must not unconditionally bump Agent after success either.
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

    /// Exactly roll back a committed Agent creation generation. Successful create
    /// revokes the old lease, so compensation compares owner/revision recorded at
    /// host commit. User takeover or any later mutation makes CAS return None and
    /// preserves the page in use.
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

    /// Mark a begun atomic dispatch. Every browser tool records complete
    /// authorization; only tools producing trusted input also open a 750ms input
    /// suppression fuse. Registration and lease validation share the control lock,
    /// so popup callbacks cannot trust a lease invalidated by user takeover.
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

    /// A popup callback obtains Agent authorization only within an unfinished
    /// dispatch whose control and lease remain identical. It returns an opaque
    /// lease from Rust memory that is never exposed to the page or React.
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

    /// Revalidate one exact Agent operation and enqueue its native mutation
    /// while the control lock remains held. A trusted user-takeover callback
    /// therefore commits either before this check (and rejects it) or after the
    /// enqueue; there is no authorize-then-dispatch window between the two.
    pub(super) fn dispatch_if_agent_authorized<T, F>(
        &self,
        lease: &NativeTabLease,
        dispatch: F,
    ) -> Result<Option<T>, String>
    where
        F: FnOnce() -> Result<T, String>,
    {
        let mut state = self.state.lock();
        state.clear_expired_authorization(Instant::now());
        if lease.owner != NativeControlOwner::Agent
            || state.snapshot.owner != NativeControlOwner::Agent
            || state.snapshot.revision != lease.revision
            || state.active_lease.as_deref() != Some(lease.lease.as_str())
            || !state.active_operation_matches(lease)
        {
            return Ok(None);
        }
        let result = dispatch().map(Some);
        drop(state);
        result
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
        // Prevent a wrapper crash from suppressing user takeover forever. The
        // normal path clears the active operation after dispatch and leaves only
        // brief callback grace after real native input. 750ms is a process-failure
        // fuse, not ordinary user-takeover latency.
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
    /// Opaque host capability token. Observable revision/targetId values cannot
    /// serve as authorization by themselves.
    pub(crate) lease: String,
}

impl NativeTabLease {
    /// Construct a validated host lease from the wrapper assert_host_lease payload.
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
            return Err("Browser host lease payload is invalid".to_string());
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
        serde_json::to_value(self)
            .map_err(|error| format!("Failed to serialize browser host lease: {error}"))
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
    /// Request entered the execution critical section. Retain cancellation until
    /// the executor commits compensation metadata.
    AwaitingCompletion,
    AlreadyCanceled,
    /// Request committed. The caller must use the result to roll back its created
    /// WebView or workspace.
    AlreadyCompleted(Value),
}

#[derive(Debug, Clone)]
enum RequestState {
    Pending,
    Completed(Value),
    /// Cancellation arrived before execution completed. It cannot be treated as
    /// an acknowledged terminal state, or a later-created resource would have no
    /// retryable compensation record.
    CancelAwaitingCompletion,
    /// Complete compensation exists but has not succeeded. Repeated cancellation
    /// must return the same record.
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
                    return Err(
                        "Browser request ledger is full; wait for in-flight requests to finish"
                            .to_string(),
                    );
                }
                self.records.insert(key, RequestState::Pending);
                NativeRequestClaim::Execute
            }
        })
    }

    /// true allows result commit. false means cancellation arrived first and the
    /// caller must roll back the resource.
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
            None => Err("Browser request is not claimed and cannot commit a result".to_string()),
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
                // When cancellation arrives later, retain the complete result
                // until the caller explicitly acknowledges successful compensation.
                // A repeated tombstone after transient close/I/O failure retrieves
                // the same rollback record.
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
                    return Err(
                        "Browser request ledger is full; wait for in-flight requests to finish"
                            .to_string(),
                    );
                }
                self.records.insert(key.clone(), RequestState::Canceled);
                self.remember_terminal(&key);
                NativeRequestCancel::Tombstoned
            }
        };
        Ok(disposition)
    }

    /// Advance cancellation to acknowledged terminal state only after successful
    /// compensation or safe supersession by a newer user/control generation. On
    /// failure the caller skips this method, retaining the record for retry.
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
            Some(RequestState::CancelAwaitingCompletion) => Err(
                "Browser request is still executing; cancellation cannot be acknowledged early"
                    .to_string(),
            ),
            Some(RequestState::Pending | RequestState::Completed(_)) => Err(
                "Browser request has not entered an acknowledgeable cancellation state".to_string(),
            ),
            None => Err("Browser cancellation request does not exist".to_string()),
        }
    }

    /// Removes every request generation owned by one durably deleted task.
    /// Callers must first drain hosted request scanners and finish all native
    /// teardown; otherwise removing a pending compensation record would make a
    /// retry unable to close the resource it created.
    pub(super) fn purge_session(&mut self, session_id: &str) -> Result<usize, String> {
        // Reuse the request-key validator so purge cannot turn an invalid,
        // attacker-controlled prefix into a cross-session deletion.
        let prefix = request_key(session_id, "purge")?
            .strip_suffix("purge")
            .expect("fixed request id suffix")
            .to_string();
        let before = self.records.len();
        self.records.retain(|key, _| !key.starts_with(&prefix));
        self.terminal_order.retain(|key| !key.starts_with(&prefix));
        Ok(before.saturating_sub(self.records.len()))
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.records.len()
    }

    fn remember_terminal(&mut self, request_id: &str) {
        // A completed request can later become an acknowledged cancellation.
        // Refresh its eviction position instead of keeping two entries for the
        // same record: otherwise evicting the stale entry would remove the
        // current terminal state and allow the request to execute again.
        self.terminal_order
            .retain(|existing| existing != request_id);
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
        return Err("Browser request sessionId is invalid".to_string());
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
        Err("Browser requestId is invalid".to_string())
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
            last_known_url: Arc::new(parking_lot::RwLock::new("about:blank".to_string())),
            last_known_title: Arc::new(parking_lot::RwLock::new(None)),
            user_navigation: Arc::new(parking_lot::Mutex::new(UserNavigationState::default())),
        }
    }

    #[test]
    fn cloned_surface_entries_share_the_host_owned_last_valid_url() {
        let entry = entry("tab-a", "view-a");
        let clone = entry.clone();

        assert!(entry.remember_url("https://example.com/next"));
        assert_eq!(clone.last_known_url(), "https://example.com/next");
        assert!(!clone.remember_url("https://example.com/next"));
        clone.remember_title("https://example.com/next", "Next page");
        assert_eq!(
            entry.title_for_url("https://example.com/next").as_deref(),
            Some("Next page")
        );
        assert_eq!(entry.title_for_url("https://example.com/other"), None);
        clone.begin_external_navigation(true);
        assert!(entry.navigation_in_flight());
    }

    #[test]
    fn overlapping_user_navigations_reject_the_superseded_finished_generation() {
        let entry = entry("tab-a", "view-a");
        entry
            .begin_user_navigation("request-a", "https://example.com/a", true)
            .unwrap();
        entry.observe_requested_navigation_target("https://example.com/a");
        entry
            .user_navigation
            .lock()
            .observe_started("https://example.com/a");

        entry
            .begin_user_navigation("request-b", "https://example.com/b", true)
            .unwrap();
        assert_eq!(
            entry.finish_navigation("https://example.com/a"),
            NavigationCommitDecision::Stale
        );
        entry.observe_requested_navigation_target("https://example.com/b");
        // A matching policy callback alone is not enough: an old same-URL
        // Finished cannot complete B until its document Started is observed.
        assert_eq!(
            entry.finish_navigation("https://example.com/b"),
            NavigationCommitDecision::Stale
        );
        entry
            .user_navigation
            .lock()
            .observe_started("https://example.com/b");
        assert_eq!(
            entry.finish_navigation("https://example.com/a"),
            NavigationCommitDecision::Stale
        );
        assert_eq!(
            entry.finish_navigation("https://example.com/b"),
            NavigationCommitDecision::Current {
                request_id: Some("request-b".to_string())
            }
        );
    }

    #[test]
    fn current_user_navigation_redirect_keeps_its_request_identity() {
        let entry = entry("tab-a", "view-a");
        entry
            .begin_user_navigation("request-http", "http://example.com/start", true)
            .unwrap();
        entry.observe_requested_navigation_target("http://example.com/start");
        entry
            .user_navigation
            .lock()
            .observe_started("http://example.com/start");
        entry
            .user_navigation
            .lock()
            .observe_started("https://example.com/final");

        assert_eq!(
            entry.finish_navigation("https://example.com/final"),
            NavigationCommitDecision::Current {
                request_id: Some("request-http".to_string())
            }
        );
    }

    #[test]
    fn same_document_change_during_load_finishes_inside_the_started_generation() {
        let entry = entry("tab-a", "view-a");
        entry
            .begin_user_navigation("request-a", "https://example.com/a", true)
            .unwrap();
        assert!(entry.observe_requested_navigation_target("https://example.com/a"));
        {
            let mut navigation = entry.user_navigation.lock();
            navigation.observe_started("https://example.com/a");
            assert!(navigation.observe_same_document_during_load("https://example.com/a#ready"));
        }

        // The nonce signal updates only the generation candidate. It must not
        // close the cross-document gate before the matching Finished callback.
        assert!(entry.navigation_in_flight());
        assert_eq!(
            entry.finish_navigation("https://example.com/a"),
            NavigationCommitDecision::Stale
        );
        assert_eq!(
            entry.finish_navigation("https://example.com/a#ready"),
            NavigationCommitDecision::Current {
                request_id: Some("request-a".to_string())
            }
        );
    }

    #[test]
    fn timed_out_generation_rejects_late_finished_then_allows_fresh_navigation() {
        let entry = entry("tab-a", "view-a");
        entry
            .begin_user_navigation("request-old", "https://example.com/old", true)
            .unwrap();
        entry.observe_requested_navigation_target("https://example.com/old");
        entry
            .user_navigation
            .lock()
            .observe_started("https://example.com/old");
        entry
            .user_navigation
            .lock()
            .active
            .as_mut()
            .unwrap()
            .expires_at = Instant::now();

        assert_eq!(
            entry.current_request_id_for_blocked_target("https://example.com/old"),
            None
        );
        assert!(!entry.navigation_admission_busy());
        assert_eq!(
            entry.finish_navigation("https://example.com/old"),
            NavigationCommitDecision::Stale
        );

        entry
            .user_navigation
            .lock()
            .observe_started("https://example.com/fresh");
        assert!(entry.navigation_admission_busy());
        assert_eq!(
            entry.finish_navigation("https://example.com/fresh"),
            NavigationCommitDecision::Current { request_id: None }
        );
        assert!(!entry.navigation_admission_busy());
    }

    #[test]
    fn same_url_retry_is_rejected_without_superseding_the_active_request() {
        let entry = entry("tab-a", "view-a");
        entry
            .begin_user_navigation("request-a", "https://example.com/same", true)
            .unwrap();
        entry.observe_requested_navigation_target("https://example.com/same");
        entry
            .user_navigation
            .lock()
            .observe_started("https://example.com/same");

        let error = entry
            .begin_user_navigation("request-b", "https://example.com/same", true)
            .unwrap_err();
        assert!(error.contains("browser/navigation-same-url-in-flight"));
        assert_eq!(
            entry.finish_navigation("https://example.com/same"),
            NavigationCommitDecision::Current {
                request_id: Some("request-a".to_string())
            }
        );
    }

    #[test]
    fn same_url_retry_cannot_replace_a_canceled_generation_tombstone() {
        let entry = entry("tab-a", "view-a");
        entry
            .begin_user_navigation("request-a", "https://example.com/same", true)
            .unwrap();
        entry.observe_requested_navigation_target("https://example.com/same");
        entry.fail_user_navigation("request-a");

        assert!(!entry.navigation_admission_busy());
        let error = entry
            .begin_user_navigation("request-b", "https://example.com/same", true)
            .unwrap_err();
        assert!(error.contains("browser/navigation-same-url-in-flight"));
        entry
            .user_navigation
            .lock()
            .observe_started("https://example.com/same");
        assert_eq!(
            entry.finish_navigation("https://example.com/same"),
            NavigationCommitDecision::Stale
        );
    }

    #[test]
    fn admission_remains_busy_for_same_document_history_until_terminal() {
        let entry = entry("tab-a", "view-a");
        entry.begin_external_navigation(false);

        assert!(!entry.navigation_in_flight());
        assert!(entry.navigation_admission_busy());
        entry.cancel_active_navigation();
        assert!(!entry.navigation_admission_busy());
    }

    #[test]
    fn admission_epoch_advances_for_every_navigation_semantic_transition() {
        fn advanced(state: &mut UserNavigationState, previous: u128) -> u128 {
            let current = state.navigation_admission_epoch();
            assert!(
                current > previous,
                "navigation semantic transition must advance the admission epoch"
            );
            current
        }

        let mut state = UserNavigationState::default();
        let mut epoch = state.navigation_admission_epoch();

        state
            .begin_user("request-a", "https://example.com/a", true)
            .expect("begin user navigation");
        epoch = advanced(&mut state, epoch);
        assert!(state.observe_requested_target("https://example.com/a"));
        epoch = advanced(&mut state, epoch);
        state.observe_started("https://example.com/a");
        epoch = advanced(&mut state, epoch);
        assert!(state.observe_same_document_during_load("https://example.com/a#ready"));
        epoch = advanced(&mut state, epoch);
        assert!(matches!(
            state.finish("https://example.com/a#ready"),
            NavigationCommitDecision::Current { .. }
        ));
        epoch = advanced(&mut state, epoch);

        assert!(matches!(
            state.finish_same_document("https://example.com/a#idle"),
            NavigationCommitDecision::Current { request_id: None }
        ));
        epoch = advanced(&mut state, epoch);

        state
            .begin_user("request-failed", "https://example.com/failed", true)
            .expect("begin navigation that will fail");
        epoch = advanced(&mut state, epoch);
        state.fail_user("request-failed");
        epoch = advanced(&mut state, epoch);

        state.begin_external(false);
        epoch = advanced(&mut state, epoch);
        state.cancel_active();
        epoch = advanced(&mut state, epoch);

        state.begin_external(true);
        epoch = advanced(&mut state, epoch);
        state
            .active
            .as_mut()
            .expect("active navigation before timeout")
            .expires_at = Instant::now();
        let expired_epoch = state.navigation_admission_epoch();
        assert!(
            expired_epoch > epoch,
            "timeout retirement must advance epoch"
        );
        assert!(!state.navigation_admission_busy());
    }

    #[test]
    fn callback_before_exact_user_target_cannot_join_the_new_generation() {
        let entry = entry("tab-a", "view-a");
        entry.begin_external_navigation(true);
        entry
            .user_navigation
            .lock()
            .observe_started("https://example.com/old");
        entry
            .begin_user_navigation("request-new", "https://example.com/new", true)
            .unwrap();

        assert!(!entry.observe_requested_navigation_target("https://example.com/old"));
        assert_eq!(
            entry.finish_navigation("https://example.com/old"),
            NavigationCommitDecision::Stale
        );
        entry.observe_requested_navigation_target("https://example.com/new");
        entry
            .user_navigation
            .lock()
            .observe_started("https://example.com/new");
        assert_eq!(
            entry.finish_navigation("https://example.com/new"),
            NavigationCommitDecision::Current {
                request_id: Some("request-new".to_string())
            }
        );
    }

    #[test]
    fn policy_callbacks_cannot_promote_iframe_urls_into_the_top_level_generation() {
        let entry = entry("tab-a", "view-a");
        entry
            .begin_user_navigation("request-main", "https://example.com/main", true)
            .unwrap();
        assert!(entry.observe_requested_navigation_target("https://example.com/main"));
        assert!(!entry.observe_requested_navigation_target("https://frames.test/child"));
        entry
            .user_navigation
            .lock()
            .observe_started("https://example.com/main");

        assert_eq!(
            entry.finish_navigation("https://example.com/main"),
            NavigationCommitDecision::Current {
                request_id: Some("request-main".to_string())
            }
        );
    }

    #[test]
    fn only_the_latest_top_level_started_redirect_may_finish() {
        let entry = entry("tab-a", "view-a");
        entry
            .begin_user_navigation("request-http", "http://example.com/start", true)
            .unwrap();
        entry.observe_requested_navigation_target("http://example.com/start");
        {
            let mut navigation = entry.user_navigation.lock();
            navigation.observe_started("http://example.com/start");
            navigation.observe_started("https://example.com/final");
        }

        assert_eq!(
            entry.finish_navigation("http://example.com/start"),
            NavigationCommitDecision::Stale
        );
        assert_eq!(
            entry.finish_navigation("https://example.com/final"),
            NavigationCommitDecision::Current {
                request_id: Some("request-http".to_string())
            }
        );
    }

    #[test]
    fn same_document_commit_closes_only_its_exact_user_generation() {
        let entry = entry("tab-a", "view-a");
        entry
            .begin_user_navigation("request-fragment", "https://example.com/page#next", false)
            .unwrap();
        assert!(!entry.navigation_in_flight());
        assert!(entry.navigation_admission_busy());
        assert_eq!(
            entry.finish_same_document_navigation("https://example.com/page#other"),
            NavigationCommitDecision::Stale
        );
        assert_eq!(
            entry.finish_same_document_navigation("https://example.com/page#next"),
            NavigationCommitDecision::Current {
                request_id: Some("request-fragment".to_string())
            }
        );
        assert!(!entry.navigation_admission_busy());
    }

    #[test]
    fn blocked_subframe_cannot_borrow_the_top_level_request_id() {
        let entry = entry("tab-a", "view-a");
        entry
            .begin_user_navigation("request-main", "https://example.com/main", true)
            .unwrap();
        entry.observe_requested_navigation_target("https://example.com/main");

        assert_eq!(
            entry.current_request_id_for_blocked_target("custom://child"),
            None
        );
        assert_eq!(
            entry
                .current_request_id_for_blocked_target("https://example.com/main")
                .as_deref(),
            Some("request-main")
        );
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
            .expect("restored page without user action allows the Agent's first claim");
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
            .expect("control returns automatically after the latest user-action idle window");
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
    fn popup_holder_cannot_refresh_an_ended_upstream_operation() {
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
        let retained = control
            .retain_agent_operation_for_popup("session-a", "0123456789abcdef")
            .expect("the popup must retain the live upstream operation");
        control.end_agent_operation(&authorization);
        assert!(control.authorize_retained_agent_operation(&retained));

        let deadline_after_end = control
            .state
            .lock()
            .agent_input_until
            .expect("ending dispatched input retains only callback grace");
        assert!(deadline_after_end <= Instant::now() + POST_DISPATCH_CALLBACK_GRACE);
        assert!(!control.refresh_agent_operation(&authorization));
        assert!(!control.refresh_agent_input_window(&authorization));
        assert_eq!(
            control.state.lock().agent_input_until,
            Some(deadline_after_end)
        );

        control.release_retained_agent_operation(&retained);
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
    fn final_dispatch_guard_never_runs_after_takeover_or_operation_end() {
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

        let dispatches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let first_dispatch = Arc::clone(&dispatches);
        assert_eq!(
            control
                .dispatch_if_agent_authorized(&authorization, move || {
                    first_dispatch.fetch_add(1, Ordering::SeqCst);
                    Ok("queued")
                })
                .unwrap(),
            Some("queued")
        );
        assert_eq!(dispatches.load(Ordering::SeqCst), 1);

        control.bump(Some(NativeControlOwner::User));
        let stale_dispatch = Arc::clone(&dispatches);
        assert_eq!(
            control
                .dispatch_if_agent_authorized(&authorization, move || {
                    stale_dispatch.fetch_add(1, Ordering::SeqCst);
                    Ok("must-not-run")
                })
                .unwrap(),
            None
        );
        assert_eq!(
            dispatches.load(Ordering::SeqCst),
            1,
            "takeover must win before the native dispatch closure can run"
        );

        let (next_snapshot, next_opaque_lease) = control.issue_agent_lease();
        let next = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            next_snapshot.revision,
            next_opaque_lease,
        )
        .unwrap();
        assert!(control.begin_agent_operation(&next, false));
        control.end_agent_operation(&next);
        let ended_dispatch = Arc::clone(&dispatches);
        assert_eq!(
            control
                .dispatch_if_agent_authorized(&next, move || {
                    ended_dispatch.fetch_add(1, Ordering::SeqCst);
                    Ok("must-not-run")
                })
                .unwrap(),
            None
        );
        assert_eq!(dispatches.load(Ordering::SeqCst), 1);
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
        // Before compensation is acknowledged, a repeated tombstone must return
        // the same record and cannot degrade to a bare Canceled state.
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

    #[test]
    fn refreshing_a_terminal_request_does_not_evict_its_current_state() {
        let mut ledger = RequestLedger::default();
        for index in 0..MAX_TERMINAL_REQUESTS {
            let request_id = format!("request-{index}");
            assert_eq!(
                ledger.claim("session-a", &request_id).unwrap(),
                NativeRequestClaim::Execute
            );
            assert!(ledger
                .complete("session-a", &request_id, json!({ "requestIndex": index }))
                .unwrap());
        }

        assert_eq!(
            ledger.cancel("session-a", "request-0").unwrap(),
            NativeRequestCancel::AlreadyCompleted(json!({ "requestIndex": 0 }))
        );
        ledger
            .acknowledge_cancellation("session-a", "request-0")
            .unwrap();

        assert_eq!(ledger.len(), MAX_TERMINAL_REQUESTS);
        assert_eq!(
            ledger
                .terminal_order
                .iter()
                .filter(|key| key.ends_with(":request-0"))
                .count(),
            1
        );
        assert_eq!(
            ledger.claim("session-a", "request-0").unwrap(),
            NativeRequestClaim::Canceled
        );
    }

    #[test]
    fn session_purge_removes_all_request_states_for_only_the_target_session() {
        let mut ledger = RequestLedger::default();

        assert_eq!(
            ledger.claim("session-a", "pending").unwrap(),
            NativeRequestClaim::Execute
        );
        assert_eq!(
            ledger.claim("session-a", "completed").unwrap(),
            NativeRequestClaim::Execute
        );
        assert!(ledger
            .complete("session-a", "completed", json!({ "kind": "complete" }))
            .unwrap());
        assert_eq!(
            ledger.claim("session-a", "awaiting").unwrap(),
            NativeRequestClaim::Execute
        );
        assert_eq!(
            ledger.cancel("session-a", "awaiting").unwrap(),
            NativeRequestCancel::AwaitingCompletion
        );
        assert_eq!(
            ledger.claim("session-a", "rollback").unwrap(),
            NativeRequestClaim::Execute
        );
        assert!(ledger
            .complete("session-a", "rollback", json!({ "kind": "rollback" }))
            .unwrap());
        assert_eq!(
            ledger.cancel("session-a", "rollback").unwrap(),
            NativeRequestCancel::AlreadyCompleted(json!({ "kind": "rollback" }))
        );
        assert_eq!(
            ledger.cancel("session-a", "canceled").unwrap(),
            NativeRequestCancel::Tombstoned
        );

        assert_eq!(
            ledger.claim("session-b", "completed").unwrap(),
            NativeRequestClaim::Execute
        );
        assert!(ledger
            .complete("session-b", "completed", json!({ "keep": true }))
            .unwrap());
        assert_eq!(ledger.len(), 6);

        assert_eq!(ledger.purge_session("session-a").unwrap(), 5);
        assert_eq!(ledger.len(), 1);
        assert_eq!(
            ledger.claim("session-b", "completed").unwrap(),
            NativeRequestClaim::Replay(json!({ "keep": true }))
        );
    }

    #[test]
    fn failed_teardown_retains_pending_rollback_until_successful_session_purge() {
        let mut ledger = RequestLedger::default();
        assert_eq!(
            ledger.claim("session-a", "request-retry").unwrap(),
            NativeRequestClaim::Execute
        );
        let rollback = json!({ "rollback": { "kind": "created_tab" } });
        assert!(ledger
            .complete("session-a", "request-retry", rollback.clone())
            .unwrap());
        assert_eq!(
            ledger.cancel("session-a", "request-retry").unwrap(),
            NativeRequestCancel::AlreadyCompleted(rollback.clone())
        );

        // A failed browser teardown skips purge. The exact rollback record must
        // remain available to the next cancellation/cleanup retry.
        assert_eq!(
            ledger.cancel("session-a", "request-retry").unwrap(),
            NativeRequestCancel::AlreadyCompleted(rollback)
        );
        assert_eq!(ledger.len(), 1);

        assert_eq!(ledger.purge_session("session-a").unwrap(), 1);
        assert_eq!(ledger.len(), 0);
        assert_eq!(
            ledger.claim("session-a", "request-retry").unwrap(),
            NativeRequestClaim::Execute
        );
    }
}
