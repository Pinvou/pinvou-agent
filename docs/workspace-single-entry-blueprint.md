# Single-Entry Workspace Blueprint

> **Provenance.** This document is the design reference for PR #484
> (branch `feat/workspace-entry`: the in-app workspace picker, project
> channel, keychain snapshot, manage-folders panel, and the §9 channel
> rulings). It was reconstructed retroactively from the implementation so
> that every `§N` citation in the code is auditable. Section numbers below
> are exactly the ones the code comments refer to — e.g. `§9.9` in
> `pinvou3-app/src-tauri/src/features/projects/store.rs` or `§2/§3/§9.3/§9.4`
> in `pinvou3-app/src/features/projects/workspacePickerState.js` resolve to
> the matching sections of this document. Where a comment and this document
> could ever disagree, the code is the source of truth.
>
> **Scope note.** `§N` citations in files outside the workspace feature
> (checkpoints/rewind, image-input capability, marketplace, knowledge, …)
> point at their own design documents (`docs/code-mode-改动随对话回退-设计.md`,
> `docs/marketplace-unification.md`, etc.) and are **not** governed by this
> blueprint.

## §1 Goals and non-goals

**Goals.** Make the **project** the only organizing unit of workspace-bound
sessions, across both session lanes (the native chat lane and the code/ACP
lane):

- One in-app "choose workspace" entry replaces ad-hoc per-lane folder
  pickers (§2).
- Physical folders are absorbed as *single-root projects* via
  Codex-client-style auto-materialization, with a user-controlled
  anti-materialization exclusion list (§3).
- A manage-folders panel gives full control over a project's folder
  territory (§4).
- Sidebar grouping is split into a physical folder view and a logical
  project view with a two-tier membership rule (§5).
- Every bound session locks a **keychain snapshot** — the full set of
  accessible roots — at creation time (§6).
- The per-channel rulings (§9.1–§9.9) define how each entry point grants
  folder access, how access is announced, and how overlap, deletion, and
  re-adoption behave.

**Non-goals.**

- Projects never delete, move, or rebind session *data*; membership is a
  pure logical layer on top of the physical working-directory binding.
- The projects domain is desktop-only; project events are never forwarded
  to the remote-control relay (the web stance is §9.8).
- No automatic project deletion when folders disappear from disk: root
  availability is surfaced ("folder unavailable · rebind") but never acted
  on automatically.

## §2 The unified "choose workspace" picker (single entry)

One picker dialog serves both lanes as the single in-app entry for choosing
a workspace (`WorkspacePickerDialog.jsx`, pure display; all consequential
actions are handed to the host container in `main.jsx` via callbacks).

- **Host-owned landing.** The host owns the open flag and the result. The
  chat lane stages the choice as a bridge *draft*
  (`bridge.sessions.setDraftWorkspace(path, { projectId, workspaceRoots })`),
  carried into `create_session` at materialization; the code lane is handed
  the request through `workspacePickerRequest` into `CodexAcpView`
  (draft ownership staging; `createAcpSession` passes
  `workspaceRoots`/`projectId` at materialization). The picker itself never
  touches the backend.
- **Row model.** The picker offers: project rows (multi-root projects
  expand inline to re-pick the root), a browse entry (§9.9 folder channel),
  and an explicit **temporary session** option (§9.1).
- **Hot view.** Rows are sorted by most recent activity descending
  (`computePickerRows`). A materialized `origin=folder` project with no
  explicit members and no activity for 30 days
  (`COLD_PROJECT_IDLE_MS`) is hidden from the picker as an accumulation
  mitigation (§3); the sidebar's full view is unaffected and nothing is
  deleted. Unparseable timestamps are treated as active (rather show than
  wrongly hide). Tag-only (rootless) projects never appear: there is no
  root to bind, and the row renderer has no disabled state for them.
- **Grant notices** shown by the picker follow §9.4.

## §3 Auto-materialization and the anti-materialization exclusion list

**Auto-materialization** (`ensure_folder_projects` /
`ProjectStore::ensure_folder_roots`) is client-driven: the frontend
aggregates the distinct workspace folders backing sessions
(`uncoveredWorkspaceRoots`) and asks the backend to adopt them, isomorphic
to Codex's `project/import` being initiated by the desktop client. Per-root
outcomes:

- `Created` — a new `origin=folder` project named after the directory
  basename, anchored at exactly that folder;
- `Covered` — an existing materialized project anchored at this folder is
  reused untouched (anchored coverage, §9.9);
- `Failed` — per-root failure (e.g. non-absolute path) that never blocks
  the rest of the batch.

Input is deduplicated by canonical identity key; the batch shares a single
persist; `projects:list_changed` is broadcast only when something was
created.

**Exclusion list.** `never_materialize_roots` (canonical identity keys,
persisted in the same `projects.json`) is the user's explicit "never
auto-create a project for this folder":

- `projects_set_never_materialize(root, never)` adds/revokes entries;
  idempotent; affects only *future* auto-materialization — existing
  projects and session assignments are untouched.
- The list is visible and revocable in the manage-folders panel (§4).
- A folder on the list is skipped by `ensure` **without producing an
  outcome** (same as input dedup). The picker's browse channel detects this
  (no `Created`/`Covered` outcome) and says so honestly in an inline panel,
  while still allowing the user to start a plain folder session
  (no `projectId`) — exclusion bans the *project*, not the folder.

**Accumulation mitigation.** Two mechanisms keep materialized projects from
piling up: the 30-day cold-project hiding in the picker's hot view (§2),
and the explicit-move-out tombstones written on project deletion, which
suppress re-adoption of old members (§9.9).

## §4 The manage-folders panel

Per-project panel (`ManageProjectFoldersDialog.jsx` + pure logic in
`manageFoldersState.js`) reached from the project group header; covers:

- **View** roots with per-root availability badges (directory still on
  disk) and the primary marker;
- **Add** a root via the system folder picker; exact duplicates are blocked
  early with a notice (nesting/overlap across projects is legal, §9.9, but
  an exact intra-set duplicate would trip the backend's in-group dedup
  validation);
- **Remove** roots — whole-set replacement through `update_project`;
  removal semantics and the primary-root rule are §9.5;
- **Set as primary folder** — writes the project-remembered primary root
  (§9.2);
- **Rename** the project;
- **View/revoke** the anti-materialization exclusion list (§3).

Backend invariants for root replacement:

- Removed roots are computed as a canonical-form diff
  (`removed_roots`): old roots neither equal to nor nested under any new
  root. The command layer normalizes the payload *before* diffing so a
  no-op edit spelled differently (macOS `/var` vs `/private/var`, symlinked
  home, autofs) is not misjudged as a removal.
- Sessions under removed roots that have **no assignment entry** are
  written as explicit move-outs (`None`) so tier-② grouping or a later
  `ensure` cannot immediately overturn the removal; sessions with existing
  entries (explicitly assigned here or elsewhere, already moved out) are
  untouched — tier-① semantics win (§5).
- Root replacement and member expulsion happen in **one store transaction**
  (`update_project_and_expel`): one lock, one persist, so a failed expel
  cannot leave replaced roots with members that would be silently
  re-adopted on retry.
- Removal is purely logical: it never touches any session's
  working-directory binding or keychain snapshot (§6/§9.5).

## §5 Sidebar grouping and membership resolution

Two independent views (`projectGrouping.js`):

- **Folder view (physical layer):** group by workspace directory —
  byte-identical to the pre-project sidebar; projects never affect it.
- **Project view (logical layer):** named projects as groups plus a
  trailing *ungrouped* bucket (drag source for moving in, and the landing
  place of explicit move-outs). Temporary sessions never auto-join.

Membership of a session is resolved in two tiers, identically on the
frontend (`resolveSessionProjectId`) and the backend
(`ProjectStore::resolve_session_project`):

- **Tier ① explicit assignment.** `assignments[session_id] = Some(pid)`
  wins outright. `None` is an **explicit move-out**: the session lands in
  ungrouped and must not be revived by tier-②. No entry means undecided.
- **Tier ② root auto-match.** The session's workspace path is matched
  against project roots with component-aware "equal to or nested under"
  semantics on folded identity keys (Windows folds case/separators; POSIX
  is case-sensitive and verbatim — the known macOS APFS case-folding
  residue is documented in `store.rs` `root_key`). Multiple hits are
  adopted by the **smallest position** (sidebar order is the
  user-controllable knob; `id` breaks residual ties) — *not* by longest
  root. This is the §9.9 overlap ruling's resolution rule.

## §6 The session keychain snapshot

Every workspace-bound session locks a **keychain snapshot** at creation:
the full set of accessible roots, primary root first. The primary slot is
always the session's own creation-time `cwd` — a snapshot never re-labels
it (§9.2). An **empty snapshot is single-root semantics** (only `cwd`);
this is also the reading for pre-feature records and sidecars missing the
key.

- **Validation** (`validate_workspace_roots`, shared by `create_session`
  and `create_codex_acp_session`): every additional root must be absolute
  (hard reject, same gate as `cwd`); nonexistent/non-directory roots are
  kept with a soft warning — the folder may be moved away and recreated
  later, and the snapshot faithfully records the user's choice at the time.
- **Storage, per session kind:** plain chat sessions persist it in the
  per-session `workspace-binding.json` sidecar; native code sessions in the
  session-agents record *and* the authoritative code-session sidecar; ACP
  sessions in the session-agents index only (temporary sessions always
  store an empty snapshot).
- **Restore** backfills the snapshot so a session restart does not lose
  roots; engine spawn/resume resolves the snapshot into
  `EngineConfig.workspace_roots` (the CodeWhale foundation normalizes
  `cwd`-first, deduped).
- **Rebind migration** (§7): roots under the rebound prefix are translated
  `from → to`; roots outside the prefix are untouched.
- **Replacement** happens only through the explicit "align to project"
  action (§9.7) — permissions only ever grow by user action.
- **Surfacing:** the workspace keychain chip shows the primary directory
  name plus `+N` for additional roots and pops the root list
  (`WorkspaceKeychainChip.jsx`, `describeKeychain`); empty/single-root
  sessions show no `+N`.

## §7 Binding immutability and the rebind repair channel

- A session's working directory and agent are **immutable once the session
  has started**: ACP sessions reject changing agent or workspace after
  `acp_session_id` exists; native code sessions reject rebinding to a
  different workspace (same-value rebinds are idempotently allowed). The
  answer to "wrong folder" is a new session, not a mutation.
- The **rebind** channel (`rebind_workspace_root`) exists solely for
  broken-link repair when a directory moved: project roots under the
  `from` prefix shift to `to`, keychain snapshots migrate with their
  binding (§6), intra-set constraints are revalidated per project with
  rollback on failure, and active-turn fences reject rebinds while a
  prompt/turn/scheduled run is in flight. The frontend two-stages the
  confirmation when the old directory still exists.

## §8 Storage and consistency invariants

- One JSON file, `~/.pinvou3/projects/projects.json`, holds projects, the
  assignment map, and the exclusion list; one tmp+rename atomic write
  covers all three views. The empty state removes the file (no empty
  shell); a corrupt file boots as empty and the next mutation self-heals —
  assignments are pure preference data.
- `schema_version` guards forward compatibility: reading a newer schema
  degrades to the empty state **and sets a write-refusal flag** so a
  downgraded process cannot overwrite the newer structure.
- Roots are stored in canonical display form (`fs::canonicalize`, with an
  ancestor-based fallback for moved-away folders so overlap checks stay
  decidable; Windows `\\?\` verbatim prefixes are stripped). Comparisons
  use folded identity keys (§5).
- Session deletion drops the session's assignment entry
  (`forget_session`); boot reconciliation prunes entries whose sessions no
  longer exist (`retain_sessions`).
- Project events (`projects:list_changed`) are emitted to the desktop
  webview only — never forwarded to the remote-control relay (§9.8).

## §9 Channel rulings

### §9.1 Temporary sessions

A temporary session binds no workspace: its directory is derived from the
session id (session-private), its keychain snapshot is always empty, and it
never auto-joins a project — it enters one only via explicit assignment
(the adopt flow, §9.6). Temporary sessions are excluded from
auto-materialization aggregation. "Temporary session" is an explicit
first-class option in the picker (§2), not an error state.

### §9.2 The project-remembered primary folder

`last_primary_root` is the project's remembered primary folder: the default
`cwd` for the project channel (§9.3).

- Written explicitly: by project-channel session creation (§9.3) and by
  the manage panel's "set as primary folder" (§4). The **browse channel
  never writes it** (ruling A, §9.9) — browsing is a physical choice, not a
  project-memory update.
- Must be a roots member; the store rejects foreign paths.
- Demoted to `None` when a root replacement drops it from roots
  (`demote_stale_primary_root`), so a stale primary folder cannot keep
  serving as the project channel's `cwd`.
- It never re-labels a *session's* primary slot: a session keychain's
  primary is always its own `cwd` ("no door change", §6/§9.7).

### §9.3 The project channel

Starting a session from a project (picker project row, or the group
header's dedicated "new session" entry, §9.9):

- `cwd` = the picked root, defaulting to the project's remembered primary
  folder while it is still a roots member, otherwise the first roots entry
  (`pickerPrimaryRoot`). Tag-only projects offer no channel (no root to
  bind).
- Keychain = a snapshot of **all the project's roots at that moment** (§6).
- Creation writes the project-remembered primary folder (§9.2) **inside the
  same backend command** (`create_session` / `create_codex_acp_session`) —
  atomic, no second RPC; a memory-write failure only logs and never fails
  session creation (the next creation retries).
- Both lanes stage the channel state as draft data
  (`draftWorkspaceRoots`/`draftProjectId`, or the codex lane's draft
  ownership) that travels into the materializing create call.

### §9.4 Mode-aware grant notices (parity at the moment of grant)

Every channel that grants folder access announces it **at the moment of
grant**, with wording that follows the session's permission mode
(`workspaceNoticeTone`):

- **Restricted modes** (Plan / read-only): *grant* semantics — "the session
  will be able to access these N folders".
- **YOLO / full-access**: *visibility* semantics — "the model will know
  these N folders belong to this conversation" (YOLO is not
  workspace-restricted anyway).
- An unknown mode falls back to the restricted wording: the heavier notice
  is the safer default.

Parity rule: channels that grant access **without** the picker's expansion
panel carry the same-weight notice through a different surface — picker
single-root rows and the browse entry show it inline; the no-detour
channels (project-row "new session", composer recents) surface it as a
toast at grant time. The notice copy comes from one shared pure function so
no channel drifts.

### §9.5 Root-removal semantics

Decided by `removeRootPlan` (panel) and enforced by `update_project`
(backend):

- Removing the **primary** root while other roots remain requires picking a
  new primary first — a downgrade hint; the UI never picks for the user.
- Removing the **only** root degrades the project to tag-only (empty
  roots); allowed.
- Non-primary roots are removable directly; duplicate/nonexistent paths are
  no-ops.
- Removing a root never affects in-flight sessions: their keychain
  snapshots (§6) and working-directory bindings are untouched; only the
  logical membership of auto-grouped sessions under the removed roots is
  expelled (§4).

### §9.6 Moving sessions between projects

`move_session_to_project` is a pure logical-layer write and is allowed on
running sessions; the physical binding is never touched.

- Move-in writes tier-① `Some(pid)`; with `add_workspace_root` the
  session's working directory is additionally adopted into the target's
  roots — idempotently skipped when already covered, and when the workspace
  is an *ancestor* of existing roots the covered descendants are absorbed
  (preserving the intra-set no-nesting invariant). The frontend reports
  "added to project (and added folder xx)" and pre-confirms via
  `needsAddFolderConfirm`.
- Move-out writes `None` (explicit move-out, §5): auto-grouping cannot
  revive the session into its original project.
- Temporary sessions have no project folder, so the
  `add_workspace_root` combination is rejected for them (§9.1).

### §9.7 Align to project

Session-level explicit action offered by the keychain chip (§6):
`align_session_to_project` replaces the session's keychain snapshot with
its owning project's **full root set at that moment**.

- Shape: primary slot = the session's own `cwd` (no door change, §9.2);
  additional roots = project roots minus `cwd`, order preserved
  (`keychain_for_workspace`).
- Ownership resolution matches the frontend exactly (§5): explicit
  assignment first, then tier-② with the smallest-position tiebreak.
- Writes both binding stores (agent record + code-session sidecar / plain
  binding sidecar); an in-flight engine is pushed the new set via
  `Op::SyncSession`, effective next turn.
- Fences: an active prompt/turn/scheduled run rejects with typed
  `ALIGN_BUSY`; a session with no bound workspace (temporary/scheduled)
  rejects with `ALIGN_NO_WORKSPACE`. Idempotent: an already-matching
  keychain returns `applied=false, reason=no_change`; no owning project
  returns `reason=no_project`.
- Desktop-only (§9.8).

### §9.8 Web host stance

The web host has exactly **one authorized root directory** (a
host-authorized `workspace_` handle), so the whole multi-root machinery is
absent there:

- The picker renders only the "temporary session" option (`webOnly`).
- No keychain snapshot, no project memory writes: web sessions are
  temporary/single-root only; `align to project` rejects as desktop-only.
- The projects domain is absent from the web bridge, and project events are
  not forwarded through the remote-control relay (§8).

### §9.9 Root-overlap legalization (2026-09-11 ruling) and its consequences

**Ruling:** cross-project root overlap and nesting are **legal**. The same
physical folder (or mutually nested folders) may be referenced by several
projects. Consequences, all locked by tests:

- **Validation.** `validate_roots` still rejects non-absolute paths and
  *intra-set* duplicates/nesting, but never cross-project overlap.
- **Multi-hit membership.** A session covered by several projects is
  adopted by the smallest-position project (§5 tier-②), identically on
  frontend and backend. The old longest-root-wins rule is retired.
- **Anchored coverage.** Auto-materialization reuse is judged by *exact
  anchoring*: only an `origin=folder` project whose roots contain exactly
  this path counts as covering it. A folder merely referenced by another
  project — even as its primary root — still materializes a new same-named
  project; the browse channel is always homed at the chosen folder.
- **Project channel without detour.** The project group header's "new
  session" entry starts directly at the project's remembered primary root
  with the full-root keychain (§9.3), no picker detour — with §9.4 grant
  notice parity via toast.
- **Browse/folder channel.** System folder picker → `ensure` (anchor reuse
  or materialize; the §3 exclusion list skips) → start at the picked folder
  `F` with `cwd = F`, `roots = [F]`, **no `projectId`**, and — ruling A —
  no write to any project's `last_primary_root` (§9.2). The session groups
  via tier-② anchoring and needs no explicit assignment.
- **Delete-project tombstones.** Deleting a project writes **all** members
  (explicitly assigned + enumerated auto-grouped) as explicit move-outs
  (`None`); sessions themselves are never deleted. The tombstone is
  deliberately *global*: if the deleted project A's root is still
  referenced by a surviving project B, the auto members under that root are
  moved out too and are **not** re-adopted by B's tier-② grouping —
  deletion is the user's explicit statement, and automatic revival would
  overturn it; joining B requires an explicit move (§9.6). Sessions created
  later in that folder have no entry and auto-group as usual.
- **Auto-member re-adoption.** Explicit move-outs suppress both tier-②
  grouping and `ensure` materialization, so a recreated folder project only
  picks up *new* (entry-less) sessions. Root removal expels only
  entry-less sessions; tier-① entries always win (§4).

## §10 Verification anchors

The invariants above are pinned by:

- `pinvou3-app/tests/workspace_picker_state.test.mjs` — §2/§3/§9.3/§9.4
  (hot view, cold-project hiding, primary-root resolution, notice tone);
- `pinvou3-app/tests/workspace_picker_wiring.test.mjs` and
  `workspace_entry_batch2_wiring.test.mjs` — §2/§9.4 wiring and
  no-detour-channel notice parity;
- `pinvou3-app/tests/manage_folders_state.test.mjs` — §4/§9.5/§9.9
  (removal decisions, duplicate vs. legal nesting);
- `pinvou3-app/tests/project_grouping_logic.test.mjs` — §5/§9.9
  (tier resolution, anchored coverage, position tiebreak);
- `pinvou3-app/src-tauri/src/features/projects/tests.rs` — §3/§4/§6/§9.2/
  §9.5/§9.7/§9.9 backend invariants (exclusion list, expulsion, remembered
  primary, alignment shape, overlap legalization, delete tombstones).
