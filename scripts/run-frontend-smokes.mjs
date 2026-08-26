#!/usr/bin/env node

import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { FULL_FRONTEND_SMOKES } from "./select-frontend-smokes.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const appRoot = path.join(repoRoot, "pinvou3-app");

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

function run(item) {
  const { executable, args } = commandFor(item);
  process.stdout.write(`\n== ${item.kind}:${item.target} ==\n`);
  return new Promise((resolve, reject) => {
    const child = spawn(executable, args, {
      cwd: appRoot,
      env: process.env,
      stdio: "inherit",
    });
    child.once("error", reject);
    child.once("exit", (code, signal) => {
      if (signal) {
        reject(new Error(`${item.target} terminated by ${signal}`));
      } else if (code !== 0) {
        reject(new Error(`${item.target} exited with status ${code}`));
      } else {
        resolve();
      }
    });
  });
}

export async function runFrontendSmokes(items = FULL_FRONTEND_SMOKES) {
  for (const item of items) {
    await run(item);
  }
}

async function main() {
  if (process.argv.length !== 3 || process.argv[2] !== "--full") {
    console.error("usage: node scripts/run-frontend-smokes.mjs --full");
    process.exitCode = 2;
    return;
  }
  await runFrontendSmokes();
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
