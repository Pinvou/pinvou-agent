#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const read = (...segments) =>
  readFileSync(path.join(here, '..', 'src-tauri', 'src', ...segments), 'utf8');
// rustfmt can reflow any of these constructions; the pins match on the
// whitespace-squeezed text so a re-layout cannot silence them.
const squeezed = (source) => source.replace(/\s+/g, ' ');

// --- steer 20s (r16-m1: the r9 wrap sweep pinned the ACP sites but missed
// this sibling; the backend 20s must stay UNDER the frontend's 25s transport
// timer so the specific nothing-enqueued diagnosis wins the race). ---
const chat = squeezed(read('app', 'commands', 'chat.rs'));
assert.match(
  chat,
  /const STEER_CHAT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs\(20\);/,
  'STEER_CHAT_TIMEOUT must stay at the shipped 20s',
);
// The value alone is not the bound: the wrap must sit on the real steer call.
assert.match(
  chat,
  /tokio::time::timeout\(STEER_CHAT_TIMEOUT, pool\.steer\(&sid, content\)\)/,
  'the steer call must stay wrapped in the STEER_CHAT_TIMEOUT timeout',
);

// --- rebind baseline drain 60s (r16-m1: deadline shipped unpinned; the
// gate-hold is what stops a contended pool from stalling a rebind). ---
const projects = squeezed(read('app', 'commands', 'projects.rs'));
assert.match(
  projects,
  /const REBIND_BASELINE_DRAIN_DEADLINE: Duration = Duration::from_secs\(60\);/,
  'REBIND_BASELINE_DRAIN_DEADLINE must stay at the shipped 60s',
);
// The deadline must stay wired into the drain gate, and the abandonment log
// must keep deriving its seconds figure from the constant (a reworded log
// that hard-codes 60 while the const moves would lie).
assert.match(
  projects,
  /let drain_deadline = tokio::time::Instant::now\(\) \+ REBIND_BASELINE_DRAIN_DEADLINE;/,
  'the drain deadline must stay wired into the rebind baseline gate',
);
assert.match(
  projects,
  /REBIND_BASELINE_DRAIN_DEADLINE\.as_secs\(\)/,
  'the abandonment log must keep deriving its figure from the constant',
);

// --- dbus-send 5s (r16-m1: inline literal shipped unpinned; a stuck
// dbus-daemon must not block the command thread indefinitely — the fall-
// back is xdg-open on the parent directory). ---
const linuxSystem = squeezed(read('platform', 'os', 'linux', 'linux_system.rs'));
assert.match(
  linuxSystem,
  /const DBUS_SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs\(5\);/,
  'DBUS_SEND_TIMEOUT must stay at the shipped 5s',
);
assert.match(
  linuxSystem,
  /output_with_timeout\(dbus_cmd, DBUS_SEND_TIMEOUT\)/,
  'the dbus-send call must stay bounded by DBUS_SEND_TIMEOUT',
);
