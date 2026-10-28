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
// 连接器 CLI 解析实现已收敛到 posix.rs（Wave 3 去重），linux/macos 仅 re-export。
assert.match(posix, /bundled_connector_npm_cli/);
assert.match(posix, /cli_bin == "tmeet"/);
assert.match(posix, /bundled_connector_node/);
for (const platformSource of [linux, macos]) {
  assert.match(platformSource, /connector_cli_command/);
}

// 版本联动：工具卡展示版本必须与 lock 钉扎（及 tmeet.rs npm 钉扎）一致，
// 防止再次出现卡片版本与实际安装版本脱节（如历史上的 v1.0.56 vs lock 1.0.65）。
// 5 份平台 lock 全量校验（NOTICE 承诺「5 份任一即可，版本字段一致」由此背书）：
// 只改其中一份的版本而漏改其余（或漏改卡片）都会在此失败。
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
// 逐段比较而非加权 1*10000+…:加权在段 ≥100 时给出错误答案(锁钉 1.0.164 vs
// 下限 1.1.0 → 10100 ≤ 10164 误过,而语义上下限已高于锁)。
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

// ---- 两份冒烟脚本自身的契约(floor 之外的行为钉) ----
// 1) 版本比较必须用逐段数值排序键:丢掉 -k 键退化为字典序会把 1.0.9 排到
//    1.0.64 之后,低于下限的 CLI 误 PASS,而 floor 字面量钉全绿。
for (const [name, source] of [
  ["dingtalk-smoke.sh", dingtalkSmoke],
  ["wecom-smoke.sh", wecomSmoke],
]) {
  assert.match(
    source,
    /sort -t\. -k1,1n -k2,2n -k3,3n/,
    `${name}: 版本比较必须保留逐段数值排序键(字典序会让 1.0.9 >= 1.0.64)`,
  );
  // 2) 版本号解析必须锚定含 CLI 名的行并取名字之后的点分数字:同族 lark-cli
  //    实测在版本行之前打「a new version 1.0.96 is available」(feishu.rs 钉有
  //    该夹具),「全文首个点分数字」会把更新号/代理 IP 读成本机版本误 PASS。
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
  // 3) 所有 CLI 输出回显(含 PASS 与 SKIP 行)都要过 redact:直接钉 [1] PASS 回显
  //    与 [3] SKIP 回显的管道形态(FAIL 路径本就过 redact,按出现次数钉会被
  //    FAIL 路径凑数而漏掉 PASS 路径——经变异验证)。
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
}
// 4) wecom 冒烟必须扫应用自装布局:只搜 PATH 会漏掉 ~/.pinvou3/assets/cli/wecom-cli/<ver>/
//    (native_installer 落盘处),并把关不上的报错引向 npm -g 自装(NOTICE-wecom
//    登记的哈希循环路径)。
assert.match(
  wecomSmoke,
  /for d in "\$HOME"\/\.pinvou3\/assets\/cli\/wecom-cli\/\*\/; do/,
  "wecom-smoke.sh 必须扫描应用安装布局 ~/.pinvou3/assets/cli/wecom-cli/<version>/(钉循环本身,注释提及不算)",
);
assert.doesNotMatch(
  wecomSmoke,
  /npm i -g/,
  "wecom-smoke.sh 找不到 CLI 时不得再建议 npm -g 自装(绕过 lock 钉扎)",
);

// 5) redact() 行为钉(从两份脚本源码现场抽取函数体真跑):host safe_auth_log_line
//    四标记整行打码 + 引号 JSON 字段打码。删掉 awk 阶段/sed 阶段或换标记词,
//    这里先红——floor 钉不覆盖脱敏。
{
  const { execFileSync } = require("node:child_process");
  const extractRedact = (source, name) => {
    const lines = source.split("\n");
    const start = lines.findIndex((l) => l.startsWith("redact(){"));
    assert.ok(start >= 0, `${name}: 未找到 redact(){ 定义`);
    // 定义共 3 行(redact(){ … awk … sed …)。
    const defn = lines.slice(start, start + 3).join("\n");
    assert.match(defn, /access_token\|refresh_token\|authorization\|bearer/, `${name}: redact 必须覆盖 host 四标记`);
    return defn;
  };
  // 每条形态都断言期望的打码标记,而不只断言 SECRET 不在输出里:只传函数定义不调用时
  // bash 不读 stdin、输出为空,"不含 SECRET" 会空真(CI 以 spawnSync EPIPE 暴露)。
  const leakForms = [
    ["access_token=SECRET1", "[已脱敏"],
    ['refresh_token: "SECRET2"', "[已脱敏"],
    ["https://x.example/api?access_token=SECRET3&v=2", "[已脱敏"],
    ["AUTHORIZATION: BEARER SECRET4", "[已脱敏"],
    ["x-auth: bearer SECRET5", "[已脱敏"],
    ['{"token": "SECRET6"}', "token=[REDACTED]"],
  ];
  for (const [name, source] of [
    ["dingtalk-smoke.sh", dingtalkSmoke],
    ["wecom-smoke.sh", wecomSmoke],
  ]) {
    const defn = extractRedact(source, name);
    if (process.platform === "win32") continue; // bash 不在 PATH 的平台跳过行为段
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
  }
}

console.log("✓ connector first-use online install contract passed");
