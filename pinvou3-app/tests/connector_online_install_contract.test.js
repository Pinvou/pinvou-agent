const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "..");
const read = (...parts) => fs.readFileSync(path.join(root, ...parts), "utf8");
const installer = read("src-tauri", "src", "features", "connectors", "native_installer.rs");
const paths = read("src-tauri", "src", "platform", "paths.rs");
const build = read("scripts", "tauri", "build.js");
const tmeet = read("src-tauri", "src", "features", "connectors", "tmeet.rs");
const toolCommon = read("src", "features", "tools", "tool-common.jsx");
const posix = read("src-tauri", "src", "platform", "os", "posix.rs");
const linux = read("src-tauri", "src", "platform", "os", "linux", "linux_path.rs");
const macos = read("src-tauri", "src", "platform", "os", "macos", "macos_path.rs");

assert.doesNotMatch(build, /prepareConnectorClis|fetch-connectors/);
assert.match(paths, /pinvou3_home\(\)\.join\("connectors"\)/);
assert.doesNotMatch(paths, /bundle_root\(\)\.join\("connectors"\)/);

for (const connector of ["lark-cli", "wecom-cli", "dws"]) {
  const feature = read(
    "src-tauri",
    "src",
    "features",
    "connectors",
    connector === "lark-cli" ? "feishu.rs" : connector === "wecom-cli" ? "wecom.rs" : "dingtalk.rs",
  );
  assert.match(feature, new RegExp(`ensure_native_cli\\("${connector}"\\)`));
}

assert.match(installer, /archive_sha256/);
assert.match(installer, /binary_sha256/);
// Double HTTPS gate: candidate URLs must pass the scheme check before any
// download (non-HTTPS candidates are skipped outright), and the redirect
// policy likewise refuses to downgrade to non-HTTPS.
assert.match(installer, /url\.scheme\(\) == "https"/);
assert.match(installer, /scheme\(\) != "https"/);
assert.match(installer, /MAX_ARCHIVE_BYTES/);
assert.match(installer, /normalized_path_eq/);
assert.match(installer, /\.installing-/);

assert.match(tmeet, /@tencentcloud\/tmeet@1\.0\.19/);
// Connector CLI resolution has converged into posix.rs (Wave 3 dedup); linux/macos only re-export it.
assert.match(posix, /bundled_connector_npm_cli/);
assert.match(posix, /cli_bin == "tmeet"/);
assert.match(posix, /bundled_connector_node/);
for (const platformSource of [linux, macos]) {
  assert.match(platformSource, /connector_cli_command/);
}

// Version lockstep: the version shown on the tool cards must match the lock pin (and the tmeet.rs npm pin),
// preventing a repeat of the card version drifting from the actually installed version (historically
// v1.0.56 vs lock 1.0.65). All five platform locks are verified in full (this backs the NOTICE promise
// that "any one of the five suffices, provided the version fields agree"): bumping only one lock and missing
// the others (or the card) fails here.
const lockVersionsByPlatform = {};
for (const [osDir, archDir] of [
  ["macos", "aarch64"],
  ["macos", "x86_64"],
  ["linux", "aarch64"],
  ["linux", "x86_64"],
  ["windows", "x86_64"],
]) {
  const lock = JSON.parse(
    read("src-tauri", "resources", "platforms", osDir, archDir, "bundle", "connectors", "connectors.lock.json"),
  );
  lockVersionsByPlatform[`${osDir}/${archDir}`] = Object.fromEntries(lock.artifacts.map((a) => [a.name, a.version]));
}
const lockVersions = lockVersionsByPlatform["macos/aarch64"];
for (const [platform, versions] of Object.entries(lockVersionsByPlatform)) {
  assert.deepEqual(versions, lockVersions, `${platform} connectors.lock.json 版本与其他平台不一致`);
}
const cardVersion = (marker) => {
  const line = toolCommon.split("\n").find((l) => l.includes(marker));
  const match = line && line.match(/version:\s*['"]v([\d.]+)['"]/);
  assert.ok(match, `tool-common.jsx 未在 ${marker} 行找到 version: 'v…'（格式变更须同步本断言）`);
  return match[1];
};
assert.equal(cardVersion("backendId: 'feishu', feishuCli: true"), lockVersions["lark-cli"]);
assert.equal(cardVersion("backendId: 'dingtalk', dingtalkCli: true"), lockVersions["dws"]);
assert.equal(cardVersion("backendId: 'wecom', wecomCli: true"), lockVersions["wecom-cli"]);
const tmeetPin = tmeet.match(/@tencentcloud\/tmeet@([\d.]+)/)[1];
assert.equal(cardVersion("backendId: 'tmeet', tmeetCli: true"), tmeetPin);

// The manual smoke's version floor must equal the host's minimum acceptable
// wecom-cli version: a stale floor would let wecom-smoke.sh certify a CLI
// that wecom_ensure_cli force-replaces on first use (the 1.1.0 smoke floor
// vs the 1.2.1 host gate mismatch fixed here).
const wecom = read("src-tauri", "src", "features", "connectors", "wecom.rs");
const wecomSmoke = read("scripts", "wecom-smoke.sh");
const wecomMin = wecom.match(
  /WECOM_MIN_VERSION:\s*\(u64, u64, u64\)\s*=\s*\((\d+),\s*(\d+),\s*(\d+)\)/,
);
assert.ok(wecomMin, "wecom.rs must declare WECOM_MIN_VERSION as a tuple");
assert.match(
  wecomSmoke,
  new RegExp(`MIN_WECOM="${wecomMin[1]}\\.${wecomMin[2]}\\.${wecomMin[3]}"`),
  "wecom-smoke.sh version floor must stay in lockstep with WECOM_MIN_VERSION",
);
assert.match(
  wecomSmoke,
  /grep -qxF "\$MIN_WECOM"/,
  "wecom-smoke.sh must compare against the pinned MIN_WECOM floor, not a drift copy",
);
// The gate itself must also equal the wecom-cli version pinned in the five
// platform locks (all five are forced identical by the deepEqual above):
// raising WECOM_MIN_VERSION and the smoke floor together while a lock still
// pins an older CLI would make the host download that older CLI and
// force-replace it on first use.
const wecomMinSemver = `${wecomMin[1]}.${wecomMin[2]}.${wecomMin[3]}`;
assert.equal(
  wecomMinSemver,
  lockVersions["wecom-cli"],
  "WECOM_MIN_VERSION must equal the wecom-cli version pinned in the platform locks",
);

// Same lockstep for the dws manual smoke. The host has no DWS_MIN_VERSION gate
// (it installs the exact lock version by hash), so the smoke floor follows the
// cli_version floor declared in the bundled dws SKILL.md frontmatter, and that
// floor must not exceed the lock pin — otherwise the smoke would reject the
// very binary the host installs.
const dwsSkill = read(
  "src-tauri", "resources", "common", "bundle", "dingtalk-skills", "dws", "SKILL.md",
);
const dingtalkSmoke = read("scripts", "dingtalk-smoke.sh");
const dwsFloor = dwsSkill.match(/^cli_version:\s*"?>=\s*(\d+)\.(\d+)\.(\d+)"?\s*$/m);
assert.ok(dwsFloor, 'dws SKILL.md must declare cli_version as ">=X.Y.Z"');
assert.match(
  dingtalkSmoke,
  new RegExp(`MIN_TRI="${dwsFloor[1]}\\.${dwsFloor[2]}\\.${dwsFloor[3]}"`),
  "dingtalk-smoke.sh version floor must stay in lockstep with the dws SKILL.md cli_version",
);
assert.match(
  dingtalkSmoke,
  /grep -qxF "\$MIN_TRI"/,
  "dingtalk-smoke.sh must compare against the pinned MIN_TRI floor, not a drift copy",
);
// Per-segment comparison instead of weighted 1*10000+…: weighting gives the wrong answer once a segment
// reaches ≥100 (lock pins 1.0.164 vs floor 1.1.0 → 10100 ≤ 10164 wrongly passes, though semantically the
// floor is already above the lock).
const seg = (v) => v.split(".").map(Number);
const segLe = (a, b) => {
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const x = a[i] ?? 0;
    const y = b[i] ?? 0;
    if (x !== y) return x < y;
  }
  return true;
};
assert.ok(
  segLe(seg(`${dwsFloor[1]}.${dwsFloor[2]}.${dwsFloor[3]}`), seg(lockVersions["dws"])),
  "dws SKILL.md cli_version floor must not exceed the dws version pinned in the platform locks",
);

// ---- Contracts of the two smoke scripts themselves (behavior pins beyond the floors) ----
// 1) Version comparison must use the per-segment numeric sort keys: dropping the -k keys degrades to lexical
//    order, which sorts 1.0.9 after 1.0.64 and lets a below-floor CLI pass while the floor-literal pins stay green.
for (const [name, source] of [
  ["dingtalk-smoke.sh", dingtalkSmoke],
  ["wecom-smoke.sh", wecomSmoke],
]) {
  assert.match(
    source,
    /sort -t\. -k1,1n -k2,2n -k3,3n/,
    `${name}: 版本比较必须保留逐段数值排序键(字典序会让 1.0.9 >= 1.0.64)`,
  );
  // 2) Version parsing must anchor on the line containing the CLI name and take dotted numbers after the name:
  //    the same-family lark-cli was observed printing "a new version 1.0.96 is available" before the version
  //    line (feishu.rs pins that fixture); "first dotted number in the whole text" would read the update
  //    number/proxy IP as the local version and pass by mistake.
  assert.match(
    source,
    /tolower\(\$0\) ~ \/(dws|wecom-cli)\//,
    `${name}: 版本解析必须先过滤含 CLI 名字的行(更新提示/代理 IP 行不算版本)`,
  );
  assert.match(
    source,
    /if \(seen && \$i ~/,
    `${name}: 版本号必须取 CLI 名字之后的点分数字(seen 标志)`,
  );
  // 3) Every CLI output echo (including PASS and SKIP lines) must go through redact: pin the pipe shapes of
  //    the [1] PASS echo and the [3] SKIP echo directly (the FAIL path already goes through redact; pinning
  //    by occurrence count would let the FAIL path pad the count and miss the PASS path — verified via mutation).
  assert.match(
    source,
    /可执行: \$\(printf '%s\\n' "\$VER" \| redact \|/,
    `${name}: [1] --version PASS 回显必须过 redact`,
  );
  assert.match(
    source,
    /域结果未知\(exit \$d_rc\): \$\(printf '%s' "\$OUT" \| redact \|/,
    `${name}: [3] 域探测 SKIP 回显必须过 redact`,
  );
  // 3b) The probe deadline must have a second-level hard kill: with GNU timeout missing -k or the watchdog
  //     missing the KILL escalation, a TERM-swallowing CLI drags the probe into a permanent wait (source-shape
  //     pin; behavior pin in section 6 below).
  assert.match(
    source,
    /-k "\$\{?(DWS|WECOM)_KILL_GRACE\}?"/,
    `${name}: coreutils timeout 必须带 -k 强杀宽限(吞 TERM 的 CLI 会挂住 timeout 本身)`,
  );
  // 3b-process-group ownership (reviewer P1 correction): the CLI starts via set -m as leader of its own
  // process group, stdin on /dev/null (non-interactive contract: the CLI must not consume the caller's input
  // or wait on input until timeout), set +m immediately after the spawn; TERM/KILL both go to the whole
  // group (-$pid), falling back to single-process kill only when group signals are unavailable.
  assert.match(
    source,
    /set -m\s*\n\s*"\$CLI" "\$@" <\/dev\/null >"\$t" 2>&1 &\s*\n\s*local pid=\$!\s*\n\s*set \+m/,
    `${name}: CLI 必须以 set -m 自有进程组启动、stdin 接 /dev/null,派生后立即 set +m`,
  );
  assert.match(
    source,
    /"\$TIMEOUT_BIN" -k "\$\{?(DWS|WECOM)_KILL_GRACE\}?" "\$\{?(DWS|WECOM)_TIMEOUT\}?" "\$CLI" "\$@" <\/dev\/null >"\$t" 2>&1 &\s*\n\s*local tpid=\$!\s*\n\s*set \+m\s*\n\s*wait "\$tpid"/,
    `${name}: coreutils timeout 分支必须同样以 set -m 后台启动(组 id=timeout pid 可组信号寻址)、stdin 接 /dev/null、wait 回收;勿加 --foreground`,
  );
  // GNU timeout monitors only its direct child: it returns as soon as the leader dies, and -k does not
  // follow up on the rest of the group, so a TERM-resistant grandchild survives the probe (reviewer
  // reproduced on GNU coreutils 8.32). After a 124 finish, the script's own TERM->grace->group-KILL
  // escalation chain must run, gated by a group-liveness probe (kill -0 on the negative pid): return at once
  // when the group is empty (compliant TERM death, or -k already cleaned up), escalate only while the group
  // survives.
  assert.match(
    source,
    /if \[ "\$rc" -eq 124 \] && kill -0 "-\$tpid" 2>\/dev\/null; then\s*\n\s*kill -TERM "-\$tpid" 2>\/dev\/null \|\| kill -TERM "\$tpid" 2>\/dev\/null\s*\n\s*sleep "\$\{?(DWS|WECOM)_KILL_GRACE\}?"\s*\n\s*kill -KILL "-\$tpid" 2>\/dev\/null \|\| kill -KILL "\$tpid" 2>\/dev\/null/,
    `${name}: coreutils timeout 分支 124 收场后必须补组升级链(TERM->KILL_GRACE->组 KILL),用 kill -0 组存活探针把守——leader 之死不得带走升级所有权`,
  );
  assert.match(
    source,
    /sleep "\$\{?(DWS|WECOM)_TIMEOUT\}?" && \[ -e "\$t" \] && : >"\$tflag" \|\| exit 0\s*\n\s*kill -TERM "-\$pid" 2>\/dev\/null \|\| kill -TERM "\$pid"/,
    `${name}: 看门狗必须先确认捕获仍在再落「到点」标记,TERM 送整组(-$pid;只杀直接子进程会让 wrapper 的孙进程活过超时),组信号不可用退回单进程`,
  );
  assert.match(
    source,
    /sleep "\$\{?(DWS|WECOM)_KILL_GRACE\}?"\s*\n\s*kill -KILL "-\$pid" 2>\/dev\/null \|\| kill -KILL "\$pid"/,
    `${name}: KILL 升级必须同样送整组(吞 TERM 的孙进程只有组 KILL 收得到)`,
  );
  // Watchdog cleanup line: without it the watchdog subshell outlives a fast probe and, after PID reuse,
  // kills innocent processes, and the late .deadline is never cleaned up — no behavior fixture backs this
  // line, so it can only be shape-pinned.
  assert.match(
    source,
    /kill "\$wpid" 2>\/dev\/null; wait "\$wpid" 2>\/dev\/null/,
    `${name}: 看门狗必须在 wait 后回收子壳(孤儿看门狗会在 PID 复用后误杀进程)`,
  );
  // The deadline fact must be independent of the child's final exit code: a CLI that exits 0 gracefully
  // after TERM makes wait return 0, disguising the timeout as a pass (GNU timeout returns 124 for the same
  // shape; behavior pin in section 6, term-zero).
  assert.match(
    source,
    /\[ -e "\$tflag" \] && rc=124/,
    `${name}: 看门狗到点必须覆盖子进程最终退出码(TERM 后优雅 exit 0 不得伪装成通过)`,
  );
  assert.match(
    source,
    /\[ "\$rc" -eq 137 \] && rc=124/,
    `${name}: KILL 死(137)必须归一为超时 124,不能伪装成「结果未知」`,
  );
  // 3c) Version/connection PASS presumes the probe exited 0: healthy-looking output plus a non-zero exit
  //     (exit 7 was observed going all green) is not a pass; timeout 124 is caught by the same precondition.
  assert.match(
    source,
    /if \[ "\$v_rc" -eq 0 \] && \[ -n "\$TRI" \]/,
    `${name}: [1] 版本 PASS 必须先要求探测退出 0(exit 7 + 版本形状不是通过)`,
  );
  assert.match(
    source,
    /if \[ "\$a_rc" -eq 0 \] && echo "\$AUTH"/,
    `${name}: [2] 连接 PASS 必须先要求探测退出 0(exit 7 + authenticated:true 不是已连接)`,
  );
  // 3d) The redact field pattern must accept digit-suffixed keys (the accessToken2 shape once leaked bare).
  // The leading-digit class has only a shape pin and no behavior fixture — not a gap: the sed match does not
  // anchor at line start, so when a key contains token, the value masking can always restart matching after
  // the digit (measured: {"2fa_token":"SECRET9"} is masked under both classes; the leading class is redundant
  // for leak behavior and only fixes the output shape). The trailing class is different: digits after token
  // must be consumed by the trailing class to reach the ": " value, which is why the accessToken2 fixture
  // goes red behaviorally. If the class text changes, this pin (the whole run-on shape) goes red — a missing
  // character class always trips here first.
  assert.match(
    source,
    /\[A-Za-z0-9_-\]\*\[Tt\]\[Oo\]\[Kk\]\[Ee\]\[Nn\]\[A-Za-z0-9_-\]\*/,
    `${name}: redact 字段形态必须接受数字后缀键`,
  );
}
// 4) The wecom smoke must scan the app-installed layout: searching PATH only would miss
//    <data root>/assets/cli/wecom-cli/<ver>/ (where native_installer lays the binary down) and steer an
//    ungated error toward a self-installed npm -g copy (the hash-loop path registered in NOTICE-wecom). The
//    data root goes through PINVOU3_ROOT (paths.rs pinvou3_home supports relocating the data root wholesale
//    via PINVOU3_HOME; honoring only $HOME/.pinvou3 would miss relocated installs).
assert.match(
  wecomSmoke,
  /for d in "\$PINVOU3_ROOT"\/assets\/cli\/wecom-cli\/\*\/; do/,
  "wecom-smoke.sh 必须扫描应用安装布局 <数据根>/assets/cli/wecom-cli/<version>/(钉循环本身,注释提及不算)",
);
// Data-root resolution must match the app (PINVOU3_HOME first, default $HOME/.pinvou3): pin the definition
// line to prevent falling back to a hardcoded $HOME/.pinvou3 (relocated installs would be missed again).
for (const [name, source] of [
  ["dingtalk-smoke.sh", dingtalkSmoke],
  ["wecom-smoke.sh", wecomSmoke],
]) {
  assert.match(
    source,
    /^PINVOU3_ROOT="\$\{PINVOU3_HOME:-\$HOME\/\.pinvou3\}"$/m,
    `${name}: 数据根必须按应用同口径解析(PINVOU3_HOME 优先,默认 $HOME/.pinvou3)`,
  );
}
assert.doesNotMatch(
  wecomSmoke,
  /npm\s+(i|install)\s+(-g|--global)\b/,
  "wecom-smoke.sh 找不到 CLI 时不得再建议 npm -g 自装(绕过 lock 钉扎;缩写与全拼一并禁)",
);
assert.doesNotMatch(
  dingtalkSmoke,
  /npm\s+(i|install)\s+(-g|--global)\b/,
  "dingtalk-smoke.sh 找不到 CLI 时不得建议 npm -g 自装(绕过 lock 钉扎)",
);

// ---- 4b) Resolution order: app-managed assets first, PATH only as an explicit dev fallback ----
// The smoke must verify the binary the app installed from connectors.lock.json and hash-verified; a
// same-named PATH entry (stale/malicious) used to be resolved before the assets and could shadow the
// lock-pinned asset, letting the smoke pass "another binary". Pins: the version-directory scan loop must
// appear before PATH resolution, and selecting the PATH fallback must carry the source annotation (moving
// resolution back to PATH-first, or dropping the annotation, goes red here).
for (const [name, source] of [
  ["dingtalk-smoke.sh", dingtalkSmoke],
  ["wecom-smoke.sh", wecomSmoke],
]) {
  const assetLoopIdx = source.indexOf('for d in "$PINVOU3_ROOT"/assets/cli/');
  const pathResolveIdx = source.indexOf('CLI="$(command -v ');
  assert.ok(assetLoopIdx >= 0, `${name}: 未找到应用版本目录扫描循环`);
  assert.ok(
    pathResolveIdx > assetLoopIdx,
    `${name}: PATH 解析必须晚于应用管理资产扫描(应用资产优先;PATH 同名项不得遮住 lock 钉扎资产)`,
  );
  assert.match(
    source,
    /开发回退,非应用管理的 lock 钉扎资产/,
    `${name}: PATH 回退选中时必须大声标注「开发回退」来源`,
  );
}

// 5) redact() behavior pin (the function body is extracted live from both script sources and actually run):
//    the host's safe_auth_log_line masks the whole line for the four markers plus quoted JSON field masking.
//    Removing the awk stage/sed stage or swapping the marker words goes red here first — the floor pins do
//    not cover redaction.
{
  const { execFileSync } = require("node:child_process");
  const extractRedact = (source, name) => {
    const lines = source.split("\n");
    const start = lines.findIndex((l) => l.startsWith("redact(){"));
    assert.ok(start >= 0, `${name}: 未找到 redact(){ 定义`);
    // The definition is 3 lines in total (redact(){ … awk … sed …).
    const defn = lines.slice(start, start + 3).join("\n");
    assert.match(defn, /access_token\|refresh_token\|authorization\|bearer/, `${name}: redact 必须覆盖 host 四标记`);
    // Generalized token assignment shapes (reviewer P1 addition): api_token=…, bare token: …, token = …
    // are not among the four host markers and used to leak into failure excerpts. Changing the class text
    // goes red on this pin.
    assert.match(
      defn,
      /\[a-z0-9_-\]\*token\[a-z0-9_-\]\*\[\[:space:\]\]\*\[=:\]/,
      `${name}: redact 必须覆盖泛化 token 赋值形态(api_token=…、裸 token: …、token = …)`,
    );
    return defn;
  };
  // Each shape asserts its expected masking marker, not merely that SECRET is absent from the output: when
  // only the function definition is passed without a call, bash never reads stdin and the output is empty,
  // so "no SECRET" would pass vacuously (CI exposed this via spawnSync EPIPE).
  const leakForms = [
    ["access_token=SECRET1", "[已脱敏"],
    ['refresh_token: "SECRET2"', "[已脱敏"],
    ["https://x.example/api?access_token=SECRET3&v=2", "[已脱敏"],
    ["AUTHORIZATION: BEARER SECRET4", "[已脱敏"],
    ["x-auth: bearer SECRET5", "[已脱敏"],
    ['{"token": "SECRET6"}', "token=[REDACTED]"],
    // authorization is a marker in its own right (the two bearer shapes above happen to hit it; this one
    // pins bare authorization); the digit-suffixed key pins the 0-9 character class of the field pattern
    // (accessToken2 once leaked in full).
    ["x-authorization: SECRET7", "[已脱敏"],
    ['{"accessToken2": "SECRET8"}', "token=[REDACTED]"],
    // Generalized token assignment shapes (reviewer P1 addition): bare assignment keys used to leak whole lines.
    ["api_token=SECRET10", "[已脱敏"],
    ["token: SECRET11", "[已脱敏"],
    ["token =SECRET12", "[已脱敏"],
    ["API_TOKEN=SECRET13", "[已脱敏"],
    ["x-token: SECRET14", "[已脱敏"],
    ["https://x.example/api?token=SECRET15&v=2", "[已脱敏"],
  ];
  for (const [name, source] of [
    ["dingtalk-smoke.sh", dingtalkSmoke],
    ["wecom-smoke.sh", wecomSmoke],
  ]) {
    const defn = extractRedact(source, name);
    if (process.platform === "win32") continue; // skip the behavior section on platforms without bash on PATH
    const runRedact = (line) =>
      execFileSync("bash", ["-c", `${defn}\nredact`], { input: `${line}\n` }).toString();
    for (const [line, marker] of leakForms) {
      const out = runRedact(line);
      assert.ok(
        out.includes(marker) && !out.includes("SECRET"),
        `${name}: redact 必须把「${line}」打码为含 ${marker} 的输出,实得: ${out.trim()}`,
      );
    }
    assert.equal(runRedact("plain diagnostic line"), "plain diagnostic line\n", `${name}: redact 不得改写无凭证形态的行`);
    // Negative control for the generalized rule: an ordinary non-assignment line merely containing the word
    // token must not be swallowed whole (over-masking makes timeout/failure diagnostics unreadable).
    assert.equal(
      runRedact("token count: 3 items in cache"),
      "token count: 3 items in cache\n",
      `${name}: 非「token 键赋值」形态的行不得整行打码`,
    );
  }
}

// ---- 6) The probe deadline must stay bounded and report timeout for both TERM-swallowing and graceful-exit-after-TERM CLIs ----
// GNU timeout without -k: once the child ignores TERM, timeout itself waits forever; a watchdog that sends
// TERM only once leaves wait equally unbounded. The run function is extracted live from both scripts and run
// against TERM-resistant children: both paths must finish with 124 within deadline+kill grace, and output
// produced before the hard kill must be preserved (timeout diagnostics need the scene).
// Another shape: the child receives TERM, cleans up gracefully, and exits 0 — wait's 0 must not overwrite
// the "deadline reached" fact, and the result must still be 124 (GNU timeout returns 124 for the same shape;
// the watchdog branch was observed masquerading as rc=0).
// spawnSync's timeout+SIGKILL is only a regression backstop — once fixed, this test itself finishes in seconds.
{
  const { spawnSync, execFileSync } = require("node:child_process");
  const os = require("node:os");
  if (process.platform !== "win32") {
    const hasTimeout =
      execFileSync("bash", ["-c", "command -v timeout >/dev/null 2>&1 && echo yes || echo no"])
        .toString()
        .trim() === "yes";
    const timeoutBin = hasTimeout
      ? execFileSync("bash", ["-c", "command -v timeout"]).toString().trim()
      : "";
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "smoke-probe-"));
    const stubborn = path.join(tmp, "stubborn-cli");
    fs.writeFileSync(
      stubborn,
      "#!/usr/bin/env bash\ntrap '' TERM\nprintf 'stubborn 9.9.9\\n'\nsleep 300\n",
    );
    fs.chmodSync(stubborn, 0o755);
    // The graceful-exit fixture must use sleep&wait, not a foreground sleep: bash defers trap execution
    // while a foreground child runs (that is really the "swallow TERM" shape, taking the KILL path); only
    // wait runs the trap immediately.
    const termZero = path.join(tmp, "term-zero-cli");
    fs.writeFileSync(
      termZero,
      "#!/usr/bin/env bash\ntrap 'exit 0' TERM\nprintf 'term-zero 9.9.9\\n'\nsleep 300 &\nwait\n",
    );
    fs.chmodSync(termZero, 0o755);
    const quick = path.join(tmp, "quick-cli");
    fs.writeFileSync(quick, "#!/usr/bin/env bash\nprintf 'quick 1.2.3\\n'\n");
    fs.chmodSync(quick, 0o755);
    for (const [name, source] of [
      ["dingtalk-smoke.sh", dingtalkSmoke],
      ["wecom-smoke.sh", wecomSmoke],
    ]) {
      const lines = source.split("\n");
      const prefix = (source.match(/^([A-Z]+)_TIMEOUT=/m) || [])[1];
      assert.ok(prefix, `${name}: 未找到 *_TIMEOUT= 预备段`);
      const fn = `${prefix.toLowerCase()}_run`;
      const start = lines.findIndex((l) => l.startsWith(`${prefix}_TIMEOUT=`));
      const fnStart = lines.findIndex((l) => l.startsWith(`${fn}() {`));
      const end = lines.indexOf("}", fnStart);
      assert.ok(start >= 0 && fnStart > start && end > fnStart, `${name}: run 函数抽取段不完整`);
      const prelude = lines.slice(start, end + 1).join("\n");
      const harness = (cli, bin) =>
        [
          prelude,
          `CLI=${JSON.stringify(cli)}`,
          `TIMEOUT_BIN=${JSON.stringify(bin)}`,
          `${prefix}_TIMEOUT=2`,
          `${prefix}_KILL_GRACE=1`,
          `${fn} --version`,
        ].join("\n");
      const runBounded = (label, bin, cli = stubborn, marker = "stubborn 9.9.9") => {
        const t0 = Date.now();
        const res = spawnSync("bash", ["-c", harness(cli, bin)], {
          encoding: "utf8",
          timeout: 20000,
          killSignal: "SIGKILL",
        });
        const elapsed = Date.now() - t0;
        assert.strictEqual(
          res.status,
          124,
          `${label}: 超时探测必须以 124 收场(status=${res.status} signal=${res.signal} err=${res.error})`,
        );
        assert.ok(
          elapsed < 15000,
          `${label}: 探测必须在时限+杀宽限内有界返回,实得 ${elapsed}ms(无界等待即回归)`,
        );
        assert.ok(
          res.stdout.includes(marker),
          `${label}: 收场前已产出的探测输出必须保留(超时诊断要能看到现场)`,
        );
      };
      if (hasTimeout) runBounded(`${name} coreutils timeout 分支`, timeoutBin);
      runBounded(`${name} 看门狗分支`, "");
      // Graceful exit 0 after TERM: neither path may disguise the timeout as rc=0 (observed on the watchdog branch).
      if (hasTimeout) runBounded(`${name} coreutils 分支 TERM 后 exit 0 仍判超时`, timeoutBin, termZero, "term-zero 9.9.9");
      runBounded(`${name} 看门狗分支 TERM 后 exit 0 仍判超时`, "", termZero, "term-zero 9.9.9");
      // Follow-up call control: a timeout finish must not pollute the next probe (leftover markers/orphans
      // would hang or fail a fast call).
      const after = spawnSync("bash", ["-c", harness(quick, "")], {
        encoding: "utf8",
        timeout: 20000,
        killSignal: "SIGKILL",
      });
      assert.strictEqual(
        after.status,
        0,
        `${name}: 看门狗超时后的后续快速调用必须照常成功(status=${after.status})`,
      );
      assert.ok(after.stdout.includes("quick 1.2.3"), `${name}: 后续快速调用的输出必须完整回放`);
      const res = spawnSync("bash", ["-c", harness(quick, timeoutBin)], {
        encoding: "utf8",
        timeout: 20000,
        killSignal: "SIGKILL",
      });
      assert.strictEqual(res.status, 0, `${name}: 健康子进程不得被误杀(status=${res.status})`);
      assert.ok(res.stdout.includes("quick 1.2.3"), `${name}: 健康探测输出必须完整回放`);
      // An allocation failure (mktemp failing/empty) must early-return before spawning any cleanup path: the
      // old shape spawned the empty t into a relative .deadline in the caller's directory, and the trailing
      // plain rm deleted the same-named file (reviewer reproduced; remove the guard and this block goes red —
      // the old shape measured status=124 here with the sentinel deleted). After the failure, the next call
      // with a healthy TMPDIR must succeed as usual (a failure must not pollute later probes).
      const allocCaller = fs.mkdtempSync(path.join(os.tmpdir(), "smoke-alloc-caller-"));
      fs.writeFileSync(path.join(allocCaller, ".deadline"), "caller owns this file");
      const runFromCaller = (env) =>
        spawnSync("bash", ["-c", harness(quick, timeoutBin)], {
          encoding: "utf8",
          timeout: 20000,
          killSignal: "SIGKILL",
          cwd: allocCaller,
          env,
        });
      const failedAlloc = runFromCaller({
        ...process.env,
        TMPDIR: path.join(tmp, "alloc-missing-tmpdir"),
      });
      assert.strictEqual(
        failedAlloc.status,
        1,
        `${name}: mktemp 失败的探测必须以 1 原路返回(status=${failedAlloc.status})`,
      );
      assert.deepEqual(
        fs.readdirSync(allocCaller),
        [".deadline"],
        `${name}: 分配失败后调用方目录必须原样(哨兵被删=清理路径未拦住空分配)`,
      );
      const nextAlloc = runFromCaller({ ...process.env });
      assert.strictEqual(
        nextAlloc.status,
        0,
        `${name}: 分配失败不得污染下一次正常探测(status=${nextAlloc.status})`,
      );
      assert.ok(
        nextAlloc.stdout.includes("quick 1.2.3"),
        `${name}: 分配失败后的下一次调用输出必须完整回放`,
      );
      // Group-ownership fixture (reviewer P1): when a TERM-swallowing wrapper spawns a native grandchild,
      // killing only the direct child lets the grandchild outlive the timeout (a credential-holding process
      // escaping the smoke boundary). Both wrapper and grandchild must finish with the probe — remove set -m
      // (group startup) or demote the group kill back to a single-process kill, and a surviving grandchild
      // goes red here. Both wrapper and grandchild carry trap '' TERM: a group TERM cannot pierce either, so
      // the fix must reap them via the group-KILL escalation, and the fixture pins the group delivery of that
      // KILL escalation too (the wrapper fixture once lacked the trap — it died straight on the group TERM,
      // which did not match the "TERM-swallowing wrapper" it claimed to simulate; the coreutils timeout
      // branch's group escalation is completed by the post-124 sweep chain, see the leader-early-exit fixture
      // below).
      // (Strict POSIX semantics; Windows MSYS2 kill semantics differ, so this block does not run on win32.)
      const gpidFile = path.join(tmp, "gpid");
      const grandchild = path.join(tmp, "grandchild-cli");
      fs.writeFileSync(
        grandchild,
        "#!/usr/bin/env bash\nprintf '%s' \"$$\" > \"$GPID_FILE\"\ntrap '' TERM\nsleep 300\n",
      );
      fs.chmodSync(grandchild, 0o755);
      const wrapper = path.join(tmp, "wrapper-cli");
      fs.writeFileSync(
        wrapper,
        "#!/usr/bin/env bash\ntrap '' TERM\n\"$GRANDCHILD_BIN\" &\nwait\n",
      );
      fs.chmodSync(wrapper, 0o755);
      // Leader-early-exit fixture (reviewer P2): the wrapper carries no trap, so the leader dies on the group
      // TERM while the TERM-resistant grandchild survives — the parent shell's wait returns early, and the
      // escalation chain is easiest to sever here: the old shape's kill $wpid took down the watchdog before it
      // ever sent the group KILL. Both branches must run: the watchdog branch verifies the escort fix; the
      // coreutils timeout branch verifies the post-124 group-escalation sweep (GNU timeout monitors only its
      // direct child and returns once the leader dies, so the grandchild can only be reaped by the script's
      // own group KILL).
      const wrapperEarlyExit = path.join(tmp, "wrapper-early-exit-cli");
      fs.writeFileSync(wrapperEarlyExit, '#!/usr/bin/env bash\n"$GRANDCHILD_BIN" &\nwait\n');
      fs.chmodSync(wrapperEarlyExit, 0o755);
      const grandchildDead = () => {
        const gpid = fs.existsSync(gpidFile) ? fs.readFileSync(gpidFile, "utf8").trim() : "";
        if (!gpid) return false; // pid not registered yet: treat as alive and keep waiting
        return spawnSync("bash", ["-c", `kill -0 ${JSON.stringify(gpid)} 2>/dev/null`]).status !== 0;
      };
      const runTree = (label, bin, wrapperBin = wrapper) => {
        fs.rmSync(gpidFile, { force: true });
        const res = spawnSync("bash", ["-c", harness(wrapperBin, bin)], {
          encoding: "utf8",
          timeout: 20000,
          killSignal: "SIGKILL",
          env: { ...process.env, GRANDCHILD_BIN: grandchild, GPID_FILE: gpidFile },
        });
        assert.strictEqual(
          res.status,
          124,
          `${label}: wrapper+孙进程超时必须以 124 收场(status=${res.status})`,
        );
        for (let i = 0; i < 20; i++) {
          if (grandchildDead()) return;
          spawnSync("bash", ["-c", "sleep 0.3"]);
        }
        assert.fail(`${label}: wrapper 被收场后孙进程必须随进程组一并终止(组所有权缺失)`);
      };
      runTree(`${name} 看门狗分支 wrapper+孙进程`, "");
      if (hasTimeout) runTree(`${name} coreutils timeout 分支 wrapper+孙进程`, timeoutBin);
      runTree(`${name} 看门狗分支 leader 提前退场后孙抗 TERM 仍被组 KILL`, "", wrapperEarlyExit);
      if (hasTimeout) runTree(`${name} coreutils timeout 分支 leader 提前退场后孙抗 TERM 仍被组 KILL`, timeoutBin, wrapperEarlyExit);
      // stdin contract fixture (reviewer P2): probes must not inherit the caller's stdin — the CLI could
      // consume pipe input or wait on a no-TTY prompt until timeout. Feed one fake input line: the CLI must
      // see EOF (stdin-closed) instead of reading it; the secret appearing in output goes red.
      const stdinCli = path.join(tmp, "stdin-cli");
      fs.writeFileSync(
        stdinCli,
        '#!/usr/bin/env bash\nif read -r line; then printf \'consumed:%s\' "$line"; else printf \'stdin-closed\'; fi\n',
      );
      fs.chmodSync(stdinCli, 0o755);
      const runStdin = (label, bin) => {
        const res = spawnSync("bash", ["-c", harness(stdinCli, bin)], {
          encoding: "utf8",
          timeout: 20000,
          killSignal: "SIGKILL",
          input: "SECRET-STDIN-7\n",
        });
        assert.strictEqual(res.status, 0, `${label}: stdin 探测必须正常收场(status=${res.status})`);
        assert.ok(
          res.stdout.includes("stdin-closed") && !res.stdout.includes("SECRET-STDIN-7"),
          `${label}: CLI 必须见到 EOF 而不是吃掉调用方 stdin(实得: ${res.stdout.trim()})`,
        );
      };
      runStdin(`${name} 看门狗分支 stdin`, "");
      if (hasTimeout) runStdin(`${name} coreutils timeout 分支 stdin`, timeoutBin);
    }
  }
}

// ---- 7) A failing probe (non-zero exit) must not PASS ----
// Healthy-looking output plus exit 7 once let both scripts finish all green with exit 0 (reviewer
// reproduced with a fake CLI). Full runs of the real scripts with a fake CLI: version and connection must
// both FAIL, the overall exit must be non-zero, and it must not fall into the "all domain probes skipped"
// exit 3 escape hatch (the fake CLI's domain probes all exit 0).
{
  const { spawnSync } = require("node:child_process");
  const os = require("node:os");
  if (process.platform !== "win32") {
    const home = fs.mkdtempSync(path.join(os.tmpdir(), "smoke-home-"));
    const bin = path.join(home, "bin");
    fs.mkdirSync(bin, { recursive: true });
    fs.writeFileSync(
      path.join(bin, "dws"),
      [
        "#!/usr/bin/env bash",
        'case "$1" in',
        '  --version) echo "dws 9.9.9 (fake)"; exit 7 ;;',
        '  auth) echo \'{"authenticated": true}\'; exit 7 ;;',
        "  *) exit 0 ;;",
        "esac",
        "",
      ].join("\n"),
    );
    fs.writeFileSync(
      path.join(bin, "wecom-cli"),
      [
        "#!/usr/bin/env bash",
        'case "$1" in',
        '  --version) echo "wecom-cli 9.9.9 (fake)"; exit 7 ;;',
        '  auth) echo "authorized"; exit 7 ;;',
        "  *) exit 0 ;;",
        "esac",
        "",
      ].join("\n"),
    );
    fs.chmodSync(path.join(bin, "dws"), 0o755);
    fs.chmodSync(path.join(bin, "wecom-cli"), 0o755);
    for (const name of ["dingtalk-smoke.sh", "wecom-smoke.sh"]) {
      const res = spawnSync("bash", [path.join(root, "scripts", name)], {
        encoding: "utf8",
        timeout: 120000,
        killSignal: "SIGKILL",
        env: { ...process.env, HOME: home, PATH: bin + path.delimiter + process.env.PATH },
      });
      assert.notStrictEqual(
        res.status,
        0,
        `${name}: exit 7 + 健康输出形状的 CLI 不得全绿退出 0`,
      );
      assert.notStrictEqual(
        res.status,
        3,
        `${name}: 假 CLI 域探测全 exit 0,不得落入 exit 3 掩护路径`,
      );
      assert.ok(!res.stdout.includes("[PASS] --version"), `${name}: 版本探测非零退出不得 PASS`);
      assert.ok(!res.stdout.includes("[PASS] 已连接"), `${name}: 连接探测非零退出不得 PASS`);
      assert.ok(res.stdout.includes("[FAIL]"), `${name}: 非 0 退出必须伴随 FAIL 行`);
    }
  }
}

// ---- 8) Behavior pins for the parse/compare/discovery pipelines (shape pins do not execute, so mutations do not bite) ----
// "Update notice on an earlier line" (no CLI name on the line) is excluded by the line-name filter — that
// fixture pins the line-name filter; the seen anchor is pinned by the "digits before the name on the same
// line" fixture, and only removing seen goes red there. Flipping the compare pipeline head→tail or dropping
// the -k numeric keys flips the floor verdict; dropping the discovery sort's keys lets 1.0.9 outrank 1.0.64.
// All of it is actually run via live eval.
{
  const { execFileSync } = require("node:child_process");
  if (process.platform !== "win32") {
    for (const [name, source, cliName, min, below, aboveLexSmall] of [
      ["dingtalk-smoke.sh", dingtalkSmoke, "dws", "1.0.64", "1.0.9", "1.0.164"],
      ["wecom-smoke.sh", wecomSmoke, "wecom-cli", "1.3.4", "1.2.99", "1.3.10"],
    ]) {
      const runLine = (line, outVar, env) =>
        execFileSync("bash", ["-c", `eval "$SMOKE_LINE"; printf '%s' "$${outVar}"`], {
          env: { ...process.env, SMOKE_LINE: line, ...env },
        }).toString();
      const triLine = source.split("\n").find((l) => l.startsWith('TRI="$(printf'));
      assert.ok(triLine, `${name}: 未找到 TRI 解析行`);
      assert.strictEqual(
        runLine(triLine, "TRI", { VER: `${cliName} ${min} (build)` }),
        min,
        `${name}: 健康版本行解析`,
      );
      assert.strictEqual(
        runLine(triLine, "TRI", { VER: `a new version 1.0.96 is available\n${cliName} ${min}` }),
        min,
        `${name}: 更新提示行(数字在前、行内无 CLI 名)不得被读成本机版本——删行名过滤此处即红`,
      );
      // Dedicated fixture for the seen anchor: dotted digits appear before the CLI name on the same line —
      // the line-name filter lets the line through, and only seen can skip the digits before the name and
      // take the version after it (independent negative control: removing seen goes red here).
      assert.strictEqual(
        runLine(triLine, "TRI", { VER: `2.5.1 available for ${cliName} needs ${min}` }),
        min,
        `${name}: 同行数字在名字之前仍须取名字之后的版本——删 seen 锚此处即红`,
      );
      assert.strictEqual(
        runLine(triLine, "TRI", { VER: `proxy http://10.1.2.3:8080\n${cliName} ${min}` }),
        min,
        `${name}: 代理 IP 行不得被读成本机版本`,
      );
      assert.strictEqual(
        runLine(triLine, "TRI", { VER: "unrelated text 1.2.3" }),
        "",
        `${name}: 无 CLI 名的行不产生版本`,
      );
      const geLine = source.split("\n").find((l) => l.startsWith('GE="$(printf'));
      assert.ok(geLine, `${name}: 未找到 GE 比较行`);
      const minVar = (geLine.match(/\$(MIN_[A-Z]+)/) || [])[1];
      assert.ok(minVar, `${name}: GE 比较行必须引用钉扎下限变量`);
      const runGe = (tri) => runLine(geLine, "GE", { TRI: tri, [minVar]: min });
      assert.strictEqual(runGe(min), "yes", `${name}: 等于下限必须判过`);
      assert.strictEqual(runGe("9.9.9"), "yes", `${name}: 远高于下限必须判过(head→tail 变异此处红)`);
      assert.strictEqual(runGe(below), "no", `${name}: 低于下限必须判不过(head→tail 变异此处红)`);
      assert.strictEqual(
        runGe(aboveLexSmall),
        "yes",
        `${name}: 高于下限但字典序更小的段值必须判过(丢 -k 数值键此处红)`,
      );
      const discLine = source.split("\n").find((l) => l.includes('ver="$(printf'));
      assert.ok(discLine, `${name}: 未找到发现排序行`);
      const runDisc = (cands) => runLine(discLine, "ver", { ver: cands });
      assert.strictEqual(
        runDisc("1.0.9\n1.0.64"),
        "1.0.64",
        `${name}: 发现排序必须逐段数值取最高(丢键或 tail→head 此处红)`,
      );
      assert.strictEqual(runDisc("1.0.64\n1.0.65\n1.0.9"), "1.0.65", `${name}: 发现排序取最高版本`);
    }
  }
}

// ---- 9) wecom [3] branch order: timeout (124) before the permission words ----
// A timed-out probe's partial output may contain "permissions" — checking words first would mislabel the
// timeout as "domain unauthorized"; both are SKIP, but a timeout calls for a rerun, and the labeling must
// not mislead.
{
  const lines = wecomSmoke.split("\n");
  const timeoutIdx = lines.findIndex((l) => l.includes('elif [ "$d_rc" -eq 124 ]'));
  const permIdx = lines.findIndex((l) => l.includes("'权限|暂不支持|未授权'"));
  assert.ok(timeoutIdx >= 0, "wecom-smoke.sh: 未找到 [3] 超时分支");
  assert.ok(permIdx > timeoutIdx, "wecom-smoke.sh: [3] 超时分支必须先于权限词分支");
}

// ---- 9b) dingtalk [3] branch order: timeout (124) before the error words (same as wecom [3]) ----
// A timed-out probe's partial output may contain not found/unknown — checking words first would mislabel
// the timeout as a hard "domain missing" failure; the two are handled differently, a timeout calls for a
// rerun, and the labeling must not mislead.
{
  const lines = dingtalkSmoke.split("\n");
  const timeoutIdx = lines.findIndex((l) => l.includes('elif [ "$d_rc" -eq 124 ]'));
  const errIdx = lines.findIndex((l) => l.includes("'unknown command|not found|不存在|未知'"));
  assert.ok(timeoutIdx >= 0, "dingtalk-smoke.sh: 未找到 [3] 超时分支");
  assert.ok(errIdx > timeoutIdx, "dingtalk-smoke.sh: [3] 超时分支必须先于错误词分支(超时的部分输出可能恰含 not found/未知)");
}

// ---- 10) wecom [4] error keys are checked in the envelope only, never in body values ----
// Every string field of the wecomcli-doc SKILL.md return table carries document data: content is "the
// original text returned directly when the document is short" — a legitimate document whose value is
// exactly "error" once went red as a failed read under a bare grep (reviewer P3, full runs of the real
// script with a fake CLI); name/url/file_path are string fields of the same table and go red the same way
// when the document name is exactly "error", so the script masks them all. After masking body values, the
// envelope layer's structured error keys (errcode:3) must still FAIL — masking must not swallow the check.
{
  const { spawnSync } = require("node:child_process");
  const os = require("node:os");
  if (process.platform !== "win32") {
    const fakeCli = (docPayload) =>
      [
        "#!/usr/bin/env bash",
        'case "$1" in',
        '  --version) echo "wecom-cli 1.3.4 (fake)"; exit 0 ;;',
        '  auth) echo "authorized"; exit 0 ;;',
        "  doc)",
        '    if [ "${2:-}" = contents ]; then printf \'%s\' \'' + docPayload + "'; exit 0; fi",
        "    exit 0 ;;",
        "  *) exit 0 ;;",
        "esac",
        "",
      ].join("\n");
    const runSmoke = (docPayload) => {
      const home = fs.mkdtempSync(path.join(os.tmpdir(), "wecom-doc-home-"));
      const bin = path.join(home, "bin");
      fs.mkdirSync(bin, { recursive: true });
      fs.writeFileSync(path.join(bin, "wecom-cli"), fakeCli(docPayload));
      fs.chmodSync(path.join(bin, "wecom-cli"), 0o755);
      return spawnSync("bash", [path.join(root, "scripts", "wecom-smoke.sh")], {
        encoding: "utf8",
        timeout: 120000,
        killSignal: "SIGKILL",
        env: {
          ...process.env,
          HOME: home,
          PATH: bin + path.delimiter + process.env.PATH,
          WECOM_TEST_DOCID: "DOC123",
        },
      });
    };
    // Legitimate envelope with a body value exactly "error": after masking the body it must read as success,
    // not go red by mistake.
    const good = runSmoke(
      '{"url":"https://doc.example/x","name":"周报","content":"error","file_path":"","document":{},"version":"3"}',
    );
    assert.strictEqual(
      good.status,
      0,
      `wecom-smoke.sh: 正文值为 error 的合法 envelope 不得误红(status=${good.status})\n${good.stdout}`,
    );
    assert.ok(
      good.stdout.includes("[PASS] 读到文档内容"),
      `wecom-smoke.sh: 正文屏蔽后读取必须 PASS\n${good.stdout}`,
    );
    // Real envelope error key: masking must not swallow the envelope layer's structured error.
    const bad = runSmoke(
      '{"url":"https://doc.example/x","name":"t","content":"ok","file_path":"","document":{},"version":"3","errcode":3}',
    );
    assert.notStrictEqual(
      bad.status,
      0,
      `wecom-smoke.sh: envelope errcode:3 必须 FAIL(屏蔽不得吞掉错误键)\n${bad.stdout}`,
    );
    assert.ok(
      !bad.stdout.includes("[PASS] 读到文档内容"),
      `wecom-smoke.sh: errcode:3 不得判读成功\n${bad.stdout}`,
    );
    // The other string fields of the same table carry document data: a legitimate envelope whose document
    // name/link/on-disk path contains "error" must not go red either (the masked keys must cover every
    // string field of the return table).
    const named = runSmoke(
      '{"url":"https://doc.example/error","name":"error","content":"ok","file_path":"/tmp/error.md","document":{},"version":"3"}',
    );
    assert.strictEqual(
      named.status,
      0,
      `wecom-smoke.sh: name/url/file_path 含 error 的合法 envelope 不得误红(status=${named.status})\n${named.stdout}`,
    );
    assert.ok(
      named.stdout.includes("[PASS] 读到文档内容"),
      `wecom-smoke.sh: 返回表 string 字段屏蔽后读取必须 PASS\n${named.stdout}`,
    );
  }
}

// ---- 10b) dingtalk [4] error keys are checked in the envelope only, never in body values (same as section 10) ----
// Full runs of the real script with a fake CLI: a legitimate envelope whose body value is exactly "error"
// must not go red; masking must not swallow the envelope layer's structured error keys. The masked keys are
// the body-carrier keys the bundled docs pin (--format json envelope is {"success": true, "body": {...}}
// per global-reference.md; result/data are envelope keys from the --fields doc and the attendance script;
// content is the same family as section 10; jsonml is the payload key of doc-read.md). The "status":"error"
// value shape must still FAIL — masking covers body-carrier keys only and must not swallow error value
// shapes on non-carrier keys.
{
  const { spawnSync } = require("node:child_process");
  const os = require("node:os");
  if (process.platform !== "win32") {
    const fakeCli = (docPayload) =>
      [
        "#!/usr/bin/env bash",
        'case "$1" in',
        '  --version) echo "dws 1.0.65 (fake)"; exit 0 ;;',
        '  auth) echo \'{"authenticated":true}\'; exit 0 ;;',
        "  doc)",
        '    if [ "${2:-}" = read ]; then printf \'%s\' \'' + docPayload + "'; exit 0; fi",
        "    exit 0 ;;",
        "  *) exit 0 ;;",
        "esac",
        "",
      ].join("\n");
    const runSmoke = (docPayload) => {
      const home = fs.mkdtempSync(path.join(os.tmpdir(), "dws-doc-home-"));
      const bin = path.join(home, "bin");
      fs.mkdirSync(bin, { recursive: true });
      fs.writeFileSync(path.join(bin, "dws"), fakeCli(docPayload));
      fs.chmodSync(path.join(bin, "dws"), 0o755);
      return spawnSync("bash", [path.join(root, "scripts", "dingtalk-smoke.sh")], {
        encoding: "utf8",
        timeout: 120000,
        killSignal: "SIGKILL",
        env: {
          ...process.env,
          HOME: home,
          PATH: bin + path.delimiter + process.env.PATH,
          DINGTALK_TEST_DOC_ID: "DOC123",
        },
      });
    };
    // Legitimate envelope with a body value exactly "error": after masking the body it must read as success,
    // not go red by mistake.
    const good = runSmoke(
      '{"url":"https://doc.example/x","name":"周报","content":"error","version":"1"}',
    );
    assert.strictEqual(
      good.status,
      0,
      `dingtalk-smoke.sh: 正文值为 error 的合法 envelope 不得误红(status=${good.status})\n${good.stdout}`,
    );
    assert.ok(
      good.stdout.includes("[PASS] 读到文档内容"),
      `dingtalk-smoke.sh: 正文屏蔽后读取必须 PASS\n${good.stdout}`,
    );
    // Real envelope error key: masking must not swallow the envelope layer's structured error.
    const bad = runSmoke(
      '{"url":"https://doc.example/x","name":"t","content":"ok","version":"1","error":"invalid node"}',
    );
    assert.notStrictEqual(
      bad.status,
      0,
      `dingtalk-smoke.sh: envelope "error" 键必须 FAIL(屏蔽不得吞掉错误键)\n${bad.stdout}`,
    );
    assert.ok(
      !bad.stdout.includes("[PASS] 读到文档内容"),
      `dingtalk-smoke.sh: envelope error 键不得判读成功\n${bad.stdout}`,
    );
    // Body-carrier key (the documented envelope's body): when the value is exactly "error", a bare grep's
    // "error" pattern is assembled from the value's delimiting quotes (false red); masking must cover the
    // body's string value.
    // (A body containing quotes is JSON-escaped into \"error\", from which the delimiting quotes cannot
    // assemble "error", so it never false-reds — the fixture pins only the genuinely false-red shape.)
    const bodyCarrier = runSmoke(
      '{"success":true,"body":"error","version":"1"}',
    );
    assert.strictEqual(
      bodyCarrier.status,
      0,
      `dingtalk-smoke.sh: body 载体值恰为 error 的合法 envelope 不得误红(status=${bodyCarrier.status})\n${bodyCarrier.stdout}`,
    );
    assert.ok(
      bodyCarrier.stdout.includes("[PASS] 读到文档内容"),
      `dingtalk-smoke.sh: body 载体屏蔽后读取必须 PASS\n${bodyCarrier.stdout}`,
    );
    // Masking covers body-carrier keys only: an error value shape on a non-carrier key ("status":"error")
    // must still FAIL — this is why masking is not widened into "swallow every string value".
    const statusValue = runSmoke(
      '{"success":true,"body":"ok","status":"error"}',
    );
    assert.notStrictEqual(
      statusValue.status,
      0,
      `dingtalk-smoke.sh: "status":"error" 值形态必须 FAIL(屏蔽不得盖住非载体键)\n${statusValue.stdout}`,
    );
    assert.ok(
      !statusValue.stdout.includes("[PASS] 读到文档内容"),
      `dingtalk-smoke.sh: status 值形态错误不得判读成功\n${statusValue.stdout}`,
    );
  }
}

// ---- 11) EXIT cleanup ownership: the caller's directory must not be touched, and the real capture is cleaned by the probe subshell itself ----
// Shape pin: the backstop cleanup trap must hang inside the run function with an empty-value guard. The run
// function is always invoked via $( ), and the DWS_TMP assignment lives only inside the probe subshell —
// cleanup hung on the outer script's EXIT would expand to rm -f "" ".deadline" and delete the caller's
// same-named file (reviewer reproduced; when mktemp fails, DWS_TMP is equally empty inside the subshell and
// the guard blocks the same shape).
{
  for (const [name, source] of [
    ["dingtalk-smoke.sh", dingtalkSmoke],
    ["wecom-smoke.sh", wecomSmoke],
  ]) {
    const lines = source.split("\n");
    const prefix = (source.match(/^([A-Z]+)_TIMEOUT=/m) || [])[1];
    assert.ok(prefix, `${name}: 未找到 *_TIMEOUT= 预备段`);
    const fn = `${prefix.toLowerCase()}_run`;
    const fnStart = lines.findIndex((l) => l.startsWith(`${fn}() {`));
    const fnEnd = lines.indexOf("}", fnStart);
    assert.ok(fnStart >= 0 && fnEnd > fnStart, `${name}: 未找到 ${fn}() 定义`);
    assert.doesNotMatch(
      source,
      /^DWS_TMP=""/m,
      `${name}: 外层脚本不得初始化 DWS_TMP(${fn}() 总经 $() 调用,外层永远看到空值;外层 EXIT 挂清理必展开成 rm -f "" ".deadline" 误删调用方文件)`,
    );
    assert.doesNotMatch(
      source,
      /^trap .*DWS_TMP/m,
      `${name}: 清理 trap 不得挂在外层脚本(清理必须归拥有临时文件的探测子壳)`,
    );
    const trapIdx = lines.findIndex((l) =>
      /^\s*trap 'if \[ -n "\$DWS_TMP" \]; then rm -f "\$DWS_TMP" "\$\{DWS_TMP\}\.deadline" 2>\/dev\/null; fi' EXIT$/.test(l),
    );
    assert.ok(
      trapIdx > fnStart && trapIdx < fnEnd,
      `${name}: 带空值守卫的清理 trap 必须挂在 ${fn}() 内部(实得行号 ${trapIdx + 1} vs 函数 ${fnStart + 1}-${fnEnd + 1})`,
    );
  }
}
// Behavior pin (real scripts + fake CLI, run from a caller directory holding a .deadline sentinel, skipped
// on Windows): on normal exit the sentinel must survive and the real capture must be cleaned. The assertion
// boundary for the interrupt scenario (the reviewer flagged that earlier comments overstated coverage and
// must align with the actual assertions): the fixture signals the script process itself with a positive pid
// (SIGINT first, then SIGTERM to guarantee both bash versions finish), not process-group signals; the
// capture file being on disk acts as the readiness gate before signaling, preventing the fixture from
// landing before the probe starts and passing vacuously; after the interrupt only sentinel survival is
// asserted, not capture cleanup — real Ctrl-C's process-group delivery shape and subshell EXIT cleanup are
// covered by section 6's bounded probes and the trap shape pin, and this fixture does not re-endorse them.
{
  const { spawnSync, spawn } = require("node:child_process");
  const os = require("node:os");
  if (process.platform !== "win32") {
    // spawnSync cannot send process-group signals; use Atomics.wait for synchronous sleeping here (safe on the main thread).
    const sleepSync = (ms) => {
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
    };
    const fakeCli = (name) =>
      [
        "#!/usr/bin/env bash",
        'case "$1" in',
        `  --version) echo "${name} 9.9.9 (fake)"; exit 0 ;;`,
        "  auth)",
        '    if [ "${2:-}" = show ]; then echo "authorized"; else echo \'{"authenticated":true}\'; fi',
        "    exit 0 ;;",
        "  *) exit 0 ;;",
        "esac",
        "",
      ].join("\n");
    const slowCli = (name) =>
      [
        "#!/usr/bin/env bash",
        "trap '' TERM",
        `printf '${name} 9.9.9\\n'`,
        "sleep 300",
        "",
      ].join("\n");
    const mkCase = (tag) => {
      const home = fs.mkdtempSync(path.join(os.tmpdir(), `smoke-${tag}-home-`));
      const bin = path.join(home, "bin");
      fs.mkdirSync(bin, { recursive: true });
      const cap = fs.mkdtempSync(path.join(os.tmpdir(), `smoke-${tag}-cap-`));
      const caller = fs.mkdtempSync(path.join(os.tmpdir(), `smoke-${tag}-caller-`));
      const sentinel = path.join(caller, ".deadline");
      fs.writeFileSync(sentinel, "caller owns this file");
      const env = {
        ...process.env,
        HOME: home,
        PATH: bin + path.delimiter + process.env.PATH,
        TMPDIR: cap,
      };
      return { cap, caller, env, sentinel, bin };
    };
    for (const [name, script] of [
      ["dws", "dingtalk-smoke.sh"],
      ["wecom-cli", "wecom-smoke.sh"],
    ]) {
      // Normal exit: a healthy fake CLI runs the whole script to exit 0; sentinel survives; capture cleaned.
      {
        const { cap, caller, env, sentinel, bin } = mkCase("own-normal");
        fs.writeFileSync(path.join(bin, name), fakeCli(name));
        fs.chmodSync(path.join(bin, name), 0o755);
        const run = spawnSync("bash", [path.join(root, "scripts", script)], {
          encoding: "utf8",
          timeout: 120000,
          killSignal: "SIGKILL",
          cwd: caller,
          env,
        });
        assert.strictEqual(
          run.status,
          0,
          `${script}: 健康假 CLI 全跑必须 exit 0(基线本身挂了则哨兵/清理断言不可信,status=${run.status})\n${run.stdout}\n${run.stderr}`,
        );
        assert.ok(
          fs.existsSync(sentinel),
          `${script}: 正常退出后调用方目录的 .deadline 哨兵必须保留(外层空值 trap 形状会误删)`,
        );
        assert.deepEqual(
          fs.readdirSync(cap),
          [],
          `${script}: 正常退出后真实捕获文件必须已清(TMPDIR 残留=探测子壳清理被破坏)`,
        );
      }
      // Interrupted exit: the slow CLI hangs inside a probe; send a positive-pid SIGINT first. bash ≥5.3 was
      // observed swallowing/deferring the INT during wait and surviving (the script then finishes probe by
      // probe, each probe cleaning up after itself), while 3.2 dies outright — on both versions "the script
      // exits killed by a signal" is guaranteed by the subsequent positive-pid SIGTERM: when the parent shell
      // dies from a signal, the old shape's outer EXIT trap would expand rm -f "" ".deadline" on the spot and
      // delete the sentinel; in the new shape the parent shell has no trap and the sentinel must survive.
      {
        const { cap, caller, env, sentinel, bin } = mkCase("own-int");
        fs.writeFileSync(path.join(bin, name), slowCli(name));
        fs.chmodSync(path.join(bin, name), 0o755);
        const child = spawn("bash", [path.join(root, "scripts", script)], {
          cwd: caller,
          env,
          stdio: ["ignore", "pipe", "pipe"],
        });
        // While the main thread is blocked in sleepSync (Atomics.wait), libuv does not dispatch the exit
        // event: exitCode/signalCode stay null forever, and the dead child is never reaped, leaving a zombie
        // — kill(pid,0) also succeeds against zombies. Liveness can only be asked of the OS: ps's state
        // column (Z = zombie = dead, empty = ESRCH = already gone); neither polling child.exitCode nor
        // looking only at kill(pid,0) is sound.
        const alive = () => {
          try {
            process.kill(child.pid, 0);
          } catch {
            return false;
          }
          const stat = spawnSync("ps", ["-o", "stat=", "-p", String(child.pid)], {
            encoding: "utf8",
          });
          const st = (stat.stdout || "").trim();
          return st !== "" && !st.startsWith("Z");
        };
        sleepSync(1200);
        // Readiness gate: the signal must land only after the probe has written its capture to disk — a fixed
        // sleep can only wait and cannot prove the probe is in flight, and the sentinel assertion would
        // otherwise pass vacuously (the reviewer flagged that the readiness verification the comments claimed
        // did not previously exist).
        const readyT0 = Date.now();
        while (fs.readdirSync(cap).length === 0 && Date.now() - readyT0 < 10000) {
          sleepSync(100);
        }
        assert.ok(
          fs.readdirSync(cap).length > 0,
          `${script}: 信号前探测捕获必须已落盘(否则中断断言空真)`,
        );
        process.kill(child.pid, "SIGINT");
        sleepSync(1000);
        if (alive()) process.kill(child.pid, "SIGTERM");
        const t0 = Date.now();
        while (alive() && Date.now() - t0 < 15000) {
          sleepSync(100);
        }
        if (alive()) {
          process.kill(child.pid, "SIGKILL");
          assert.fail(`${script}: 信号中断后 15s 内脚本进程必须退出`);
        }
        sleepSync(300);
        assert.ok(
          fs.existsSync(sentinel),
          `${script}: 中断退出后调用方目录的 .deadline 哨兵必须保留(评审实测的外层空值 trap 形状会误删)`,
        );
      }
    }
  }
}

console.log("✓ connector first-use online install contract passed");
