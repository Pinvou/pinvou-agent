# Composer tool/skill toggles: pending-enable semantics, prompt-cache impact, and MCP perception audit

Status: analysis + implemented fix (pending-enable). MCP perception sealing is proposed but not yet implemented.

## 1. Bug: mid-session skill enable was locked in by the "only-add" rule

### Symptom

In the composer tool menu (`ComposerToolMenu`), the "only-add, never-remove"
guard (`hasActiveSession && enabled`) blocked every disable while a session was
active. A user who toggled a skill ON by mistake could not toggle it back OFF
before sending the next round — the switch was locked immediately, even though
the skill had not yet entered the model context.

### Root cause

The backend hot reload (`set_disabled_connectors` → `refresh_live_sessions_skills`)
only rewrites the per-session composite skills directory; the change lands in the
model context with the *next* prompt. So a mid-session enable is semantically
"pending" until the next round is admitted, but the UI treated it as committed
instantly.

### Fix (this branch)

- Toggling ON records the bundle id in a module-level pending set keyed by scope
  (`plain` / `code`), so menu remounts do not lose it. While pending, the row
  stays switchable — a mistaken enable can be reverted, and the revert persists
  and hot-reloads normally.
- The send pipeline dispatches `pinvou:chat-round-committed` once the backend
  admits a new round; the menu then clears the pending set and the rows lock
  under the usual only-add rule.
- Dispatch points: desktop `src/platform/tauri/bridge/chat.js` `doSendFor`
  (covers queued-message flush), web `src/platform/web/bridge.js` `doSendFor`
  and the first-turn submission path, and the native code lane
  (`CodexAcpView.sendNative` → `notifyChatRoundCommitted('code')` from
  `src/features/tools/tool-events.js`). Bridge-layer dispatches are inlined
  because `platform/` must not depend on `features/`.
- Queued messages commit when they actually start executing, not at enqueue
  time — until then the skill genuinely has not entered context, so reverting
  remains correct. Failed sends never commit.
- Project-skills toggle (code scope) follows the same rule.

Verification: `tests/web_access_contract.test.mjs` (guard + dispatch contract)
and the new browser smoke `tests/composer_pending_enable_smoke.js`
(`npm run test:composer-pending-enable-smoke`), which drives the real bridge
against a mocked backend: enable stays reversible before send and locks after a
successful send.

## 2. Prompt-cache impact of mid-session skill changes

Question raised during review: does enabling a skill mid-session inject content
into the prompt head and destroy the prefix cache?

Findings (base engine, CodeWhale):

- The skills list is rendered as a `## Skills` block inside the system prompt's
  stable prefix region (`crates/tui/src/prompts.rs:1182`), after locale
  preamble / constitution / project context, before the Core Execution Profile.
  The base assumes the skills directory is session-static
  (`prompts.rs:1244`: "Skills stay in the constitution prefix
  (skills-dir-static)"); moving it to the volatile tail was considered and
  deliberately rejected.
- The engine has no local KV cache; it relies on provider-side prefix caching
  (Anthropic explicit `cache_control` breakpoints; OpenAI-style implicit prefix
  cache). Prefix caches match on the longest common prefix, so a changed
  `## Skills` block invalidates everything after it: Core Execution, volatile
  sections, the tool catalog, and the entire conversation history — i.e. close
  to a full re-prefill of the next round (plus a cache-write premium on
  Anthropic).
- `refresh_system_prompt()` is hash-guarded (`core/engine.rs:5248`): unchanged
  content does not replace the prompt, so no invalidation happens without an
  actual change.

Cost assessment: one full prefill per change, then subsequent rounds rebuild on
the new prefix — a one-time cost, not a persistent leak. Frequent toggling
would cost more than no caching, but the only-add product rule already keeps
mid-session changes rare.

Interaction with the pending-enable fix: before the fix, a mistaken enable
hot-reloaded the composite directory immediately and the cache rebuild was
unavoidable. With the fix, reverting before the next send restores the
directory, the prompt hash never changes, and no rebuild happens. The one
re-prefill is paid only when the user actually sends a round with the new
skill — which is the inherent cost of using it.

If a systematic optimization is ever wanted (e.g. moving `## Skills` behind the
volatile boundary to shrink the invalidated span), that is base-engine work and
should go to CodeWhale upstream first per the fork policy.

## 3. MCP perception audit: what a disabled connector still leaks

Question: does disabling an MCP connector only block it at the tool-catalog
level, while the model can still learn of its existence through commands?

### Already sealed

- **Tool catalog**: `disallowed_tools` removes tools from the catalog sent to
  the model, not merely denies calls
  (`crates/tui/src/core/engine/tool_catalog.rs:462`). Unknown-tool "Did you
  mean" suggestions use the same filtered catalog.
- **`tool_search`**: its haystack is the post-filter catalog
  (`turn_loop.rs:3230` → `execute_tool_search`), so disabled tools and their
  `mcp_{server}` prefixes are unsearchable.
- **`load_skill`**: lookup is confined to the per-session composite skills
  directory — a host-injected `skills_dir` is an authority boundary and is not
  unioned with workspace/home roots (`crates/tui/src/skills/mod.rs:987`).
  Disabled skills are never materialized there, and the not-found error does
  not confirm existence.
- **System prompt / Environment block**: contains no MCP server information.
- **CLI main path**: execpolicy typed Deny rules hard-block disabled connector
  CLI binaries before spawn (`bridge.rs:1549`), in all permission modes.

### Leaking

- **`list_mcp_resources` family** (`crates/tui/src/mcp.rs:3138-3294`):
  `list_mcp_resources`, `list_mcp_resource_templates`, `read_mcp_resource`,
  `mcp_get_prompt` do not match the `mcp_{server}_*` deny rules and are
  explicitly on pinvou's allow list. Execution `connect_all()`s every
  mcp.json-enabled server — including disabled ones — and returns
  `{"server": <name>, ...}` entries straight into the model context. With an
  explicit `server` argument, `get_or_connect` only checks mcp.json
  `is_enabled()` (`mcp.rs:2953`), which pinvou never writes, so the model can
  even start a disabled server's process and read its resources.
- **Shell side channels** (not part of the three-path scope, recorded for
  completeness): disabling never removes the mcp.json entry, so the disabled
  server's config stays readable at `~/.pinvou3/bundle/mcp.json` and its
  process stays alive and enumerable via `ps`/`tasklist`. execpolicy deny
  rules are word-boundary prefix matches with documented bypasses
  (`bridge.rs:1534-1548`); adding read-path deny rules for the bundle directory
  would only raise the bar (relative paths, copy-then-read, wrappers, process
  enumeration all bypass) — treated as cosmetic, not a fix.

### Proposed sealing (not yet implemented)

Preferred: base-side filtering — when executing the MCP meta tools, filter the
server dimension by the `disallowed_tools` `mcp_{server}_*` rules: skip denied
servers during enumeration and reject an explicit `server` argument that
matches a deny rule. This is a generic upstream-worthy semantic (disallowed
should cover meta tools); per the fork policy it should be contributed to
CodeWhale upstream first, with the fork carrying it in the meantime
(`docs/fork-modifications.md` + fingerprints + behavior tests +
`./scripts/fork-guard.sh --fast`).

Rejected: writing mcp.json `enabled=false` on disable. The base honors that
flag, but mcp.json is global while disable sets are per-scope (plain/code), so
a connector enabled in one scope and disabled in the other would conflict.

## 4. Related discussion notes

- Mid-session disable stays blocked by design: a tool that already entered the
  context cannot be retracted, and hiding it from later rounds while it appears
  in earlier ones is misleading. Disabling is only possible before a session
  starts.
- `start_mcp_server` / `registry_sync` are not on pinvou's allow list and are
  filtered by the allow-list path, so they neither list local servers nor
  start disabled ones.
