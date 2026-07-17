import assert from "node:assert/strict";
import { copyFileSync, existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const source = new URL("../src/features/pet/pet-animation.js", import.meta.url);
const petManifestUrl = new URL("../src/features/pet/pet-manifest.json", import.meta.url);

assert.equal(
  existsSync(source),
  true,
  "pet-animation.js should define the Codex animation contract",
);

const petManifest = JSON.parse(readFileSync(petManifestUrl, "utf8"));
assert.deepEqual(
  Object.fromEntries(petManifest.map((pet) => [pet.id, pet.spriteVersionNumber])),
  { lingling: 1, langlang: 2, "ace-taffy": 1 },
  "the manifest must explicitly distinguish nine-row v1 and eleven-row v2 atlases",
);

const dir = mkdtempSync(join(tmpdir(), "pinvou3-pet-animation-"));
const modulePath = join(dir, "pet-animation.mjs");
copyFileSync(source, modulePath);

try {
  const {
    CODEX_ANIMATIONS,
    CODEX_IDLE_FRAME_DURATIONS_MS,
    buildAnimationSequence,
    buildPreviewSequence,
  } = await import(`${new URL(`file:///${modulePath.replaceAll("\\", "/")}`).href}?t=${Date.now()}`);

  assert.deepEqual(CODEX_ANIMATIONS, {
    idle: { row: 0, frames: 6, frameDurationMs: 140 },
    "running-right": { row: 1, frames: 8, frameDurationMs: 120, lastFrameDurationMs: 220 },
    "running-left": { row: 2, frames: 8, frameDurationMs: 120, lastFrameDurationMs: 220 },
    waving: { row: 3, frames: 4, frameDurationMs: 140, lastFrameDurationMs: 280 },
    jumping: { row: 4, frames: 5, frameDurationMs: 140, lastFrameDurationMs: 280 },
    failed: { row: 5, frames: 8, frameDurationMs: 140, lastFrameDurationMs: 240 },
    waiting: { row: 6, frames: 6, frameDurationMs: 150, lastFrameDurationMs: 260 },
    running: { row: 7, frames: 6, frameDurationMs: 120, lastFrameDurationMs: 220 },
    review: { row: 8, frames: 6, frameDurationMs: 150, lastFrameDurationMs: 280 },
  });
  assert.deepEqual(CODEX_IDLE_FRAME_DURATIONS_MS, [280, 110, 110, 140, 140, 320]);

  const idle = buildAnimationSequence("idle");
  assert.equal(idle.loopStartIndex, 0);
  assert.deepEqual(
    idle.frames.map((frame) => frame.durationMs),
    CODEX_IDLE_FRAME_DURATIONS_MS.map((duration) => duration * 6),
  );

  const running = buildAnimationSequence("running");
  assert.equal(running.loopStartIndex, 18, "action should play three times before idle");
  assert.equal(running.frames.length, 24);
  assert.deepEqual(running.frames.slice(0, 6).map((frame) => frame.column), [0, 1, 2, 3, 4, 5]);
  assert.equal(running.frames[5].durationMs, 220);
  assert.equal(running.frames[18].row, 0, "the looping tail should be the idle row");

  const reduced = buildAnimationSequence("review", { reducedMotion: true });
  assert.deepEqual(reduced, {
    frames: [{ row: 8, column: 0, durationMs: 150 }],
    loopStartIndex: 0,
  });

  // 选择器卡片悬停预览：挥手、跳跃、观察各一遍后进入慢速 idle 循环。
  const preview = buildPreviewSequence();
  assert.equal(preview.loopStartIndex, 15, "preview should finish all three actions before resting");
  assert.deepEqual(
    preview.frames.slice(0, 15).map((frame) => frame.row),
    [3, 3, 3, 3, 4, 4, 4, 4, 4, 8, 8, 8, 8, 8, 8],
    "the preview introduction should combine waving, jumping, and review",
  );
  assert.equal(preview.frames[0].durationMs, 252, "settings previews should use a slower showcase pace");
  assert.equal(preview.frames[3].durationMs, 504, "the last waving frame keeps its scaled long hold");
  assert.equal(preview.frames[8].durationMs, 504, "the last jumping frame keeps its scaled long hold");
  assert.equal(preview.frames[14].durationMs, 504, "the last review frame keeps its scaled long hold");
  assert.equal(
    preview.frames.slice(0, 15).reduce((total, frame) => total + frame.durationMs, 0),
    4626,
    "the three-action showcase should remain visible for about 4.6 seconds",
  );
  assert.equal(preview.frames.length, 15 + CODEX_IDLE_FRAME_DURATIONS_MS.length);
  assert.equal(preview.frames[15].row, 0, "the preview rest loop should be the idle row");

  const previewReduced = buildPreviewSequence({ reducedMotion: true });
  assert.deepEqual(previewReduced, {
    frames: [{ row: 0, column: 0, durationMs: 280 }],
    loopStartIndex: 0,
  }, "reduced motion previews must render a single idle rest frame");

  console.log("pet animation logic tests passed");
} finally {
  rmSync(dir, { recursive: true, force: true });
}
