//! Cohesive helpers used by the engine event bridge.
//!
//! Keeping notification, scheduled policy, artifact persistence, and turn
//! signal correlation here prevents the already-large event forwarder from
//! accumulating unrelated lifecycle support code.

use std::path::{Path, PathBuf};

use anyhow::Result;
use deepseek_tui::ApprovalMode;
use deepseek_tui::core::events::TurnOutcomeStatus;
use deepseek_tui::core::ops::{Op, UserInputProvenance};
use tauri::{AppHandle, Manager};

use crate::features::sessions::{ScheduledRunProfile, SessionStore};

/// Per-turn `<system-reminder>` envelope assembly, shared by every path that
/// prepends a reminder to the user message: the reminder body goes on its own
/// line inside the envelope, followed by a blank line, then the untouched
/// user content. Keeping one formatter ensures every injection site stays
/// byte-identical (tests assert on the exact envelope shape).
pub(super) fn wrap_system_reminder(body: &str, content: &str) -> String {
    format!("<system-reminder>\n{body}\n</system-reminder>\n\n{content}")
}

/// Dedup key a global desktop notification is filed under. The `':'` is
/// load-bearing: `NotificationState::should_notify` prunes by the session
/// prefix before the first colon, so the key must keep the session id and
/// the event id separable.
fn desktop_notification_key(session_id: &str, event_id: &str) -> String {
    format!("{session_id}:{event_id}")
}

/// Shared tail of the global desktop-notification pipeline, in gate order:
/// aux exclusion → per-kind pref gate (`check_enabled` runs only here, so
/// aux sessions never pay the settings-file read) → per-session dedup →
/// benchmark/eval suppression → send. Both notification kinds name no
/// session, which is what the aux gate protects.
fn maybe_send_desktop_notification(
    app: &AppHandle,
    session_id: &str,
    notify_key: String,
    check_enabled: impl FnOnce() -> bool,
    send: fn(&AppHandle),
) {
    if !global_desktop_notification_in_scope(session_id) {
        return;
    }
    if !check_enabled() {
        return;
    }
    let notification_state = app
        .try_state::<crate::platform::notifications::NotificationState>()
        .map(|state| state.should_notify(notify_key));
    // Eval observation runs must stay quiet, and the predicate must be
    // checked directly: riding `unwrap_or_else` on `notification_state`
    // would be dead code — the app setup always manages that state, so the
    // fallback arm is unreachable and observation sessions would bubble
    // (fresh review 2026-11-12).
    #[cfg(feature = "benchmark-hooks")]
    let should_notify = !crate::features::assistant::timing::eval_observation_enabled(session_id)
        && notification_state.unwrap_or(true);
    #[cfg(not(feature = "benchmark-hooks"))]
    let should_notify = notification_state.unwrap_or(true);
    if should_notify {
        send(app);
    }
}

pub(super) fn maybe_notify_task_completed(
    app: &AppHandle,
    session_id: &str,
    turn_id: Option<String>,
    status: TurnOutcomeStatus,
    error: Option<&str>,
) {
    if status != TurnOutcomeStatus::Completed || error.is_some() {
        return;
    }
    let key = turn_id.unwrap_or_else(|| chrono::Utc::now().timestamp_millis().to_string());
    maybe_send_desktop_notification(
        app,
        session_id,
        desktop_notification_key(session_id, &key),
        crate::platform::notifications::task_completion_enabled,
        crate::platform::notifications::notify_task_completed,
    );
}

/// Desktop notification when the engine parks on `request_user_input`: the
/// model is blocked until the user answers, and without this the only signal
/// lives inside the app window. The unattended-scheduled path cancels the
/// input before reaching the forwarder's interactive branch, so this only
/// fires for sessions with a human operator — save a narrow window after
/// `wait_for_scheduled_terminal`'s external-cancel deadline, where the
/// unattended guard has already dropped but the engine may not be dead yet
/// (at most one pref-gated bubble per distinct input id: the engine emits
/// once per park, and the dedup bounds only same-id repeats). Eval hosts
/// suppress it the same way as completion notifications (observation runs
/// must stay quiet).
pub(super) fn maybe_notify_input_needed(app: &AppHandle, session_id: &str, input_id: &str) {
    maybe_send_desktop_notification(
        app,
        session_id,
        desktop_notification_key(session_id, input_id),
        crate::platform::notifications::needs_input_enabled,
        crate::platform::notifications::notify_input_needed,
    );
}

/// Whether a turn of this session may raise a global desktop notification
/// (task-completed or input-needed — both name no session). Aux side-chat
/// turns are excluded (round-26 minor M3): an aux notification popping while
/// its main task still runs would be indistinguishable from the main task's
/// own notification — the same global-side-effect class as the round-15
/// memory-review exclusion (forwarder.rs).
fn global_desktop_notification_in_scope(session_id: &str) -> bool {
    crate::features::sessions::aux_side_effect_exclusion(session_id)
}

pub(super) fn persist_successful_tool_artifact(
    store: &SessionStore,
    session_id: &str,
    workspace: &Path,
    tool_name: &str,
    tool_input: &serde_json::Value,
    output: &str,
) -> Result<Option<PathBuf>> {
    if store.scheduled_profile(session_id).is_none() {
        return Ok(None);
    }
    let mut raw_paths = artifact_paths_from_tool_output(output);
    if is_file_artifact_tool(tool_name, tool_input) {
        raw_paths.extend(artifact_paths_from_value(tool_input));
    }
    raw_paths.sort();
    raw_paths.dedup();
    if raw_paths.is_empty() {
        return Ok(None);
    }
    let patch_tool = is_patch_artifact_tool(tool_name, tool_input);
    let mut persisted = Vec::new();
    for raw_path in raw_paths {
        let resolved =
            crate::platform::path_policy::resolve_artifact_path_in_workspace(&raw_path, workspace);
        // PatchResult.touched_files also reports deletions. They are successful
        // mutations, but no file remains to append to the scheduled artifact list.
        if patch_tool && !Path::new(&resolved).exists() {
            continue;
        }
        let path = crate::platform::path_policy::validate_user_path(&resolved)
            .map_err(|error| anyhow::anyhow!(error))?;
        if !path.is_file() {
            anyhow::bail!("tool artifact is not an existing file: {}", path.display());
        }
        store.append_scheduled_artifact_path(session_id, path.clone())?;
        persisted.push(path);
    }
    Ok(persisted.into_iter().next())
}

fn is_patch_artifact_tool(name: &str, input: &serde_json::Value) -> bool {
    (name.eq_ignore_ascii_case("File")
        && input.get("action").and_then(serde_json::Value::as_str) == Some("patch"))
        || name == "apply_patch"
        || name.ends_with("_apply_patch")
}

fn push_artifact_path(paths: &mut Vec<String>, path: &str) {
    let path = path.trim();
    if !path.is_empty() && !paths.iter().any(|existing| existing == path) {
        paths.push(path.to_string());
    }
}

fn artifact_paths_from_patch(patch: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in patch.lines() {
        if let Some(path) = ["*** Add File:", "*** Update File:", "*** Delete File:"]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
        {
            push_artifact_path(&mut paths, path);
            continue;
        }
        if let Some(path) = line.strip_prefix("+++ ") {
            let path = path.strip_prefix("b/").unwrap_or(path);
            if path != "/dev/null" {
                push_artifact_path(&mut paths, path);
            }
        }
    }
    paths
}

fn artifact_paths_from_changes(value: &serde_json::Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(artifact_paths_from_value)
        .collect()
}

fn artifact_paths_from_tool_output(output: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(output) else {
        return Vec::new();
    };
    let payload = value
        .get("content")
        .and_then(serde_json::Value::as_array)
        .and_then(|content| content.first())
        .and_then(|item| item.get("text"))
        .and_then(serde_json::Value::as_str)
        .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
        .unwrap_or(value);
    artifact_paths_from_value(&payload)
}

fn artifact_paths_from_value(value: &serde_json::Value) -> Vec<String> {
    let mut paths = Vec::new();
    for key in ["abs_path", "path", "file_path", "local_path", "filename"] {
        if let Some(path) = value.get(key).and_then(serde_json::Value::as_str) {
            push_artifact_path(&mut paths, path);
        }
    }
    if let Some(touched) = value
        .get("touched_files")
        .and_then(serde_json::Value::as_array)
    {
        for path in touched.iter().filter_map(serde_json::Value::as_str) {
            push_artifact_path(&mut paths, path);
        }
    }
    paths.extend(artifact_paths_from_changes(value, "replace"));
    paths.extend(artifact_paths_from_changes(value, "changes"));
    if let Some(patch) = value.get("patch").and_then(serde_json::Value::as_str) {
        paths.extend(artifact_paths_from_patch(patch));
    }
    paths.sort();
    paths.dedup();
    paths
}

fn is_file_artifact_tool(name: &str, input: &serde_json::Value) -> bool {
    if name.eq_ignore_ascii_case("File") {
        return input
            .get("action")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|action| matches!(action, "write" | "edit" | "patch"));
    } else {
        matches!(
            name,
            "write" | "edit" | "write_file" | "edit_file" | "apply_patch"
        ) || ["write_file", "edit_file", "apply_patch"]
            .iter()
            .any(|tool| name.ends_with(&format!("_{tool}")))
    }
}

/// Lifecycle signal emitted by the single authoritative engine event consumer.
/// Scheduled-task execution subscribes to this channel instead of competing for
/// `EngineHandle::rx_event` with the UI forwarder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EngineTurnSignal {
    Started {
        turn_id: String,
    },
    Terminal {
        turn_id: String,
        status: TurnOutcomeStatus,
        error: Option<String>,
    },
    ForwarderStopped {
        error: String,
    },
}

/// Correlates terminal events with the currently running turn. A fatal
/// `Event::Error` is advisory until the engine emits its authoritative
/// `Event::TurnComplete`; the cached error fills a missing terminal error.
#[derive(Debug, Default)]
pub(super) struct TurnCompletionTracker {
    active_turn_id: Option<String>,
    pending_fatal_error: Option<String>,
}

impl TurnCompletionTracker {
    pub(super) fn on_started(&mut self, turn_id: String) -> EngineTurnSignal {
        self.active_turn_id = Some(turn_id.clone());
        self.pending_fatal_error = None;
        EngineTurnSignal::Started { turn_id }
    }

    pub(super) fn on_fatal_error(&mut self, error: String) {
        self.pending_fatal_error = Some(error);
    }

    pub(super) fn on_terminal(
        &mut self,
        status: TurnOutcomeStatus,
        error: Option<String>,
    ) -> Option<EngineTurnSignal> {
        let error = error.or_else(|| self.pending_fatal_error.take());
        self.active_turn_id
            .take()
            .map(|turn_id| EngineTurnSignal::Terminal {
                turn_id,
                status,
                error,
            })
    }
}

/// Apply the persisted automation policy to the normal bridge-built operation.
/// The bridge remains the source of hooks, tool restrictions, and provider
/// details; scheduled execution only overrides fields explicitly owned by the
/// automation record.
pub(super) fn apply_scheduled_turn_policy(
    op: &mut Op,
    profile: &ScheduledRunProfile,
    resolved_route: deepseek_tui::route_runtime::ResolvedRuntimeRoute,
    compaction_config: deepseek_tui::compaction::CompactionConfig,
) -> Result<()> {
    let Op::SendMessage {
        mode,
        route,
        compaction,
        auto_model,
        allow_shell,
        trust_mode,
        auto_approve,
        approval_mode,
        provenance,
        ..
    } = op
    else {
        anyhow::bail!("scheduled turn requires a SendMessage operation");
    };

    *mode = profile.execution_mode().to_app_mode();
    **route = resolved_route;
    **compaction = compaction_config;
    *auto_model = false;
    *allow_shell = profile.allow_shell;
    *trust_mode = profile.trust_mode;
    *auto_approve = profile.auto_approve;
    *approval_mode = if profile.auto_approve {
        ApprovalMode::Auto
    } else {
        ApprovalMode::Never
    };
    *provenance = UserInputProvenance::ExternalUser;
    Ok(())
}

/// The `None` arm denies (round-19 review minor 13): unreachable today — the
/// caller gates on `scheduled_profile.is_some()` — but a missing profile must
/// default to the strict direction, so a future guard change that ever makes
/// `None` reachable fails closed instead of silently auto-approving.
pub(super) fn scheduled_tool_should_auto_approve(
    profile: Option<&ScheduledRunProfile>,
    approval_force_prompt: bool,
) -> bool {
    profile.is_some_and(|profile| profile.auto_approve && !approval_force_prompt)
}

#[cfg(test)]
mod tests {
    use super::{desktop_notification_key, global_desktop_notification_in_scope};

    #[test]
    fn desktop_notification_key_keeps_session_and_event_separable() {
        // NotificationState::should_notify prunes by the prefix before the
        // first ':' — a separator drop (or a session-only key) would make
        // every session share one dedup slot and let one event suppress
        // another's notification. Pin the shape at the only key builder.
        let key = desktop_notification_key("0123456789abcdef", "turn_ab12cd34");
        assert_eq!(
            key.split_once(':'),
            Some(("0123456789abcdef", "turn_ab12cd34")),
            "the dedup key must keep the session id separable from the event id",
        );
    }

    #[test]
    fn desktop_notifications_exclude_aux_sessions() {
        assert!(!global_desktop_notification_in_scope(
            "aux-0123456789abcdef"
        ));
        assert!(global_desktop_notification_in_scope("0123456789abcdef"));
        assert!(global_desktop_notification_in_scope("sched-daily-report"));
    }
}

#[cfg(test)]
mod desktop_notification_wiring_pins {
    //! The two `maybe_notify_*` wrappers pair each notification kind with
    //! its own pref gate and sender. Swapping the pair (or the dedup-key
    //! argument order) compiles cleanly — `fn() -> bool` and
    //! `fn(&AppHandle)` parameter shapes are interchangeable — and no
    //! behavior test drives the real wiring, so the pairing is pinned here
    //! against this file's own source, the same way `forwarder.rs` pins its
    //! call-site wiring shape.

    /// Whitespace-stripped body of the fn with the given signature prefix,
    /// after a two-pass comment strip (the same caliber as
    /// `forwarder_body`): line-comment tails first — a `//`-commented-out
    /// `/*` marker cannot eat live code — then closed `/* … */` spans
    /// removed with nesting-depth tracking (review JensenChen28 4521432771
    /// and 4525154638). The strip runs before brace matching, so a
    /// mutation hiding braces inside comments cannot derail the body
    /// scan. Residual exposure, unchanged: needles inside string literals
    /// or trailing `//` mentions still satisfy a `contains` pin; the
    /// pinned wrappers contain no braces in string or char literals (a
    /// future edit adding one fails loudly here instead of silently
    /// weakening the pin).
    fn pinned_fn_body(signature: &str) -> String {
        pinned_body_from_src(include_str!("engine_support.rs"), signature)
    }

    /// [`pinned_fn_body`] against a caller-supplied source, so the strip
    /// caliber itself is testable against mutated sources.
    fn pinned_body_from_src(src: &str, signature: &str) -> String {
        let stripped = strip_block_comments(&without_line_tails(src));
        let sig_at = stripped.find(signature).expect("pinned fn must exist");
        let body_at = stripped[sig_at..].find('{').expect("fn body brace") + sig_at;
        let mut depth = 0usize;
        for (offset, ch) in stripped[body_at..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return stripped[body_at..=body_at + offset]
                            .chars()
                            .filter(|c| !c.is_whitespace())
                            .collect();
                    }
                }
                _ => {}
            }
        }
        panic!("unbalanced braces in pinned fn body");
    }

    /// Line-comment tails dropped per line (a `//`-commented-out `/*`
    /// marker cannot make the block pass eat live code), preserving
    /// newlines.
    fn without_line_tails(src: &str) -> String {
        src.split('\n')
            .map(|line| {
                line.split("//")
                    .next()
                    .expect("split always yields one item")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Newline-preserving removal of closed `/* … */` spans, tracking
    /// nesting depth: Rust block comments nest, so a span's balancing
    /// close is the `*/` that returns the depth to zero, not the first
    /// `*/` seen (review JensenChen28 4525154638, same scanner class as
    /// the shared `comment_filtered` and `forwarder_body`). An
    /// unterminated `/*` is kept verbatim (scan past the marker).
    fn strip_block_comments(src: &str) -> String {
        let mut out = String::with_capacity(src.len());
        let mut rest = src;
        while let Some(open) = rest.find("/*") {
            let after_open = &rest[open + 2..];
            let mut depth = 1usize;
            let mut scan = 0usize;
            let mut close_end = None;
            while depth > 0 {
                let next_open = after_open[scan..].find("/*").map(|rel| scan + rel);
                let next_close = after_open[scan..].find("*/").map(|rel| scan + rel);
                match (next_open, next_close) {
                    (Some(open_at), Some(close_at)) if open_at < close_at => {
                        depth += 1;
                        scan = open_at + 2;
                    }
                    (_, Some(close_at)) => {
                        depth -= 1;
                        if depth == 0 {
                            close_end = Some(close_at + 2);
                        } else {
                            scan = close_at + 2;
                        }
                    }
                    (Some(open_at), None) => {
                        depth += 1;
                        scan = open_at + 2;
                    }
                    (None, None) => break,
                }
            }
            match close_end {
                Some(end) => {
                    out.push_str(&rest[..open]);
                    let inner = &after_open[..end - 2];
                    for _ in 0..inner.matches('\n').count() {
                        out.push('\n');
                    }
                    rest = &after_open[end..];
                }
                None => {
                    // Unterminated block: keep it verbatim, scan past the marker.
                    out.push_str(&rest[..open + 2]);
                    rest = after_open;
                }
            }
        }
        out.push_str(rest);
        out
    }

    #[test]
    fn each_notification_kind_keeps_its_own_pref_and_sender() {
        let input = pinned_fn_body("pub(super) fn maybe_notify_input_needed(");
        // Key argument order is semantics: it must be (session, input).
        // Swapping silently loses the per-session dedup pruning
        // (should_notify prunes by the ':' prefix).
        assert!(
            input.contains("desktop_notification_key(session_id,input_id)"),
            "the input notification must dedup under (session, input) argument order",
        );
        // Cross-kind pairing mutations (swapping the pref or the sender) are
        // invisible to the compiler, so pin the adjacent shape.
        assert!(
            input.contains(
                "needs_input_enabled,crate::platform::notifications::notify_input_needed",
            ),
            "the input notification must pair needs_input_enabled with notify_input_needed",
        );
        assert!(
            !input.contains("task_completion") && !input.contains("notify_task_completed"),
            "the input notification must not carry completion wiring",
        );

        let completed = pinned_fn_body("pub(super) fn maybe_notify_task_completed(");
        assert!(
            completed.contains("desktop_notification_key(session_id,&key)"),
            "the completion notification must dedup under (session, turn) argument order",
        );
        assert!(
            completed.contains(
                "task_completion_enabled,crate::platform::notifications::notify_task_completed",
            ),
            "the completion notification must pair task_completion_enabled with notify_task_completed",
        );
        assert!(
            !completed.contains("needs_input_enabled")
                && !completed.contains("notify_input_needed"),
            "the completion notification must not carry input wiring",
        );
    }

    #[test]
    fn eval_suppression_checks_the_predicate_directly() {
        // The app setup always manages `NotificationState` (lib.rs), so an
        // eval check riding `unwrap_or_else` on that state is unreachable —
        // observation sessions would pass the gate and bubble. The
        // predicate must be ANDed in front of the dedup fallback instead
        // (fresh review 2026-11-12).
        let gate = pinned_fn_body("fn maybe_send_desktop_notification(");
        assert!(
            gate.contains(
                "!crate::features::assistant::timing::eval_observation_enabled(session_id)\
                 &&notification_state.unwrap_or(true)",
            ),
            "eval suppression must AND the predicate before the dedup fallback",
        );
        assert!(
            !gate.contains("unwrap_or_else"),
            "the eval check must not ride unwrap_or_else on the always-managed dedup state",
        );
    }

    #[test]
    fn block_commented_wrapper_body_keeps_no_needle_alive() {
        // Review JensenChen28 4521432771: pinned_fn_body used to strip
        // line-comment tails only, so a compiling mutation that wrapped the
        // executable wrapper body in `/* … */` kept every needle substring
        // alive while the wrapper stopped issuing notifications. The block
        // pass must remove the needle with the span.
        let mutated = "pub(super) fn maybe_notify_input_needed(\n    session_id: &str,\n    input_id: &str,\n) {\n    /*\n    let key = desktop_notification_key(session_id, input_id);\n    crate::platform::notifications::notify_input_needed(needs_input_enabled, &key);\n    */\n}";
        let body = pinned_body_from_src(mutated, "pub(super) fn maybe_notify_input_needed(");
        assert!(
            !body.contains("desktop_notification_key(session_id,input_id)"),
            "a block-commented wrapper body must not satisfy a pin needle",
        );
        assert!(
            !body.contains("notify_input_needed(needs_input_enabled"),
            "every needle inside the commented span must be removed",
        );
    }

    #[test]
    fn nested_block_commented_wrapper_body_keeps_no_needle_alive() {
        // Nesting depth (review JensenChen28 4525154638 on the shared
        // filter; this scanner is the same class): Rust block comments
        // nest, so the first `*/` must not close the outer span —
        // otherwise an outer comment holding an inner `/* … */` keeps the
        // needle's substring alive past the inner close.
        let mutated = "pub(super) fn maybe_notify_input_needed(\n    session_id: &str,\n    input_id: &str,\n) {\n    /* outer\n    /* inner */\n    let key = desktop_notification_key(session_id, input_id);\n    */\n}";
        let body = pinned_body_from_src(mutated, "pub(super) fn maybe_notify_input_needed(");
        assert!(
            !body.contains("desktop_notification_key(session_id,input_id)"),
            "a needle behind a nested block-comment span must be removed",
        );
        assert!(
            !body.contains("*/"),
            "the balancing close must be consumed with the span",
        );
    }

    /// Review P2: windowless hosts (`pinvou agent run` and other boot lanes) have no approval decision
    /// consumer, so interactive approvals must decline deterministically instead of pinning the task to the deadline. The forward
    /// command body cannot be built in a unit test, so per repo convention this pins the assembly order: the windowless arm sits after the scheduled
    /// unattended arm (task-preservation policy unchanged) and before the interactive surface; inside the arm only the deterministic
    /// decline (deny_tool_call) happens — no pending registration, no approval event — a card no one
    /// can answer must not be minted.
    #[test]
    fn forkguard_forwarder_windowless_arm_declines_without_minting_a_card() {
        let src = include_str!("forwarder.rs");
        let production = src.split("#[cfg(test)]").next().expect("non-empty source");
        let scheduled_at = production
            .find("scheduled_tool_should_auto_approve(")
            .expect("the scheduled unattended arm must stay first");
        // Anchor from the scheduled arm: the UserInputRequired routing now
        // carries its own (earlier) windowless marker — round-11 typed-ask
        // decline — so a file-global find would shadow this arm.
        let windowless_at = scheduled_at
            + production[scheduled_at..]
                .find("windowless_host::is_windowless_host()")
                .expect("the windowless host arm is missing from the ApprovalRequired routing");
        let interactive_at = scheduled_at
            + production[scheduled_at..]
                .find("approval_surface_decision(")
                .expect("the interactive arm must stay last");
        assert!(
            scheduled_at < windowless_at && windowless_at < interactive_at,
            "arm order: scheduled stored-policy → windowless decline → interactive surface"
        );
        let arm = &production[windowless_at..interactive_at];
        assert!(
            arm.contains("deny_tool_call("),
            "the windowless arm must resolve the ask deterministically with a decline"
        );
        assert!(
            arm.contains("windowless host has no approval consumer"),
            "the decline must name its reason in the boot-lane log line"
        );
        assert!(
            !arm.contains("pending_approvals::record("),
            "a card no one can answer must not be minted"
        );
        assert!(
            !arm.contains("approval_required"),
            "the windowless arm must not raise an approval event"
        );
        assert!(
            !arm.contains("approve_tool_call("),
            "the decline is fail-safe: no grant path in the windowless arm"
        );
    }

    /// round-11 review MAJOR: the `Event::UserInputRequired` (typed ask) routing previously had
    /// only scheduled/interactive arms — windowless hosts fell into the interactive arm, and a confirmation
    /// card no one can answer pinned the agentic run to the whole-task timeout (run reported timeout instead of
    /// an explainable result). This pin locks the third arm between scheduled and interactive: inside the arm a
    /// deterministic cancellation (cancel_user_input, funneled by the base into a tool result), no pending
    /// registration, and no `chat:user_input_required`.
    #[test]
    fn forkguard_forwarder_windowless_typed_ask_declines_without_minting() {
        let src = include_str!("forwarder.rs");
        let production = src.split("#[cfg(test)]").next().expect("non-empty source");
        let typed_at = production
            .find("Event::UserInputRequired")
            .expect("the typed-ask routing must stay in the forwarder");
        let windowless_at = production[typed_at..]
            .find("windowless_host::is_windowless_host()")
            .expect("the windowless host arm is missing from the UserInputRequired routing");
        let interactive_at = production[typed_at..]
            .find("pending_user_input::record(")
            .expect("the interactive arm must keep the pending registration");
        assert!(
            windowless_at < interactive_at,
            "arm order: scheduled stored-policy → windowless decline → interactive pending"
        );
        let arm = &production[typed_at + windowless_at..typed_at + interactive_at];
        assert!(
            arm.contains("cancel_user_input("),
            "the windowless typed-ask arm must resolve the ask deterministically with a cancel"
        );
        assert!(
            arm.contains("windowless host has no input consumer"),
            "the decline must name its reason in the boot-lane log line"
        );
        assert!(
            !arm.contains("chat:user_input_required"),
            "the windowless arm must not raise a user-input event"
        );
    }
}
