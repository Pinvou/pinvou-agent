#!/usr/bin/env node
// artifact 面板「成品」门控单测:tmp/ 路径段(中间文件)+ 成品扩展名。
// 两侧实现同源——tauri artifact-tracker.js、web bridge.js(v1 relay 页第三侧已随页面退役);
// 全部从真实源码抽取函数执行,不在测试里复刻逻辑。
const assert = require("assert");
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const appRoot = path.resolve(__dirname, "..");
const repoRoot = path.resolve(appRoot, "..");

// 三侧同一组预期:tmp/ 段一律 false,成品扩展名普通路径 true
const CASES = [
  ["tmp/draft.md", false],
  ["./tmp/x.png", false],
  ["report.md", true],
  ["C:\\x\\tmp\\a.md", false],
  ["docs/report.md", true],
  ["tmp/nested/deep.html", false],
  ["tmpfile.md", true],        // 段精确匹配 tmp,前缀子串不误伤
  ["temporary/a.md", true],    // 同上
  ["report.txt", false],       // 非成品扩展名
];

function checkAll(isDeliverable, label) {
  for (const [p, expected] of CASES) {
    assert.strictEqual(
      isDeliverable(p),
      expected,
      `${label}: isDeliverable(${JSON.stringify(p)}) 应为 ${expected}`,
    );
  }
  assert.strictEqual(isDeliverable(""), false, `${label}: 空路径应为 false`);
  assert.strictEqual(isDeliverable(null), false, `${label}: null 应为 false`);
}

// 从源码中抽取某个顶层函数(按花括号配对找到函数结束),保证测的是线上真实代码
function extractFunction(src, name) {
  const start = src.indexOf(`function ${name}(`);
  assert.ok(start >= 0, `源码中应存在 function ${name}`);
  const braceStart = src.indexOf("{", start);
  let depth = 0;
  for (let i = braceStart; i < src.length; i++) {
    if (src[i] === "{") depth++;
    else if (src[i] === "}") {
      depth--;
      if (depth === 0) return src.slice(start, i + 1);
    }
  }
  throw new Error(`function ${name} 花括号未闭合`);
}

function evalIsDeliverable(snippet, label) {
  const ctx = {};
  vm.createContext(ctx);
  vm.runInContext(`${snippet}\nthis.isDeliverable = isDeliverable;`, ctx);
  assert.strictEqual(typeof ctx.isDeliverable, "function", `${label}: 应导出 isDeliverable`);
  return ctx.isDeliverable;
}

// 1) tauri 侧:artifact-tracker feature 工厂直接返回 isDeliverable
{
  const file = path.join(appRoot, "src", "platform", "tauri", "bridge", "artifact-tracker.js");
  const code = fs.readFileSync(file, "utf8");
  const ctx = { window: {} };
  vm.createContext(ctx);
  vm.runInContext(code, ctx, { filename: file });
  const factory = ctx.window.__PINVOU_TAURI_BRIDGE_FEATURES__["artifact-tracker"];
  const feature = factory({
    state: { artifacts: [], chatItems: [], turnDirtyArtifacts: [] },
    invoke: async () => { throw new Error("invoke not expected"); },
    notify: () => {},
    sessionStates: {},
    isScheduledRunSession: () => false,
  });
  checkAll(feature.isDeliverable, "tauri artifact-tracker.js");
  assert.strictEqual(feature.fileMutationAction("File", { action: "write" }), "write");
  assert.strictEqual(feature.fileMutationAction("File", { action: "edit" }), "edit");
  assert.strictEqual(feature.fileMutationAction("File", { action: "patch" }), "patch");
  assert.strictEqual(feature.fileMutationAction("File", { action: "read" }), null);
  assert.deepStrictEqual(
    Array.from(feature.extractArtifactPaths({
      action: "patch",
      changes: [{ path: "report.md" }],
      patch: "*** Update File: report.md\n*** Add File: appendix.md\n+++ b/summary.md",
    })),
    ["report.md", "appendix.md", "summary.md"],
    "File.patch 必须追踪全部去重后的产物路径",
  );
}

// 2) web 侧 bridge.js:isDeliverable 是闭包内部函数,抽取 DELIVERABLE_EXTS..isDeliverable
//    连续代码块 + normalizedPath(isTmpPath 复用它)执行
{
  const src = fs.readFileSync(path.join(appRoot, "src", "platform", "web", "bridge.js"), "utf8");
  const blockStart = src.indexOf("var DELIVERABLE_EXTS");
  const blockEnd = src.indexOf("function trackArtifact");
  assert.ok(blockStart >= 0 && blockEnd > blockStart, "web bridge.js 应存在 DELIVERABLE_EXTS..isDeliverable 代码块");
  const snippet = extractFunction(src, "normalizedPath") + "\n" + src.slice(blockStart, blockEnd);
  checkAll(evalIsDeliverable(snippet, "web bridge.js"), "web bridge.js");
  // web 侧 isTmpPath 必须复用 normalizedPath(与 tauri 侧写法对齐),不得内联重写归一化
  assert.ok(
    extractFunction(src, "isTmpPath").includes("normalizedPath("),
    "web bridge.js isTmpPath 应复用 normalizedPath",
  );
}

// 3) remote-control-relay v1 页面已退役(2026-08):原第三侧 isTmpPath/isDeliverable
//    抽测随 web/index.html 一并移除;v2 WebUI 走 web bridge.js 实现(上方第 2 侧)。

// 4) 回放(rerender)兜底补首卡门控:两侧 bridge 的预扫只在 isDeliverable 通过时
//    记 writtenArtifacts → tmp/ 文件切 session 重放后不再冒出成品卡
{
  const GATE = 'if (dbMutation !== "edit" && isDeliverable(dap)) writtenArtifacts[dap] = true;';
  for (const rel of [
    path.join("src", "platform", "web", "bridge.js"),
    path.join("src", "platform", "tauri", "bridge.js"),
  ]) {
    const src = fs.readFileSync(path.join(appRoot, rel), "utf8");
    assert.ok(src.includes(GATE), `${rel} 回放预扫兜底补卡应带 isDeliverable 门控`);
  }
}

console.log("artifact_deliverable_logic.test.js: all assertions passed");
