#!/usr/bin/env node
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const bridgeSource = [
  fs.readFileSync(path.join(__dirname, "..", "src", "shared", "bridge-messages.js"), "utf8"),
  fs.readFileSync(path.join(__dirname, "..", "src", "platform", "web", "bridge", "turn-terminal.js"), "utf8"),
  fs.readFileSync(path.join(__dirname, "..", "src", "platform", "web", "bridge.js"), "utf8"),
].join("\n");

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

async function waitFor(predicate) {
  for (let attempt = 0; attempt < 10; attempt += 1) {
    if (predicate()) return;
    await new Promise(resolve => setImmediate(resolve));
  }
  assert.fail("timed out waiting for the expected bridge call");
}

function createVoiceHarness({ asrStatus, media }) {
  const calls = [];
  const storage = {
    getItem() { return null; },
    setItem() {},
    removeItem() {},
  };
  const document = {
    readyState: "loading",
    addEventListener() {},
  };

  class FakeAudioContext {
    constructor() {
      this.state = "running";
      this.sampleRate = 16_000;
    }

    close() {
      this.state = "closed";
      return Promise.resolve();
    }

    createMediaStreamSource() {
      return { connect() {}, disconnect() {} };
    }

    createScriptProcessor() {
      return { connect() {}, disconnect() {}, onaudioprocess: null };
    }

    createGain() {
      return { gain: { value: 1 }, connect() {}, disconnect() {} };
    }

    get destination() {
      return {};
    }
  }

  function invoke(command, args) {
    calls.push({ command, args: args || null });
    if (command === "voice_asr_status") return asrStatus.promise;
    return Promise.resolve(null);
  }

  const navigator = {
    mediaDevices: {
      getUserMedia() {
        calls.push({ command: "getUserMedia", args: null });
        return media.promise;
      },
    },
  };
  const window = {
    __TAURI__: {
      core: { invoke },
      event: { listen() { return Promise.resolve(() => {}); } },
      dialog: { open() { return Promise.resolve(null); } },
    },
    PinvouPlatform: {
      kind: "web",
      isWeb: true,
      capabilities: {},
      can() { return false; },
      canInvoke() { return false; },
    },
    AudioContext: FakeAudioContext,
    navigator,
    document,
    localStorage: storage,
    location: { search: "" },
    addEventListener() {},
    atob(value) { return Buffer.from(String(value), "base64").toString("binary"); },
    btoa(value) { return Buffer.from(String(value), "binary").toString("base64"); },
  };
  window.window = window;

  vm.runInNewContext(bridgeSource, {
    window,
    document,
    navigator,
    localStorage: storage,
    console: { log() {}, info() {}, warn() {}, error() {} },
    setTimeout,
    clearTimeout,
    setInterval() { return 0; },
    clearInterval() {},
    structuredClone(value) { return JSON.parse(JSON.stringify(value)); },
    TextDecoder,
    Uint8Array,
    Float32Array,
  }, { filename: "web-bridge.js" });

  return { bridge: window.TauriBridge, calls };
}

test("web voice start publishes a cancellable requesting state before ASR status settles", async () => {
  const asrStatus = deferred();
  const media = deferred();
  media.promise.catch(() => {});
  const { bridge, calls } = createVoiceHarness({ asrStatus, media });

  const start = bridge.startVoiceInput("draft", () => {});
  assert.equal(bridge.getState().voiceInput.status, "requesting_permission");
  assert.equal(calls.some(call => call.command === "voice_asr_status"), true);

  bridge.cancelVoiceInput();
  assert.equal(bridge.getState().voiceInput.status, "cancelled");

  asrStatus.reject(new Error("late status failure"));
  media.reject(new Error("unused or late microphone failure"));
  await start;
  assert.equal(bridge.getState().voiceInput.status, "cancelled");
});

test("web voice cancellation wins over a late getUserMedia rejection", async () => {
  const asrStatus = deferred();
  const media = deferred();
  const { bridge, calls } = createVoiceHarness({ asrStatus, media });

  const start = bridge.startVoiceInput("", () => {});
  asrStatus.resolve({ ready: true });
  await waitFor(() => calls.some(call => call.command === "getUserMedia"));
  assert.equal(calls.some(call => call.command === "getUserMedia"), true);
  assert.equal(bridge.getState().voiceInput.status, "requesting_permission");

  bridge.cancelVoiceInput();
  assert.equal(bridge.getState().voiceInput.status, "cancelled");

  media.reject(new Error("late microphone failure"));
  await start;
  assert.equal(bridge.getState().voiceInput.status, "cancelled");
});

test("web voice stops every track from a MediaStream that resolves after cancellation", async () => {
  const asrStatus = deferred();
  const media = deferred();
  const { bridge, calls } = createVoiceHarness({ asrStatus, media });
  let stoppedTracks = 0;
  const lateStream = {
    getTracks() {
      return [
        { stop() { stoppedTracks += 1; } },
        { stop() { stoppedTracks += 1; } },
      ];
    },
  };

  const start = bridge.startVoiceInput("", () => {});
  asrStatus.resolve({ ready: true });
  await waitFor(() => calls.some(call => call.command === "getUserMedia"));
  assert.equal(calls.some(call => call.command === "getUserMedia"), true);

  bridge.cancelVoiceInput();
  media.resolve(lateStream);
  await start;

  assert.equal(stoppedTracks, 2);
  assert.equal(bridge.getState().voiceInput.status, "cancelled");
});
