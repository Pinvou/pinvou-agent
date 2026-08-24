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
  assert.match(chatSource, /onClick=\{\(\) => bridge\.voice\.cancelVoiceAsrSetup\(\)\}/);
  assert.match(chatSource, /disabled=\{su\.cancelling\}/);
  assert.match(chatSource, /const chatCopy = t\.uiChat;/);
  assert.match(
    chatSource,
    /su\.installing \? \(su\.cancelling \? chatCopy\.cancelling : chatCopy\.cancelDownload\) : chatCopy\.cancel/,
  );
  assert.match(chatSource, /su\.installing \? chatCopy\.asrDownloadTitle : chatCopy\.asrEnableTitle/);
  assert.match(chatSource, /\{!su\.installing && \(/);

  const voiceInputStart = bridgeSource.indexOf("  async function startVoiceInput(");
  const installingGuard = bridgeSource.indexOf("if (state.voiceAsrSetup.installing)", voiceInputStart);
  const statusCheck = bridgeSource.indexOf('invoke("voice_asr_status")', voiceInputStart);
  assert.notStrictEqual(voiceInputStart, -1, "startVoiceInput must exist");
  assert.ok(installingGuard > voiceInputStart, "startVoiceInput must guard an active ASR download");
  assert.ok(installingGuard < statusCheck, "active download guard must run before dependency detection");
  assert.match(
    bridgeSource.slice(installingGuard, statusCheck),
    /Object\.assign\(\{\}, state\.voiceAsrSetup, \{ open: true \}\);[\s\S]*?notify\(\);[\s\S]*?return;/,
  );

  console.log("voice_asr_cancel_logic: ok");
// eslint-disable-next-line unicorn/prefer-top-level-await -- smoke 脚本既有 async main() 结构
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
