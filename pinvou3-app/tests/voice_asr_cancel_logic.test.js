#!/usr/bin/env node
const assert = require("assert");
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const bridgePath = path.join(__dirname, "..", "src", "platform", "tauri", "bridge", "voice.js");
const bridgeSource = fs.readFileSync(bridgePath, "utf8");
const start = bridgeSource.indexOf("  async function installVoiceAsr() {");
const end = bridgeSource.indexOf("\n  async function startVoiceInput", start);

assert.notStrictEqual(start, -1, "installVoiceAsr must exist");
assert.notStrictEqual(end, -1, "voice ASR setup function boundary must exist");

const invokes = [];
const context = {
  state: {
    voiceAsrSetup: {
      open: true,
      installing: true,
      cancelling: false,
      progress: { stage: "model", downloaded: 1, total: 10 },
      error: null,
    },
  },
  invoke: async (command) => {
    invokes.push(command);
  },
  notify: () => {},
};
vm.createContext(context);
vm.runInContext(
  `${bridgeSource.slice(start, end)}\nthis.cancelVoiceAsrSetup = cancelVoiceAsrSetup;`,
  context,
  { filename: bridgePath },
);

(async () => {
  await context.cancelVoiceAsrSetup();

  assert.deepStrictEqual(invokes, ["cancel_voice_asr"]);
  assert.strictEqual(context.state.voiceAsrSetup.installing, true);
  assert.strictEqual(context.state.voiceAsrSetup.cancelling, true);
  assert.strictEqual(context.state.voiceAsrSetup.progress.stage, "cancelling");
  assert.strictEqual(context.state.voiceAsrSetup.open, false);

  const chatPath = path.join(__dirname, "..", "src", "features", "chat", "ChatView.jsx");
  const chatSource = fs.readFileSync(chatPath, "utf8");
  assert.match(
    chatSource,
    /onClick=\{\(\) => \{ pendingVoiceAfterIntroRef\.current = null; bridge\.voice\.cancelVoiceAsrSetup\?\.\(\); \}\}/,
    "the setup dialog cancel button must also drop the pending voice intent",
  );
  assert.match(chatSource, /disabled=\{su\.cancelling\}/);
  assert.match(chatSource, /const chatCopy = t\.uiChat;/);
  assert.match(
    chatSource,
    /su\.installing \? \(su\.cancelling \? chatCopy\.cancelling : chatCopy\.cancelDownload\) : chatCopy\.cancel/,
  );
  assert.match(chatSource, /su\.installing \? chatCopy\.asrDownloadTitle : chatCopy\.asrEnableTitle/);
  assert.match(chatSource, /\{!su\.installing && \(/);

  const voiceInputStart = bridgeSource.indexOf("  async function startVoiceInput(");
  const voiceInputEnd = bridgeSource.indexOf("\n  function cancelVoiceInput", voiceInputStart);
  assert.notStrictEqual(voiceInputStart, -1, "startVoiceInput must exist");
  assert.notStrictEqual(voiceInputEnd, -1, "startVoiceInput boundary must exist");

  const guardContext = {
    activeVoiceInput: null,
    invokes: [],
    notifyCount: 0,
    invoke: async (command) => {
      guardContext.invokes.push(command);
    },
    notify: () => {
      guardContext.notifyCount += 1;
    },
    state: {
      voiceAsrSetup: {
        open: true,
        installing: true,
        cancelling: false,
        progress: { stage: "model", downloaded: 3, total: 10 },
        error: null,
      },
    },
  };
  vm.createContext(guardContext);
  vm.runInContext(
    `${bridgeSource.slice(voiceInputStart, voiceInputEnd)}\nthis.startVoiceInput = startVoiceInput;`,
    guardContext,
    { filename: bridgePath },
  );

  await guardContext.startVoiceInput("草稿", () => {});
  assert.deepStrictEqual(guardContext.invokes, [], "active ASR download guard must skip dependency detection");
  assert.strictEqual(guardContext.state.voiceAsrSetup.open, true, "active ASR download guard must preserve the setup open state");
  assert.strictEqual(guardContext.state.voiceAsrSetup.installing, true, "active ASR download guard must keep the installing state");
  assert.strictEqual(guardContext.state.voiceAsrSetup.cancelling, false, "active ASR download guard must keep the cancelling state");
  assert.deepStrictEqual(
    guardContext.state.voiceAsrSetup.progress,
    { stage: "model", downloaded: 3, total: 10 },
    "active ASR download guard must keep the download progress",
  );
  assert.strictEqual(guardContext.notifyCount, 1, "active ASR download guard must only re-render without touching setup state");

  console.log("voice_asr_cancel_logic: ok");
// eslint-disable-next-line unicorn/prefer-top-level-await -- smoke script keeps its existing async main() structure
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
