# PR #668 round-52 review draft v2 (WORKING NOTES)

## Verdict: CHANGES_REQUESTED
Substance approve-grade (r49-51 verdicts re-confirmed: useful/complete, elegant, smuggle-CLEAN per lane 9); blocked on (a) five author-accepted fixes still undelivered, (b) one new PR-level MAJOR pair from this round, (c) delivery-integrity (fake-merge head, discarded lineage, body-vs-tree drift).

## Owed (author-accepted r50, all re-verified open at rebased tree)
- M1 reconcile two-state discriminator (checkpoints.rs:178-194; completion arm :1105-1114 has the three-state proof) — persist-split + non-protocol writer → silent pending clear, code/conversation fork, undo dead. Lane 2 re-derived with full crash matrix; fix = third condition (truncated_revision == current or count==keep).
- M2 Windows reparse tag-blindness (platform/mod.rs:41-49) — OneDrive/cloud placeholders brick snapshot/diff/restore; lane 4 verified std is_symlink is tag-agnostic; fix = FSCTL_GET_REPARSE_POINT / FindFirstFileW dwReserved0, refuse MOUNT_POINT only.
- M3 icase survivor purge missing on create/diff paths (mod.rs:1925 vs :1546/:2802) — dual-variant secret bricks snapshots every turn; lane 1 ADDITIONALLY found the DIFF path lacks even the batch secret purge (MAJOR: `!.env` negation → checkpoint_diff bails forever → confirm-gating kills rewind UI while create snapshots fine). Fix both: purge (batch + exact survivors) on diff path like create.
- M4 refuseWhenBusy stale buffer for ACTIVE session (sessions.js:219-226) — lane 7 PROVED via real factory repro; foreground mid-turn compaction → queued follow-up silently lost, composer re-enables mid-turn. Fix = consult live state for active session.
- T1 zero flow-level step-budget coverage — lane 10 confirmed: only guard is one regex pin; need behavioral fixture through count/truncate/fork.

## New this round (fresh findings)
- MAJOR-N1 (caliber migration gap): merged counting = era+cut vs main-built raw-era v2 indexes vs PR-built per-message v2 indexes; no v3 migration; CheckpointMeta.turn stored at write → wrong checkpoint↔boundary pairing → silent wrong-point restore on notice/pasted-header transcripts. Fix: v3 + conservative Turn-entry retire (r13-M1 pattern) + fixture.
- MAJOR-N2 (fork echo asymmetry, lane 3): frontendEcho skip added to rewind builder only; forkTurnEntries.js (main's) lacks it → fork ordinals +1 from echo onward, fork-one-past-click, no gate; "口径一致" comments false. Fix: one-line skip + fixture.
- MAJOR-N3 (edit-selector divergence, lane 3): foundation edit_last_turn selector still targets step-budget notices (Editable/ExternalUser) while PR hides them from timeline + cuts them from caliber → regenerate/edit on visible last bubble truncates at the INVISIBLE notice → duplicate turn, desync, gate-hides-chips. Benign on main (notice renders); PR's hiding creates the mismatch. Fix: foundation cut-aware selector or app-side verification + docs/§3.
- MINOR: steer_chat lacks ACP fence (7th entry); edit_last_turn lacks aux fence; fork-on-ACP-records (NIT); pending-delete arms bypass pending flock (lane 1/2); replay-arm compaction latch captured bool (lane 2); quarantine 7th writer (disclosed); redundant spawns (entry count-objects, write-only update-ref head ref, double shadow_repo_bytes walk); rewind_undo_state 3×/turn full parse; shrink-hook clone under global lock.
- Body accuracy: r49/50 addenda describe discarded lineage (chat-view split, DCO, vite pins) not present at this head; rusqlite "0.41.1 kept" stale (main at 0.43); shared-fn 318 → 322 measured; v4 stub... (drop). CI line tightens ✓. i18n parity ✓. Boundaries ✓. Fork boundary: gitlink unchanged, no fork-modifications needed ✓.

## Merge-resolution debts (MINE — all fixed in-tree, to amend)
1. turns.rs test import (E0425) → restored. cargo check --lib --tests GREEN.
2. Bare-header doctrine: dropped coarse isCompactionCarrierBlocks from rerender core + code lane (stamped-only pill arm kept; tool-result carriers kept) — matches main #836-era contract + its behavioral pins.
3. Five stale pins/tests re-anchored: web_access turns needle (era+cut), wiring needle (userText + stamped-only pins), steer fixtures (text), session_import_contract path, byte gate (web listener ported w/ null invalidateSessionBuffer binding), lane_deps 322.
4. eslint cognitive-complexity directive on inner callback, identical text both lanes; stale directives removed; useless path segment fixed.
5. mod.rs/fork.rs/engine_pool comment accuracy.

## Verification state
- arch-guard ✓, fork-guard --fast ✓
- Targeted suites green post-fix: code_native_lane, chat_rewind_wiring, codex_checkpoints_logic, chat_manual_compact, session_import_contract, bridge_lane_deps, bridge_domain_protocol, artifact_replay (42)
- lint:ui: 0 errors (3 pre-existing main-side warnings in untouched files)
- Full npm test + Rust battery: RUNNING
- Mutations (frontend): echo-skip RED, refuseWhenBusy RED ✓

## Review mechanics
- Post as review comment (REQUEST_CHANGES via REST). Include: rebased merge SHA 81119ad96 + amend note, per-item mapping, author actions (deliver M1-M4+T1+N1+N2+N3 wave from healthy machine; or I can push the rebased merge on request).
- Not pushing the branch: author's workstation corrupted; their head is their declared verified state; my merge offered as reference.
