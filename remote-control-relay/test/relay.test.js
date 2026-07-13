import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { spawn } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import WebSocket from "ws";

const relayDir = dirname(dirname(fileURLToPath(import.meta.url)));
const port = 20_000 + Math.floor(Math.random() * 10_000);
const httpUrl = `http://127.0.0.1:${port}`;
const wsUrl = `ws://127.0.0.1:${port}/pinvou3/remote/ws`;
let relay;

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

function openSocketAt(url) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url);
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

async function registerDesktop(roomId, token, secret, sessionId = "session-test") {
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

before(async () => {
  relay = spawn(process.execPath, [join(relayDir, "server.js")], {
    cwd: relayDir,
    env: {
      ...process.env,
      PORT: String(port),
      PINVOU_REMOTE_PUBLIC_BASE_PATH: "/pinvou3/remote",
      DESKTOP_RECONNECT_GRACE_MS: "1000",
      HEARTBEAT_INTERVAL_MS: "5000",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  await waitForOutput(relay, /pinvou remote relay listening/);
});

after(() => {
  relay?.kill("SIGTERM");
});

test("healthz only exposes aggregate counters", async () => {
  const response = await fetch(`${httpUrl}/pinvou3/remote/healthz`);
  assert.equal(response.status, 200);
  const health = await response.json();
  assert.deepEqual(Object.keys(health).sort(), [
    "desktop_open_count",
    "mobile_open_count",
    "ok",
    "paired_count",
    "room_count",
  ]);
});

test("later mobile takes over and explicit close preserves reason", async () => {
  const room = `rc_takeover_${Date.now()}`;
  const token = `token_${Date.now()}`;
  const secret = `secret_${Date.now()}`;
  const desktop = await registerDesktop(room, token, secret);
  const first = await joinMobile(room, token);
  const replaced = nextMessage(first, "room_replaced");
  const second = await joinMobile(room, token);
  await replaced;

  const closed = nextMessage(second, "room_closed");
  desktop.send(JSON.stringify({ type: "desktop_disconnect", payload: { reason: "qr_refreshed" } }));
  assert.equal((await closed).reason, "qr_refreshed");
  closeSocket(first);
  closeSocket(second);
  closeSocket(desktop);
});

test("desktop reconnects within grace without closing mobile", async () => {
  const room = `rc_grace_${Date.now()}`;
  const token = `token_${Date.now()}`;
  const secret = `secret_${Date.now()}`;
  const firstDesktop = await registerDesktop(room, token, secret);
  const mobile = await joinMobile(room, token);

  const reconnecting = nextMessage(mobile, "desktop_connection_state");
  firstDesktop.terminate();
  assert.equal((await reconnecting).status, "reconnecting");

  const restored = nextMessage(mobile, "desktop_connection_state");
  const secondDesktop = await registerDesktop(room, token, secret);
  assert.equal((await restored).status, "connected");

  const closed = nextMessage(mobile, "room_closed");
  secondDesktop.send(JSON.stringify({ type: "desktop_disconnect", payload: { reason: "stopped" } }));
  assert.equal((await closed).reason, "stopped");
  closeSocket(mobile);
  closeSocket(secondDesktop);
});

test("invalid desktop secret cannot replace an active room", async () => {
  const room = `rc_auth_${Date.now()}`;
  const token = `token_${Date.now()}`;
  const secret = `secret_${Date.now()}`;
  const desktop = await registerDesktop(room, token, secret);
  const mobile = await joinMobile(room, token);

  const attacker = await openSocket();
  const rejected = nextMessage(attacker, "error");
  attacker.send(JSON.stringify({
    type: "desktop_register",
    room_id: room,
    session_id: "stolen",
    pairing_token: "attacker-token",
    desktop_secret: "wrong-secret",
  }));
  assert.equal((await rejected).message, "invalid desktop secret");

  const closed = nextMessage(mobile, "room_closed");
  desktop.send(JSON.stringify({ type: "desktop_disconnect", payload: { reason: "stopped" } }));
  assert.equal((await closed).reason, "stopped");
  closeSocket(attacker);
  closeSocket(mobile);
  closeSocket(desktop);
});

test("desktop reconnect timeout closes the room", async () => {
  const room = `rc_timeout_${Date.now()}`;
  const token = `token_${Date.now()}`;
  const secret = `secret_${Date.now()}`;
  const desktop = await registerDesktop(room, token, secret);
  const mobile = await joinMobile(room, token);

  const reconnecting = nextMessage(mobile, "desktop_connection_state");
  const closed = nextMessage(mobile, "room_closed", 3000);
  desktop.terminate();
  assert.equal((await reconnecting).status, "reconnecting");
  assert.equal((await closed).reason, "desktop_disconnected");
  closeSocket(mobile);
});

test("relay accepts a payload larger than the previous 1 MiB limit", async () => {
  const room = `rc_payload_${Date.now()}`;
  const token = `token_${Date.now()}`;
  const secret = `secret_${Date.now()}`;
  const desktop = await registerDesktop(room, token, secret);
  const mobile = await joinMobile(room, token);
  const forwarded = nextMessage(mobile, "large_snapshot", 5000);
  desktop.send(JSON.stringify({
    type: "large_snapshot",
    room_id: room,
    session_id: "session-test",
    payload: { content: "x".repeat(1024 * 1024 + 64 * 1024) },
  }));
  assert.equal((await forwarded).payload.content.length, 1024 * 1024 + 64 * 1024);
  desktop.send(JSON.stringify({ type: "desktop_disconnect", payload: { reason: "stopped" } }));
  closeSocket(mobile);
  closeSocket(desktop);
});

test("relay caps new rooms while allowing existing room reconnect", async () => {
  const limitedPort = port + 1;
  const limited = spawn(process.execPath, [join(relayDir, "server.js")], {
    cwd: relayDir,
    env: {
      ...process.env,
      PORT: String(limitedPort),
      MAX_ROOMS: "1",
      ROOM_CREATE_LIMIT: "100",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  await waitForOutput(limited, /pinvou remote relay listening/);
  const limitedWsUrl = `ws://127.0.0.1:${limitedPort}/pinvou3/remote/ws`;
  const first = await openSocketAt(limitedWsUrl);
  const firstRegistered = nextMessage(first, "room_registered");
  first.send(JSON.stringify({
    type: "desktop_register",
    room_id: "capacity-first",
    session_id: "session-first",
    pairing_token: "token-first",
    desktop_secret: "secret-first",
  }));
  await firstRegistered;

  const rejected = await openSocketAt(limitedWsUrl);
  const capacityError = nextMessage(rejected, "error");
  rejected.send(JSON.stringify({
    type: "desktop_register",
    room_id: "capacity-second",
    session_id: "session-second",
    pairing_token: "token-second",
    desktop_secret: "secret-second",
  }));
  assert.equal((await capacityError).code, "room_capacity_reached");

  const reconnected = await openSocketAt(limitedWsUrl);
  const reconnectedMessage = nextMessage(reconnected, "room_registered");
  reconnected.send(JSON.stringify({
    type: "desktop_register",
    room_id: "capacity-first",
    session_id: "session-first",
    pairing_token: "token-first",
    desktop_secret: "secret-first",
  }));
  await reconnectedMessage;
  closeSocket(first);
  closeSocket(rejected);
  closeSocket(reconnected);
  limited.kill("SIGTERM");
});

test("relay rate limits new room creation per client", async () => {
  const limitedPort = port + 2;
  const limited = spawn(process.execPath, [join(relayDir, "server.js")], {
    cwd: relayDir,
    env: {
      ...process.env,
      PORT: String(limitedPort),
      MAX_ROOMS: "10",
      ROOM_CREATE_LIMIT: "1",
      ROOM_CREATE_WINDOW_MS: "60000",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  await waitForOutput(limited, /pinvou remote relay listening/);
  const limitedWsUrl = `ws://127.0.0.1:${limitedPort}/pinvou3/remote/ws`;
  const first = await openSocketAt(limitedWsUrl);
  const firstRegistered = nextMessage(first, "room_registered");
  first.send(JSON.stringify({
    type: "desktop_register",
    room_id: "rate-first",
    session_id: "session-first",
    pairing_token: "token-first",
    desktop_secret: "secret-first",
  }));
  await firstRegistered;

  const rejected = await openSocketAt(limitedWsUrl);
  const rateError = nextMessage(rejected, "error");
  rejected.send(JSON.stringify({
    type: "desktop_register",
    room_id: "rate-second",
    session_id: "session-second",
    pairing_token: "token-second",
    desktop_secret: "secret-second",
  }));
  assert.equal((await rateError).code, "room_creation_rate_limited");
  closeSocket(first);
  closeSocket(rejected);
  limited.kill("SIGTERM");
});

test("proxy allowlist keeps local health checks available", async () => {
  const restrictedPort = port + 3;
  const restricted = spawn(process.execPath, [join(relayDir, "server.js")], {
    cwd: relayDir,
    env: {
      ...process.env,
      PORT: String(restrictedPort),
      PINVOU_REMOTE_ALLOWED_PROXY_IPS: "192.0.2.10",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  await waitForOutput(restricted, /pinvou remote relay listening/);
  const response = await fetch(`http://127.0.0.1:${restrictedPort}/pinvou3/remote/healthz`);
  assert.equal(response.status, 200, "loopback health checks must remain available");
  restricted.kill("SIGTERM");
});
