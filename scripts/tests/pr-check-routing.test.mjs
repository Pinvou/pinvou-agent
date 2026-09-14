// Routing contract tests for .github/workflows/pr-check.yml.
//
// Guards against gate-routing regressions that paths-filter YAML makes easy:
// a lint step advertised as a hard gate can be silently bypassed when its
// path filter is narrower than the changes it claims to cover.
import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

const workflow = await readFile(
  new URL("../../.github/workflows/pr-check.yml", import.meta.url),
  "utf8",
);

// Extract the path list of a named dorny/paths-filter output.
function filterPaths(name) {
  const section = workflow.match(new RegExp(`^ {12}${name}:\\n((?: {14}- .*\\n?)+)`, "m"));
  if (!section) throw new Error(`paths-filter output '${name}' not found`);
  return section[1]
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.startsWith("- "))
    .map((line) => line.slice(2).trim().replace(/^['"]|['"]$/g, ""));
}

// Extract the `if:` condition of a named workflow step.
function stepCondition(stepName) {
  const escaped = stepName.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = workflow.match(new RegExp(`- name: ${escaped}\\n[\\s\\S]*?if: \\$\\{\\{ (.*?) \\}\\}`, "m"));
  if (!match) throw new Error(`step '${stepName}' or its if-condition not found`);
  return match[1];
}

// Extract the single-line `if:` condition of a named top-level job. Lines in
// between may be any depth except a sibling job key (exactly 2-space indent).
function jobCondition(jobName) {
  const escaped = jobName.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = workflow.match(
    new RegExp(`^  ${escaped}:\\n(?:^(?! {2}\\S).*\\n)*?^    if: \\$\\{\\{ (.*?) \\}\\}$`, "m"),
  );
  if (!match) throw new Error(`job '${jobName}' or its single-line if-condition not found`);
  return match[1];
}

test("rust_code filter covers plain Rust source changes", () => {
  const paths = filterPaths("rust_code");
  assert.ok(paths.includes("**/*.rs"), "rust_code must match '*.rs' files");
});

test("cargo-shear gate routes on rust_code so orphan .rs files cannot bypass it", () => {
  // cargo-shear detects unlinked source files as well as unused dependencies,
  // so gating it only on rust_dependencies lets an orphan .rs file added
  // without Cargo metadata changes skip the gate entirely.
  for (const stepName of ["Install cargo-shear", "cargo shear (hard gate, both workspaces)"]) {
    const condition = stepCondition(stepName);
    assert.match(
      condition,
      /needs\.changes\.outputs\.rust_code == 'true'/,
      `${stepName} must be gated on rust_code (covers *.rs-only changes)`,
    );
  }
});

test("dependency-only gates (cargo-deny) stay on rust_dependencies", () => {
  // cargo-deny inspects the dependency graph only; keep it on the narrower
  // filter so plain .rs changes do not pay the install cost.
  const condition = stepCondition("cargo deny check (hard gate)");
  assert.match(condition, /needs\.changes\.outputs\.rust_dependencies == 'true'/);
  assert.doesNotMatch(condition, /rust_code/);
});

function pullRequestTrigger() {
  const start = workflow.indexOf("\n  pull_request:");
  const end = workflow.indexOf("\n  merge_group:");
  assert.ok(start !== -1 && end !== -1 && start < end, "pull_request trigger block not found");
  return workflow.slice(start, end);
}

test("PR title edits re-trigger the workflow so the title gate cannot go stale", () => {
  // Squash merges build the main-branch commit subject from "<PR title> (#N)"
  // and the merge queue never runs commit-message. A title edited after the
  // last synchronize/labeled event must therefore start a fresh gate run:
  // without `edited` in types the previous green check stays attached to the
  // same head SHA and the title gate is bypassable.
  const types = pullRequestTrigger().match(/^    types: \[(.+)\]$/m);
  assert.ok(types, "pull_request.types list not found");
  const actions = types[1].split(",").map((entry) => entry.trim());
  for (const expected of ["opened", "synchronize", "reopened", "ready_for_review", "labeled", "edited", "closed"]) {
    assert.ok(actions.includes(expected), `pull_request.types must include ${expected}`);
  }
});

test("body-only PR edits rerun nothing (edited is routed to title/base changes)", () => {
  // Re-running the full suite on every description edit would burn CI minutes
  // (pr-check.yml itself sits in several path filters), so only a title or
  // base change re-triggers. Jobs consuming needs.changes outputs
  // (frontend-test, rust-test, ...) cascade off the `changes` job below.
  for (const jobName of ["commit-message", "version-consistency", "changes", "fast-gate", "required-gate"]) {
    const condition = jobCondition(jobName);
    assert.match(
      condition,
      /github\.event\.action != 'edited'/,
      `${jobName} must skip body-only edited events`,
    );
    assert.match(
      condition,
      /github\.event\.changes\.title\.from != ''/,
      `${jobName} must rerun when the title changed`,
    );
    assert.match(
      condition,
      /github\.event\.changes\.base\.ref\.from != ''/,
      `${jobName} must rerun when the base branch changed`,
    );
  }
});

test("edited runs cannot cancel in-flight checks on the same head SHA", () => {
  // A body-only edit produces an all-skipped run; if it could cancel the
  // running full gate, the PR would be left with no valid required check.
  // Title/base edits may cancel-and-replace (the rerun is a superset).
  const concurrency = workflow.slice(
    workflow.indexOf("\nconcurrency:"),
    workflow.indexOf("\njobs:"),
  );
  assert.match(
    concurrency,
    /cancel-in-progress: \$\{\{ github\.event_name == 'pull_request' && \(github\.event\.action != 'edited' \|\| github\.event\.changes\.title\.from != '' \|\| github\.event\.changes\.base\.ref\.from != ''\) \}\}/,
    "cancel-in-progress must exclude body-only edited events",
  );
});

test("commit-message enforces the convention on the PR title (squash subject)", () => {
  // The squash merge subject is "<PR title> (#N)" and the merge queue never
  // runs this job, so the pull_request leg must validate the title itself.
  const step = workflow.match(
    /- name: Validate commit messages\n[\s\S]*?python3 scripts\/validate-commit-msg\.py "\$RUNNER_TEMP\/pr-title"/,
  );
  assert.ok(step, "pull_request leg must validate the PR title file");
  assert.match(step[0], /PR_TITLE: \$\{\{ github\.event\.pull_request\.title \}\}/);
});
