import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { request as httpRequest } from "node:http";
import { connect as netConnect } from "node:net";
import { createServer } from "node:net";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import WebSocket from "ws";

function getFreePort() {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.unref();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => {
        if (address && typeof address === "object") resolve(address.port);
        else reject(new Error("failed to allocate test port"));
      });
    });
  });
}

const relayDir = dirname(dirname(fileURLToPath(import.meta.url)));
const port = await getFreePort();
const httpUrl = `http://127.0.0.1:${port}`;
const wsUrl = `ws://127.0.0.1:${port}/pinvou3/remote/ws`;
let relay;
let testRoot;
let webDir;
let relayStatePath;

function waitForOutput(child, pattern, timeoutMs = 5000) {
  return new Promise((resolve, reject) => {
    let stdout = "";
    let stderr = "";
    const timer = setTimeout(() => reject(new Error(`relay startup timeout: ${pattern}`)), timeoutMs);
    const onData = (chunk) => {
      const text = String(chunk);
      stdout += text;
      if (!pattern.test(text)) return;
      clearTimeout(timer);
      child.stdout.off("data", onData);
      child.stderr.off("data", onErr);
      resolve(text);
    };
    const onErr = (chunk) => { stderr += String(chunk); };
    child.stdout.on("data", onData);
    child.stderr.on("data", onErr);
    child.once("exit", (code) => {
      clearTimeout(timer);
      child.stdout.off("data", onData);
      child.stderr.off("data", onErr);
      reject(new Error(`relay exited during startup: ${code}\nstdout:\n${stdout}\nstderr:\n${stderr}`));
    });
  });
}

function openSocket(url = wsUrl, options = {}) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url, options);
    ws.once("open", () => resolve(ws));
    ws.once("error", reject);
  });
}

function rejectedUpgrade(url, options = {}) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url, options);
    ws.once("open", () => {
      ws.close();
      reject(new Error("websocket upgrade unexpectedly succeeded"));
    });
    ws.once("unexpected-response", (_request, response) => {
      response.resume();
      resolve(response.statusCode);
    });
    ws.once("error", (error) => {
      if (/Unexpected server response/.test(error.message)) return;
      reject(error);
    });
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

// Like nextMessage, but resolves null instead of rejecting when no matching
// message arrives in time, so callers can branch on "the relay replied" vs
// "the relay stayed silent".
function nextMessageOrTimeout(ws, type, timeoutMs = 2500) {
  return nextMessage(ws, type, timeoutMs).catch((error) => {
    if (!String(error.message).startsWith("timeout waiting for")) throw error;
    return null;
  });
}

function expectNoMessage(ws, type, timeoutMs = 250) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      ws.off("message", onMessage);
      resolve();
    }, timeoutMs);
    const onMessage = (data) => {
      const message = JSON.parse(String(data));
      if (message.type !== type) return;
      clearTimeout(timer);
      ws.off("message", onMessage);
      reject(new Error(`unexpected ${type}: ${String(data)}`));
    };
    ws.on("message", onMessage);
  });
}

function waitForClose(ws, timeoutMs = 3000) {
  if (ws.readyState === ws.CLOSED) return Promise.resolve();
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("timeout waiting for websocket close")), timeoutMs);
    ws.once("close", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

function waitForCloseInfo(ws, timeoutMs = 3000) {
  if (ws.readyState === ws.CLOSED) {
    return Promise.resolve({ code: null, reason: "" });
  }
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("timeout waiting for websocket close")), timeoutMs);
    ws.once("close", (code, reason) => {
      clearTimeout(timer);
      resolve({ code, reason: reason.toString() });
    });
  });
}

// Inverse of waitForClose, like expectNoMessage is the inverse of nextMessage:
// resolves after timeoutMs with the socket never having closed, rejects as
// soon as it does.
function expectStillOpen(ws, timeoutMs = 250) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      ws.off("close", onClose);
      resolve();
    }, timeoutMs);
    const onClose = () => {
      clearTimeout(timer);
      reject(new Error("websocket closed unexpectedly"));
    };
    ws.once("close", onClose);
  });
}

async function registerDesktop(endpointId, accessToken, desktopSecret, url = wsUrl) {
  const ws = await openSocket(url);
  const registered = nextMessage(ws, "desktop_endpoint_registered");
  ws.send(JSON.stringify({
    v: 2,
    type: "desktop_endpoint_register",
    endpoint_id: endpointId,
    access_token: accessToken,
    desktop_secret: desktopSecret,
  }));
  return { ws, registered: await registered };
}

async function joinWeb(endpointId, accessToken, options = {}, url = wsUrl) {
  const ws = await openSocket(url);
  const joined = nextMessage(ws, "web_client_joined");
  ws.send(JSON.stringify({
    v: 2,
    type: "web_client_join",
    endpoint_id: endpointId,
    access_token: accessToken,
    stream_epoch: options.streamEpoch || null,
    after_seq: options.afterSeq || 0,
  }));
  return { ws, joined: await joined };
}

function closeSocket(ws) {
  if (!ws) return;
  try { ws.close(); } catch {}
}

function waitForExit(child, timeoutMs = 3000) {
  if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve();
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("timeout waiting for relay exit")), timeoutMs);
    child.once("exit", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

async function stopRelay(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  child.kill("SIGTERM");
  await waitForExit(child);
}

async function spawnRelay(env = {}) {
  const isolatedPort = await getFreePort();
  const statePath = env.PINVOU_REMOTE_STATE_PATH
    || join(testRoot, `relay-state-${isolatedPort}-${Date.now()}-${Math.random().toString(16).slice(2)}.json`);
  const child = spawn(process.execPath, [join(relayDir, "server.js")], {
    cwd: relayDir,
    env: {
      ...process.env,
      PORT: String(isolatedPort),
      PINVOU_REMOTE_PUBLIC_BASE_PATH: "/pinvou3/remote",
      PINVOU_REMOTE_WEB_DIR: webDir,
      HEARTBEAT_INTERVAL_MS: "5000",
      PINVOU_REMOTE_STATE_PATH: statePath,
      ...env,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  await waitForOutput(child, /pinvou remote relay listening/);
  return {
    child,
    port: isolatedPort,
    httpUrl: `http://127.0.0.1:${isolatedPort}`,
    wsUrl: `ws://127.0.0.1:${isolatedPort}/pinvou3/remote/ws`,
    statePath,
  };
}

before(async () => {
  testRoot = await mkdtemp(join(tmpdir(), "pinvou-relay-v2-test-"));
  webDir = join(testRoot, "web-dist");
  relayStatePath = join(testRoot, "relay-state.json");
  await mkdir(webDir, { recursive: true });
  await mkdir(join(webDir, "assets"), { recursive: true });
  await writeFile(join(webDir, "index.html"), "<!doctype html><title>PINVOU WebUI v2</title><main id=app></main>");
  await writeFile(join(webDir, "app.js"), "window.__pinvou_webui_v2__ = true;\n");
  await writeFile(join(webDir, "assets", "app-01234567.js"), "window.__hashed__ = true;\n");
  await writeFile(join(webDir, "assets", "brand-banner.jpg"), "fixed-name-asset\n");
  relay = spawn(process.execPath, [join(relayDir, "server.js")], {
    cwd: relayDir,
    env: {
      ...process.env,
      PORT: String(port),
      PINVOU_REMOTE_PUBLIC_BASE_PATH: "/pinvou3/remote",
      PINVOU_REMOTE_WEB_DIR: webDir,
      PINVOU_REMOTE_STATE_PATH: relayStatePath,
      HEARTBEAT_INTERVAL_MS: "5000",
      PINVOU_REMOTE_TRUSTED_PROXY_IPS: "127.0.0.1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  await waitForOutput(relay, /pinvou remote relay listening/);
});

after(async () => {
  await stopRelay(relay);
  await rm(testRoot, { recursive: true, force: true });
});

test("serves the built WebUI SPA at the public base path", async () => {
  const root = await fetch(`${httpUrl}/pinvou3/remote/`);
  assert.equal(root.status, 200);
  assert.match(await root.text(), /PINVOU WebUI v2/);
  assert.equal(root.headers.get("cache-control"), "no-store");
  assert.match(root.headers.get("content-security-policy"), /connect-src 'self' ws: wss:/);
  assert.equal(root.headers.get("referrer-policy"), "no-referrer");

  const spa = await fetch(`${httpUrl}/pinvou3/remote/conversations/current`);
  assert.equal(spa.status, 200);
  assert.match(await spa.text(), /PINVOU WebUI v2/);

  const asset = await fetch(`${httpUrl}/pinvou3/remote/app.js`);
  assert.equal(asset.status, 200);
  assert.match(asset.headers.get("content-type"), /text\/javascript/);
  assert.equal(asset.headers.get("cache-control"), "no-cache");

  const hashedAsset = await fetch(`${httpUrl}/pinvou3/remote/assets/app-01234567.js`);
  assert.equal(hashedAsset.status, 200);
  assert.match(hashedAsset.headers.get("cache-control"), /immutable/);

  const fixedAsset = await fetch(`${httpUrl}/pinvou3/remote/assets/brand-banner.jpg`);
  assert.equal(fixedAsset.status, 200);
  assert.equal(fixedAsset.headers.get("cache-control"), "no-cache");

  const missingAsset = await fetch(`${httpUrl}/pinvou3/remote/missing.css`);
  assert.equal(missingAsset.status, 404);
  const outside = await fetch(`${httpUrl}/not-the-webui`);
  assert.equal(outside.status, 404);
});

test("healthz exposes aggregate endpoint counters and no identifiers", async () => {
  const response = await fetch(`${httpUrl}/pinvou3/remote/healthz`);
  assert.equal(response.status, 200);
  const health = await response.json();
  assert.deepEqual(Object.keys(health).sort(), [
    "connected_endpoint_count",
    "desktop_offline_count",
    "desktop_open_count",
    "endpoint_count",
    "ok",
    "room_count",
    "unauthenticated_connection_count",
    "web_client_open_count",
    "ws_connection_count",
  ]);
  assert.equal(JSON.stringify(health).includes("token"), false);
});

test("registers a persistent endpoint and blind-forwards only v2 message types", async (t) => {
  const endpointId = `endpoint_forward_${Date.now()}`;
  const accessToken = "access-token-forward-0000000000000001";
  const desktopSecret = "desktop-secret-forward-0000000000001";
  const desktop = await registerDesktop(endpointId, accessToken, desktopSecret);
  const webConnected = nextMessage(desktop.ws, "web_client_connected");
  const web = await joinWeb(endpointId, accessToken, { streamEpoch: "epoch-a", afterSeq: 41 });
  t.after(() => { closeSocket(web.ws); closeSocket(desktop.ws); });

  assert.equal(desktop.registered.endpoint_id, endpointId);
  assert.equal(desktop.registered.web_client_connected, false);
  assert.equal(web.joined.desktop_connected, true);
  assert.match(web.joined.lease_id, /^lease_/);
  assert.deepEqual(await webConnected, {
    v: 2,
    type: "web_client_connected",
    web_rejoining: false,
    endpoint_id: endpointId,
    lease_id: web.joined.lease_id,
    stream_epoch: "epoch-a",
    after_seq: 41,
  });

  const forwardedRpc = nextMessage(desktop.ws, "rpc_request");
  web.ws.send(JSON.stringify({
    v: 2,
    type: "rpc_request",
    lease_id: "attacker-controlled",
    id: "rpc-1",
    client_request_id: "rpc-1",
    command: "list_sessions",
    args: { offset: 3 },
    desktop_secret: "must-not-cross-relay",
  }));
  assert.deepEqual(await forwardedRpc, {
    v: 2,
    type: "rpc_request",
    endpoint_id: endpointId,
    lease_id: web.joined.lease_id,
    id: "rpc-1",
    client_request_id: "rpc-1",
    command: "list_sessions",
    args: { offset: 3 },
  });

  const response = nextMessage(web.ws, "rpc_response");
  desktop.ws.send(JSON.stringify({
    v: 2,
    type: "rpc_response",
    id: "rpc-1",
    ok: true,
    result: ["session-a"],
    access_token: "must-not-cross-relay",
  }));
  assert.deepEqual(await response, {
    v: 2,
    type: "rpc_response",
    endpoint_id: endpointId,
    lease_id: web.joined.lease_id,
    id: "rpc-1",
    ok: true,
    result: ["session-a"],
  });

  for (const type of ["event_subscribe", "event_unsubscribe", "client_ready"]) {
    const forwarded = nextMessage(desktop.ws, type);
    web.ws.send(JSON.stringify({ v: 2, type, event: "engine:event", after_seq: 4 }));
    assert.equal((await forwarded).lease_id, web.joined.lease_id);
  }
  for (const type of ["event", "stream_reset", "desktop_snapshot"]) {
    const forwarded = nextMessage(web.ws, type);
    desktop.ws.send(JSON.stringify({ v: 2, type, payload: { marker: type } }));
    const message = await forwarded;
    assert.equal(message.lease_id, web.joined.lease_id);
    assert.deepEqual(message.payload, { marker: type });
  }

  const unsupported = nextMessage(web.ws, "error");
  web.ws.send(JSON.stringify({ v: 2, type: "desktop_endpoint_revoke", endpoint_id: endpointId }));
  assert.equal((await unsupported).code, "invalid_desktop_secret");
});

test("a newer Web client takes over the endpoint lease", async (t) => {
  const endpointId = `endpoint_takeover_${Date.now()}`;
  const accessToken = "access-token-takeover-000000000000001";
  const desktopSecret = "desktop-secret-takeover-0000000000001";
  const desktop = await registerDesktop(endpointId, accessToken, desktopSecret);
  const firstConnected = nextMessage(desktop.ws, "web_client_connected");
  const first = await joinWeb(endpointId, accessToken);
  await firstConnected;
  const replaced = nextMessage(first.ws, "endpoint_replaced");
  const secondConnected = nextMessage(desktop.ws, "web_client_connected");
  const second = await joinWeb(endpointId, accessToken, { afterSeq: 9 });
  t.after(() => { closeSocket(first.ws); closeSocket(second.ws); closeSocket(desktop.ws); });

  assert.equal((await replaced).lease_id, first.joined.lease_id);
  await waitForClose(first.ws);
  assert.notEqual(second.joined.lease_id, first.joined.lease_id);
  assert.equal((await secondConnected).lease_id, second.joined.lease_id);

  desktop.ws.send(JSON.stringify({
    v: 2,
    type: "rpc_response",
    lease_id: first.joined.lease_id,
    id: "stale-rpc",
    ok: true,
  }));
  await expectNoMessage(second.ws, "rpc_response");

  const current = nextMessage(second.ws, "rpc_response");
  desktop.ws.send(JSON.stringify({
    v: 2,
    type: "rpc_response",
    lease_id: second.joined.lease_id,
    id: "current-rpc",
    ok: true,
  }));
  assert.equal((await current).id, "current-rpc");
});

test("desktop can reconnect to the same endpoint within the offline TTL", async (t) => {
  const endpointId = `endpoint_reconnect_${Date.now()}`;
  const accessToken = "access-token-reconnect-00000000000001";
  const desktopSecret = "desktop-secret-reconnect-000000000001";
  const firstDesktop = await registerDesktop(endpointId, accessToken, desktopSecret);
  const initialConnected = nextMessage(firstDesktop.ws, "web_client_connected");
  const web = await joinWeb(endpointId, accessToken, { streamEpoch: "epoch-r", afterSeq: 77 });
  await initialConnected;
  t.after(() => { closeSocket(web.ws); closeSocket(firstDesktop.ws); });

  const offline = nextMessage(web.ws, "desktop_connection_state");
  firstDesktop.ws.terminate();
  assert.equal((await offline).status, "offline");
  const offlineHealth = await (await fetch(`${httpUrl}/pinvou3/remote/healthz`)).json();
  assert.ok(offlineHealth.desktop_offline_count >= 1);

  const secondDesktopSocket = await openSocket();
  const registered = nextMessage(secondDesktopSocket, "desktop_endpoint_registered");
  const online = nextMessage(web.ws, "desktop_connection_state");
  const connected = nextMessage(secondDesktopSocket, "web_client_connected");
  secondDesktopSocket.send(JSON.stringify({
    v: 2,
    type: "desktop_endpoint_register",
    endpoint_id: endpointId,
    access_token: accessToken,
    desktop_secret: desktopSecret,
  }));
  t.after(() => closeSocket(secondDesktopSocket));
  assert.equal((await registered).web_client_connected, true);
  assert.equal((await online).status, "connected");
  assert.deepEqual(await connected, {
    v: 2,
    type: "web_client_connected",
    web_rejoining: false,
    endpoint_id: endpointId,
    lease_id: web.joined.lease_id,
    stream_epoch: "epoch-r",
    after_seq: 77,
  });
});

test("invalid credentials cannot join or replace a desktop", async (t) => {
  const endpointId = `endpoint_auth_${Date.now()}`;
  const accessToken = "access-token-auth-000000000000000001";
  const desktopSecret = "desktop-secret-auth-000000000000001";
  const desktop = await registerDesktop(endpointId, accessToken, desktopSecret);
  t.after(() => closeSocket(desktop.ws));

  const badWeb = await openSocket();
  const badWebError = nextMessage(badWeb, "error");
  badWeb.send(JSON.stringify({
    v: 2,
    type: "web_client_join",
    endpoint_id: endpointId,
    access_token: "wrong-token",
  }));
  assert.equal((await badWebError).code, "invalid_token");
  await waitForClose(badWeb);

  const badDesktop = await openSocket();
  const badDesktopError = nextMessage(badDesktop, "error");
  badDesktop.send(JSON.stringify({
    v: 2,
    type: "desktop_endpoint_register",
    endpoint_id: endpointId,
    access_token: accessToken,
    desktop_secret: "wrong-secret",
  }));
  assert.equal((await badDesktopError).code, "invalid_desktop_secret");
  await waitForClose(badDesktop);
  assert.equal(desktop.ws.readyState, desktop.ws.OPEN);
});

test("requires exact v2 messages and rejects legacy token aliases before authentication", async (t) => {
  const sockets = [];
  t.after(() => sockets.forEach(closeSocket));

  const missingVersion = await openSocket();
  sockets.push(missingVersion);
  const versionError = nextMessage(missingVersion, "error");
  missingVersion.send(JSON.stringify({
    type: "web_client_join",
    endpoint_id: "endpoint_missing_version",
    access_token: "access-token-missing-version-00000001",
  }));
  assert.equal((await versionError).code, "unsupported_protocol_version");
  await waitForClose(missingVersion);

  for (const alias of ["token", "pairing_token"]) {
    const socket = await openSocket();
    sockets.push(socket);
    const aliasError = nextMessage(socket, "error");
    socket.send(JSON.stringify({
      v: 2,
      type: "web_client_join",
      endpoint_id: `endpoint_legacy_${alias}`,
      [alias]: "legacy-token-must-not-authenticate",
    }));
    assert.equal((await aliasError).code, "legacy_token_alias_unsupported");
    await waitForClose(socket);
  }

  const mixedCredentials = await openSocket();
  sockets.push(mixedCredentials);
  const mixedError = nextMessage(mixedCredentials, "error");
  mixedCredentials.send(JSON.stringify({
    v: 2,
    type: "desktop_endpoint_register",
    endpoint_id: "endpoint_mixed_credentials",
    access_token: "access-token-current-0000000000000001",
    token: "legacy-token-must-not-be-tolerated",
    desktop_secret: "desktop-secret-current-0000000000001",
  }));
  assert.equal((await mixedError).code, "legacy_token_alias_unsupported");
  await waitForClose(mixedCredentials);
});

test("rejects non-v2 rpc, event, client_ready, and revoke after authentication", async (t) => {
  const endpointId = `endpoint_version_${Date.now()}`;
  const accessToken = "access-token-version-0000000000000001";
  const desktopSecret = "desktop-secret-version-0000000000001";
  const sockets = [];
  t.after(() => sockets.forEach(closeSocket));

  let desktop = await registerDesktop(endpointId, accessToken, desktopSecret);
  sockets.push(desktop.ws);
  let connected = nextMessage(desktop.ws, "web_client_connected");
  let web = await joinWeb(endpointId, accessToken);
  sockets.push(web.ws);
  await connected;

  let notForwarded = expectNoMessage(desktop.ws, "rpc_request");
  let versionError = nextMessage(web.ws, "error");
  web.ws.send(JSON.stringify({
    type: "rpc_request",
    id: "rpc-without-version",
    command: "list_sessions",
    args: {},
  }));
  assert.equal((await versionError).code, "unsupported_protocol_version");
  await notForwarded;
  await waitForClose(web.ws);

  connected = nextMessage(desktop.ws, "web_client_connected");
  web = await joinWeb(endpointId, accessToken);
  sockets.push(web.ws);
  await connected;
  notForwarded = expectNoMessage(desktop.ws, "client_ready");
  versionError = nextMessage(web.ws, "error");
  web.ws.send(JSON.stringify({ v: 1, type: "client_ready" }));
  assert.equal((await versionError).code, "unsupported_protocol_version");
  await notForwarded;
  await waitForClose(web.ws);

  connected = nextMessage(desktop.ws, "web_client_connected");
  web = await joinWeb(endpointId, accessToken);
  sockets.push(web.ws);
  await connected;
  notForwarded = expectNoMessage(web.ws, "event");
  versionError = nextMessage(desktop.ws, "error");
  desktop.ws.send(JSON.stringify({
    v: "2",
    type: "event",
    payload: { must_not_cross: true },
  }));
  assert.equal((await versionError).code, "unsupported_protocol_version");
  await notForwarded;
  await waitForClose(desktop.ws);

  desktop = await registerDesktop(endpointId, accessToken, desktopSecret);
  sockets.push(desktop.ws);
  versionError = nextMessage(desktop.ws, "error");
  desktop.ws.send(JSON.stringify({
    type: "desktop_endpoint_revoke",
    endpoint_id: endpointId,
    desktop_secret: desktopSecret,
  }));
  assert.equal((await versionError).code, "unsupported_protocol_version");
  await waitForClose(desktop.ws);

  const survivingEndpoint = await joinWeb(endpointId, accessToken);
  sockets.push(survivingEndpoint.ws);
  assert.equal(survivingEndpoint.joined.endpoint_id, endpointId);
});

test("desktop revoke invalidates the persistent link", async (t) => {
  const endpointId = `endpoint_revoke_${Date.now()}`;
  const accessToken = "access-token-revoke-00000000000001";
  const desktopSecret = "desktop-secret-revoke-000000000001";
  const desktop = await registerDesktop(endpointId, accessToken, desktopSecret);
  const connected = nextMessage(desktop.ws, "web_client_connected");
  const web = await joinWeb(endpointId, accessToken);
  await connected;
  t.after(() => { closeSocket(web.ws); closeSocket(desktop.ws); });

  const revokedAck = nextMessage(desktop.ws, "desktop_endpoint_revoked");
  const revokedWeb = nextMessage(web.ws, "endpoint_revoked");
  desktop.ws.send(JSON.stringify({
    v: 2,
    type: "desktop_endpoint_revoke",
    endpoint_id: endpointId,
    desktop_secret: desktopSecret,
    reason: "refreshed",
  }));
  assert.equal((await revokedAck).endpoint_id, endpointId);
  assert.equal((await revokedWeb).reason, "refreshed");

  const persistedState = JSON.parse(await readFile(relayStatePath, "utf8"));
  assert.equal(persistedState.version, 1);
  assert.ok(persistedState.revoked_endpoints.some((entry) => entry.endpoint_id === endpointId));

  const stale = await openSocket();
  const error = nextMessage(stale, "error");
  stale.send(JSON.stringify({
    v: 2,
    type: "web_client_join",
    endpoint_id: endpointId,
    access_token: accessToken,
  }));
  assert.equal((await error).code, "endpoint_not_found");
  closeSocket(stale);

  const staleDesktop = await openSocket();
  const staleDesktopError = nextMessage(staleDesktop, "error");
  staleDesktop.send(JSON.stringify({
    v: 2,
    type: "desktop_endpoint_register",
    endpoint_id: endpointId,
    access_token: accessToken,
    desktop_secret: desktopSecret,
  }));
  assert.equal((await staleDesktopError).code, "endpoint_revoked");
  await waitForClose(staleDesktop);
});

test("revoke tombstone survives Relay restart and permits a fresh endpoint id", async () => {
  const statePath = join(testRoot, `restart-revoke-${Date.now()}.json`);
  const endpointId = `endpoint_restart_revoke_${Date.now()}`;
  const accessToken = "access-token-restart-revoke-0000000001";
  const desktopSecret = "desktop-secret-restart-revoke-000000001";
  let isolated = await spawnRelay({ PINVOU_REMOTE_STATE_PATH: statePath });
  try {
    const desktop = await registerDesktop(
      endpointId,
      accessToken,
      desktopSecret,
      isolated.wsUrl,
    );
    const revoked = nextMessage(desktop.ws, "desktop_endpoint_revoked");
    desktop.ws.send(JSON.stringify({
      v: 2,
      type: "desktop_endpoint_revoke",
      endpoint_id: endpointId,
      desktop_secret: desktopSecret,
      reason: "restart-test",
    }));
    await revoked;
    await waitForClose(desktop.ws);
    await stopRelay(isolated.child);

    isolated = await spawnRelay({ PINVOU_REMOTE_STATE_PATH: statePath });
    const stale = await openSocket(isolated.wsUrl);
    const staleError = nextMessage(stale, "error");
    stale.send(JSON.stringify({
      v: 2,
      type: "desktop_endpoint_register",
      endpoint_id: endpointId,
      access_token: accessToken,
      desktop_secret: desktopSecret,
    }));
    assert.equal((await staleError).code, "endpoint_revoked");
    await waitForClose(stale);

    const fresh = await registerDesktop(
      `${endpointId}_fresh`,
      "access-token-restart-fresh-00000000001",
      "desktop-secret-restart-fresh-000000001",
      isolated.wsUrl,
    );
    assert.match(fresh.registered.endpoint_id, /_fresh$/);
    const freshRevoked = nextMessage(fresh.ws, "desktop_endpoint_revoked");
    fresh.ws.send(JSON.stringify({
      v: 2,
      type: "desktop_endpoint_revoke",
      endpoint_id: `${endpointId}_fresh`,
      desktop_secret: "desktop-secret-restart-fresh-000000001",
    }));
    await freshRevoked;
    const updatedState = JSON.parse(await readFile(statePath, "utf8"));
    assert.deepEqual(
      updatedState.revoked_endpoints.map((entry) => entry.endpoint_id).sort(),
      [endpointId, `${endpointId}_fresh`].sort(),
    );
  } finally {
    await stopRelay(isolated.child);
  }
});

test("revoke fails closed without ACK when the tombstone cannot be persisted", async () => {
  const blockedParent = join(testRoot, `blocked-state-parent-${Date.now()}`);
  const statePath = join(blockedParent, "relay-state.json");
  const isolated = await spawnRelay({ PINVOU_REMOTE_STATE_PATH: statePath });
  const endpointId = `endpoint_revoke_failure_${Date.now()}`;
  const accessToken = "access-token-revoke-failure-000000001";
  const desktopSecret = "desktop-secret-revoke-failure-00000001";
  try {
    const desktop = await registerDesktop(
      endpointId,
      accessToken,
      desktopSecret,
      isolated.wsUrl,
    );
    await writeFile(blockedParent, "not-a-directory");
    const persistenceError = nextMessage(desktop.ws, "error");
    desktop.ws.send(JSON.stringify({
      v: 2,
      type: "desktop_endpoint_revoke",
      endpoint_id: endpointId,
      desktop_secret: desktopSecret,
    }));
    assert.equal((await persistenceError).code, "revoke_persistence_failed");
    await waitForClose(desktop.ws);

    await rm(blockedParent, { force: true });
    const reconnect = await registerDesktop(
      endpointId,
      accessToken,
      desktopSecret,
      isolated.wsUrl,
    );
    assert.equal(reconnect.registered.endpoint_id, endpointId);
    closeSocket(reconnect.ws);
  } finally {
    await stopRelay(isolated.child);
  }
});

test("forwards payloads larger than the previous 1 MiB limit", async (t) => {
  const endpointId = `endpoint_payload_${Date.now()}`;
  const accessToken = "access-token-payload-0000000000001";
  const desktopSecret = "desktop-secret-payload-00000000001";
  const desktop = await registerDesktop(endpointId, accessToken, desktopSecret);
  const connected = nextMessage(desktop.ws, "web_client_connected");
  const web = await joinWeb(endpointId, accessToken);
  await connected;
  t.after(() => { closeSocket(web.ws); closeSocket(desktop.ws); });
  const snapshot = nextMessage(web.ws, "desktop_snapshot", 5000);
  desktop.ws.send(JSON.stringify({
    v: 2,
    type: "desktop_snapshot",
    payload: { text: "x".repeat(1_200_000) },
  }));
  assert.equal((await snapshot).payload.text.length, 1_200_000);
});

test("closes a websocket that does not authenticate in time", async () => {
  const isolated = await spawnRelay({ WS_AUTH_TIMEOUT_MS: "1000" });
  try {
    const ws = await openSocket(isolated.wsUrl);
    const error = nextMessage(ws, "error", 2500);
    assert.equal((await error).code, "authentication_timeout");
    await waitForClose(ws);
  } finally {
    await stopRelay(isolated.child);
  }
});

test("expires offline endpoints after TTL and releases endpoint capacity", async () => {
  const isolated = await spawnRelay({
    MAX_ENDPOINTS: "1",
    ENDPOINT_CREATE_LIMIT: "10",
    ENDPOINT_OFFLINE_TTL_MS: "1000",
  });
  const first = await registerDesktop(
    "endpoint_ttl_one",
    "ttl-token-one-000000000000000000001",
    "ttl-secret-one-00000000000000000001",
    isolated.wsUrl,
  );
  const connected = nextMessage(first.ws, "web_client_connected");
  const web = await joinWeb(
    "endpoint_ttl_one",
    "ttl-token-one-000000000000000000001",
    {},
    isolated.wsUrl,
  );
  await connected;
  try {
    const offline = nextMessage(web.ws, "desktop_connection_state");
    first.ws.terminate();
    assert.equal((await offline).status, "offline");

    const retained = await (await fetch(`${isolated.httpUrl}/pinvou3/remote/healthz`)).json();
    assert.equal(retained.endpoint_count, 1);
    assert.equal(retained.desktop_offline_count, 1);

    await new Promise((resolve) => setTimeout(resolve, 1100));
    const expired = await (await fetch(`${isolated.httpUrl}/pinvou3/remote/healthz`)).json();
    assert.equal(expired.endpoint_count, 0);
    await waitForClose(web.ws);

    const second = await registerDesktop(
      "endpoint_ttl_two",
      "ttl-token-two-000000000000000000002",
      "ttl-secret-two-00000000000000000002",
      isolated.wsUrl,
    );
    assert.equal(second.registered.endpoint_id, "endpoint_ttl_two");
    closeSocket(second.ws);
  } finally {
    closeSocket(web.ws);
    closeSocket(first.ws);
    await stopRelay(isolated.child);
  }
});

test("allows native and approved browser origins while rejecting other browser origins", async () => {
  const isolated = await spawnRelay({
    PINVOU_REMOTE_ALLOWED_WEB_ORIGINS: "https://trusted.example",
    WS_CONNECT_LIMIT: "20",
  });
  const sockets = [];
  try {
    assert.equal(
      await rejectedUpgrade(isolated.wsUrl, { origin: "https://evil.example" }),
      403,
    );
    sockets.push(await openSocket(isolated.wsUrl, { origin: "https://trusted.example" }));
    sockets.push(await openSocket(isolated.wsUrl, { origin: isolated.httpUrl }));
    sockets.push(await openSocket(isolated.wsUrl));
  } finally {
    sockets.forEach(closeSocket);
    await stopRelay(isolated.child);
  }
});

test("rate limits websocket ingress by message count", async () => {
  const isolated = await spawnRelay({
    WS_INGRESS_MESSAGE_LIMIT: "2",
    WS_INGRESS_BYTE_LIMIT: "1048576",
  });
  const desktop = await registerDesktop(
    "endpoint_ingress_count",
    "ingress-count-token-0000000000000001",
    "ingress-count-secret-000000000000001",
    isolated.wsUrl,
  );
  try {
    const unsupported = nextMessage(desktop.ws, "error");
    desktop.ws.send(JSON.stringify({ v: 2, type: "unsupported-one" }));
    assert.equal((await unsupported).code, "unsupported_message_type");

    const limited = nextMessage(desktop.ws, "error");
    desktop.ws.send(JSON.stringify({ v: 2, type: "unsupported-two" }));
    assert.equal((await limited).code, "ingress_rate_limited");
    await waitForClose(desktop.ws);
  } finally {
    closeSocket(desktop.ws);
    await stopRelay(isolated.child);
  }
});

test("rate limits websocket ingress by byte count", async () => {
  const isolated = await spawnRelay({
    WS_INGRESS_MESSAGE_LIMIT: "100",
    WS_INGRESS_BYTE_LIMIT: "1024",
  });
  const desktop = await registerDesktop(
    "endpoint_ingress_bytes",
    "ingress-bytes-token-0000000000000001",
    "ingress-bytes-secret-000000000000001",
    isolated.wsUrl,
  );
  try {
    const limited = nextMessage(desktop.ws, "error");
    desktop.ws.send(JSON.stringify({
      v: 2,
      type: "unsupported-large",
      padding: "x".repeat(1024),
    }));
    assert.equal((await limited).code, "ingress_rate_limited");
    await waitForClose(desktop.ws);
  } finally {
    closeSocket(desktop.ws);
    await stopRelay(isolated.child);
  }
});

test("clamps payload and buffer floors so a legal 2 MiB response is forwarded", async () => {
  const isolated = await spawnRelay({
    MAX_PAYLOAD_BYTES: "1024",
    WS_MAX_BUFFERED_BYTES: "1024",
  });
  const desktop = await registerDesktop(
    "endpoint_buffer_high_water",
    "buffer-token-0000000000000000000001",
    "buffer-secret-000000000000000000001",
    isolated.wsUrl,
  );
  const connected = nextMessage(desktop.ws, "web_client_connected");
  const web = await joinWeb(
    "endpoint_buffer_high_water",
    "buffer-token-0000000000000000000001",
    {},
    isolated.wsUrl,
  );
  await connected;
  try {
    const snapshot = nextMessage(web.ws, "desktop_snapshot", 5000);
    desktop.ws.send(JSON.stringify({
      v: 2,
      type: "desktop_snapshot",
      payload: { text: "x".repeat(2 * 1024 * 1024) },
    }));
    assert.equal((await snapshot).payload.text.length, 2 * 1024 * 1024);
    assert.equal(web.ws.readyState, web.ws.OPEN);
    assert.equal(desktop.ws.readyState, desktop.ws.OPEN);
  } finally {
    closeSocket(web.ws);
    closeSocket(desktop.ws);
    await stopRelay(isolated.child);
  }
});

test("caps concurrent websocket upgrades", async () => {
  const isolated = await spawnRelay({ MAX_WS_CONNECTIONS: "2", WS_CONNECT_LIMIT: "20" });
  const first = await openSocket(isolated.wsUrl);
  const second = await openSocket(isolated.wsUrl);
  try {
    assert.equal(await rejectedUpgrade(isolated.wsUrl), 503);
  } finally {
    closeSocket(first);
    closeSocket(second);
    await stopRelay(isolated.child);
  }
});

test("rate limits websocket upgrades per client", async () => {
  const isolated = await spawnRelay({ WS_CONNECT_LIMIT: "1", WS_CONNECT_WINDOW_MS: "60000" });
  const first = await openSocket(isolated.wsUrl);
  try {
    assert.equal(await rejectedUpgrade(isolated.wsUrl), 429);
  } finally {
    closeSocket(first);
    await stopRelay(isolated.child);
  }
});

test("caps endpoint creation while allowing an existing endpoint to reconnect", async () => {
  const isolated = await spawnRelay({ MAX_ENDPOINTS: "1", ENDPOINT_CREATE_LIMIT: "10" });
  const first = await registerDesktop(
    "endpoint_capacity_one",
    "capacity-token-one-0000000000000001",
    "capacity-secret-one-000000000000001",
    isolated.wsUrl,
  );
  try {
    first.ws.terminate();
    const denied = await openSocket(isolated.wsUrl);
    const deniedError = nextMessage(denied, "error");
    denied.send(JSON.stringify({
      v: 2,
      type: "desktop_endpoint_register",
      endpoint_id: "endpoint_capacity_two",
      access_token: "capacity-token-two-0000000000000002",
      desktop_secret: "capacity-secret-two-000000000000002",
    }));
    assert.equal((await deniedError).code, "endpoint_capacity_reached");

    const reconnect = await registerDesktop(
      "endpoint_capacity_one",
      "capacity-token-one-0000000000000001",
      "capacity-secret-one-000000000000001",
      isolated.wsUrl,
    );
    assert.equal(reconnect.registered.endpoint_id, "endpoint_capacity_one");
    closeSocket(reconnect.ws);
  } finally {
    closeSocket(first.ws);
    await stopRelay(isolated.child);
  }
});

test("rate limits new endpoint creation per client", async () => {
  const isolated = await spawnRelay({ ENDPOINT_CREATE_LIMIT: "1", ENDPOINT_CREATE_WINDOW_MS: "60000" });
  const first = await registerDesktop(
    "endpoint_rate_one",
    "rate-token-one-00000000000000000001",
    "rate-secret-one-000000000000000001",
    isolated.wsUrl,
  );
  try {
    const denied = await openSocket(isolated.wsUrl);
    const deniedError = nextMessage(denied, "error");
    denied.send(JSON.stringify({
      v: 2,
      type: "desktop_endpoint_register",
      endpoint_id: "endpoint_rate_two",
      access_token: "rate-token-two-00000000000000000002",
      desktop_secret: "rate-secret-two-000000000000000002",
    }));
    assert.equal((await deniedError).code, "endpoint_creation_rate_limited");
  } finally {
    closeSocket(first.ws);
    await stopRelay(isolated.child);
  }
});

test("proxy allowlist keeps local health checks available", async () => {
  const isolated = await spawnRelay({ PINVOU_REMOTE_ALLOWED_PROXY_IPS: "203.0.113.10" });
  try {
    const response = await fetch(`${isolated.httpUrl}/pinvou3/remote/healthz`);
    assert.equal(response.status, 200);
  } finally {
    await stopRelay(isolated.child);
  }
});

// The deep frames below exercise the send() serialization guard, but open PR
// #781 adds an ingress wire-nesting cap (MAX_WIRE_NESTING_DEPTH = 128) that
// rejects any frame nested deeper than that as `bad_json` before JSON.parse.
// On a tree that includes #781 the cap fires first: the relay replies to the
// sender, keeps the sender open, and never touches the destination socket, so
// the destination-termination assertions in these tests do not apply there.
// The Merge Queue gates this branch against the combined tree, so probe the
// sender first. A `bad_json` error reply means the ingress cap handled the
// frame; verify that behavior and pass — on such a tree the send() guard this
// PR adds remains as defense-in-depth for frames that parse but cannot be
// serialized. Returning false means no reply arrived, i.e. the relay under
// test has no ingress cap and the frame reached the forward path, so the
// caller proceeds with the original assertions.
async function deepFrameRejectedAtIngress(sender, destination, isolated) {
  const error = await nextMessageOrTimeout(sender.ws, "error");
  if (!error) return false;
  assert.equal(error.code, "bad_json");
  assert.equal(isolated.child.exitCode, null);
  assert.equal(isolated.child.signalCode, null);
  // #781 rejects the frame without closing anything: both sockets stay open.
  assert.equal(sender.ws.readyState, sender.ws.OPEN);
  assert.equal(destination.ws.readyState, destination.ws.OPEN);
  await expectStillOpen(destination.ws);
  return true;
}

test("survives a web frame the relay cannot serialize and keeps the desktop reconnectable", async () => {
  const isolated = await spawnRelay();
  const endpointId = `endpoint_deep_request_${Date.now()}`;
  const accessToken = "access-token-deep-request-0000000000001";
  const desktopSecret = "desktop-secret-deep-request-000000000001";
  try {
    const desktop = await registerDesktop(endpointId, accessToken, desktopSecret, isolated.wsUrl);
    const connected = nextMessage(desktop.ws, "web_client_connected");
    const web = await joinWeb(endpointId, accessToken, {}, isolated.wsUrl);
    await connected;

    // ~120 KB, far below the 4 MiB ingress cap, but 20000 levels deep:
    // JSON.parse accepts it while JSON.stringify in send() cannot.
    const deepArgs = `${'{"a":'.repeat(20000)}1${"}".repeat(20000)}`;
    web.ws.send(
      `{"v":2,"type":"rpc_request","id":"deep","client_request_id":"deep",`
      + `"command":"list_sessions","args":${deepArgs}}`,
    );

    if (await deepFrameRejectedAtIngress(web, desktop, isolated)) return;

    // The unserializable destination socket is terminated; the sending web
    // socket stays open and the frame is dropped without an error reply.
    await waitForClose(desktop.ws);
    assert.equal(web.ws.readyState, web.ws.OPEN);
    await expectNoMessage(web.ws, "error");
    assert.equal(isolated.child.exitCode, null);
    assert.equal(isolated.child.signalCode, null);

    // The endpoint still works: the desktop reconnects onto the same lease
    // and a normal round-trip completes.
    const reconnected = await registerDesktop(endpointId, accessToken, desktopSecret, isolated.wsUrl);
    assert.equal(reconnected.registered.lease_id, web.joined.lease_id);
    const forwarded = nextMessage(reconnected.ws, "rpc_request");
    web.ws.send(JSON.stringify({
      v: 2,
      type: "rpc_request",
      id: "after-deep",
      client_request_id: "after-deep",
      command: "list_sessions",
      args: { offset: 1 },
    }));
    assert.equal((await forwarded).id, "after-deep");
    const response = nextMessage(web.ws, "rpc_response");
    reconnected.ws.send(JSON.stringify({
      v: 2,
      type: "rpc_response",
      id: "after-deep",
      ok: true,
      result: [],
    }));
    assert.equal((await response).id, "after-deep");
    assert.equal(isolated.child.exitCode, null);
  } finally {
    await stopRelay(isolated.child);
  }
});

test("survives a desktop frame the relay cannot serialize and keeps the web client joined", async () => {
  const isolated = await spawnRelay();
  const endpointId = `endpoint_deep_event_${Date.now()}`;
  const accessToken = "access-token-deep-event-00000000000001";
  const desktopSecret = "desktop-secret-deep-event-0000000000001";
  try {
    const desktop = await registerDesktop(endpointId, accessToken, desktopSecret, isolated.wsUrl);
    const connected = nextMessage(desktop.ws, "web_client_connected");
    const web = await joinWeb(endpointId, accessToken, {}, isolated.wsUrl);
    await connected;

    const deepPayload = `${"[".repeat(20000)}0${"]".repeat(20000)}`;
    desktop.ws.send(`{"v":2,"type":"event","payload":${deepPayload}}`);

    if (await deepFrameRejectedAtIngress(desktop, web, isolated)) return;

    await waitForClose(web.ws);
    assert.equal(desktop.ws.readyState, desktop.ws.OPEN);
    await expectNoMessage(desktop.ws, "error");
    assert.equal(isolated.child.exitCode, null);
    assert.equal(isolated.child.signalCode, null);

    // A fresh web client joins and the desktop forwards normally again.
    const rejoined = await joinWeb(endpointId, accessToken, {}, isolated.wsUrl);
    assert.equal(rejoined.joined.desktop_connected, true);
    const event = nextMessage(rejoined.ws, "event");
    desktop.ws.send(JSON.stringify({
      v: 2,
      type: "event",
      payload: { marker: "after-deep" },
    }));
    assert.deepEqual((await event).payload, { marker: "after-deep" });
    assert.equal(isolated.child.exitCode, null);
  } finally {
    await stopRelay(isolated.child);
  }
});

test("a newer desktop registration replaces the old socket and keeps the web lease", async (t) => {
  const endpointId = `endpoint_desktop_replace_${Date.now()}`;
  const accessToken = "access-token-desktop-replace-000000001";
  const desktopSecret = "desktop-secret-desktop-replace-00000001";
  const first = await registerDesktop(endpointId, accessToken, desktopSecret);
  const connected = nextMessage(first.ws, "web_client_connected");
  const web = await joinWeb(endpointId, accessToken);
  await connected;
  const replacedNotice = nextMessage(first.ws, "desktop_endpoint_replaced");
  const replacedClose = waitForCloseInfo(first.ws);
  const second = await registerDesktop(endpointId, accessToken, desktopSecret);
  t.after(() => { closeSocket(web.ws); closeSocket(second.ws); });

  assert.deepEqual(await replacedNotice, {
    v: 2,
    type: "desktop_endpoint_replaced",
    endpoint_id: endpointId,
  });
  assert.equal(second.registered.web_client_connected, true);
  assert.equal(second.registered.lease_id, web.joined.lease_id);
  assert.deepEqual(await replacedClose, { code: 4001, reason: "desktop endpoint replaced" });

  // A late frame from the replaced desktop socket must not cross the endpoint.
  first.ws.send(JSON.stringify({
    v: 2,
    type: "rpc_request",
    id: "from-replaced",
    client_request_id: "from-replaced",
    command: "list_sessions",
    args: {},
  }));
  await expectNoMessage(second.ws, "rpc_request");

  // The untouched lease still round-trips through the replacement desktop.
  const forwarded = nextMessage(second.ws, "rpc_request");
  web.ws.send(JSON.stringify({
    v: 2,
    type: "rpc_request",
    id: "after-replace",
    client_request_id: "after-replace",
    command: "list_sessions",
    args: {},
  }));
  const received = await forwarded;
  assert.equal(received.lease_id, web.joined.lease_id);
  assert.equal(received.id, "after-replace");
});

test("a web client keeps its lease while the desktop is offline and loses it on takeover", async (t) => {
  const endpointId = `endpoint_offline_lease_${Date.now()}`;
  const accessToken = "access-token-offline-lease-000000001";
  const desktopSecret = "desktop-secret-offline-lease-00000001";
  let secondDesktop;
  let second;
  t.after(() => {
    closeSocket(web.ws);
    closeSocket(secondDesktop?.ws);
    closeSocket(second?.ws);
  });
  const firstDesktop = await registerDesktop(endpointId, accessToken, desktopSecret);
  const connected = nextMessage(firstDesktop.ws, "web_client_connected");
  const web = await joinWeb(endpointId, accessToken);
  await connected;

  const offline = nextMessage(web.ws, "desktop_connection_state");
  firstDesktop.ws.terminate();
  assert.equal((await offline).status, "offline");

  const offlineError = nextMessage(web.ws, "error");
  web.ws.send(JSON.stringify({
    v: 2,
    type: "rpc_request",
    id: "while-away",
    client_request_id: "while-away",
    command: "list_sessions",
    args: {},
  }));
  assert.equal((await offlineError).code, "desktop_offline");
  assert.equal(web.ws.readyState, web.ws.OPEN);

  secondDesktop = await registerDesktop(endpointId, accessToken, desktopSecret);
  assert.equal(secondDesktop.registered.lease_id, web.joined.lease_id);

  const forwarded = nextMessage(secondDesktop.ws, "rpc_request");
  web.ws.send(JSON.stringify({
    v: 2,
    type: "rpc_request",
    id: "after-return",
    client_request_id: "after-return",
    command: "list_sessions",
    args: {},
  }));
  assert.equal((await forwarded).lease_id, web.joined.lease_id);
  const response = nextMessage(web.ws, "rpc_response");
  secondDesktop.ws.send(JSON.stringify({
    v: 2,
    type: "rpc_response",
    id: "after-return",
    ok: true,
  }));
  assert.equal((await response).id, "after-return");

  // A takeover replaces the lease holder and closes it with 4001; its late
  // frames must not reach the desktop as if they were still active traffic.
  const replaced = nextMessage(web.ws, "endpoint_replaced");
  const replacedClose = waitForCloseInfo(web.ws);
  second = await joinWeb(endpointId, accessToken);
  assert.equal((await replaced).lease_id, web.joined.lease_id);
  assert.deepEqual(await replacedClose, { code: 4001, reason: "endpoint replaced" });

  web.ws.send(JSON.stringify({
    v: 2,
    type: "rpc_request",
    id: "stale-after-takeover",
    client_request_id: "stale-after-takeover",
    command: "list_sessions",
    args: {},
  }));
  await expectNoMessage(secondDesktop.ws, "rpc_request");

  // The replacement lease still round-trips.
  const forwardedAgain = nextMessage(secondDesktop.ws, "rpc_request");
  second.ws.send(JSON.stringify({
    v: 2,
    type: "rpc_request",
    id: "from-replacement",
    client_request_id: "from-replacement",
    command: "list_sessions",
    args: {},
  }));
  assert.equal((await forwardedAgain).lease_id, second.joined.lease_id);
});

test("reports recoverable framing errors without closing the socket", async (t) => {
  const ws = await openSocket();
  t.after(() => closeSocket(ws));

  const binary = nextMessage(ws, "error");
  ws.send(Buffer.from("not-json-bytes"));
  assert.equal((await binary).code, "binary_unsupported");

  const badJson = nextMessage(ws, "error");
  ws.send("not json{");
  assert.equal((await badJson).code, "bad_json");

  const numberFrame = nextMessage(ws, "error");
  ws.send("42");
  assert.equal((await numberFrame).code, "bad_message");

  const arrayFrame = nextMessage(ws, "error");
  ws.send("[1,2]");
  assert.equal((await arrayFrame).code, "bad_message");

  // Every framing error above was informational: the same socket still
  // authenticates as a desktop afterwards.
  const endpointId = `endpoint_framing_${Date.now()}`;
  const registered = nextMessage(ws, "desktop_endpoint_registered");
  ws.send(JSON.stringify({
    v: 2,
    type: "desktop_endpoint_register",
    endpoint_id: endpointId,
    access_token: "access-token-framing-000000000000001",
    desktop_secret: "desktop-secret-framing-0000000000001",
  }));
  assert.equal((await registered).endpoint_id, endpointId);
  assert.equal(ws.readyState, ws.OPEN);
});

test("closes a socket that forwards before authenticating", async (t) => {
  const ws = await openSocket();
  t.after(() => closeSocket(ws));
  const unauthorized = nextMessage(ws, "error");
  const closed = waitForCloseInfo(ws);
  ws.send(JSON.stringify({
    v: 2,
    type: "rpc_request",
    id: "pre-auth",
    client_request_id: "pre-auth",
    command: "list_sessions",
    args: {},
  }));
  assert.equal((await unauthorized).code, "authentication_required");
  assert.deepEqual(await closed, { code: 1008, reason: "authenticate first" });
});

test("rejects a concurrent revoke while the first is still being persisted", async () => {
  // A large tombstone state keeps the first revoke's persistence busy long
  // enough for the second revoke to land inside the in-progress window.
  const statePath = join(testRoot, `revoke-window-${Date.now()}.json`);
  const tombstones = [];
  for (let i = 0; i < 50000; i++) {
    tombstones.push(`{"endpoint_id":"endpoint_tombstone_${i}","revoked_at":"2026-01-01T00:00:00.000Z"}`);
  }
  await writeFile(statePath, `{"version":1,"revoked_endpoints":[${tombstones.join(",")}]}\n`);
  const isolated = await spawnRelay({ PINVOU_REMOTE_STATE_PATH: statePath });
  const endpointId = `endpoint_double_revoke_${Date.now()}`;
  const accessToken = "access-token-double-revoke-00000001";
  const desktopSecret = "desktop-secret-double-revoke-0000001";
  try {
    const desktop = await registerDesktop(endpointId, accessToken, desktopSecret, isolated.wsUrl);
    const connected = nextMessage(desktop.ws, "web_client_connected");
    const web = await joinWeb(endpointId, accessToken, {}, isolated.wsUrl);
    await connected;

    const revokedAck = nextMessage(desktop.ws, "desktop_endpoint_revoked", 10000);
    const inProgress = nextMessage(web.ws, "error", 10000);
    const inProgressClose = waitForCloseInfo(web.ws, 10000);
    desktop.ws.send(JSON.stringify({
      v: 2,
      type: "desktop_endpoint_revoke",
      endpoint_id: endpointId,
      reason: "first",
    }));
    // Let the relay begin persisting the first revoke before racing it:
    // a macrotask hop lets the first frame enter relayStateMutation (the
    // in-persistence flag is set synchronously on arrival), so the race is
    // deterministic instead of trusting a wall-clock sleep to lose.
    await new Promise((resolve) => setTimeout(resolve, 0));
    web.ws.send(JSON.stringify({
      v: 2,
      type: "desktop_endpoint_revoke",
      endpoint_id: endpointId,
      desktop_secret: desktopSecret,
      reason: "second",
    }));

    assert.equal((await inProgress).code, "revocation_in_progress");
    assert.deepEqual(
      await inProgressClose,
      { code: 1008, reason: "endpoint revocation is already in progress" },
    );
    assert.equal((await revokedAck).reason, "first");
    await waitForClose(desktop.ws);

    const persistedState = JSON.parse(await readFile(statePath, "utf8"));
    assert.ok(persistedState.revoked_endpoints.some((entry) => entry.endpoint_id === endpointId));
  } finally {
    await stopRelay(isolated.child);
  }
});

test("notifies the desktop when the active web client disconnects", async (t) => {
  const endpointId = `endpoint_disconnect_notice_${Date.now()}`;
  const accessToken = "access-token-disconnect-notice-000001";
  const desktopSecret = "desktop-secret-disconnect-notice-0001";
  const desktop = await registerDesktop(endpointId, accessToken, desktopSecret);
  const connected = nextMessage(desktop.ws, "web_client_connected");
  const web = await joinWeb(endpointId, accessToken);
  await connected;
  t.after(() => closeSocket(desktop.ws));

  const disconnected = nextMessage(desktop.ws, "web_client_disconnected");
  closeSocket(web.ws);
  assert.deepEqual(await disconnected, {
    v: 2,
    type: "web_client_disconnected",
    endpoint_id: endpointId,
    lease_id: web.joined.lease_id,
  });
});

test("rejects a hostile Host header without killing the relay", async () => {
  // An out-of-range port in Host makes the URL constructor throw; the relay
  // must answer 400 and keep serving, not die on an unhandled rejection.
  const poisoned = await new Promise((resolve, reject) => {
    const req = httpRequest(
      { host: "127.0.0.1", port, path: "/pinvou3/remote/healthz", headers: { host: "h:99999999" } },
      (res) => {
        res.resume();
        res.on("end", () => resolve(res.statusCode));
      },
    );
    req.on("error", reject);
    req.end();
  });
  assert.equal(poisoned, 400);
  // The process survived and still answers a well-formed probe.
  const healthy = await new Promise((resolve, reject) => {
    const req = httpRequest(`${httpUrl}/pinvou3/remote/healthz`, (res) => {
      res.resume();
      res.on("end", () => resolve(res.statusCode));
    });
    req.on("error", reject);
    req.end();
  });
  assert.equal(healthy, 200);
});

test("destroys an upgrade request with a hostile Host header without killing the relay", async () => {
  // Same poison over the upgrade path: the sync listener must not turn the
  // throw into an uncaught exception.
  await new Promise((resolve, reject) => {
    const raw = netConnect(port, "127.0.0.1");
    const timer = setTimeout(() => { raw.destroy(); resolve(); }, 1500);
    raw.on("close", () => { clearTimeout(timer); resolve(); });
    raw.on("error", () => { /* destroyed by the relay: expected */ });
    raw.write(
      "GET /pinvou3/remote/ws HTTP/1.1\r\n" +
      "Host: h:99999999\r\n" +
      "Upgrade: websocket\r\n" +
      "Connection: Upgrade\r\n" +
      // No Sec-WebSocket-Key: the hostile-Host guard fires before the
      // handshake is validated, and a credential-shaped probe value would
      // trip the secret scanner.
      "Sec-WebSocket-Version: 13\r\n\r\n",
    );
  });
  assert.equal(relay.exitCode, null);
  assert.equal(relay.signalCode, null);
  // A well-formed WebSocket connection still completes afterwards.
  const ws = await openSocket();
  assert.equal(ws.readyState, ws.OPEN);
  closeSocket(ws);
});

test("marks a rejoin after the web socket died, but not a takeover", async (t) => {
  const endpointId = `endpoint_rejoin_flag_${Date.now()}`;
  const accessToken = "access-token-rejoin-flag-00000001";
  const desktopSecret = "desktop-secret-rejoin-flag-000001";
  const desktop = await registerDesktop(endpointId, accessToken, desktopSecret);
  const firstJoin = nextMessage(desktop.ws, "web_client_connected");
  const web = await joinWeb(endpointId, accessToken);
  const first = await firstJoin;
  assert.equal(first.web_rejoining, false);
  t.after(() => closeSocket(desktop.ws));

  // Watchdog-style recovery: the previous socket dies, then the same token
  // holder rejoins. The desktop must see web_rejoining=true so it can keep
  // lease-scoped state instead of wiping it.
  const disconnected = nextMessage(desktop.ws, "web_client_disconnected");
  closeSocket(web.ws);
  await disconnected;
  const rejoinNotice = nextMessage(desktop.ws, "web_client_connected");
  const rejoined = await joinWeb(endpointId, accessToken);
  const rejoin = await rejoinNotice;
  assert.equal(rejoin.web_rejoining, true);
  assert.notEqual(rejoin.lease_id, first.lease_id);

  // A join that replaces a live client is a takeover, not a rejoin.
  const takeoverNotice = nextMessage(desktop.ws, "web_client_connected");
  const second = await joinWeb(endpointId, accessToken);
  const takeover = await takeoverNotice;
  assert.equal(takeover.web_rejoining, false);
  assert.notEqual(takeover.lease_id, rejoin.lease_id);

  // One more death-and-rejoin cycle: the flag must be recomputed to true
  // again (the takeover reset it, the fresh rejoin sets it back).
  const disconnectSecond = nextMessage(desktop.ws, "web_client_disconnected");
  closeSocket(second.ws);
  await disconnectSecond;
  const reRejoinNotice = nextMessage(desktop.ws, "web_client_connected");
  const third = await joinWeb(endpointId, accessToken);
  const reRejoin = await reRejoinNotice;
  assert.equal(reRejoin.web_rejoining, true);

  // After a REJOIN (not a takeover) dies, a desktop re-registration must
  // see the consistent no-web state (web_client_connected:false AND
  // web_rejoining:false) — the rejoin flag describes the incumbent web
  // session and dies with it; a stale true here would read as phantom
  // lease state to preserve. This is exactly the sequence the takeover
  // tail above cannot catch: only a death right after a rejoin leaves the
  // stale true behind without the close-path reset.
  const disconnectThird = nextMessage(desktop.ws, "web_client_disconnected");
  closeSocket(third.ws);
  await disconnectThird;
  closeSocket(desktop.ws);
  const reregistered = await registerDesktop(endpointId, accessToken, desktopSecret);
  t.after(() => closeSocket(reregistered.ws));
  assert.equal(reregistered.registered.web_client_connected, false);
  assert.equal(reregistered.registered.web_rejoining, false);
});
