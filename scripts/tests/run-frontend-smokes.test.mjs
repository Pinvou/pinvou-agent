import assert from "node:assert/strict";
import test from "node:test";

import { parseSelection, runSelected } from "../run-frontend-smokes.mjs";

const item = (kind, target) => ({ kind, target });
const label = ({ kind, target }) => `${kind}:${target}`;

test("parseSelection reads the selector's kind<TAB>target lines", () => {
  assert.deepEqual(
    parseSelection("npm\ttest:ui-smoke\nnode\ttests/kb_smoke.js\n"),
    [item("npm", "test:ui-smoke"), item("node", "tests/kb_smoke.js")],
  );
});

test("parseSelection tolerates blank lines and CRLF", () => {
  assert.deepEqual(
    parseSelection("\rnpm\ttest:ui-smoke\r\n\r\nnode\ttests/kb_smoke.js\n"),
    [item("npm", "test:ui-smoke"), item("node", "tests/kb_smoke.js")],
  );
});

test("parseSelection rejects malformed lines fail-closed", () => {
  assert.throws(() => parseSelection("npm test:ui-smoke\n"), /malformed smoke selection line/);
  assert.throws(() => parseSelection("shell\trm -rf /\n"), /unsupported frontend smoke command kind: shell/);
  assert.throws(() => parseSelection("npm\t\n"), /empty target/);
  assert.throws(() => parseSelection("npm\t   \n"), /empty target/);
});

test("runSelected runs every passing smoke exactly once in order", async () => {
  const runOrder = [];
  await runSelected(
    [item("npm", "a"), item("node", "b")],
    { run: async (smoke) => { runOrder.push(label(smoke)); } },
  );
  assert.deepEqual(runOrder, ["npm:a", "node:b"]);
});

test("runSelected runs nothing for an empty selection", async () => {
  let ran = false;
  await runSelected([], { run: async () => { ran = true; } });
  assert.equal(ran, false);
});

test("runSelected retries a failed smoke once and then continues", async () => {
  const attempts = [];
  const warnings = [];
  let firstAttempt = true;
  await runSelected(
    [item("npm", "flaky"), item("node", "next")],
    {
      settleMs: 0,
      warn: (message) => warnings.push(message),
      run: async (smoke) => {
        attempts.push(label(smoke));
        if (firstAttempt) {
          firstAttempt = false;
          throw new Error("launch timed out");
        }
      },
    },
  );
  assert.deepEqual(attempts, ["npm:flaky", "npm:flaky", "node:next"]);
  assert.equal(warnings.length, 1);
  assert.match(warnings[0], /npm:flaky failed on attempt 1\/2 \(launch timed out\)/);
});

test("runSelected stops the gate when a smoke fails on both attempts", async () => {
  let attempts = 0;
  await assert.rejects(
    runSelected([item("npm", "broken")], {
      settleMs: 0,
      warn: () => {},
      run: async () => {
        attempts += 1;
        throw new Error(`still failing ${attempts}`);
      },
    }),
    /still failing 2/,
  );
  assert.equal(attempts, 2);
});

test("runSelected never retries the deterministic exit-2 skip", async () => {
  let attempts = 0;
  await assert.rejects(
    runSelected([item("node", "skipped")], {
      settleMs: 0,
      warn: () => {},
      run: async () => {
        attempts += 1;
        throw Object.assign(new Error("skipped"), { exitCode: 2 });
      },
    }),
    /skipped/,
  );
  assert.equal(attempts, 1);
});

test("runSelected retries real failures of any other exit code", async () => {
  let attempts = 0;
  await assert.rejects(
    runSelected([item("node", "broken")], {
      settleMs: 0,
      warn: () => {},
      run: async () => {
        attempts += 1;
        throw Object.assign(new Error("ui assertion failed"), { exitCode: 1 });
      },
    }),
    /ui assertion failed/,
  );
  assert.equal(attempts, 2);
});
