import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import WebSocket from "ws";

const relayDir = dirname(dirname(fileURLToPath(import.meta.url)));
const port = 20_000 + Math.floor(Math.random() * 10_000);
const httpUrl = `http://127.0.0.1:${port}`;
const wsUrl = `ws://127.0.0.1:${port}/pinvou3/remote/ws`;
let relay;
let telemetryDir;
const enrollmentToken = "test-enrollment-token-at-least-24";
const adminPassword = "test-admin-password";

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
  telemetryDir = await mkdtemp(join(tmpdir(), "pinvou-telemetry-test-"));
  relay = spawn(process.execPath, [join(relayDir, "server.js")], {
    cwd: relayDir,
    env: {
      ...process.env,
      PORT: String(port),
      PINVOU_REMOTE_PUBLIC_BASE_PATH: "/pinvou3/remote",
      DESKTOP_RECONNECT_GRACE_MS: "1000",
      HEARTBEAT_INTERVAL_MS: "5000",
      PINVOU_TELEMETRY_DATA_DIR: telemetryDir,
      PINVOU_TELEMETRY_ENROLLMENT_TOKEN: enrollmentToken,
      PINVOU_TELEMETRY_DEVICE_PEPPER: "test-device-pepper-at-least-24-chars",
      PINVOU_STATS_ADMIN_PASSWORD: adminPassword,
      PINVOU_REMOTE_TRUSTED_PROXY_IPS: "127.0.0.1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  await waitForOutput(relay, /pinvou remote relay listening/);
});

after(async () => {
  relay?.kill("SIGTERM");
  await rm(telemetryDir, { recursive: true, force: true });
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
    "unauthenticated_connection_count",
    "ws_connection_count",
  ]);
});

test("mobile HTML preview ships best-effort fit and zoom controls", async () => {
  const response = await fetch(`${httpUrl}/pinvou3/remote/r/preview-fit-test`);
  assert.equal(response.status, 200);
  const html = await response.text();
  assert.match(html, /id="previewZoomOut"/);
  assert.match(html, /id="previewZoomFit"/);
  assert.match(html, /id="previewZoomIn"/);
  assert.match(html, /pinvou-remote-preview-frame/);
  assert.match(html, /html,body\{overflow:auto!important\}/);
  assert.match(html, /<meta name="pinvou-remote-client" content="1" \/>/);
});

test("stats and telemetry use dedicated top-level paths", async () => {
  const stats = await fetch(`${httpUrl}/pinvou3/stats`);
  assert.equal(stats.status, 200);
  assert.match(await stats.text(), /PINVOU · 设备运营中心/);

  const health = await fetch(`${httpUrl}/pinvou3/telemetry/healthz`);
  assert.equal(health.status, 200);
  assert.deepEqual(await health.json(), { ok: true });
});

test("telemetry deduplicates a device and usage event", async () => {
  const registration = {
    enrollment_token: enrollmentToken,
    registration_secret: "test-registration-secret-000000000001",
    hardware_claim: "test-hardware-claim-001",
    hardware_source: "test",
    identity_quality: "hardware_serial",
    app_version: "0.5.10",
    platform: "linux",
    arch: "aarch64",
  };
  const firstResponse = await fetch(`${httpUrl}/pinvou3/telemetry/v1/register`, {
    method: "POST",
    headers: { "content-type": "application/json", "x-forwarded-for": "113.118.113.77" },
    body: JSON.stringify(registration),
  });
  assert.equal(firstResponse.status, 200);
  const first = await firstResponse.json();
  const secondResponse = await fetch(`${httpUrl}/pinvou3/telemetry/v1/register`, {
    method: "POST",
    headers: { "content-type": "application/json", "x-forwarded-for": "113.118.113.77" },
    body: JSON.stringify(registration),
  });
  assert.equal(secondResponse.status, 200);
  const second = await secondResponse.json();
  assert.equal(second.device_id, first.device_id);
  assert.equal(second.device_token, first.device_token);

  const stolenResponse = await fetch(`${httpUrl}/pinvou3/telemetry/v1/register`, {
    method: "POST",
    headers: { "content-type": "application/json", "x-forwarded-for": "113.118.113.77" },
    body: JSON.stringify({
      ...registration,
      registration_secret: "different-registration-secret-000000001",
    }),
  });
  assert.equal(stolenResponse.status, 409);
  assert.deepEqual(await stolenResponse.json(), { error: "device_already_registered" });

  const event = {
    event_id: "evt_test_000000000001",
    occurred_at: Date.now(),
    input_tokens: 120,
    output_tokens: 30,
    success: true,
  };
  for (let i = 0; i < 2; i += 1) {
    const response = await fetch(`${httpUrl}/pinvou3/telemetry/v1/events`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization: `Bearer ${first.device_token}`,
      },
      body: JSON.stringify({ device_id: first.device_id, events: [event] }),
    });
    assert.equal(response.status, 200);
  }

  const login = await fetch(`${httpUrl}/pinvou3/stats/api/login`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ password: adminPassword }),
  });
  assert.equal(login.status, 200);
  const cookie = login.headers.get("set-cookie").split(";", 1)[0];
  const overview = await fetch(`${httpUrl}/pinvou3/stats/api/overview`, { headers: { cookie } });
  assert.equal(overview.status, 200);
  const data = await overview.json();
  assert.equal(data.counts.online, 1);
  assert.equal(data.counts.active_today, 1);
  assert.equal(data.counts.active_7d, 1);
  assert.deepEqual(data.active_versions, [{ version: "0.5.10", count: 1 }]);
  assert.equal(data.usage_trend.length, 30);
  assert.equal(data.usage_trend.at(-1).active_devices, 1);
  assert.equal(data.usage_trend.at(-1).turns, 1);

  const devices = await fetch(`${httpUrl}/pinvou3/stats/api/devices`, { headers: { cookie } });
  assert.equal(devices.status, 200);
  const list = await devices.json();
  assert.equal(list.devices[0].turns_7d, 1);
  assert.equal(list.devices[0].region, "未知");
  assert.equal("failure_rate_7d" in list.devices[0], false);
  assert.doesNotMatch(await readFile(join(telemetryDir, "devices.json"), "utf8"), /113\.118\.113\.77/);

  for (let index = 0; index < 7; index += 1) {
    const rejected = await fetch(`${httpUrl}/pinvou3/telemetry/v1/register`, {
      method: "POST",
      headers: { "content-type": "application/json", "x-forwarded-for": "113.118.113.77" },
      body: JSON.stringify({ ...registration, enrollment_token: `invalid-${index}` }),
    });
    assert.equal(rejected.status, 401);
  }
  const limited = await fetch(`${httpUrl}/pinvou3/telemetry/v1/register`, {
    method: "POST",
    headers: { "content-type": "application/json", "x-forwarded-for": "113.118.113.77" },
    body: JSON.stringify(registration),
  });
  assert.equal(limited.status, 429);
});

test("mobile composer stays inside the iOS visual viewport", async () => {
  const response = await fetch(`${httpUrl}/pinvou3/remote/r/composer-viewport-test`);
  assert.equal(response.status, 200);
  const html = await response.text();
  assert.match(html, /textarea \{[^}]*flex: 1 1 0;[^}]*min-width: 0;[^}]*font-size: 16px;/);
  assert.match(html, /\.composer button \{[^}]*flex: 0 0 36px;/);
  assert.match(html, /--visual-viewport-width/);
  assert.match(html, /viewport\.offsetLeft/);
  assert.match(html, /visualViewport\.addEventListener\('scroll', syncViewportMetrics/);
  assert.doesNotMatch(html, /user-scalable\s*=\s*no|maximum-scale\s*=\s*1/);
});

test("relay closes a websocket that does not authenticate in time", async () => {
  const authPort = port + 4;
  const authLimited = spawn(process.execPath, [join(relayDir, "server.js")], {
    cwd: relayDir,
    env: {
      ...process.env,
      PORT: String(authPort),
      WS_AUTH_TIMEOUT_MS: "1000",
      WS_CONNECT_LIMIT: "100",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  await waitForOutput(authLimited, /pinvou remote relay listening/);
  const idle = await openSocketAt(`ws://127.0.0.1:${authPort}/pinvou3/remote/ws`);
  const error = await nextMessage(idle, "error", 3000);
  assert.equal(error.code, "authentication_timeout");
  closeSocket(idle);
  authLimited.kill("SIGTERM");
});

test("relay rejects websocket upgrades above total capacity", async () => {
  const capacityPort = port + 5;
  const capacityLimited = spawn(process.execPath, [join(relayDir, "server.js")], {
    cwd: relayDir,
    env: {
      ...process.env,
      PORT: String(capacityPort),
      MAX_WS_CONNECTIONS: "2",
      WS_AUTH_TIMEOUT_MS: "5000",
      WS_CONNECT_LIMIT: "100",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  await waitForOutput(capacityLimited, /pinvou remote relay listening/);
  const url = `ws://127.0.0.1:${capacityPort}/pinvou3/remote/ws`;
  const first = await openSocketAt(url);
  const second = await openSocketAt(url);
  await assert.rejects(openSocketAt(url), /Unexpected server response: 503/);
  closeSocket(first);
  closeSocket(second);
  capacityLimited.kill("SIGTERM");
});

test("relay rate limits websocket upgrades per client", async () => {
  const ratePort = port + 6;
  const rateLimited = spawn(process.execPath, [join(relayDir, "server.js")], {
    cwd: relayDir,
    env: {
      ...process.env,
      PORT: String(ratePort),
      MAX_WS_CONNECTIONS: "10",
      WS_AUTH_TIMEOUT_MS: "5000",
      WS_CONNECT_LIMIT: "1",
      WS_CONNECT_WINDOW_MS: "60000",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  await waitForOutput(rateLimited, /pinvou remote relay listening/);
  const url = `ws://127.0.0.1:${ratePort}/pinvou3/remote/ws`;
  const first = await openSocketAt(url);
  await assert.rejects(openSocketAt(url), /Unexpected server response: 429/);
  closeSocket(first);
  rateLimited.kill("SIGTERM");
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
