#!/usr/bin/env node

// Runs the web-ui smoke against remote-control-relay. By default it builds
// the web dist first, so a local `npm run test:webui` works from a clean
// tree. CI's frontend-test builds the same dist unconditionally before
// selecting smokes and sets PINVOU_WEBUI_SKIP_BUILD=1 to reuse it instead of
// paying a second identical vite build inside the smoke.

import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function npmRun(target) {
  if (process.platform === "win32") {
    // Same Node 24 .cmd-shim avoidance as scripts/run-frontend-smokes.mjs.
    const npmCli = process.env.npm_execpath
      || path.join(path.dirname(process.execPath), "node_modules", "npm", "bin", "npm-cli.js");
    return spawnSync(process.execPath, [npmCli, "run", target], { cwd: appRoot, stdio: "inherit" });
  }
  return spawnSync("npm", ["run", target], { cwd: appRoot, stdio: "inherit" });
}

if (process.env.PINVOU_WEBUI_SKIP_BUILD !== "1") {
  const build = npmRun("build:web");
  if (build.status !== 0) {
    process.exit(build.status ?? 1);
  }
}

const smoke = spawnSync(process.execPath, ["../remote-control-relay/test/web-ui.smoke.cjs"], {
  cwd: appRoot,
  stdio: "inherit",
});
process.exit(smoke.status ?? 1);
