#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const read = (...segments) =>
  readFileSync(path.join(here, '..', 'src-tauri', 'src', ...segments), 'utf8');
// Frontend pins read from the JS/JSX source root instead of src-tauri/src.
const readSrc = (...segments) =>
  readFileSync(path.join(here, '..', 'src', ...segments), 'utf8');
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

// --- steer frontend 25s transport timer (r17: the const shipped unpinned;
// only the backend 20s above was held). Ordering contract: the backend
// STEER_CHAT_TIMEOUT (20s) must stay UNDER this frontend 25s transport
// timer so the backend's specific nothing-enqueued diagnosis wins the race
// instead of the frontend's generic "steer_chat timed out" (see the r16-m1
// steer section above). ---
const chatBridge = squeezed(readSrc('platform', 'tauri', 'bridge', 'chat.js'));
assert.match(
  chatBridge,
  /const STEER_INVOKE_TIMEOUT_MS = 25000;/,
  'STEER_INVOKE_TIMEOUT_MS must stay at the shipped 25s transport timer',
);
// The value alone is not the bound: the steer_chat invoke must stay raced
// against the shared constant (the withdraw/cancel siblings consume the
// same constant; renaming the constant away from the steer lane would
// unbound the hanging-chip scenario the timer exists for).
assert.match(
  chatBridge,
  /invoke\("steer_chat", \{ sessionId: sid, content: String\(content \|\| ""\) \}\).{0,400}?reject\(new Error\("steer_chat timed out"\)\).{0,80}?\}, STEER_INVOKE_TIMEOUT_MS\);/,
  'the steer_chat invoke must stay raced against STEER_INVOKE_TIMEOUT_MS',
);
// Enforce the ordering contract itself, not just the two constants: moving
// either side (backend up, frontend down) silently flips which diagnosis
// the user sees, and no value-only pin would notice.
const backendSteerMs =
  Number(
    chat.match(/STEER_CHAT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs\((\d+)\)/)?.[1],
  ) * 1000;
const frontendSteerMs = Number(
  chatBridge.match(/STEER_INVOKE_TIMEOUT_MS = (\d+);/)?.[1],
);
assert.ok(
  backendSteerMs > 0 && backendSteerMs < frontendSteerMs,
  `the backend STEER_CHAT_TIMEOUT (${backendSteerMs}ms) must stay below the frontend STEER_INVOKE_TIMEOUT_MS (${frontendSteerMs}ms) so the specific nothing-enqueued diagnosis wins the race`,
);

// --- GTK RUNNING-ack 60s (r17: the post-start acknowledgement wait shipped
// unpinned. Once the GTK closure has begun it must not be cancelled, so the
// only bound is this wait — and on expiry the outcome must be reported as
// UNKNOWN because the closure may still commit after the caller returns. ---
const linuxSurface = squeezed(
  read('features', 'browser', 'platform', 'linux_surface.rs'),
);
assert.match(
  linuxSurface,
  /const GTK_RUNNING_ACK_TIMEOUT: Duration = Duration::from_secs\(60\);/,
  'GTK_RUNNING_ACK_TIMEOUT must stay at the shipped 60s',
);
assert.match(
  linuxSurface,
  /receiver\.recv_timeout\(GTK_RUNNING_ACK_TIMEOUT\)/,
  'the RUNNING/FINISHED acknowledgement wait must stay bounded by GTK_RUNNING_ACK_TIMEOUT',
);
assert.match(
  linuxSurface,
  /the GTK closure outcome is UNKNOWN/,
  'the expiry error must keep reporting the outcome as UNKNOWN (the closure may still commit after the return)',
);

// --- browser waitFor 12s clamp disclosure (r17: the clamp itself was
// acceptable, but it used to be silent — the clamp_note mechanism that
// discloses the 12s clamp to callers shipped with no pin, and deleting the
// disclosure shipped every suite green. Both consumption sites are held:
// the success "Found text" text and the error string. ---
const browserCore = squeezed(read('features', 'browser', 'core.rs'));
// Construction: the note exists only when the caller asked for more than
// the clamped timeout, and names the clamped values plus the 15s budget.
assert.match(
  browserCore,
  /let clamp_note = match requested_timeout \{ Some\(requested\) if requested > timeout => format!\(/,
  'the waitFor clamp must keep its disclosed clamp_note construction gated on requested > timeout',
);
assert.match(
  browserCore,
  /the requested timeout of \{requested\} ms was clamped to \{timeout\} ms to fit the platform's 15s evaluation budget/,
  'the clamp_note must keep naming the requested value, the clamped value, and the 15s evaluation budget',
);
// Consumption (error side): only the clamped case appends the note.
assert.match(
  browserCore,
  /Err\(error\) if !clamp_note\.is_empty\(\) => \{ return Err\(format!\("\{error\}\{clamp_note\}"\)\); \}/,
  'waitFor failures must keep appending the clamp disclosure when the clamp fired',
);
// Consumption (success side): the "Found text" result carries the note too.
assert.match(
  browserCore,
  /Found text: \{\}\{clamp_note\}/,
  'successful waitFor results must keep appending the clamp disclosure',
);

// --- settings image-capability probe 60s (r17: the probe client timeout
// shipped unpinned; slow thinking-model endpoints get a 4096-token budget
// and a 30s timeout would misreport "can read images" as "cannot verify"). ---
// settings.rs carries other .timeout(...) builders (8s, 2s), so scan the
// run_image_capability_probe region only — a whole-file match could be
// satisfied by the wrong builder.
const settings = squeezed(read('app', 'commands', 'settings.rs'));
const probeStart = settings.indexOf('pub async fn run_image_capability_probe(');
const probeEnd = settings.indexOf('pub async fn test_image_input_capability(', probeStart);
assert.ok(
  probeStart >= 0 && probeEnd > probeStart,
  'the image-capability probe region is missing from settings.rs',
);
assert.match(
  settings.slice(probeStart, probeEnd),
  /reqwest::Client::builder\(\).{0,600}?\.timeout\(std::time::Duration::from_secs\(60\)\) \.build\(\)/,
  'the image-capability probe client must stay built with the 60s timeout',
);

// --- read_private_file_anchored capped read (r17: the anchored lane must
// keep routing through read_capped with the private-state cap; replacing it
// with an unbounded read would re-open the stale/swapped-blob allocation the
// hardened private-read family closed). Scanned over the production region
// only (r16-m2 self-match trap: the file's own test module reuses the cap
// constant, and the pin must not be satisfiable from there). ---
const filesystem = squeezed(read('platform', 'filesystem.rs'));
const filesystemProd = filesystem.slice(0, filesystem.indexOf('#[cfg(test)]'));
assert.ok(filesystemProd.length > 0, 'the filesystem production region is missing');
assert.match(
  filesystemProd,
  /pub\(crate\) fn read_private_file_anchored\(path: &Path\).{0,600}?read_capped\(&mut file, MAX_PRIVATE_DATA_READ_BYTES\)/,
  'read_private_file_anchored must keep its capped read via read_capped(MAX_PRIVATE_DATA_READ_BYTES)',
);

// --- lib.rs log rotation (r17: the rotation strategy and 2MB file cap
// shipped unpinned; a reverted strategy un-bounds the log file again). The
// two builder lines must stay adjacent on the same plugin builder. ---
const libRs = squeezed(read('lib.rs'));
assert.match(
  libRs,
  /\.rotation_strategy\(tauri_plugin_log::RotationStrategy::KeepOne\) \.max_file_size\(2_000_000\)/,
  'the logging plugin must keep KeepOne rotation with the 2,000,000-byte file cap',
);
