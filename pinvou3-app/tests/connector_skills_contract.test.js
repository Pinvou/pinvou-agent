// 连接器技能包口径契约：把历次审查（2026-08-16 六轮）确认的品悟适配规则固化为
// CI 门禁，防止下次上游 sync 时机械迁移把已修复的问题带回来。
// 规则来源见各 NOTICE 的「本地修改登记」；上游历史登记（NOTICE 文件本身）豁免扫描。
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "..");
const bundle = (...p) => path.join(root, "src-tauri", "resources", "common", "bundle", ...p);
const packs = ["skills", "wecom-skills", "dingtalk-skills", "tmeet-skills"];

function walk(dir, out = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full, out);
    else out.push(full);
  }
  return out;
}

const files = packs.flatMap((pack) => walk(bundle(pack)));
const docs = files.filter(
  (f) => f.endsWith(".md") && !path.basename(f).startsWith("NOTICE"),
);
const read = (f) => fs.readFileSync(f, "utf8");
const rel = (f) => path.relative(root, f);

// 1) 已删除的 CLI 形态不得回潮（--api-version v2 在 lark-cli 1.0.87 移除等）
for (const f of docs) {
  const text = read(f);
  assert.ok(!/--api-version/.test(text), `${rel(f)}: 残留 --api-version`);
  for (const gone of [
    // 注意：sheets +read/+find 是 lark-cli 1.0.87 的隐藏别名（→ +cells-get/+cells-search），
    // 真实存在，不得列入黑名单；whiteboard +query 才是已删除命令。
    "whiteboard +query",
    "skills/multi/",
    "unsupported-scripts",
    "channel-login",
    "recovery-guide",
    "lark-calendar-agenda.md",
    "lark-calendar-freebusy.md",
    "comments-guide",
    "core-operations",
  ]) {
    assert.ok(!text.includes(gone), `${rel(f)}: 引用已删除对象 ${gone}`);
  }
}

// 2) 引擎工具名唯一口径 + 自更新禁令
for (const f of docs) {
  const text = read(f);
  assert.ok(!/Read 工具/.test(text), `${rel(f)}: 残留「Read 工具」`);
  assert.ok(!/\bread_file\b/.test(text), `${rel(f)}: 残留 read_file`);
  for (const line of text.split("\n")) {
    if (line.includes("lark-cli update")) {
      assert.ok(
        line.includes("不要") || line.includes("勿"),
        `${rel(f)}: lark-cli update 出现在非禁止语境: ${line.trim()}`,
      );
    }
  }
}

// 3) 安装/升级一律由品悟宿主代管
for (const f of docs) {
  const text = read(f);
  assert.ok(
    !/\bnpm\s+(?:install|i)\s+(?:-g|--global)\b/.test(text),
    `${rel(f)}: 残留 npm 全局安装教学（npm install -g / npm i -g / --global 均禁止）`,
  );
  assert.ok(!/\bnpx\s+\S*skills\b/.test(text), `${rel(f)}: 残留 npx skills 教学（含 scoped 形态）`);
  // dws 脚本示例统一 python3：宿主环境无裸 `python` 命令（macOS/Homebrew/Win embeddable 均只装 python3）
  assert.ok(!/\bpython\s+(?!3)\S*\.py/.test(text), `${rel(f)}: 脚本调用用裸 python（应为 python3）`);
}

// 4) 上游宿主断言（Hermes/OpenClaw，含小写形态）必须以品悟为锚
for (const f of docs) {
  for (const line of read(f).split("\n")) {
    if (/(hermes|openclaw(?!_workspace))/i.test(line)) {
      // dws dev 渠道枚举（opencode/claudecode/.../hermes/openclaw/custom）是真实
      // CLI 渠道值而非宿主断言，豁免含「渠道」的行。
      if (line.includes("渠道")) continue;
      assert.ok(
        line.includes("品悟"),
        `${rel(f)}: 上游宿主断言未锚定品悟语境: ${line.trim()}`,
      );
    }
  }
}

// 5) lark 域不得引导裸 auth login（按需授权走 --scope/--domain；行首 `|` 的表格行为描述性语境，豁免）
for (const f of docs.filter((f) => path.relative(bundle("skills"), f).startsWith("lark-"))) {
  for (const line of read(f).split("\n")) {
    if (/^\s*\|/.test(line)) continue;
    if (/auth login/.test(line) && !/logout|--scope|--domain|--device-code|--no-wait|--recommend|\bstatus\b|不要|无需|不必|禁止|按需|规则|授权，|等\s/.test(line)) {
      assert.fail(`${rel(f)}: lark 域裸 auth login: ${line.trim()}`);
    }
  }
}

// 6) frontmatter 契约：连接器技能 description ≤280、「何时用」开头、bins 正确
const binsByPack = {
  "skills/lark-": "lark-cli",
  "wecom-skills/wecomcli-": "wecom-cli",
  "dingtalk-skills/dws": "dws",
  "tmeet-skills/tmeet-skill": "tmeet",
};
for (const f of files.filter((f) => path.basename(f) === "SKILL.md")) {
  const relPath = path.relative(bundle(), f);
  const match = Object.keys(binsByPack).find((prefix) => relPath.startsWith(prefix));
  if (!match) continue; // visual-design 等本地技能不适用
  const text = read(f);
  const descLine = text.split("\n").find((l) => l.startsWith("description:"));
  const desc = descLine?.replace(/^description:\s*/, "").replace(/^["']|["']$/g, "");
  assert.ok(desc, `${rel(f)}: 缺 description`);
  assert.ok(desc.length <= 280, `${rel(f)}: description ${desc.length} > 280（引擎截断上限）`);
  assert.ok(/^(【)?何时用[:：]/.test(desc), `${rel(f)}: description 未以「何时用」开头（防误用契约）`);
  assert.ok(
    text.includes(`bins: ["${binsByPack[match]}"]`),
    `${rel(f)}: requires.bins 应为 ["${binsByPack[match]}"]`,
  );
  const name = text.match(/name:\s*(\S+)/)?.[1];
  assert.equal(name, path.basename(path.dirname(f)), `${rel(f)}: name 与目录名不一致`);
}

// 7) 语义扫描豁免登记：以上规则若在上游 sync 后出现合理新豁免，必须在本清单登记文件+理由
const EXEMPT_FILES = [
  // dws scripts 的 OPENCLAW_WORKSPACE 为路径护栏 env（未设时回退 cwd），非宿主断言
  // 历史审查记录（NOTICE*.md）整体豁免由 docs 过滤实现
];

console.log("✓ connector skills pinvou-contract lint passed");
