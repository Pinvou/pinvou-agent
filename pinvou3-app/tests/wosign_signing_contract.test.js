const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const appRoot = path.resolve(__dirname, "..");
const signScript = fs.readFileSync(
  path.join(appRoot, "src-tauri", "packaging", "windows", "wosign", "sign.ps1"),
  "utf8",
);
const signingConfig = JSON.parse(
  fs.readFileSync(path.join(appRoot, "src-tauri", "config", "tauri.wosign.conf.json"), "utf8"),
);
const packageJson = JSON.parse(fs.readFileSync(path.join(appRoot, "package.json"), "utf8"));
const secretsExample = fs.readFileSync(
  path.join(appRoot, "..", "scripts", ".builtin-secrets.env.example"),
  "utf8",
);

assert.match(signScript, /Join-Path \$PSScriptRoot "wosigncodecmd\.exe"/u);
assert.doesNotMatch(signScript, /PINVOU3_WOSIGN_TOOL_PATH/u);
assert.match(signScript, /scripts\/\.builtin-secrets\.env/u);
assert.match(signScript, /PINVOU3_WOSIGN_THUMBPRINT/u);
assert.match(signScript, /PINVOU3_WOSIGN_PASSWORD/u);
assert.doesNotMatch(signScript, /\$Thumbprint\s*=\s*"[0-9A-Fa-f]{40}"/u);
assert.doesNotMatch(signScript, /\$Password\s*=\s*"[^"]+"/u);
assert.match(signScript, /"\/isf"/u);
assert.match(signScript, /Push-Location -LiteralPath \$toolDirectory/u);
assert.match(signScript, /"\/tr", \$TimestampUrl/u);
assert.doesNotMatch(signScript, /TimeStamperCertificate/u);
assert.doesNotMatch(signScript, /Get-AuthenticodeSignature/u);
assert.doesNotMatch(signScript, /SignerCertificate/u);
assert.doesNotMatch(signScript, /SignatureStatus/u);
assert.match(signScript, /WoSign signing completed/u);
assert.match(secretsExample, /PINVOU3_WOSIGN_THUMBPRINT=/u);
assert.match(secretsExample, /PINVOU3_WOSIGN_PASSWORD=/u);
assert.ok(packageJson.scripts["build:nsis"].includes("--verbose"));
assert.ok(packageJson.scripts["bundle:nsis"].includes("--verbose"));
assert.deepEqual(signingConfig.bundle.windows.signCommand.args.slice(0, 2), [
  "-NoProfile",
  "-NonInteractive",
]);
assert.equal(
  signingConfig.bundle.windows.signCommand.args[
    signingConfig.bundle.windows.signCommand.args.indexOf("-File") + 1
  ],
  "packaging/windows/wosign/sign.ps1",
);

console.log("wosign signing contract: ok");
