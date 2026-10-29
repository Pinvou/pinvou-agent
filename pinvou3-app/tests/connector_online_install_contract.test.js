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
  // 3b) 探测限时必须有第二级强制终止:GNU timeout 缺 -k 或看门狗缺 KILL 升级时,
  //     吞 TERM 的 CLI 会把探测拖成永久等待(源形态钉;行为钉见下方第 6 节)。
  assert.match(
    source,
    /-k "\$\{?(DWS|WECOM)_KILL_GRACE\}?"/,
    `${name}: coreutils timeout 必须带 -k 强杀宽限(吞 TERM 的 CLI 会挂住 timeout 本身)`,
  );
  assert.match(
    source,
    /kill "\$pid" 2>\/dev\/null && sleep "\$\{?(DWS|WECOM)_KILL_GRACE\}?" && kill -9 "\$pid"/,
    `${name}: 看门狗必须在 TERM 后升级 SIGKILL(只发一次 TERM 对抗性 CLI 无界)`,
  );
  assert.match(
    source,
    /\[ "\$rc" -eq 137 \] && rc=124/,
    `${name}: KILL 死(137)必须归一为超时 124,不能伪装成「结果未知」`,
  );
  // 3c) 版本/连接 PASS 以探测退出 0 为前提:健康形状的输出 + 非零退出(实测
  //     exit 7 曾全绿)不构成通过;超时 124 亦被同一前提拦截。
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
  // 3d) redact 字段形态必须含数字后缀键(accessToken2 形态曾裸过)。
  assert.match(
    source,
    /\[A-Za-z0-9_-\]\*\[Tt\]\[Oo\]\[Kk\]\[Ee\]\[Nn\]\[A-Za-z0-9_-\]\*/,
    `${name}: redact 字段形态必须接受数字后缀键`,
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
    // authorization 单独成标记(前两条 bearer 形态顺带命中它,这条钉裸 authorization);
    // 数字后缀键钉字段形态的 0-9 字符类(accessToken2 曾整段裸过)。
    ["x-authorization: SECRET7", "[已脱敏"],
    ['{"accessToken2": "SECRET8"}', "token=[REDACTED]"],
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

// ---- 6) 探测限时对吞 TERM 的 CLI 必须有界 ----
// GNU timeout 缺 -k:子进程无视 TERM 后 timeout 自身无限等待;看门狗只发一次
// TERM:wait 同样无界。现场抽取两份脚本的 run 函数真跑 TERM 抗性子进程:两条
// 路径都必须在 时限+杀宽限 内以 124 收场,且强杀前的输出保留(超时诊断可用)。
// spawnSync 的 timeout+SIGKILL 只是回归兜底——修回归后此测试自身数秒内结束。
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
      const runBounded = (label, bin) => {
        const t0 = Date.now();
        const res = spawnSync("bash", ["-c", harness(stubborn, bin)], {
          encoding: "utf8",
          timeout: 20000,
          killSignal: "SIGKILL",
        });
        const elapsed = Date.now() - t0;
        assert.strictEqual(
          res.status,
          124,
          `${label}: 吞 TERM 的子进程必须以超时 124 收场(status=${res.status} signal=${res.signal} err=${res.error})`,
        );
        assert.ok(
          elapsed < 15000,
          `${label}: 探测必须在时限+杀宽限内有界返回,实得 ${elapsed}ms(无界等待即回归)`,
        );
        assert.ok(
          res.stdout.includes("stubborn 9.9.9"),
          `${label}: 强杀前已产出的探测输出必须保留(超时诊断要能看到现场)`,
        );
      };
      if (hasTimeout) runBounded(`${name} coreutils timeout 分支`, timeoutBin);
      runBounded(`${name} 看门狗分支`, "");
      const res = spawnSync("bash", ["-c", harness(quick, timeoutBin)], {
        encoding: "utf8",
        timeout: 20000,
        killSignal: "SIGKILL",
      });
      assert.strictEqual(res.status, 0, `${name}: 健康子进程不得被误杀(status=${res.status})`);
      assert.ok(res.stdout.includes("quick 1.2.3"), `${name}: 健康探测输出必须完整回放`);
    }
  }
}

// ---- 7) 探测失败(非零退出)不得发 PASS ----
// 健康形状的输出 + exit 7 曾让两份脚本全绿退出 0(评审以假 CLI 实测复现)。
// 真脚本 + 假 CLI 全跑:版本/连接两处都必须 FAIL,整体非 0 退出,且不得落入
// 「全部域探测跳过」的 exit 3 掩护路径(假 CLI 域探测全 exit 0)。
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

// ---- 8) 解析/比较/发现管道行为钉(形状钉不执行,变异不咬)----
// seen 锚被删时「更新提示在前行」的输入会读进提示号;比较管道 head→tail 或丢
// -k 数值键时下限判定翻转;发现排序丢键时 1.0.9 会压过 1.0.64。全部现场 eval 真跑。
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
        `${name}: 更新提示行(数字在前)不得被读成本机版本——删 seen 锚此处即红`,
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

// ---- 9) wecom [3] 分支序:超时(124)先于权限词 ----
// 超时探测的部分输出可能恰含「权限」——先查词会把超时误标成「域未授权」;
// 两者都是 SKIP,但超时要求的是重跑,标注不能误导。
{
  const lines = wecomSmoke.split("\n");
  const timeoutIdx = lines.findIndex((l) => l.includes('elif [ "$d_rc" -eq 124 ]'));
  const permIdx = lines.findIndex((l) => l.includes("'权限|暂不支持|未授权'"));
  assert.ok(timeoutIdx >= 0, "wecom-smoke.sh: 未找到 [3] 超时分支");
  assert.ok(permIdx > timeoutIdx, "wecom-smoke.sh: [3] 超时分支必须先于权限词分支");
}

console.log("✓ connector first-use online install contract passed");
