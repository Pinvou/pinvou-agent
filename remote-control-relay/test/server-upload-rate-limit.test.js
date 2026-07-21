import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { spawn } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import WebSocket from "ws";

// 镜像 relay.test.js 的真实 server + 真实 WS 测试 harness(下载 ack 单测的同款)。
// 覆盖 G1(acknowledgeUploadChunk 透传 / NAK)+ G3(mobile 上传字节速率限流)。

const relayDir = dirname(dirname(fileURLToPath(import.meta.url)));
const port = 21_000 + Math.floor(Math.random() * 10_000);
const wsUrl = `ws://127.0.0.1:${port}/pinvou3/remote/ws`;
let relay;

// 用一个极小的窗口触发限流:整窗只有 1 MiB / 5s。
// 单测无需(也不应)真的发 100 MiB;靠 env 覆盖把窗口调小即可快速、确定性触发。
const WINDOW_BYTES = 1024 * 1024;
const WINDOW_SECS = 5;

function waitForOutput(child, pattern, timeoutMs = 5000) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`relay startup timeout: ${pattern}`)), timeoutMs);
    const onData = (chunk) => {
      const text = String(chunk);
      if (!pattern.test(text)) return;
      clearTimeout(timer);
      child.stdout.off("data", onData);
      resolve(text);
    };
    child.stdout.on("data", onData);
    child.once("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`relay exited during startup: ${code}`));
    });
  });
}

function openSocket() {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(wsUrl);
    ws.once("open", () => resolve(ws));
    ws.once("error", reject);
  });
}

function nextMessage(ws, type, timeoutMs = 3000) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      ws.off("message", onMessage);
      reject(new Error(`timeout waiting for ${type}`));
    }, timeoutMs);
    const onMessage = (data) => {
      const message = JSON.parse(String(data));
      if (message.type !== type) return;
      clearTimeout(timer);
      ws.off("message", onMessage);
      resolve(message);
    };
    ws.on("message", onMessage);
  });
}

// 等到下一条匹配 type 的消息;超时则 resolve(null)(用于断言「不应到达」)。
function maybeNextMessage(ws, type, timeoutMs = 500) {
  return nextMessage(ws, type, timeoutMs).catch(() => null);
}

// 在 desktop 侧等到某条被转发上来的 attach_file_chunk mobile_action。
// 必须 filter:mobile 一加入,relay 会先向 desktop 发一条 request_snapshot 的 mobile_action,
// 用 nextMessage(desktop, "mobile_action") 会误捕到它。
function nextUploadForward(desktop, uploadId, index, timeoutMs = 5000) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      desktop.off("message", onMessage);
      reject(new Error(`timeout waiting for upload forward upload_id=${uploadId} index=${index}`));
    }, timeoutMs);
    const onMessage = (data) => {
      const message = JSON.parse(String(data));
      if (message.type !== "mobile_action") return;
      if (message.payload?.type !== "attach_file_chunk") return;
      if (message.payload.upload_id !== uploadId) return;
      if (message.payload.index !== index) return;
      clearTimeout(timer);
      desktop.off("message", onMessage);
      resolve(message);
    };
    desktop.on("message", onMessage);
  });
}

// 在 mobile 侧等到某条 attach_file_relay_ack(按 upload_id + index 过滤)。
function nextUploadAck(mobile, uploadId, index, timeoutMs = 5000) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      mobile.off("message", onMessage);
      reject(new Error(`timeout waiting for upload ack upload_id=${uploadId} index=${index}`));
    }, timeoutMs);
    const onMessage = (data) => {
      const message = JSON.parse(String(data));
      if (message.type !== "attach_file_relay_ack") return;
      if (message.payload?.upload_id !== uploadId) return;
      if (index !== undefined && message.payload.index !== index) return;
      clearTimeout(timer);
      mobile.off("message", onMessage);
      resolve(message);
    };
    mobile.on("message", onMessage);
  });
}

async function registerDesktop(roomId, token, secret, sessionId = "session-upload") {
  const ws = await openSocket();
  const registered = nextMessage(ws, "room_registered");
  ws.send(JSON.stringify({
    type: "desktop_register",
    room_id: roomId,
    session_id: sessionId,
    pairing_token: token,
    desktop_secret: secret,
  }));
  await registered;
  return ws;
}

async function joinMobile(roomId, token) {
  const ws = await openSocket();
  const joined = nextMessage(ws, "mobile_joined");
  ws.send(JSON.stringify({ type: "mobile_join", room_id: roomId, token }));
  await joined;
  return ws;
}

function closeSocket(ws) {
  if (!ws) return;
  try { ws.close(); } catch {}
}

function uploadChunk(uploadId, index, byteLen, last = false) {
  return {
    type: "mobile_action",
    payload: {
      type: "attach_file_chunk",
      upload_id: uploadId,
      index,
      data_base64: "A".repeat(Math.ceil(byteLen * 4 / 3)),
      last,
    },
  };
}

before(async () => {
  relay = spawn(process.execPath, [join(relayDir, "server.js")], {
    cwd: relayDir,
    env: {
      ...process.env,
      PORT: String(port),
      PINVOU_REMOTE_PUBLIC_BASE_PATH: "/pinvou3/remote",
      DESKTOP_RECONNECT_GRACE_MS: "1000",
      HEARTBEAT_INTERVAL_MS: "5000",
      PINVOU_REMOTE_TRUSTED_PROXY_IPS: "127.0.0.1",
      MOBILE_UPLOAD_WINDOW_BYTES: String(WINDOW_BYTES),
      MOBILE_UPLOAD_WINDOW_SECS: String(WINDOW_SECS),
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  await waitForOutput(relay, /pinvou remote relay listening/);
});

after(async () => {
  relay?.kill("SIGTERM");
});

test("relay 回送 attach_file_relay_ack(ok:true) 在转发 attach_file_chunk 到 desktop 之后", async () => {
  const room = `rc_upload_ack_${Date.now()}`;
  const token = `token_${Date.now()}`;
  const secret = `secret_${Date.now()}`;
  const desktop = await registerDesktop(room, token, secret);
  const mobile = await joinMobile(room, token);

  const forwarded = nextUploadForward(desktop, "ul-ack", 3);
  const acknowledged = nextUploadAck(mobile, "ul-ack", 3);
  mobile.send(JSON.stringify(uploadChunk("ul-ack", 3, 64 * 1024, true)));

  const fwd = await forwarded;
  assert.equal(fwd.payload.type, "attach_file_chunk");
  assert.equal(fwd.payload.upload_id, "ul-ack");
  assert.equal(fwd.payload.index, 3);

  const ack = await acknowledged;
  assert.deepEqual(ack.payload, {
    upload_id: "ul-ack",
    index: 3,
    ok: true,
  });

  closeSocket(mobile);
  closeSocket(desktop);
});

test("relay 在 desktop 不可达时立即回 attach_file_relay_ack(ok:false)", async () => {
  const room = `rc_upload_nak_${Date.now()}`;
  const token = `token_${Date.now()}`;
  const secret = `secret_${Date.now()}`;
  const desktop = await registerDesktop(room, token, secret);
  const mobile = await joinMobile(room, token);

  // 等 desktop 断开、room.desktop 被清空(reconnecting 通知到达即可保证)。
  const reconnecting = nextMessage(mobile, "desktop_connection_state", 3000);
  desktop.terminate();
  assert.equal((await reconnecting).status, "reconnecting");

  const acknowledged = nextUploadAck(mobile, "ul-nak", undefined);
  mobile.send(JSON.stringify(uploadChunk("ul-nak", 0, 32 * 1024)));
  const ack = await acknowledged;
  assert.equal(ack.payload.upload_id, "ul-nak");
  assert.equal(ack.payload.index, 0);
  assert.equal(ack.payload.ok, false);
  assert.match(ack.payload.message, /not open/);

  closeSocket(mobile);
});

test("mobile 上传超出字节窗口时被限流,desktop 不收到被拒分片", async () => {
  const room = `rc_upload_ratelimit_${Date.now()}`;
  const token = `token_${Date.now()}`;
  const secret = `secret_${Date.now()}`;
  const desktop = await registerDesktop(room, token, secret);
  const mobile = await joinMobile(room, token);

  // 第一块 384 KiB:在 1 MiB 窗口内,正常转发,desktop 收到 + mobile 收到 ok:true ack。
  const firstForwarded = nextUploadForward(desktop, "ul-rl", 0);
  const firstAck = nextUploadAck(mobile, "ul-rl", 0);
  mobile.send(JSON.stringify(uploadChunk("ul-rl", 0, 384 * 1024)));
  const fwd0 = await firstForwarded;
  assert.equal(fwd0.payload.index, 0);
  const ack0 = await firstAck;
  assert.equal(ack0.payload.ok, true);

  // 第二块 768 KiB:384+768 = 1152 KiB > 1 MiB 窗口 → 被限流。
  const rateLimited = nextMessage(mobile, "attach_file_rate_limited", 5000);
  mobile.send(JSON.stringify(uploadChunk("ul-rl", 1, 768 * 1024)));
  const rl = await rateLimited;
  assert.equal(rl.type, "attach_file_rate_limited");
  assert.equal(rl.payload.upload_id, "ul-rl");
  assert.equal(rl.payload.retry_after_ms, 1000);

  // desktop 不应收到被限流的第二块:500ms 内没有任何 attach_file_chunk 到达。
  const leaked = await maybeNextMessage(desktop, "mobile_action", 500);
  assert.equal(leaked, null, "desktop 不应收到被限流的 attach_file_chunk");

  closeSocket(mobile);
  closeSocket(desktop);
});

test("mobile 上传字节窗口在窗口周期后重置", async () => {
  const room = `rc_upload_reset_${Date.now()}`;
  const token = `token_${Date.now()}`;
  const secret = `secret_${Date.now()}`;
  const desktop = await registerDesktop(room, token, secret);
  const mobile = await joinMobile(room, token);

  // 用 512 KiB 一块:远小于 1 MiB 窗口,稳定放行(base64 估算后仍 < 1 MiB)。
  const firstForwarded = nextUploadForward(desktop, "ul-reset", 0);
  const firstAck = nextUploadAck(mobile, "ul-reset", 0);
  mobile.send(JSON.stringify(uploadChunk("ul-reset", 0, 512 * 1024)));
  await firstForwarded;
  await firstAck;

  // 窗口已用 512 KiB,再发 768 KiB(512+768>1024)立即被限流。
  const rateLimited = nextMessage(mobile, "attach_file_rate_limited", 5000);
  mobile.send(JSON.stringify(uploadChunk("ul-reset", 1, 768 * 1024)));
  await rateLimited;

  // 等待窗口过期(测试用 5s 窗口)+ 余量后,再次发送应放行(窗口已重置)。
  const refreshedForwarded = nextUploadForward(desktop, "ul-reset", 2, 10_000);
  const refreshedAck = nextUploadAck(mobile, "ul-reset", 2, 10_000);
  await new Promise((resolve) => setTimeout(resolve, (WINDOW_SECS + 1) * 1000));
  mobile.send(JSON.stringify(uploadChunk("ul-reset", 2, 512 * 1024)));
  const fwd2 = await refreshedForwarded;
  assert.equal(fwd2.payload.index, 2);
  const ack2 = await refreshedAck;
  assert.equal(ack2.payload.ok, true);

  closeSocket(mobile);
  closeSocket(desktop);
});
