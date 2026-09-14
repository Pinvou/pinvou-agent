// Routing contract tests for .github/workflows/pr-check.yml and
// .github/workflows/pr-title-check.yml.
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
const titleWorkflow = await readFile(
  new URL("../../.github/workflows/pr-title-check.yml", import.meta.url),
  "utf8",
);

// Extract the path list of a named dorny/paths-filter output.
function filterPaths(text, name) {
  const section = text.match(new RegExp(`^ {12}${name}:\\n((?: {14}- .*\\n?)+)`, "m"));
  if (!section) throw new Error(`paths-filter output '${name}' not found`);
  return section[1]
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.startsWith("- "))
    .map((line) => line.slice(2).trim().replace(/^['"]|['"]$/g, ""));
}

// Extract the `if:` condition of a named workflow step.
function stepCondition(text, stepName) {
  const escaped = stepName.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = text.match(new RegExp(`- name: ${escaped}\\n[\\s\\S]*?if: \\$\\{\\{ (.*?) \\}\\}`, "m"));
  if (!match) throw new Error(`step '${stepName}' or its if-condition not found`);
  return match[1];
}

// Extract the single-line `if:` condition of a named top-level job. Lines in
// between may be any depth except a sibling job key (exactly 2-space indent).
function jobCondition(text, jobName) {
  const escaped = jobName.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = text.match(
    new RegExp(`^  ${escaped}:\\n(?:^(?! {2}\\S).*\\n)*?^    if: \\$\\{\\{ (.*?) \\}\\}$`, "m"),
  );
  if (!match) throw new Error(`job '${jobName}' or its single-line if-condition not found`);
  return match[1];
}

// Extract the pull_request.types list of a workflow.
function pullRequestTypes(text) {
  const match = text.match(/^  pull_request:\n(?:.*\n)*?    types: \[(.+)\]$/m);
  assert.ok(match, "pull_request.types list not found");
  return match[1].split(",").map((entry) => entry.trim());
}

test("rust_code filter covers plain Rust source changes", () => {
  const paths = filterPaths(workflow, "rust_code");
  assert.ok(paths.includes("**/*.rs"), "rust_code must match '*.rs' files");
});

test("cargo-shear gate routes on rust_code so orphan .rs files cannot bypass it", () => {
  // cargo-shear detects unlinked source files as well as unused dependencies,
  // so gating it only on rust_dependencies lets an orphan .rs file added
  // without Cargo metadata changes skip the gate entirely.
  for (const stepName of ["Install cargo-shear", "cargo shear (hard gate, both workspaces)"]) {
    const condition = stepCondition(workflow, stepName);
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
  const condition = stepCondition(workflow, "cargo deny check (hard gate)");
  assert.match(condition, /needs\.changes\.outputs\.rust_dependencies == 'true'/);
  assert.doesNotMatch(condition, /rust_code/);
});

test("the required workflow ignores PR edits; the title gate owns `edited`", () => {
  // Review finding on #501: with `edited` in pr-check.yml's types, a
  // body-only edit still creates skipped check runs named after the required
  // contexts (commit-message, required-gate, ...), and the checks API/UI
  // surfaces the latest check run per name — the skipped runs replaced the
  // green gate results (run 34818389619). Title validation therefore lives
  // in the dedicated pr-title-check.yml, and the required workflow must
  // never receive `edited` again.
  const prCheckActions = pullRequestTypes(workflow);
  assert.ok(
    !prCheckActions.includes("edited"),
    "pr-check.yml must not subscribe to edited (skipped runs shadow required contexts)",
  );
  const titleActions = pullRequestTypes(titleWorkflow);
  for (const expected of ["opened", "synchronize", "reopened", "edited"]) {
    assert.ok(titleActions.includes(expected), `pr-title-check.yml types must include ${expected}`);
  }
});

test("the pr-title context name cannot collide with pr-check.yml job names", () => {
  // The dedicated workflow fixes the shadowing only while its context stays
  // unique; pin the job name and guard it against later renames on either
  // side (job ids and explicit job names at 2-/4-space indent).
  assert.match(titleWorkflow, /^    name: pr-title$/m, "title gate context must stay `pr-title`");
  const prCheckKeys = [
    ...workflow.matchAll(/^ {2}([\w-]+):\n/gm),
  ].map((match) => match[1]);
  const prCheckJobNames = [
    ...workflow.matchAll(/^ {4}name: (.+)$/gm),
  ].map((match) => match[1].trim());
  for (const names of [prCheckKeys, prCheckJobNames]) {
    assert.ok(
      !names.includes("pr-title"),
      "`pr-title` must stay unique vs pr-check.yml job names",
    );
  }
});

test("title gate reruns only on title or base changes", () => {
  // A body-only edit leaves the previous result valid for an unchanged
  // title, so the job skips; a title or base change must re-validate.
  const condition = jobCondition(titleWorkflow, "pr-title");
  assert.match(condition, /github\.event\.action != 'edited'/);
  assert.match(condition, /github\.event\.changes\.title\.from != ''/);
  assert.match(condition, /github\.event\.changes\.base\.ref\.from != ''/);
});

test("body-only edits neither cancel nor queue behind a title validation", () => {
  // An all-skipped no-op run must not cancel an in-flight validation (that
  // would leave the title unchecked) nor occupy its concurrency slot, so
  // the group suffix splits no-op from rerun kinds; same-kind runs
  // cancel-and-replace so repeated edits do not pile up.
  const concurrency = titleWorkflow.slice(
    titleWorkflow.indexOf("\nconcurrency:"),
    titleWorkflow.indexOf("\njobs:"),
  );
  assert.match(concurrency, /^  cancel-in-progress: true$/m);
  assert.match(
    concurrency,
    /group: pr-title-\$\{\{ github\.event\.pull_request\.number \}\}-\$\{\{ github\.event\.action == 'edited' && !\(github\.event\.changes\.title\.from != '' \|\| github\.event\.changes\.base\.ref\.from != ''\) && 'noop' \|\| 'rerun' \}\}/,
    "concurrency group must split no-op from rerun kinds",
  );
});

test("title gate enforces the convention on the PR title (squash subject)", () => {
  // The squash merge subject is "<PR title> (#N)" and the merge queue never
  // runs commit-message, so the title is validated at PR time. It reaches
  // the validator through a temp file to avoid injection.
  const step = titleWorkflow.match(
    /PR_TITLE: \$\{\{ github\.event\.pull_request\.title \}\}[\s\S]*?python3 scripts\/validate-commit-msg\.py "\$RUNNER_TEMP\/pr-title"/,
  );
  assert.ok(step, "title gate must validate the PR title via validate-commit-msg.py");
});
