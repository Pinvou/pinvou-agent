import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// 连接器完成/失败事件是异步送达的：晚到的 done/error 不许无条件清掉共享
// busyId（此时槽位可能已易主），否则正在跑的安装/卸载/导入被提前放闸、可被
// 重复触发。锁定「只释放自己槽位」的功能性更新写法。
const here = path.dirname(fileURLToPath(import.meta.url));
const toolStoreSource = fs.readFileSync(
  path.join(here, "..", "src", "features", "tools", "ToolStoreView.jsx"),
  "utf8",
);

assert.match(
  toolStoreSource,
  /const releaseBusy = \(busyId, toolId\) => \(busyId === toolId \? null : busyId\);/,
  "releaseBusy helper must exist and only clear its own slot",
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

// 连接器订阅回调里不允许再出现无条件清空。
const subscription = toolStoreSource.slice(
  toolStoreSource.indexOf("const useConnectorFlowSubscription"),
  toolStoreSource.indexOf("}, [enabled]);"),
);
assert.doesNotMatch(
  subscription,
  /setBusyId\(null\)/,
  "connector flow subscription must not clear the shared busy slot unconditionally",
);

console.log("connector busy release contract: ok");
