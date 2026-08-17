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
  (f) => path.basename(f).toLowerCase().endsWith(".md") && !path.basename(f).startsWith("NOTICE"),
);
const read = (f) => fs.readFileSync(f, "utf8");
const rel = (f) => path.relative(root, f);
// 规则 1/4 判定前剥掉 URL 与行内代码段:黑名单/宿主断言是子串匹配,上游正文
// 在 URL(如 …/recovery-guide)或代码标识符中合法出现这些词不是违例。
const stripAnchors = (line) =>
  line.replace(/https?:\/\/\S+/g, "").replace(/`[^`]*`/g, "");

// 1) 已删除的 CLI 形态不得回潮（--api-version v2 在 lark-cli 1.0.87 移除等）
// 叙述性提及豁免：sync 后文档说明「历史版本曾有 X，已移除」属合规内容，
// 不是引用违例；真实引用不会自带移除语境。
const removedCtx = /已移除|已删除|已删[，，,).。]|不再提供|不再收录|不随包分发/;
for (const f of docs) {
  const text = read(f);
  // 跨行拆分形态（行尾 `--api-` 换行接 `version`）与全角连字符变体一并覆盖
  const joined = text.replace(/-\r?\n\s*/g, "");
  assert.ok(!/--api[-－]?\s*version/.test(joined), `${rel(f)}: 残留 --api-version`);
  const proseLines = text.split("\n").map(stripAnchors);
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
    const hit = proseLines.find((l) => l.includes(gone) && !removedCtx.test(l));
    assert.ok(!hit, `${rel(f)}: 引用已删除对象 ${gone}: ${hit?.trim()}`);
  }
}
// 已删除文件的“本体复发”不可见（文本引用黑名单只防引用）：机械 sync 恢复整个
// 目录时，被删文件可能以不自指内容回归，直接断言路径不存在。
const removedFiles = [
  ["dingtalk-skills", "dws", "references", "channel-login.md"],
  ["skills", "lark-calendar", "references", "lark-calendar-agenda.md"],
  ["skills", "lark-calendar", "references", "lark-calendar-freebusy.md"],
];
for (const parts of removedFiles) {
  assert.ok(!fs.existsSync(bundle(...parts)), `${parts.join("/")}: 已删除文件复发（见 NOTICE 登记）`);
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
    !/\bnpm\s+(?:-g\s+|--global\s+)?(?:install|i)\s+(?:-g\s+|--global\s+)?\S+/.test(text) &&
      !/\bnpm\s+(?:install|i)\s+[^-\n]*@latest\b/.test(text) &&
      !/\b@[a-z0-9-]+\/[a-z0-9.-]+@latest\b/.test(text),
    `${rel(f)}: 残留 npm 安装教学（-g/--global/@latest 均禁止，安装由品悟代管）`,
  );
  assert.ok(!/\bnpx\s+(?:\S*skills\b|(?:@[\w.-]+\/)?[\w.-]+\s+skills\b)/.test(text), `${rel(f)}: 残留 npx skills 教学（含路径与 scoped 子命令形态）`);
  // dws 脚本示例统一 python3：宿主环境无裸 `python` 命令（macOS/Homebrew/Win embeddable 均只装 python3）
  assert.ok(!/\bpython\s+(?!3\b)(?:-\w+\s+)*\S*\.py/.test(text), `${rel(f)}: 脚本调用用裸 python（应为 python3）`);
}

// 4) 上游宿主断言（Hermes/OpenClaw，含小写形态）必须以品悟为锚。
// 判定前剥掉 URL 与行内代码段：宿主词仅出现在链接/代码标识符（如
// https://…/hermes-setup、`hermes_config_path`）时是客观引用而非宿主断言。
for (const f of docs) {
  for (const line of read(f).split("\n")) {
    if (/(hermes|openclaw(?!_workspace))/i.test(line)) {
      // dws dev 渠道枚举行是真实 CLI 渠道值而非宿主断言（如
      // “# 明确指定渠道（opencode/.../hermes/openclaw/custom）”、
      // “`hermes`/`openclaw` 渠道走官方建联”）——按枚举语境而非「渠道」二字豁免。
      if (/[（(][^）)]*hermes[^）)]*openclaw[^）)]*[）)]/i.test(line) || /hermes`?\/`?openclaw`?\s*渠道/.test(line)) continue;
      const hostInProse = /(hermes|openclaw(?!_workspace))/i.test(stripAnchors(line));
      assert.ok(
        !hostInProse || line.includes("品悟"),
        `${rel(f)}: 上游宿主断言未锚定品悟语境: ${line.trim()}`,
      );
    }
  }
}

// 5) lark 域不得引导裸 auth login（按需授权走 --scope/--domain；行首 `|` 的表格行为描述性语境，豁免）
for (const f of docs.filter((f) => path.relative(bundle("skills"), f).startsWith("lark-"))) {
  for (const line of read(f).split("\n")) {
    if (/^\s*\|/.test(line)) continue;
    if (/auth login/.test(line) && !/logout|\bscope\b|--domain|--device-code|--no-wait|--recommend|\bstatus\b|不要|无需|不必|禁止|按需|规则/.test(line)) {
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

// 7) 语义扫描豁免登记：以上规则若在上游 sync 后出现合理新豁免，必须在本清单登记文件+理由。
// EXEMPT_FILES 当前为空：OPENCLAW_WORKSPACE 为 dws scripts 的路径护栏 env（未设时回退
// cwd），非宿主断言（负向断言见上）；历史审查记录（NOTICE*.md）整体豁免由 docs 过滤实现。
const EXEMPT_FILES = [];
assert.deepEqual(EXEMPT_FILES, [], "新增豁免须在此登记文件与理由，不得静默扩权");

console.log("✓ connector skills pinvou-contract lint passed");
