// Optional host packaging orchestration, called only by the existing project wrapper.
const fs = require("node:fs");
const path = require("node:path");
const { loadBuildProfile, refuse, imageCertificatePolicy } = require("./windows-host-profile.js");
const { packageHostNSIS, nsisFileVersion } = require("./windows-host-nsis.js");

const HOST_FEATURE = "upgrade-host-lab";
const PRODUCTION_HOST_FEATURE = "upgrade-host-production";
const TRIPLE = "x86_64-pc-windows-msvc";
function requiredFile(environment, key, suffix) {
  const value = environment[key];
  if (typeof value !== "string" || !path.isAbsolute(value)
      || (suffix && !value.toLowerCase().endsWith(suffix))
      || !fs.statSync(value, { throwIfNoEntry: false })?.isFile()) refuse("HOST_BUILD_TOOL_REQUIRED");
  return value;
}
async function prepareHostBuild(args, { environment = process.env, platform = process.platform,
  architecture = process.arch, appRoot } = {}) {
  const appVersion = JSON.parse(fs.readFileSync(path.join(appRoot, "src-tauri/tauri.conf.json"), "utf8")).version;
  const profile = await loadBuildProfile({ environment, appVersion });
  if (!profile) return null;
  nsisFileVersion(profile.seed.appVersion); // Before the native application build.
  if (platform !== "win32" || architecture !== "x64" || args[0] !== "build") refuse("HOST_BUILD_PLATFORM_UNSUPPORTED");
  const rewritten = [];
  let bundles = 0;
  let target = null;
  let debug = false;
  const features = [];
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (["--", "--profile", "--runner", "--no-bundle", "--no-default-features"].includes(arg)
        || arg.startsWith("--profile=") || arg.startsWith("--runner=")) refuse("HOST_BUILD_ARGUMENT_UNSUPPORTED");
    if (["--bundles", "-b"].includes(arg) || arg.startsWith("--bundles=")) {
      const value = arg.startsWith("--bundles=") ? arg.slice(10) : args[++i];
      if (++bundles !== 1 || value !== "nsis") refuse("HOST_BUILD_ARGUMENT_UNSUPPORTED");
    } else if (arg === "--target" || arg.startsWith("--target=")) {
      const value = arg.startsWith("--target=") ? arg.slice(9) : args[++i];
      if (target || value !== TRIPLE) refuse("HOST_BUILD_ARGUMENT_UNSUPPORTED");
      target = value;
    } else if (arg === "--features" || arg === "-f" || arg.startsWith("--features=")) {
      const value = arg.startsWith("--features=") ? arg.slice(11) : args[++i];
      if (typeof value !== "string" || !/^[A-Za-z0-9_, /-]+$/u.test(value)) refuse();
      features.push(...value.split(/[ ,]+/u).filter(Boolean));
    } else {
      if (arg === "--debug" || arg === "-d") debug = true;
      rewritten.push(arg);
    }
  }
  if (bundles !== 1) refuse("HOST_BUILD_ARGUMENT_UNSUPPORTED");
  const production = profile.seed.profileKind === "production";
  const hostFeature = production ? PRODUCTION_HOST_FEATURE : HOST_FEATURE;
  const otherHostFeature = production ? HOST_FEATURE : PRODUCTION_HOST_FEATURE;
  if (features.includes(otherHostFeature)
      || (production && features.some((feature) => feature.split("/").at(-1) === "native-acceptance-hooks"))) {
    refuse("HOST_BUILD_ARGUMENT_UNSUPPORTED");
  }
  rewritten.push("--target", TRIPLE, "--features", [...new Set([...features, hostFeature])].join(","), "--no-bundle");
  const targetRoot = path.resolve(appRoot, "src-tauri", environment.CARGO_TARGET_DIR || "target");
  const certificatePolicy = imageCertificatePolicy(profile.seed);
  const signer = certificatePolicy.kind === "legacy-selected-signer"
    || environment.PINVOU_HOST_SIGNER_SCRIPT !== undefined
    ? requiredFile(environment, "PINVOU_HOST_SIGNER_SCRIPT", ".ps1") : null;
  return { profile, args: rewritten, targetRoot,
    appBinary: path.join(targetRoot, TRIPLE, debug ? "debug" : "release", "pinvou3-tauri.exe"),
    helper: requiredFile(environment, "PINVOU_HOST_HELPER_BINARY", ".exe"),
    launcher: requiredFile(environment, "PINVOU_HOST_LAUNCHER_BINARY", ".exe"),
    signer,
    makensis: requiredFile(environment, "PINVOU_HOST_MAKENSIS", ".exe") };
}
async function finishHostBuild(plan, artifacts, { appRoot, environment = process.env } = {}) {
  if (typeof plan.targetRoot !== "string" || !path.isAbsolute(plan.targetRoot)) refuse();
  const outputParent = path.join(plan.targetRoot, "windows-host-packaging");
  fs.mkdirSync(outputParent, { recursive: true });
  const result = await packageHostNSIS({ ...plan, resourceManifest: artifacts.resourceManifest,
    effectiveConfig: artifacts.effectiveConfig, tauriRoot: path.join(appRoot, "src-tauri"),
    outputParent, environment });
  // Hashes/profile are public. Detailed tool output and actual source paths stay
  // in the private build directory; the wrapper prints no signing configuration.
  console.log(`[build] Host ${plan.profile.seed.profileKind} installer completed: profile=${plan.profile.seed.profileId}`);
  return result;
}
module.exports = { HOST_FEATURE, PRODUCTION_HOST_FEATURE, TRIPLE, prepareHostBuild, finishHostBuild };
