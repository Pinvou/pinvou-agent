const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const {
  LINUX_DEB_FIXED_FILE_MODES,
  appendFinalConfigSpecs,
  assertLinuxDebFixedFilesOverlay,
  cleanupLinuxDebFixedFilesStage,
  setLinuxDebBuildUmask,
  snapshotLinuxDebArtifacts,
  stageLinuxDebFixedFiles,
  verifyLinuxDebArchitecture,
  verifyLinuxDebFixedFiles,
} = require("../scripts/tauri/build.js");

const testRoot = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-linux-deb-fixed-files-"));

function modeOf(file) {
  return fs.lstatSync(file).mode & 0o7777;
}

function symbolicMode(mode) {
  if (mode === 0o755) return "-rwxr-xr-x";
  if (mode === 0o644) return "-rw-r--r--";
  throw new Error(`unexpected test mode ${mode.toString(8)}`);
}

function contentsListing(expectedFiles, overrides = {}) {
  return `${Object.entries(expectedFiles)
    .map(([destination, expected]) => {
      const override = overrides[destination] || {};
      return `${override.symbolicMode || symbolicMode(expected.mode)} ${override.owner || "root/root"} ${expected.size} 2026-08-21 00:00 .${destination}`;
    })
    .join("\n")}\n`;
}

function fakeDpkgSpawn({ expectedFiles, bytesByDestination, listing, extractedOverrides = {} }) {
  return (_command, args) => {
    if (args[0] === "--contents") {
      return { status: 0, stdout: listing, stderr: "" };
    }
    if (args[0] === "--extract") {
      const extractionRoot = args[2];
      for (const [destination, expected] of Object.entries(expectedFiles)) {
        const target = path.join(extractionRoot, ...destination.slice(1).split("/"));
        const override = extractedOverrides[destination] || {};
        fs.mkdirSync(path.dirname(target), { recursive: true });
        if (override.hardlinkDestination) {
          const linked = path.join(
            extractionRoot,
            ...override.hardlinkDestination.slice(1).split("/"),
          );
          fs.linkSync(linked, target);
        } else if (override.symlink) {
          fs.symlinkSync(override.symlink, target);
        } else {
          fs.writeFileSync(target, override.bytes || bytesByDestination[destination]);
          fs.chmodSync(target, override.mode || expected.mode);
        }
      }
      return { status: 0, stdout: "", stderr: "" };
    }
    throw new Error(`unexpected fake dpkg-deb operation: ${args.join(" ")}`);
  };
}

try {
  assert.deepEqual(LINUX_DEB_FIXED_FILE_MODES, {
    "/usr/lib/pinvou3/supervisor/pinvou-supervisor": 0o755,
    "/usr/lib/pinvou3/supervisor/pinvou-megabook-profile": 0o755,
    "/usr/lib/systemd/user/pinvou3-supervisor.socket": 0o644,
    "/usr/lib/systemd/user/pinvou3-supervisor.service": 0o644,
    "/usr/lib/systemd/user/pinvou3-app.service": 0o644,
    "/usr/lib/systemd/user/pinvou-qwen3-asr.service.d/50-pinvou-supervisor.conf": 0o644,
    "/usr/share/pinvou3/supervisor/descriptors/pinvou-app-v1.json": 0o644,
    "/usr/share/pinvou3/supervisor/descriptors/pinvou-asr-v1.json": 0o644,
    "/usr/share/pinvou3/supervisor/profiles/megabook-canary.conf": 0o644,
    "/usr/share/pinvou3/supervisor/profiles/pinvou3-megabook-canary.desktop": 0o644,
  });

  const tauriRoot = path.join(testRoot, "src-tauri");
  const configPath = path.join(tauriRoot, "config", "platforms", "linux", "tauri.conf.json");
  const helperSource = path.join(tauriRoot, "packaging", "helper.sh");
  const profileSource = path.join(tauriRoot, "packaging", "profile.conf");
  const helperDestination = "/usr/lib/pinvou3/supervisor/pinvou-megabook-profile";
  const profileDestination = "/usr/share/pinvou3/supervisor/profiles/megabook-canary.conf";
  const fixedFileModes = {
    [helperDestination]: 0o755,
    [profileDestination]: 0o644,
  };
  const configuredFiles = {
    [helperDestination]: "packaging/helper.sh",
    [profileDestination]: "packaging/profile.conf",
  };
  fs.mkdirSync(path.dirname(configPath), { recursive: true });
  fs.mkdirSync(path.dirname(helperSource), { recursive: true });
  fs.writeFileSync(helperSource, "#!/bin/sh\nexit 0\n");
  fs.writeFileSync(profileSource, "[Service]\nMemoryMax=8G\n");
  fs.chmodSync(helperSource, 0o775);
  fs.chmodSync(profileSource, 0o664);
  fs.linkSync(helperSource, path.join(tauriRoot, "packaging", "helper.cargo-hardlink"));
  assert.equal(fs.lstatSync(helperSource).nlink, 2, "Cargo-style hardlinked source is supported");
  fs.writeFileSync(
    configPath,
    `${JSON.stringify({ bundle: { linux: { deb: { files: configuredFiles } } } }, null, 2)}\n`,
  );

  const originalUmask = process.umask(0o002);
  let staged;
  try {
    const previous = setLinuxDebBuildUmask();
    assert.equal(previous, 0o002);
    assert.equal(process.umask(), 0o022);
    staged = stageLinuxDebFixedFiles({ tauriRoot, configPath, fixedFileModes });
  } finally {
    process.umask(originalUmask);
  }

  const stagedHelper = path.join(tauriRoot, staged.overlayFiles[helperDestination]);
  const stagedProfile = path.join(tauriRoot, staged.overlayFiles[profileDestination]);
  assert.equal(modeOf(stagedHelper), 0o755);
  assert.equal(modeOf(stagedProfile), 0o644);
  assert.equal(fs.lstatSync(stagedHelper).nlink, 1);
  assert.equal(fs.lstatSync(stagedProfile).nlink, 1);
  assert.deepEqual(fs.readFileSync(stagedHelper), fs.readFileSync(helperSource));
  assert.deepEqual(fs.readFileSync(stagedProfile), fs.readFileSync(profileSource));
  assert.equal(modeOf(helperSource), 0o775, "staging must not chmod the source helper");
  assert.equal(modeOf(profileSource), 0o664, "staging must not chmod the source profile");
  assert.equal(modeOf(staged.overlayPath), 0o644);
  assert.deepEqual(
    JSON.parse(fs.readFileSync(staged.overlayPath, "utf8")).bundle.linux.deb.files,
    staged.overlayFiles,
  );

  const appended = appendFinalConfigSpecs(
    ["build", "--config", "/tmp/caller.json", "--", "--profile", "release-fast"],
    [staged.overlayPath],
  );
  assert.ok(appended.indexOf(staged.overlayPath) > appended.indexOf("/tmp/caller.json"));
  assert.ok(appended.indexOf(staged.overlayPath) < appended.indexOf("--"));
  assertLinuxDebFixedFilesOverlay(
    { bundle: { linux: { deb: { files: staged.overlayFiles } } } },
    staged.overlayFiles,
  );
  assert.throws(
    () => assertLinuxDebFixedFilesOverlay(
      { bundle: { linux: { deb: { files: { ...staged.overlayFiles, "/tmp/extra": "bad" } } } } },
      staged.overlayFiles,
    ),
    /allowlist mismatch/,
  );

  const unsafeTauriRoot = path.join(testRoot, "unsafe-src-tauri");
  const outsideTarget = path.join(testRoot, "outside-target");
  const outsideStaging = path.join(outsideTarget, "tauri-config", "linux", "deb-files");
  const outsideSentinel = path.join(outsideStaging, "do-not-delete");
  fs.mkdirSync(unsafeTauriRoot);
  fs.mkdirSync(outsideStaging, { recursive: true });
  fs.writeFileSync(outsideSentinel, "owned outside staging\n");
  fs.symlinkSync(outsideTarget, path.join(unsafeTauriRoot, "target"), "dir");
  assert.throws(
    () => stageLinuxDebFixedFiles({ tauriRoot: unsafeTauriRoot, configPath, fixedFileModes }),
    /staging ancestor must be a real directory/,
  );
  assert.equal(fs.readFileSync(outsideSentinel, "utf8"), "owned outside staging\n");
  assert.equal(fs.lstatSync(path.join(unsafeTauriRoot, "target")).isSymbolicLink(), true);

  const unsafeStagingRoot = path.join(testRoot, "unsafe-staging-src-tauri");
  const unsafeStagingParent = path.join(
    unsafeStagingRoot,
    "target",
    "tauri-config",
    "linux",
  );
  fs.mkdirSync(unsafeStagingParent, { recursive: true });
  fs.symlinkSync(outsideStaging, path.join(unsafeStagingParent, "deb-files"), "dir");
  assert.throws(
    () => stageLinuxDebFixedFiles({ tauriRoot: unsafeStagingRoot, configPath, fixedFileModes }),
    /staging ancestor must be a real directory/,
  );
  assert.equal(fs.readFileSync(outsideSentinel, "utf8"), "owned outside staging\n");

  const realProfile = path.join(tauriRoot, "packaging", "profile.real.conf");
  fs.renameSync(profileSource, realProfile);
  fs.symlinkSync(realProfile, profileSource);
  assert.throws(
    () => stageLinuxDebFixedFiles({ tauriRoot, configPath, fixedFileModes }),
    /regular non-symlink file/,
  );
  assert.equal(
    fs.existsSync(staged.stagingRoot),
    false,
    "failed staging must remove the generated tree",
  );
  fs.unlinkSync(profileSource);
  fs.renameSync(realProfile, profileSource);

  staged = stageLinuxDebFixedFiles({ tauriRoot, configPath, fixedFileModes });
  const expectedFiles = staged.expectedFiles;
  const bytesByDestination = {
    [helperDestination]: fs.readFileSync(helperSource),
    [profileDestination]: fs.readFileSync(profileSource),
  };
  const artifact = path.join(testRoot, "pinvou3_0.0.0_amd64.deb");
  fs.writeFileSync(artifact, "fake-deb-for-contract-test\n");

  const staleTarget = path.join(testRoot, "stale-target");
  const staleDebDirectory = path.join(staleTarget, "release", "bundle", "deb");
  const staleArtifact = path.join(staleDebDirectory, "pinvou3_0.0.0_amd64.deb");
  fs.mkdirSync(staleDebDirectory, { recursive: true });
  fs.writeFileSync(staleArtifact, "unchanged old package\n");
  const beforeArtifacts = snapshotLinuxDebArtifacts({
    platform: "linux",
    targetDirectory: staleTarget,
  });
  let architectureSpawned = false;
  const architectureExists = (file) => file === "/usr/bin/dpkg-deb" || fs.existsSync(file);
  assert.throws(
    () => verifyLinuxDebArchitecture({
      architecture: "x64",
      beforeArtifacts,
      exists: architectureExists,
      platform: "linux",
      spawn: () => {
        architectureSpawned = true;
        return { status: 0, stdout: "amd64\n", stderr: "" };
      },
      targetDirectory: staleTarget,
    }),
    /no new or updated deb artifact/,
  );
  assert.equal(architectureSpawned, false, "a stale artifact must be rejected before inspection");
  fs.appendFileSync(staleArtifact, "new build bytes\n");
  assert.equal(
    verifyLinuxDebArchitecture({
      architecture: "x64",
      beforeArtifacts,
      exists: architectureExists,
      platform: "linux",
      spawn: () => ({ status: 0, stdout: "amd64\n", stderr: "" }),
      targetDirectory: staleTarget,
    }),
    staleArtifact,
  );

  const verificationTemp = path.join(testRoot, "verification-tmp");
  fs.mkdirSync(verificationTemp, { mode: 0o700 });
  const exists = (file) => file === "/usr/bin/dpkg-deb" || fs.existsSync(file);
  const goodListing = contentsListing(expectedFiles);
  const goodSpawn = fakeDpkgSpawn({ expectedFiles, bytesByDestination, listing: goodListing });
  assert.equal(
    verifyLinuxDebFixedFiles({
      artifact,
      expectedFiles,
      exists,
      spawn: goodSpawn,
      temporaryDirectory: verificationTemp,
    }),
    artifact,
  );
  assert.deepEqual(fs.readdirSync(verificationTemp), [], "temporary extraction must be removed");

  const wrongModeListing = contentsListing(expectedFiles, {
    [helperDestination]: { symbolicMode: "-rwxrwxr-x" },
  });
  assert.throws(
    () => verifyLinuxDebFixedFiles({
      artifact,
      expectedFiles,
      exists,
      spawn: fakeDpkgSpawn({
        expectedFiles,
        bytesByDestination,
        listing: wrongModeListing,
      }),
      temporaryDirectory: verificationTemp,
    }),
    /mode mismatch/,
  );

  assert.throws(
    () => verifyLinuxDebFixedFiles({
      artifact,
      expectedFiles,
      exists,
      spawn: fakeDpkgSpawn({
        expectedFiles,
        bytesByDestination,
        listing: goodListing,
        extractedOverrides: {
          [helperDestination]: { mode: 0o775 },
        },
      }),
      temporaryDirectory: verificationTemp,
    }),
    /extracted fixed deb file mode mismatch/,
  );
  assert.deepEqual(fs.readdirSync(verificationTemp), [], "mode failure must clean extraction");

  assert.throws(
    () => verifyLinuxDebFixedFiles({
      artifact,
      expectedFiles,
      exists,
      spawn: fakeDpkgSpawn({
        expectedFiles,
        bytesByDestination,
        listing: contentsListing(expectedFiles, {
          [profileDestination]: { owner: "builder/builder" },
        }),
      }),
      temporaryDirectory: verificationTemp,
    }),
    /owner mismatch/,
  );

  const corruptProfile = Buffer.from(bytesByDestination[profileDestination]);
  corruptProfile[0] ^= 1;
  assert.throws(
    () => verifyLinuxDebFixedFiles({
      artifact,
      expectedFiles,
      exists,
      spawn: fakeDpkgSpawn({
        expectedFiles,
        bytesByDestination,
        listing: goodListing,
        extractedOverrides: {
          [profileDestination]: { bytes: corruptProfile },
        },
      }),
      temporaryDirectory: verificationTemp,
    }),
    /hash mismatch/,
  );
  assert.deepEqual(fs.readdirSync(verificationTemp), [], "failed verification must clean extraction");

  assert.throws(
    () => verifyLinuxDebFixedFiles({
      artifact,
      expectedFiles,
      exists,
      spawn: fakeDpkgSpawn({
        expectedFiles,
        bytesByDestination,
        listing: `${goodListing}${goodListing.split("\n")[0]}\n`,
      }),
      temporaryDirectory: verificationTemp,
    }),
    /exactly once/,
  );

  assert.throws(
    () => verifyLinuxDebFixedFiles({
      artifact,
      expectedFiles,
      exists,
      spawn: fakeDpkgSpawn({
        expectedFiles,
        bytesByDestination,
        listing: goodListing,
        extractedOverrides: {
          [profileDestination]: { symlink: helperSource },
        },
      }),
      temporaryDirectory: verificationTemp,
    }),
    /regular non-symlink file/,
  );
  assert.deepEqual(fs.readdirSync(verificationTemp), [], "symlink rejection must clean extraction");

  assert.throws(
    () => verifyLinuxDebFixedFiles({
      artifact,
      expectedFiles,
      exists,
      spawn: fakeDpkgSpawn({
        expectedFiles,
        bytesByDestination,
        listing: goodListing,
        extractedOverrides: {
          [profileDestination]: { hardlinkDestination: helperDestination },
        },
      }),
      temporaryDirectory: verificationTemp,
    }),
    /exactly one hard link/,
  );
  assert.deepEqual(fs.readdirSync(verificationTemp), [], "hardlink rejection must clean extraction");

  cleanupLinuxDebFixedFilesStage(staged);
  assert.equal(fs.existsSync(staged.stagingRoot), false);
  assert.equal(fs.existsSync(staged.overlayPath), false);

  console.log("linux deb fixed files: ok");
} finally {
  fs.rmSync(testRoot, { recursive: true, force: true });
}
