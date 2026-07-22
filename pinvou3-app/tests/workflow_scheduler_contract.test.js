const assert = require("assert");
const fs = require("fs");
const path = require("path");

const appRoot = path.resolve(__dirname, "..");
const harness = fs.readFileSync(
  path.join(appRoot, "src-tauri", "src", "harness.rs"),
  "utf8",
);
const commands = fs.readFileSync(
  path.join(appRoot, "src-tauri", "src", "commands.rs"),
  "utf8",
);

const runCommand = harness.slice(
  harness.indexOf("fn run_cmd_with_timeout"),
  harness.indexOf("fn run_scheduler"),
);
assert.ok(runCommand, "run_cmd_with_timeout implementation must exist");
assert.ok(
  !runCommand.includes("Pinvou3Bridge::boot"),
  "workflow Python subprocesses must not boot the bridge or re-extract the bundle",
);
assert.ok(
  runCommand.includes("crate::monitor::vllm_base_url"),
  "workflow Python subprocesses must resolve model configuration without bridge boot",
);
assert.ok(
  runCommand.includes("if !status.success()"),
  "every non-zero Python exit must be treated as an error even when stdout is non-empty",
);
assert.ok(
  runCommand.includes("subprocess_failure_detail(&stdout, &stderr)"),
  "Python stderr/traceback must be retained in the returned scheduler error",
);

const kickWorkflow = commands.slice(
  commands.indexOf("pub async fn kick_workflow"),
  commands.indexOf("pub async fn retry_workflow_role"),
);
assert.ok(kickWorkflow, "kick_workflow implementation must exist");
assert.ok(
  kickWorkflow.includes("HarnessAction::Error(error)"),
  "kick_workflow must explicitly match scheduler errors",
);
assert.ok(
  kickWorkflow.includes('record_runtime_failure(&ws, "", "scheduler_kick", &error)'),
  "kick_workflow must persist scheduler stderr to flow_log.jsonl",
);
assert.ok(
  kickWorkflow.includes('Err(message)'),
  "kick_workflow must return scheduler errors to the frontend",
);
assert.ok(
  !kickWorkflow.includes('_ => Ok("no dispatch'),
  "kick_workflow must not hide scheduler errors behind the no-dispatch success fallback",
);

console.log("PASS: workflow scheduler error propagation contract");
