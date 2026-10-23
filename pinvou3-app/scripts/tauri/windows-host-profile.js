// Build-time public inputs only. These objects never construct native host authority.
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const { pathToFileURL } = require("node:url");

const REPOSITORY_ROOT = path.resolve(__dirname, "../../..");
const MAXIMUM_MANIFEST_BYTES = 1024 * 1024;
const MAXIMUM_SEED_BYTES = 64 * 1024;
const HASH = /^[0-9a-f]{64}$/u;
const IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/u;
const PROFILE_ID = /^[a-z0-9][a-z0-9-]{0,47}$/u;
const VERSION = /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/u;
const TARGET = "windows-x86_64-nsis-perMachine";
const LAYOUT = "windows-host-v1";
const matches = (value, pattern) => typeof value === "string" && pattern.test(value);

function refuse(code = "HOST_BUILD_INPUT_INVALID") { throw new Error(code); }
function sha256(bytes) { return crypto.createHash("sha256").update(bytes).digest("hex"); }
function exactObject(value, keys) {
  if (!value || typeof value !== "object" || Array.isArray(value)
      || Object.keys(value).length !== keys.length
      || !keys.every((key) => Object.hasOwn(value, key))) refuse();
}
function reference(value) {
  exactObject(value, ["size", "sha256"]);
  if (!Number.isSafeInteger(value.size) || value.size < 1
      || value.size > MAXIMUM_MANIFEST_BYTES || !matches(value.sha256, HASH)) refuse();
}
function privateSeedLimits(value, depth = 0) {
  if (depth > 16) refuse();
  if (typeof value === "string" && Buffer.byteLength(value) > 4096) refuse();
  if (value && typeof value === "object") {
    if (Object.keys(value).length > 128) refuse();
    for (const [key, item] of Object.entries(value)) {
      if (Buffer.byteLength(key) > 4096) refuse();
      privateSeedLimits(item, depth + 1);
    }
  }
}

let codecPromise;
function codec() {
  codecPromise ??= import(pathToFileURL(path.join(REPOSITORY_ROOT,
    "specs/upgrade/v1/reference/canonical-json.mjs")).href);
  return codecPromise;
}
async function canonicalBytes(value) {
  const { canonicalize } = await codec();
  return Buffer.from(canonicalize(value));
}
async function parseCanonical(bytes, maximum = MAXIMUM_MANIFEST_BYTES) {
  if (!Buffer.isBuffer(bytes) || bytes.length < 1 || bytes.length > maximum) refuse();
  const { parseJson } = await codec();
  let value;
  try { value = parseJson(bytes, maximum); } catch { refuse(); }
  if (!(await canonicalBytes(value)).equals(bytes)) refuse("HOST_BUILD_INPUT_NOT_CANONICAL");
  return value;
}
function readPublicFile(name, maximum) {
  if (typeof name !== "string" || !path.isAbsolute(name)) refuse();
  const before = fs.lstatSync(name);
  if (!before.isFile() || before.isSymbolicLink() || before.nlink !== 1
      || before.size < 1 || before.size > maximum) refuse();
  const fd = fs.openSync(name, "r");
  try {
    const actual = fs.fstatSync(fd);
    if (!actual.isFile() || actual.nlink !== 1 || actual.size !== before.size
        || actual.ino !== before.ino || actual.dev !== before.dev) refuse();
    // Allocate only the fixed metadata budget, even if an already-open writer
    // races a formerly small input. A final stat alone would not bound readFile.
    const bounded = Buffer.alloc(maximum + 1);
    let count = 0;
    for (;;) {
      const read = fs.readSync(fd, bounded, count, bounded.length - count, count);
      count += read;
      if (count > maximum) refuse();
      if (!read) break;
    }
    const bytes = bounded.subarray(0, count);
    const after = fs.fstatSync(fd);
    if (bytes.length !== actual.size || after.size !== actual.size
        || after.mtimeMs !== actual.mtimeMs || after.ctimeMs !== actual.ctimeMs) refuse();
    return bytes;
  } finally { fs.closeSync(fd); }
}

// Public policy data only; this does not issue source/installation authority.
function imageCertificatePolicy(seed) {
  if (seed?.hostSeedVersion === 1 && !Object.hasOwn(seed, "codeImagePolicy")) {
    exactObject(seed.approvedInitialPublisherPolicy, ["policyId", "signerCertificateSha256"]);
    const legacy = seed.approvedInitialPublisherPolicy;
    if (!matches(legacy.policyId, IDENTIFIER) || !matches(legacy.signerCertificateSha256, HASH)) refuse();
    return { kind: "legacy-selected-signer", signerCertificateSha256: legacy.signerCertificateSha256 };
  }
  if (seed?.hostSeedVersion === 2 && !Object.hasOwn(seed, "approvedInitialPublisherPolicy")) {
    exactObject(seed.codeImagePolicy, ["policyVersion", "certificate"]);
    if (seed.codeImagePolicy.policyVersion !== 1 || seed.codeImagePolicy.certificate !== "optional") refuse();
    return { kind: "certificate-optional" };
  }
  refuse();
}

async function validateSeed(seedBytes, rootBytes, { appVersion } = {}) {
  const seed = await parseCanonical(seedBytes, MAXIMUM_SEED_BYTES);
  privateSeedLimits(seed);
  exactObject(seed, ["hostSeedVersion", "profileKind", "profileId", "product", "component",
    "targetKey", "buildId", "appVersion", "layoutId", "hostIpcVersion", "hostStateVersion",
    "originalRoot", ...(seed.hostSeedVersion === 1 ? ["approvedInitialPublisherPolicy"] : ["codeImagePolicy"])]);
  imageCertificatePolicy(seed);
  if (seed.hostIpcVersion !== 1 || seed.hostStateVersion !== 1
      || !["lab", "production"].includes(seed.profileKind) || !matches(seed.profileId, PROFILE_ID)
      || (seed.profileKind === "production" && seed.profileId.includes("-fault-"))
      || !matches(seed.product, IDENTIFIER) || !matches(seed.component, IDENTIFIER)
      || seed.targetKey !== TARGET || seed.layoutId !== LAYOUT || !matches(seed.buildId, HASH)
      || !matches(seed.appVersion, VERSION) || (appVersion && seed.appVersion !== appVersion)) refuse();
  reference(seed.originalRoot);
  if (rootBytes.length !== seed.originalRoot.size || sha256(rootBytes) !== seed.originalRoot.sha256) refuse();
  const root = await parseCanonical(rootBytes);
  // The independently supplied pin is the input. Self-signature validation checks
  // its complete original grammar/cryptographic integrity, never its provenance.
  const { verifyEnvelope } = await import(pathToFileURL(path.join(REPOSITORY_ROOT,
    "specs/upgrade/v1/reference/signatures.mjs")).href);
  try {
    verifyEnvelope(rootBytes, { trustedRoot: root.signed,
      expected: { role: "root", product: seed.product, component: null, scope: {} },
      now: root.signed.issuedAt });
  } catch { refuse("HOST_BUILD_ROOT_INVALID"); }
  return { seed, root, seedBytes: Buffer.from(seedBytes), rootBytes: Buffer.from(rootBytes),
    seedSha256: sha256(seedBytes) };
}

async function loadBuildProfile({ environment = process.env, appVersion } = {}) {
  const seedPath = environment.PINVOU_HOST_BOOTSTRAP_SEED;
  const rootPath = environment.PINVOU_HOST_INITIAL_ROOT;
  const selectedKind = environment.PINVOU_HOST_PROFILE_KIND;
  if (selectedKind !== undefined && !["lab", "production"].includes(selectedKind)) refuse("HOST_BUILD_PROFILE_KIND_INVALID");
  if (seedPath === undefined && rootPath === undefined && selectedKind === undefined) return null;
  if (!seedPath || !rootPath) refuse("HOST_BUILD_INPUT_PAIR_REQUIRED");
  const profile = await validateSeed(readPublicFile(seedPath, MAXIMUM_SEED_BYTES),
    readPublicFile(rootPath, MAXIMUM_MANIFEST_BYTES), { appVersion });
  if (profile.seed.profileKind !== (selectedKind ?? "lab")) refuse("HOST_BUILD_PROFILE_KIND_MISMATCH");
  return profile;
}

// Fixed installation namespaces, shared by the generated template and native
// host policy. Neither a path override nor a buildId selects installation identity.
function hostNamespace(seed) {
  if (!seed || !["lab", "production"].includes(seed.profileKind)
      || !matches(seed.profileId, PROFILE_ID)
      || (seed.profileKind === "production" && seed.profileId.includes("-fault-"))) refuse();
  const production = seed.profileKind === "production";
  return {
    sourceLeaf: production ? "UpdaterBootstrapSource" : "UpdaterBootstrapSourceLab",
    codeLeaf: production ? "Updater" : "UpdaterLab",
    appLeaf: production ? "App" : "AppLab",
    serviceName: `${production ? "PinvouUpgrade" : "PinvouUpgradeLab"}-${seed.profileId}`,
    productNameEN: production ? "Pinvou" : "Pinvou host lab",
    productNameZH: production ? "品悟" : "品悟宿主实验",
    productNameJA: production ? "Pinvou" : "Pinvou ホスト検証",
    displayName: `${production ? "Pinvou" : "Pinvou host lab"} ${seed.profileId}`,
  };
}

function validateHostApplicationConfig(profile, effectiveConfig) {
  // With no override Tauri uses Cargo's selected output. This repository pins
  // both package.default-run and its explicit main [[bin]] to pinvou3-tauri.
  // productName is presentation metadata and does not rename that Cargo output.
  const mainBinaryName = effectiveConfig?.mainBinaryName ?? "pinvou3-tauri";
  if (!effectiveConfig || effectiveConfig.version !== profile.seed.appVersion
      || mainBinaryName !== "pinvou3-tauri") refuse("HOST_BUILD_APPLICATION_IDENTITY_INVALID");
}

module.exports = { HASH, IDENTIFIER, PROFILE_ID, VERSION, TARGET, LAYOUT,
  MAXIMUM_MANIFEST_BYTES, MAXIMUM_SEED_BYTES, REPOSITORY_ROOT, canonicalBytes, codec,
  exactObject, loadBuildProfile, parseCanonical, readPublicFile, reference, refuse, sha256,
  validateSeed, validateHostApplicationConfig, imageCertificatePolicy, hostNamespace };
