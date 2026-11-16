const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const { mergeConfig } = require("../scripts/tauri/effective-config.js");

const appRoot = path.resolve(__dirname, "..");
const tauriRoot = path.join(appRoot, "src-tauri");
const baseConfigPath = path.join(tauriRoot, "tauri.conf.json");
const schemaPath = path.join(appRoot, "node_modules", "@tauri-apps", "cli", "config.schema.json");

// The Tauri CLI validates the merged config against this schema only when a
// real `tauri build` runs, which happens solely in release-packages on main.
// A config key that satisfies the JS merge contract (see
// tauri_effective_config.test.js) can therefore still be schema-invalid and
// first surface as a red release build (PR #883 window: `wix.languages` is
// not a WixConfig property; the key is `language`). This test reproduces the
// CLI's check in the PR gate by validating the base config and every merged
// platform overlay against the schema of the pinned CLI.
if (!fs.existsSync(schemaPath)) {
  console.warn(`[skip] Tauri CLI schema not installed (npm ci first): ${schemaPath}`);
  process.exit(0);
}

const schema = JSON.parse(fs.readFileSync(schemaPath, "utf8"));

// Minimal draft-07 subset validator covering the keywords the Tauri schema
// uses. Unknown validation keywords throw instead of being ignored so a CLI
// schema upgrade cannot silently weaken this gate.
function resolveRef(ref) {
  assert.equal(
    typeof ref,
    "string",
    "schema $ref must be a string",
  );
  assert.ok(
    ref.startsWith("#/"),
    `only local JSON-pointer refs are supported, got: ${ref}`,
  );
  let node = schema;
  for (const segment of ref.slice(2).split("/")) {
    node = node[segment.replaceAll("~1", "/").replaceAll("~0", "~")];
    assert.notEqual(node, undefined, `dangling schema ref: ${ref}`);
  }
  return node;
}

function isObject(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function deepEqual(a, b) {
  if (a === b) return true;
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((item, i) => deepEqual(item, b[i]));
  }
  if (isObject(a) && isObject(b)) {
    const aKeys = Object.keys(a);
    const bKeys = Object.keys(b);
    return (
      aKeys.length === bKeys.length && aKeys.every((key) => deepEqual(a[key], b[key]))
    );
  }
  return false;
}

function checkType(value, type, label, errors) {
  const holds = (() => {
    switch (type) {
      case "object":
        return isObject(value);
      case "array":
        return Array.isArray(value);
      case "string":
        return typeof value === "string";
      case "boolean":
        return typeof value === "boolean";
      case "null":
        return value === null;
      case "number":
        return typeof value === "number" && Number.isFinite(value);
      case "integer":
        return typeof value === "number" && Number.isInteger(value);
      default:
        throw new Error(`unsupported schema type keyword: ${type}`);
    }
  })();
  if (!holds) errors.push(`${label}: expected type ${type}`);
}

const ANNOTATION_KEYWORDS = new Set([
  "$schema",
  "$id",
  "$comment",
  "title",
  "description",
  "default",
  "examples",
  "deprecated",
  // Sub-schema containers reached only through $ref resolution.
  "definitions",
  "$defs",
]);

function validate(value, node, pointer, errors) {
  if (node === undefined) return;
  // Boolean schemas must be decided BEFORE the empty-object shortcut:
  // Object.keys(false) is [], so a `false` schema hit the shortcut and was
  // accepted as unconstrained - and a false branch counted as a match in
  // anyOf/oneOf (review JensenChen28 2026-11-16 P2).
  if (node === false) {
    errors.push(`${pointer}: schema forbids any value`);
    return;
  }
  if (node === true || Object.keys(node).length === 0) return;
  assert.ok(isObject(node), `schema node at ${pointer} must be an object`);

  for (const keyword of Object.keys(node)) {
    assert.ok(
      ANNOTATION_KEYWORDS.has(keyword) ||
        [
          "$ref",
          "type",
          "enum",
          "const",
          "allOf",
          "anyOf",
          "oneOf",
          "properties",
          "additionalProperties",
          "required",
          "items",
          "minItems",
          "maxItems",
          "uniqueItems",
          "minimum",
          "maximum",
          "pattern",
          "format", // draft-07 annotation; the CLI does not enforce it either
        ].includes(keyword),
      `unsupported schema keyword "${keyword}" at ${pointer || "<root>"}: extend the validator`,
    );
  }

  if (node.$ref !== undefined) {
    validate(value, resolveRef(node.$ref), pointer, errors);
    return;
  }
  if (node.type !== undefined) {
    const types = Array.isArray(node.type) ? node.type : [node.type];
    const matched = types.some((type) => {
      const branchErrors = [];
      checkType(value, type, pointer, branchErrors);
      return branchErrors.length === 0;
    });
    if (!matched) errors.push(`${pointer}: expected type ${types.join(" | ")}`);
  }
  if (node.const !== undefined && !deepEqual(value, node.const)) {
    errors.push(`${pointer}: must equal ${JSON.stringify(node.const)}`);
  }
  if (node.enum !== undefined && !node.enum.some((option) => deepEqual(value, option))) {
    errors.push(`${pointer}: must be one of ${JSON.stringify(node.enum)}`);
  }
  if (node.allOf !== undefined) {
    for (const branch of node.allOf) validate(value, branch, pointer, errors);
  }
  if (node.anyOf !== undefined) {
    const ok = node.anyOf.some((branch) => {
      const branchErrors = [];
      validate(value, branch, pointer, branchErrors);
      return branchErrors.length === 0;
    });
    if (!ok) errors.push(`${pointer}: matches no anyOf branch`);
  }
  if (node.oneOf !== undefined) {
    const matches = node.oneOf.filter((branch) => {
      const branchErrors = [];
      validate(value, branch, pointer, branchErrors);
      return branchErrors.length === 0;
    });
    if (matches.length !== 1) {
      errors.push(`${pointer}: matches ${matches.length} oneOf branches, expected 1`);
    }
  }
  if (Array.isArray(value)) {
    if (node.items !== undefined) {
      if (Array.isArray(node.items)) {
        node.items.forEach((branch, index) =>
          validate(value[index], branch, `${pointer}[${index}]`, errors),
        );
      } else {
        value.forEach((item, index) =>
          validate(item, node.items, `${pointer}[${index}]`, errors),
        );
      }
    }
    if (node.minItems !== undefined && value.length < node.minItems) {
      errors.push(`${pointer}: fewer than ${node.minItems} items`);
    }
    if (node.maxItems !== undefined && value.length > node.maxItems) {
      errors.push(`${pointer}: more than ${node.maxItems} items`);
    }
    if (node.uniqueItems === true) {
      for (let i = 0; i < value.length; i += 1) {
        for (let j = i + 1; j < value.length; j += 1) {
          if (deepEqual(value[i], value[j])) {
            errors.push(`${pointer}[${j}]: duplicate item`);
          }
        }
      }
    }
  }
  if (typeof value === "number") {
    if (node.minimum !== undefined && value < node.minimum) {
      errors.push(`${pointer}: below minimum ${node.minimum}`);
    }
    if (node.maximum !== undefined && value > node.maximum) {
      errors.push(`${pointer}: above maximum ${node.maximum}`);
    }
  }
  // No "u" flag: the schema carries PCRE-style identity escapes (e.g. "\:")
  // that JS unicode mode rejects but non-unicode mode accepts.
  if (
    typeof value === "string" &&
    node.pattern !== undefined &&
    !new RegExp(node.pattern).test(value)
  ) {
    errors.push(`${pointer}: does not match pattern ${node.pattern}`);
  }
  if (isObject(value)) {
    for (const key of node.required || []) {
      if (!(key in value)) errors.push(`${pointer}: missing required property "${key}"`);
    }
    const known = Object.keys(node.properties || {});
    for (const [key, child] of Object.entries(value)) {
      const childPointer = pointer ? `${pointer}.${key}` : key;
      if (known.includes(key)) {
        validate(child, node.properties[key], childPointer, errors);
      } else if (node.additionalProperties === false) {
        errors.push(`${childPointer}: unknown property`);
      } else if (isObject(node.additionalProperties)) {
        validate(child, node.additionalProperties, childPointer, errors);
      }
    }
  }
}

function findOverlayPaths() {
  const overlays = [];
  const platformsRoot = path.join(tauriRoot, "config", "platforms");
  const visit = (dir) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) =>
      a.name.localeCompare(b.name),
    )) {
      const entryPath = path.join(dir, entry.name);
      if (entry.isDirectory()) visit(entryPath);
      else if (entry.name === "tauri.conf.json") overlays.push(entryPath);
    }
  };
  visit(platformsRoot);
  assert.ok(overlays.length >= 5, `expected the platform overlays, found: ${overlays.length}`);
  return overlays;
}

const errors = [];
const baseConfig = JSON.parse(fs.readFileSync(baseConfigPath, "utf8"));
validate(baseConfig, schema, "tauri.conf.json", errors);

for (const overlayPath of findOverlayPaths()) {
  const label = path.relative(tauriRoot, overlayPath).replaceAll("\\", "/");
  const overlay = JSON.parse(fs.readFileSync(overlayPath, "utf8"));
  // The CLI validates the effective merged config, not the raw overlay, so
  // mirror the merge here: an overlay key that is schema-invalid for its
  // position must fail exactly the way release-packages failed.
  validate(mergeConfig(baseConfig, overlay), schema, label, errors);
}

assert.deepEqual(
  errors,
  [],
  `tauri config files must satisfy the pinned CLI schema (${schema.$id}):\n${errors.join("\n")}`,
);

// Regression (review JensenChen28 2026-11-16 P2): boolean schema branches.
// Object.keys(false) is [], so a `false` schema used to hit the
// empty-object shortcut and accept everything - and count as a match in
// anyOf/oneOf. `false` must reject, `true` must accept, and combinators
// must count boolean branches correctly.
{
  const booleanErrors = [];
  validate(42, false, "#", booleanErrors);
  assert.ok(
    booleanErrors.length > 0,
    "a false schema must reject any value (Object.keys(false) must not reach the empty-object shortcut)",
  );

  const trueErrors = [];
  validate(42, true, "#", trueErrors);
  assert.deepEqual(trueErrors, [], "a true schema must accept any value");

  const anyOfErrors = [];
  validate(42, { anyOf: [{ type: "string" }, false] }, "#", anyOfErrors);
  assert.deepEqual(
    anyOfErrors,
    ["#: matches no anyOf branch"],
    "a false anyOf branch must not satisfy the combinator",
  );

  const oneOfString = [];
  validate("x", { oneOf: [{ type: "string" }, false] }, "#", oneOfString);
  assert.deepEqual(oneOfString, [], "exactly the real branch matches");

  const oneOfNumber = [];
  validate(42, { oneOf: [{ type: "string" }, false] }, "#", oneOfNumber);
  assert.deepEqual(
    oneOfNumber,
    ["#: matches 0 oneOf branches, expected 1"],
    "a value matching no real branch must not be satisfied by the false branch",
  );
}
