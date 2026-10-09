#!/usr/bin/env node

import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { FULL_FRONTEND_SMOKES } from "./select-frontend-smokes.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const appRoot = path.join(repoRoot, "pinvou3-app");

// Browser smokes fail on transient hosted-runner distress even when the tree
// is healthy: headless Chrome occasionally stays alive but never prints its
// DevTools endpoint within the 30 s launch window, and a single UI assertion
// can sample a stalled render. One retry of the failing smoke absorbs that
// class of flake, while a real regression still fails both attempts and stops
// the gate. Exit code 2 means a deliberate dependency skip (the smoke prints a
// line-start `SKIP:` marker and exits 2; that pairing is machine-enforced for
// the user-journey smokes by scripts/tests/test_user_journey_skip_contract.py
// and followed by convention elsewhere), so retrying cannot change it and the
// runner keys on the exit code alone.
const DEFAULT_ATTEMPTS = 2;
const RETRY_SETTLE_MS = 5000;

export function commandFor({ kind, target }) {
  if (kind === "npm") {
    if (process.platform === "win32") {
      // Node 24 no longer spawns .cmd shims directly. Invoke npm's JavaScript
      // entry point with the current Node executable so the smoke runner stays
      // shell-free and does not depend on command-line quoting.
      const npmCli = process.env.npm_execpath
        || path.join(path.dirname(process.execPath), "node_modules", "npm", "bin", "npm-cli.js");
      return {
        executable: process.execPath,
        args: [npmCli, "run", target],
      };
    }
    return {
      executable: "npm",
      args: ["run", target],
    };
  }
  if (kind === "node") {
    return { executable: process.execPath, args: [target] };
  }
  throw new Error(`unsupported frontend smoke command kind: ${kind}`);
}

function runOnce(item) {
  const { executable, args } = commandFor(item);
  process.stdout.write(`\n== ${item.kind}:${item.target} ==\n`);
  return new Promise((resolve, reject) => {
    const child = spawn(executable, args, {
      cwd: appRoot,
      env: process.env,
      stdio: "inherit",
    });
    child.once("error", (error) => {
      reject(new Error(`${item.kind}:${item.target} failed to start: ${error.message}`));
    });
    child.once("exit", (code, signal) => {
      if (signal) {
        reject(new Error(`${item.target} terminated by ${signal}`));
      } else if (code !== 0) {
        reject(Object.assign(
          new Error(`${item.target} exited with status ${code}`),
          { exitCode: code },
        ));
      } else {
        resolve();
      }
    });
  });
}

const sleep = (ms) => new Promise((resolve) => {
  setTimeout(resolve, ms);
});

/**
 * Run browser smokes sequentially, retrying each one once when it fails.
 * The first smoke to fail on every attempt stops the run, matching the
 * fail-fast CI loop this replaces; the thrown error names the smoke.
 */
export async function runSelected(items, {
  attempts = DEFAULT_ATTEMPTS,
  settleMs = RETRY_SETTLE_MS,
  run = runOnce,
  warn = (message) => process.stderr.write(`::warning::${message}\n`),
} = {}) {
  for (const item of items) {
    for (let attempt = 1; ; attempt += 1) {
      try {
        await run(item);
        break;
      } catch (error) {
        if (attempt >= attempts || error?.exitCode === 2) throw error;
        warn(
          `${item.kind}:${item.target} failed on attempt ${attempt}/${attempts}`
            + ` (${error.message}); retrying once to absorb a transient runner flake`,
        );
        await sleep(settleMs);
      }
    }
  }
}

/**
 * Parse the `kind<TAB>target` selection emitted by
 * scripts/select-frontend-smokes.mjs. Unknown kinds or malformed lines throw
 * so a drifted producer cannot silently shrink the browser suite.
 */
export function parseSelection(text) {
  const items = [];
  for (const rawLine of text.split(/\r?\n/)) {
    if (rawLine.trim() === "") continue;
    const tab = rawLine.indexOf("\t");
    if (tab <= 0) {
      throw new Error(
        `malformed smoke selection line (expected "<kind>\\t<target>"): ${JSON.stringify(rawLine)}`,
      );
    }
    const kind = rawLine.slice(0, tab).trim();
    const target = rawLine.slice(tab + 1).trim();
    if (!target) {
      throw new Error(`smoke selection line has an empty target: ${JSON.stringify(rawLine)}`);
    }
    if (kind !== "npm" && kind !== "node") {
      throw new Error(`unsupported frontend smoke command kind: ${kind}`);
    }
    items.push({ kind, target });
  }
  return items;
}

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return chunks.join("");
}

async function main() {
  if (process.argv.length === 3 && process.argv[2] === "--full") {
    await runSelected(FULL_FRONTEND_SMOKES);
    return;
  }
  if (process.argv.length === 3 && process.argv[2] === "--selected") {
    const items = parseSelection(await readStdin());
    // selectFrontendSmokes never returns an empty set for any diff, so an
    // empty selection means the upstream selector (or its git diff) failed
    // inside a pipeline whose exit status was masked — fail closed instead of
    // silently skipping every browser smoke.
    if (items.length === 0) {
      console.error("empty frontend smoke selection; the upstream selector failed — failing closed");
      process.exitCode = 1;
      return;
    }
    await runSelected(items);
    return;
  }
  console.error("usage: node scripts/run-frontend-smokes.mjs --full | --selected");
  process.exitCode = 2;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
