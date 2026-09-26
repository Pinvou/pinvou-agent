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
  const section = text.match(
    new RegExp(`^ {12}${name}:\\r?\\n((?: {14}- .*\\r?\\n?)+)`, "m"),
  );
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
  const match = text.match(
    new RegExp(`- name: ${escaped}\\r?\\n[\\s\\S]*?if: \\$\\{\\{ (.*?) \\}\\}`, "m"),
  );
  if (!match) throw new Error(`step '${stepName}' or its if-condition not found`);
  return match[1];
}

// Extract the full YAML block of a top-level trigger key: from `  key:` up to
// the next sibling trigger (2-space indent) or top-level key. Anchoring to
// the subtree keeps the lookup fail-closed: a reformat of the types list can
// never silently match another trigger's list (a lazy cross-key match let a
// block-sequence reformat plus an `edited` regression pass this suite 8/8 —
// review finding on #501).
function triggerBlock(text, key) {
  const start = text.match(new RegExp(`^  ${key}:\\r?\\n`, "m"));
  assert.ok(start, `\`${key}:\` trigger not found`);
  const rest = text.slice(start.index + start[0].length);
  const end = rest.search(/^(?: {2}[^ #\s]|\S)/m);
  return end === -1 ? rest : rest.slice(0, end);
}

// The pull_request.types list in either flow (`types: [a, b]`) or block
// (`types:\n      - a`) form; any other shape must throw, never fall through
// to another trigger's list.
function pullRequestTypes(text) {
  const block = triggerBlock(text, "pull_request");
  const flow = block.match(/^ {4}types: \[(.+)\]\r?$/m);
  if (flow) {
    return flow[1]
      .split(",")
      .map((entry) => entry.trim().replace(/^['"]|['"]$/g, ""));
  }
  const items = [...block.matchAll(/^ {6}- (.+)$/gm)].map((match) =>
    match[1].trim().replace(/^['"]|['"]$/g, ""),
  );
  assert.ok(
    items.length > 0,
    "pull_request.types list not found (flow or block form)",
  );
  return items;
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
  // Quoted keys are legal YAML, and GitHub names the check run after the job
  // id when no explicit name is set — a bare-word-only scan let `"pr-title":`
  // recreate the shadowing (review finding on #501). Strip quotes on both
  // scans.
  const prCheckKeys = [
    ...workflow.matchAll(/^ {2}"?([\w-]+)"?:\n/gm),
  ].map((match) => match[1]);
  const prCheckJobNames = [
    ...workflow.matchAll(/^ {4}name: (.+)$/gm),
  ].map((match) => match[1].trim().replace(/^['"]|['"]$/g, ""));
  for (const names of [prCheckKeys, prCheckJobNames]) {
    assert.ok(
      !names.includes("pr-title"),
      "`pr-title` must stay unique vs pr-check.yml job names",
    );
  }
});

test("title gate revalidates on every subscribed event (no skip condition)", () => {
  // Review finding on #501: a skipped job still creates a check run named
  // `pr-title`, and the checks API surfaces the latest run per name, so a
  // body-only skip would replace the previous red or green result with
  // SKIPPED — and a skipped required check counts as success, clearing a
  // red gate. Every event must therefore run the validator; each run
  // fetches the live title, so the latest result always reflects the
  // current title.
  const jobBlock = titleWorkflow.match(
    /^  pr-title:\r?\n(?:^(?! {2}\S).*\r?\n)*/m,
  );
  assert.ok(jobBlock, "pr-title job block not found");
  assert.doesNotMatch(
    jobBlock[0],
    /^    if:/m,
    "pr-title job must not carry a skip condition (skipped runs shadow the result)",
  );
});

test("all title-gate runs share one cancel-and-replace concurrency group", () => {
  // Every run validates the title it fetched from the API at execution
  // time, so whichever run finishes last holds the freshest verdict; the
  // single PR-keyed group with cancel-and-replace just keeps repeated
  // edits from piling up runs. No edit-kind routing may remain anywhere.
  const concurrency = titleWorkflow.slice(
    titleWorkflow.indexOf("\nconcurrency:"),
    titleWorkflow.indexOf("\njobs:"),
  );
  assert.match(concurrency, /^  cancel-in-progress: true$/m);
  assert.match(
    concurrency,
    /^  group: pr-title-\$\{\{ github\.event\.pull_request\.number \}\}$/m,
    "concurrency group must be the single PR-keyed group",
  );
  assert.doesNotMatch(concurrency, /changes\./, "no edit-kind routing may remain");
  // GitHub lets a job-level `concurrency:` key override the workflow-level
  // group (review finding on #501); ban any indented key so the single
  // PR-keyed cancel-and-replace group cannot be bypassed from inside a job.
  assert.doesNotMatch(
    titleWorkflow,
    /^ {2,}concurrency:/m,
    "a job-level concurrency override would bypass the single PR-keyed group",
  );
});

test("title gate enforces the convention on the live PR title (squash subject)", () => {
  // The squash merge subject is "<PR title> (#N)" and the merge queue never
  // runs commit-message, so the title is validated at PR time. Review
  // finding on #501: GitHub does not guarantee the start order of runs in
  // one concurrency group, so an older `edited` event can run last (and
  // cancel-and-replace a newer run) while carrying a superseded event
  // payload — the event-payload title must therefore never reach the
  // validator. Each run fetches the current title from the API right
  // before validating, through a temp file to avoid injection.
  assert.doesNotMatch(
    titleWorkflow,
    /github\.event\.pull_request\.title/,
    "the event-payload title may be stale; fetch the current title from the API",
  );
  // Bracket notation or toJSON(github.event) would carry the same stale
  // payload past the dot-notation ban above (review finding on #501).
  assert.doesNotMatch(
    titleWorkflow,
    /github\s*\[|toJSON\s*\(/,
    "event-payload access must stay in auditable dot notation",
  );
  assert.match(
    titleWorkflow,
    /^  pull-requests: read$/m,
    "fetching the current PR title requires pull-requests: read",
  );
  // The whole chain must hold: a retry loop around the API fetch, the fetch
  // landing in exactly the file the validator reads (a chain that fetches
  // into /dev/null or validates another file must fail here), and the
  // validator call itself (review finding on #501).
  const chain = titleWorkflow.match(
    /for _ in 1 2 3; do[\s\S]*?gh api "repos\/\$PR_REPO\/pulls\/\$PR_NUMBER" --jq \.title > "\$RUNNER_TEMP\/pr-title"[\s\S]*?python3 scripts\/validate-commit-msg\.py "\$RUNNER_TEMP\/pr-title"/,
  );
  assert.ok(
    chain,
    "title gate must retry-fetch the live title into $RUNNER_TEMP/pr-title and validate exactly that file",
  );
});
