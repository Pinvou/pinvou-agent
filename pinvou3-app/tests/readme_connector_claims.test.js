// README 连接器宣示 ↔ 工具目录锚点。README 具名的每个连接器必须映射到一个
// 实装（非占位）的 backendId 卡片。方向是单向 containment:README 可以少写
// (高亮清单以 "and more" 收尾),不许多写——历史上 "enterprise knowledge
// bases" 指向的乐享卡就是 backendId: null 的占位卡,本测试防复发。
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "..");
const repoRoot = path.resolve(root, "..");
const toolCommon = fs.readFileSync(
  path.join(root, "src", "features", "tools", "tool-common.jsx"),
  "utf8",
);

// 每行一个连接器:[拉丁字母名(en/ja 共用), 中文名, tool-common.jsx backendId 锚点]。
// 连接器下线/降级为占位卡时锚点消失,本测试转红,README 高亮清单必须同步改写;
// 新增高亮连接器时在此登记一行(三个 README 自动同查),并补锚点。
const connectors = [
  ["Feishu (Lark)", "飞书", "backendId: 'feishu'"],
  ["DingTalk", "钉钉", "backendId: 'dingtalk'"],
  ["WeCom", "企业微信", "backendId: 'wecom'"],
  ["Tencent Meeting", "腾讯会议", "backendId: 'tmeet'"],
  ["Tencent Docs", "腾讯文档", "backendId: 'tencent-docs'"],
  ["Tencent ima", "腾讯 ima", "backendId: 'ima'"],
  ["Obsidian", "Obsidian", "backendId: 'obsidian'"],
  ["QCC", "企查查", "backendId: 'qcc'"],
  ["PatSnap", "智慧芽", "backendId: 'patsnap-search'"],
  ["Canva", "Canva", "backendId: 'canva-mcp'"],
  ["AMap Weather", "高德天气", "backendId: 'weather'"],
];
const claims = [
  { file: "README.md", zh: false },
  { file: "README.zh-CN.md", zh: true },
  { file: "README.ja.md", zh: false },
];

// 占位卡 title/subtitle + 历史超宣示短语 = 高亮行禁词(见下方循环注释)。
// 解析器只认无嵌套的扁平单引号对象;下限不写死数字,而是按 tool-common.jsx 里
// `backendId: null` 的出现次数推算(每张占位卡 title+subtitle 两条文案,允许一处
// 注释提及):新增一张解析器认不出的占位卡(嵌套 configFields / 双引号)时,出现
// 次数涨而抽取数不涨 → 红,而不是静默漏掉它的文案(写死 >= 8 只能发现「少了」)。
const placeholderCards = [...toolCommon.matchAll(/\{[^{}]*backendId: null[^{}]*\}/g)];
const placeholderTexts = placeholderCards.flatMap((m) => {
  const title = m[0].match(/title: '([^']+)'/);
  const subtitle = m[0].match(/subtitle: '([^']+)'/);
  return [title?.[1], subtitle?.[1]].filter(Boolean);
});
const nullMentions = (toolCommon.match(/backendId:\s*null/g) || []).length;
assert.ok(nullMentions >= 2, `tool-common.jsx 未见 backendId: null(实得 ${nullMentions})——占位卡表示法变更须同步本测试`);
assert.ok(
  placeholderTexts.length >= 2 * (nullMentions - 1),
  `tool-common.jsx 占位卡抽取不全(backendId: null 出现 ${nullMentions} 次,应抽到 ≥ ${2 * (nullMentions - 1)} 条 title/subtitle,实得 ${placeholderTexts.length})——新占位卡须保持扁平单引号写法,或同步本测试的抽取器`,
);
// 历史超宣示短语按三份 README 各自语言登记(ja 的「企業ナレッジベース」与 zh 的
// 「企业知识库」不是同一字符串,少登一种语言该语言就能静默复发)。
const forbiddenPhrases = [
  ...placeholderTexts,
  "enterprise knowledge bases",
  "企业知识库",
  "企業ナレッジベース",
];

for (const { file, zh } of claims) {
  const text = fs.readFileSync(path.join(repoRoot, file), "utf8");
  const names = connectors.map(([en, cn]) => (zh ? cn : en));
  // 高亮行 = 同时含首名与末名的行(顺序不钉,允许重排);名字必须落在这行上——
  // 整文件 includes 会在名字被挪去文件其他位置时静默放行(误绿,已实证过)。
  const highlight = text.split("\n").find(
    (l) => l.includes(names[0]) && l.includes(names[names.length - 1]),
  );
  assert.ok(
    highlight,
    `${file}: 找不到连接器高亮行(应同时含「${names[0]}」与「${names[names.length - 1]}」)——清单被重写/拆行时,须连同 tests/readme_connector_claims.test.js 一起更新`,
  );
  for (const name of names) {
    assert.ok(
      highlight.includes(name),
      `${file}: 连接器「${name}」已不在 README 高亮清单 —— 清单与 tests/readme_connector_claims.test.js 须一起更新`,
    );
  }
  // 防复发是双向的:上方 containment 只锁「已登记名字被挪走/降级」,历史上
  // "enterprise knowledge bases" 是往行里加未登记宣示指向占位卡——加的方向
  // 也必须红。占位卡(backendId: null,均为无嵌套的扁平对象)从 tool-common.jsx
  // 现场抽取,新增占位卡自动纳入禁词(抽取器认不出的写法由上方次数推算转红,不静默
  // 漏掉);真有同名连接器上线时先在 connectors 表登记并同步放宽本禁词,不得静默。
  for (const phrase of forbiddenPhrases) {
    assert.ok(
      !highlight.includes(phrase),
      `${file}: 高亮行出现占位卡宣示「${phrase}」——README 不得宣示未上线能力;同名连接器真上线时先在 connectors 表登记并放宽本禁词`,
    );
  }
}
for (const [, , anchor] of connectors) {
  assert.ok(
    toolCommon.includes(anchor),
    `tool-common.jsx: ${anchor} 不存在 —— README 具名的连接器已不是实装卡片(占位/下线),README 不得宣示未上线能力`,
  );
}

console.log("✓ readme connector claims anchored");
