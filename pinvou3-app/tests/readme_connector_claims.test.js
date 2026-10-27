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

// 连接器下线/降级为占位卡时锚点消失,本测试转红,README 高亮清单必须同步改写;
// 新增高亮连接器时在三个语言块各登记一行并补锚点。
const claims = [
  {
    file: "README.md",
    named: [
      ["Feishu (Lark)", "backendId: 'feishu'"],
      ["DingTalk", "backendId: 'dingtalk'"],
      ["WeCom", "backendId: 'wecom'"],
      ["Tencent Meeting", "backendId: 'tmeet'"],
      ["Tencent Docs", "backendId: 'tencent-docs'"],
      ["Tencent ima", "backendId: 'ima'"],
      ["Obsidian", "backendId: 'obsidian'"],
      ["QCC", "backendId: 'qcc'"],
      ["PatSnap", "backendId: 'patsnap-search'"],
      ["Canva", "backendId: 'canva-mcp'"],
      ["AMap Weather", "backendId: 'weather'"],
    ],
  },
  {
    file: "README.zh-CN.md",
    named: [
      ["飞书", "backendId: 'feishu'"],
      ["钉钉", "backendId: 'dingtalk'"],
      ["企业微信", "backendId: 'wecom'"],
      ["腾讯会议", "backendId: 'tmeet'"],
      ["腾讯文档", "backendId: 'tencent-docs'"],
      ["腾讯 ima", "backendId: 'ima'"],
      ["Obsidian", "backendId: 'obsidian'"],
      ["企查查", "backendId: 'qcc'"],
      ["智慧芽", "backendId: 'patsnap-search'"],
      ["Canva", "backendId: 'canva-mcp'"],
      ["高德天气", "backendId: 'weather'"],
    ],
  },
  {
    file: "README.ja.md",
    named: [
      ["Feishu (Lark)", "backendId: 'feishu'"],
      ["DingTalk", "backendId: 'dingtalk'"],
      ["WeCom", "backendId: 'wecom'"],
      ["Tencent Meeting", "backendId: 'tmeet'"],
      ["Tencent Docs", "backendId: 'tencent-docs'"],
      ["Tencent ima", "backendId: 'ima'"],
      ["Obsidian", "backendId: 'obsidian'"],
      ["QCC", "backendId: 'qcc'"],
      ["PatSnap", "backendId: 'patsnap-search'"],
      ["Canva", "backendId: 'canva-mcp'"],
      ["AMap Weather", "backendId: 'weather'"],
    ],
  },
];

for (const { file, named } of claims) {
  const text = fs.readFileSync(path.join(repoRoot, file), "utf8");
  for (const [name, anchor] of named) {
    assert.ok(
      text.includes(name),
      `${file}: 连接器「${name}」已不在 README 高亮清单 —— 清单与 tests/readme_connector_claims.test.js 须一起更新`,
    );
    assert.ok(
      toolCommon.includes(anchor),
      `tool-common.jsx: ${anchor} 不存在 —— ${file} 具名的「${name}」已不是实装卡片(占位/下线),README 不得宣示未上线能力`,
    );
  }
}

console.log("✓ readme connector claims anchored");
