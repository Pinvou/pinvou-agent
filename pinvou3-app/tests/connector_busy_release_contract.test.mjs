import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// 连接器完成/失败事件是异步送达的：晚到的 done/error 不许无条件清掉共享
// busyId（此时槽位可能已易主），否则正在跑的安装/卸载/导入被提前放闸、可被
// 重复触发。锁定两条不变量：
// ① 代码行里禁止裸 `setBusyId(null)`——React 更新队列里裸 null 无论先后都会
//   覆盖功能性归属释放，所有清空必须走 releaseBusy；
// ② 每个操作入口以 busyRef 闸门拒绝重叠启动，防止 setBusyId(id) 抢占他人槽位。
// 注意：这是针对性回归守卫。禁令扫描按 i18n_no_zh_literals.test.mjs 同款手法
// 剥除行注释、只看代码行（本文件的反模式说明注释里就写着裸清空的字面量）；
// 正向钉住用原始源码。
const here = path.dirname(fileURLToPath(import.meta.url));
const toolStoreSource = fs.readFileSync(
  path.join(here, "..", "src", "features", "tools", "ToolStoreView.jsx"),
  "utf8",
);

// 去掉行内尾随 `// ...` 注释，但保留引号字符串内的 `//`（例如 URL 'https://...'）。
// 与 i18n_no_zh_literals.test.mjs 的 stripTrailingLineComment 同一实现。
function stripTrailingLineComment(line) {
  let quote = null;
  for (let i = 0; i < line.length; i++) {
    const ch = line[i];
    if (quote) {
      if (ch === "\\") { i += 1; continue; }
      if (ch === quote) quote = null;
    } else if (ch === "'" || ch === '"' || ch === "`") {
      quote = ch;
    } else if (ch === "/" && line[i + 1] === "/") {
      return line.slice(0, i);
    }
  }
  return line;
}

const toolStoreCode = toolStoreSource.split("\n")
  .filter((line) => { const t = line.trimStart(); return !t.startsWith("//") && !t.startsWith("*"); })
  .map(stripTrailingLineComment)
  .join("\n");

assert.match(
  toolStoreSource,
  /const releaseBusy = \(busyId, toolId\) => \(busyId === toolId \? null : busyId\);/,
  "releaseBusy helper must exist and only clear its own slot",
);

// 不变量①：裸清空一律禁止（连接器订阅、各 try/finally）。新增释放点必须写
// `setBusyId((current) => releaseBusy(current, <id>))`。
assert.doesNotMatch(
  toolStoreCode,
  /setBusyId\(null\)/,
  "bare setBusyId(null) is banned: every release must go through the releaseBusy ownership check",
);

// 连接器四件套（connect 失败 / resetFlow / disconnect finally）按 cfg.key 释放。
const cfgKeyReleases = toolStoreSource.match(
  /setBusyId\(\(current\) => releaseBusy\(current, cfg\.key\)\)/g,
);
assert.ok(
  cfgKeyReleases && cfgKeyReleases.length >= 3,
  `connector quartet must release via releaseBusy(current, cfg.key); found ${cfgKeyReleases ? cfgKeyReleases.length : 0}`,
);

// 订阅回调（done/error）按 toolId 释放。
assert.match(
  toolStoreSource,
  /if \(ph === 'done'\) \{\s*\n\s*setBusyId\(\(current\) => releaseBusy\(current, toolId\)\)/,
  "late done event must release only its own busy slot",
);
assert.match(
  toolStoreSource,
  /else if \(ph === 'error'\) \{\s*\n\s*setBusyId\(\(current\) => releaseBusy\(current, toolId\)\)/,
  "late error event must release only its own busy slot",
);

// wecom 连接编排已完全并入工厂管线（组件级遗留监听器与独立 QR state 已删除）：
// 其全部释放点走通用路径——工厂四件套按 cfg.key、订阅回调按 toolId。这里反向钉住
// 组件级字面量 'wecom' 释放点不得再出现（历史上它曾与工厂管线并存且漏改过一次），
// 防止未来有人绕开通用路径新增第三条 wecom 释放管线。
assert.doesNotMatch(
  toolStoreCode,
  /setBusyId\(\(current\) => releaseBusy\(current, 'wecom'\)\)/,
  "wecom releases must go through the factory (cfg.key) and subscription (toolId) paths only; no component-level literal-id release points",
);

// 不变量②：flowDeps 把 busyRef 送进连接器四件套，connect/disconnect 占槽前
// 拒绝重叠启动（流程卡的 Retry 按钮复用 connect，同获保护）。
assert.match(
  toolStoreSource,
  /const flowDeps = \{ setBusyId, busyRef, storeCopy, detailCopy, loadBackendState, setAlert \};/,
  "flowDeps must thread busyRef into the connector flow factory",
);
const startGuards = toolStoreSource.match(/if \(busyRef\.current\) return;/g);
assert.ok(
  startGuards && startGuards.length >= 3,
  `connector connect/disconnect (+handleAction connector/uninstall branches) must refuse to start while busy; found ${startGuards ? startGuards.length : 0}`,
);

// 无守卫历史重灾区：五个直接启动的操作入口必须在能力闸门的同一行里拒绝忙碌期
// 启动（安装 / 技能安装卸载 / zip 导入 / ima 连接 / ima 断开）。
const entryGuards = toolStoreSource.match(/if \(!canMutateToolStore \|\| busyRef\.current\) return;/g);
assert.ok(
  entryGuards && entryGuards.length >= 5,
  `doInstall/handleSkillAction/doImportSkillZip/connectIma/disconnectIma must refuse to start while busy; found ${entryGuards ? entryGuards.length : 0}`,
);

console.log("connector busy release contract: ok");
