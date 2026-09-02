#!/usr/bin/env node
// artifact 面板「成品」门控单测:tmp/ 路径段(中间文件)+ 成品扩展名。
// 两侧实现同源——tauri artifact-tracker.js、web bridge.js(v1 relay 页第三侧已随页面退役);
// 全部从真实源码抽取函数执行,不在测试里复刻逻辑。
const assert = require("assert");
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const appRoot = path.resolve(__dirname, "..");

// 两侧同一组预期:tmp/ 段一律 false,成品扩展名普通路径 true
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
  const state = { artifacts: [], chatItems: [], turnDirtyArtifacts: [] };
  const feature = factory({
    state,
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
    [...feature.extractArtifactPaths({
      action: "patch",
      changes: [{ path: "report.md" }],
      patch: "*** Update File: report.md\n*** Add File: appendix.md\n+++ b/summary.md",
    })],
    ["report.md", "appendix.md", "summary.md"],
    "File.patch 必须追踪全部去重后的产物路径",
  );

  state.chatItems = [
    { id: 1, type: "artifact_card", path: "C:\\workspace\\one.md", title: "One" },
    { id: 2, type: "artifact_card", path: "C:\\workspace\\two.md", title: "Two" },
    { id: 3, type: "artifact_card", path: "C:\\workspace\\three.md", title: "Three" },
  ];
  const updatedTauriCard = feature.updatePresentedArtifact({
    type: "artifact_card",
    path: "two.md",
    title: "Two updated",
    description: "Second revision",
    time: "12:00",
    sessionId: "session-1",
  });
  assert.strictEqual(state.chatItems.length, 3,
    "updating one of three artifact cards must not append a fourth card");
  assert.strictEqual(updatedTauriCard, state.chatItems[1],
    "an artifact update must reuse the existing card object and position");
  assert.strictEqual(updatedTauriCard.id, 2,
    "an artifact update must preserve the stable card id");
  assert.strictEqual(updatedTauriCard.path, "C:\\workspace\\two.md",
    "a relative artifact update must preserve the existing absolute openable path");
  assert.strictEqual(updatedTauriCard.title, "Two updated",
    "an artifact update should refresh metadata");
  state.chatItems[2].path = "three.md";
  feature.updatePresentedArtifact({
    type: "artifact_card", path: "C:\\workspace\\three.md", title: "Three updated",
  });
  assert.strictEqual(state.chatItems[2].path, "C:\\workspace\\three.md",
    "an absolute artifact update must upgrade an existing relative path");
  const missingArtifactLength = state.chatItems.length;
  assert.strictEqual(feature.updatePresentedArtifact({
    type: "artifact_card", path: "four.md", title: "Four",
  }), null, "a new artifact must still be reported to the caller as missing");
  assert.strictEqual(state.chatItems.length, missingArtifactLength,
    "reporting a missing artifact must not append a card inside the update helper");

  // A user re-request after the previous card must receive a fresh visible card
  // from the caller instead of an invisible in-place refresh.
  state.chatItems = [
    { id: 1, type: "artifact_card", path: "C:\\workspace\\one.md", title: "One" },
    { id: 2, type: "user", text: "再推一次" },
  ];
  const userRequestLength = state.chatItems.length;
  assert.strictEqual(feature.updatePresentedArtifact({
    type: "artifact_card", path: "C:\\workspace\\one.md", title: "One v2",
  }), null, "a user message newer than the card must be answered by a fresh appended card");
  assert.strictEqual(state.chatItems.length, userRequestLength,
    "the update helper must leave appending a user-requested card to its caller");
  assert.strictEqual(state.chatItems[0].title, "One",
    "a declined in-place update must leave the existing card untouched");

  // A file mutation newer than the user message keeps the in-place update.
  state.chatItems = [
    { id: 1, type: "artifact_card", path: "C:\\workspace\\one.md", title: "One" },
    { id: 2, type: "user", text: "改一下" },
    { id: 3, type: "tool", name: "File", args: { action: "edit", path: "C:\\workspace\\one.md" } },
  ];
  assert.strictEqual(feature.updatePresentedArtifact({
    type: "artifact_card", path: "C:\\workspace\\one.md", title: "One v2",
  }), state.chatItems[0],
    "a file mutation newer than the user message must keep the in-place update");

  // A user message newer than the last mutation still requests a fresh card.
  state.chatItems = [
    { id: 1, type: "artifact_card", path: "C:\\workspace\\one.md", title: "One" },
    { id: 2, type: "tool", name: "File", args: { action: "edit", path: "C:\\workspace\\one.md" } },
    { id: 3, type: "user", text: "再推一次" },
  ];
  const newerUserLength = state.chatItems.length;
  assert.strictEqual(feature.updatePresentedArtifact({
    type: "artifact_card", path: "C:\\workspace\\one.md", title: "One v3",
  }), null, "a user message newer than the last mutation must still force a fresh card");
  assert.strictEqual(state.chatItems.length, newerUserLength,
    "the update helper must not append after declining a post-mutation user request");

  // Same-named files in different absolute directories remain distinct.
  state.chatItems = [
    { id: 1, type: "artifact_card", path: "C:\\workspace\\v1\\readme.md", title: "V1" },
  ];
  const distinctAbsoluteLength = state.chatItems.length;
  assert.strictEqual(feature.updatePresentedArtifact({
    type: "artifact_card", path: "C:\\workspace\\v2\\readme.md", title: "V2",
  }), null, "an equal basename in a different absolute directory is a distinct artifact");
  assert.strictEqual(state.chatItems.length, distinctAbsoluteLength,
    "the update helper must not append a different absolute artifact itself");
  assert.strictEqual(state.chatItems[0].title, "V1",
    "the existing card must not be rewritten by a different same-named file");

  state.chatItems = [
    { id: 1, type: "artifact_card", path: "C:\\workspace\\v1\\readme.md", title: "V1" },
    { id: 2, type: "artifact_card", path: "C:\\workspace\\v2\\readme.md", title: "V2" },
  ];
  const updatedOlderTauriCard = feature.updatePresentedArtifact({
    type: "artifact_card", path: "C:\\workspace\\v1\\readme.md", title: "V1 updated",
  });
  assert.strictEqual(updatedOlderTauriCard, state.chatItems[0],
    "an exact path must update an older card hidden by a newer same-named card");
  assert.strictEqual(state.chatItems.length, 2,
    "updating the older exact artifact must not append a duplicate card");
  assert.strictEqual(state.chatItems[1].title, "V2",
    "updating the older exact artifact must not rewrite the newer card metadata");
}

// 2) web 侧 bridge.js:isDeliverable 是闭包内部函数,抽取 DELIVERABLE_EXTS..isDeliverable
//    连续代码块 + normalizedPath(isTmpPath 复用它)执行
{
  const src = fs.readFileSync(path.join(appRoot, "src", "platform", "web", "bridge.js"), "utf8");
  const blockStart = src.search(/\b(?:var|const|let) DELIVERABLE_EXTS/);
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
  const webSrc = fs.readFileSync(path.join(appRoot, "src", "platform", "web", "bridge.js"), "utf8");
  const cardState = {
    chatItems: [
      { id: 1, type: "artifact_card", path: "/workspace/one.md", title: "One" },
      { id: 2, type: "artifact_card", path: "/workspace/two.md", title: "Two" },
      { id: 3, type: "artifact_card", path: "/workspace/three.md", title: "Three" },
    ],
  };
  const cardCtx = { state: cardState };
  vm.createContext(cardCtx);
  vm.runInContext([
    extractFunction(webSrc, "basename"),
    extractFunction(webSrc, "isAbsPath"),
    extractFunction(webSrc, "normalizedPath"),
    extractFunction(webSrc, "pushArtifactPath"),
    extractFunction(webSrc, "extractArtifactPaths"),
    extractFunction(webSrc, "fileMutationAction"),
    extractFunction(webSrc, "findPresentedArtifact"),
    extractFunction(webSrc, "updatePresentedArtifact"),
    "this.updatePresentedArtifact = updatePresentedArtifact;",
  ].join("\n"), cardCtx);
  const updatedWebCard = cardCtx.updatePresentedArtifact({
    type: "artifact_card", path: "two.md", title: "Two updated",
  });
  assert.strictEqual(cardState.chatItems.length, 3,
    "web bridge must update one of three artifact cards without appending a fourth");
  assert.strictEqual(updatedWebCard, cardState.chatItems[1],
    "web bridge must preserve the artifact card position");
  assert.strictEqual(updatedWebCard.id, 2,
    "web bridge must preserve the artifact card id");
  assert.strictEqual(updatedWebCard.path, "/workspace/two.md",
    "web bridge must preserve an existing absolute path on a relative update");
  cardState.chatItems[2].path = "three.md";
  cardCtx.updatePresentedArtifact({
    type: "artifact_card", path: "/workspace/three.md", title: "Three updated",
  });
  assert.strictEqual(cardState.chatItems[2].path, "/workspace/three.md",
    "web bridge must allow a relative artifact path to upgrade to an absolute path");
  const missingWebArtifactLength = cardState.chatItems.length;
  assert.strictEqual(cardCtx.updatePresentedArtifact({
    type: "artifact_card", path: "four.md", title: "Four",
  }), null, "web bridge must report a new artifact to the caller as missing");
  assert.strictEqual(cardState.chatItems.length, missingWebArtifactLength,
    "web update helper must not append a missing artifact itself");

  // Web follows the same re-request, mutation, and distinct-path rules.
  cardState.chatItems = [
    { id: 1, type: "artifact_card", path: "/workspace/one.md", title: "One" },
    { id: 2, type: "user", text: "show it again" },
  ];
  const webUserRequestLength = cardState.chatItems.length;
  assert.strictEqual(cardCtx.updatePresentedArtifact({
    type: "artifact_card", path: "/workspace/one.md", title: "One v2",
  }), null, "web bridge must also answer a user re-request with a fresh appended card");
  assert.strictEqual(cardState.chatItems.length, webUserRequestLength,
    "web update helper must leave user-requested appending to the event caller");
  cardState.chatItems = [
    { id: 1, type: "artifact_card", path: "/workspace/one.md", title: "One" },
    { id: 2, type: "user", text: "改一下" },
    { id: 3, type: "tool", name: "File", args: { action: "edit", path: "/workspace/one.md" } },
  ];
  assert.strictEqual(cardCtx.updatePresentedArtifact({
    type: "artifact_card", path: "/workspace/one.md", title: "One v2",
  }), cardState.chatItems[0],
    "web bridge must keep the in-place update when a mutation is newer than the user message");
  cardState.chatItems = [
    { id: 1, type: "artifact_card", path: "/workspace/v1/readme.md", title: "V1" },
  ];
  const distinctWebAbsoluteLength = cardState.chatItems.length;
  assert.strictEqual(cardCtx.updatePresentedArtifact({
    type: "artifact_card", path: "/workspace/v2/readme.md", title: "V2",
  }), null, "web bridge must treat a same-named file in another directory as a distinct artifact");
  assert.strictEqual(cardState.chatItems.length, distinctWebAbsoluteLength,
    "web update helper must not append a different absolute artifact itself");
  assert.strictEqual(cardState.chatItems[0].title, "V1",
    "web bridge must not rewrite the existing card with a different same-named file");

  cardState.chatItems = [
    { id: 1, type: "artifact_card", path: "/workspace/v1/readme.md", title: "V1" },
    { id: 2, type: "artifact_card", path: "/workspace/v2/readme.md", title: "V2" },
  ];
  const updatedOlderWebCard = cardCtx.updatePresentedArtifact({
    type: "artifact_card", path: "/workspace/v1/readme.md", title: "V1 updated",
  });
  assert.strictEqual(updatedOlderWebCard, cardState.chatItems[0],
    "web bridge must prefer the exact older path over a newer basename match");
  assert.strictEqual(cardState.chatItems.length, 2,
    "web exact-path updates must not append a duplicate card");
  assert.strictEqual(cardState.chatItems[1].title, "V2",
    "web exact-path updates must leave the newer same-named card untouched");

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
