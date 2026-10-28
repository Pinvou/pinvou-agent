import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { execFile, spawn } from "node:child_process";
import { chmod, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { createServer, connect } from "node:net";
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
      stdout += String(chunk);
      if (!pattern.test(stdout)) return;
      clearTimeout(timer);
      child.stdout.off("data", onData);
      child.stderr.off("data", onErr);
      resolve(stdout);
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
    // Resolve on "close", not "exit": only "close" guarantees stdio (which the
    // assertions below inspect) has fully drained.
    child.once("close", () => {
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

// Sends one hand-built HTTP request and returns the raw response text; used
// where the WebSocket and fetch clients refuse to send malformed headers.
function rawRequest(hostPort, requestText) {
  return new Promise((resolve, reject) => {
    const socket = connect(hostPort, "127.0.0.1", () => {
      socket.end(requestText);
    });
    // A server that accepts but never answers must fail the test, not hang CI.
    socket.setTimeout(5000, () => socket.destroy());
    let text = "";
    socket.on("data", (chunk) => { text += String(chunk); });
    socket.once("error", reject);
    socket.once("close", () => resolve(text));
  });
}

// Captures stdout/stderr for assertions. Children expected NOT to come up
// (configuration failures) are spawned with awaitListening: false.
// The relay reads a fixed set of environment variables. Strip every one of
// them from the inherited environment — the whole PINVOU_REMOTE_* family by
// prefix, plus the unprefixed tuning knobs — so a variable exported in a CI
// runner cannot silently reconfigure spawned relays. Each spawn then sets
// exactly what it needs.
const RELAY_TUNING_ENV_NAMES = new Set([
  "ENDPOINT_CREATE_LIMIT", "ENDPOINT_CREATE_WINDOW_MS", "ENDPOINT_OFFLINE_TTL_MS",
  "HEARTBEAT_INTERVAL_MS", "MAX_ENDPOINTS", "MAX_PAYLOAD_BYTES",
  "MAX_RELAY_STATE_BYTES", "MAX_REVOKED_ENDPOINTS", "MAX_ROOMS",
  "MAX_WS_CONNECTIONS", "PORT", "ROOM_CREATE_LIMIT", "ROOM_CREATE_WINDOW_MS",
  "WS_AUTH_TIMEOUT_MS", "WS_CONNECT_LIMIT", "WS_CONNECT_WINDOW_MS",
  "WS_INGRESS_BYTE_LIMIT", "WS_INGRESS_MESSAGE_LIMIT", "WS_INGRESS_WINDOW_MS",
  "WS_MAX_BUFFERED_BYTES",
]);

function relayEnvironment() {
  const inherited = { ...process.env };
  for (const name of Object.keys(inherited)) {
    if (name.startsWith("PINVOU_REMOTE_") || RELAY_TUNING_ENV_NAMES.has(name)) {
      delete inherited[name];
    }
  }
  return inherited;
}

async function spawnRelay(env = {}, { awaitListening = true } = {}) {
  const isolatedPort = await getFreePort();
  const statePath = env.PINVOU_REMOTE_STATE_PATH
    || join(testRoot, `relay-state-${isolatedPort}-${Date.now()}-${Math.random().toString(16).slice(2)}.json`);
  const child = spawn(process.execPath, [join(relayDir, "server.js")], {
    cwd: relayDir,
    env: {
      ...relayEnvironment(),
      PORT: String(isolatedPort),
      PINVOU_REMOTE_PUBLIC_BASE_PATH: "/pinvou3/remote",
      PINVOU_REMOTE_WEB_DIR: webDir,
      HEARTBEAT_INTERVAL_MS: "5000",
      PINVOU_REMOTE_STATE_PATH: statePath,
      ...env,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  let errors = "";
  child.stdout.on("data", (chunk) => { output += String(chunk); });
  child.stderr.on("data", (chunk) => { errors += String(chunk); });
  if (awaitListening) {
    // Kill before propagating: a failed wait must not orphan the child.
    try { await waitForOutput(child, /pinvou remote relay listening/); }
    catch (error) { child.kill(); throw error; }
  }
  return {
    child,
    port: isolatedPort,
    httpUrl: `http://127.0.0.1:${isolatedPort}`,
    wsUrl: `ws://127.0.0.1:${isolatedPort}/pinvou3/remote/ws`,
    statePath,
    output: () => output,
    errors: () => errors,
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
      ...relayEnvironment(),
      PORT: String(port),
      PINVOU_REMOTE_PUBLIC_BASE_PATH: "/pinvou3/remote",
      PINVOU_REMOTE_WEB_DIR: webDir,
      PINVOU_REMOTE_STATE_PATH: relayStatePath,
      HEARTBEAT_INTERVAL_MS: "5000",
      PINVOU_REMOTE_TRUSTED_PROXY_IPS: "127.0.0.1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  try { await waitForOutput(relay, /pinvou remote relay listening/); }
  catch (error) { relay.kill(); throw error; }
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

    // Persistence failures log the fixed event code and the errno name only:
    // never the endpoint id (a persistent device identifier) nor raw fs error
    // text (which embeds paths). This is the relay's only stderr output.
    assert.equal(isolated.errors().trim(), "[relay-error] revocation_persist_failed code=EEXIST");

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
  let first;
  let web;
  try {
    first = await registerDesktop(
      "endpoint_ttl_one",
      "ttl-token-one-000000000000000000001",
      "ttl-secret-one-00000000000000000001",
      isolated.wsUrl,
    );
    const connected = nextMessage(first.ws, "web_client_connected");
    web = await joinWeb(
      "endpoint_ttl_one",
      "ttl-token-one-000000000000000000001",
      {},
      isolated.wsUrl,
    );
    await connected;
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
    closeSocket(web?.ws);
    closeSocket(first?.ws);
    await stopRelay(isolated.child);
  }
});

test("sweeps stranded state temp files at startup", async () => {
  const stateDirectory = join(testRoot, `tmp-sweep-${Date.now()}`);
  await mkdir(stateDirectory, { recursive: true });
  await writeFile(join(stateDirectory, "relay-state.json.999.deadbeef.tmp"), "partial write");
  await writeFile(join(stateDirectory, "unrelated.tmp"), "keep me");
  const isolated = await spawnRelay({
    PINVOU_REMOTE_STATE_PATH: join(stateDirectory, "relay-state.json"),
  });
  try {
    const survivors = new Set(await readdir(stateDirectory));
    assert.ok(!survivors.has("relay-state.json.999.deadbeef.tmp"), "stranded temp file not swept");
    assert.ok(survivors.has("unrelated.tmp"), "sweep removed an unrelated file");
  } finally {
    await stopRelay(isolated.child);
  }
});

test("a non-numeric heartbeat falls back instead of a 1 ms NaN tick", async () => {
  // setInterval(NaN) beats at 1 ms and prints a Node warning; the bounded
  // parser must fall back to the default cadence instead.
  const isolated = await spawnRelay({ HEARTBEAT_INTERVAL_MS: "abc" });
  try {
    await new Promise((resolve) => setTimeout(resolve, 1200));
    assert.ok(!isolated.errors().includes("TimeoutNaNWarning"), "NaN heartbeat reached setInterval");
    const health = await (await fetch(`${isolated.httpUrl}/pinvou3/remote/healthz`)).json();
    assert.equal(health.endpoint_count, 0);
  } finally {
    await stopRelay(isolated.child);
  }
});

test("tombstone capacity stays inside what the state limit can hold", async () => {
  // MAX_REVOKED_ENDPOINTS is clamped to what MAX_RELAY_STATE_BYTES can hold:
  // revocations past the clamp refuse through the normal capacity path instead
  // of persisting entries until the size check fails every further revoke.
  const isolated = await spawnRelay({
    MAX_RELAY_STATE_BYTES: "2048",
    MAX_REVOKED_ENDPOINTS: "100",
    ENDPOINT_CREATE_LIMIT: "100",
  });
  try {
    for (let i = 0; i < 8; i += 1) {
      const endpointId = `endpoint_clamp_${String(i).padStart(2, "0")}`;
      const desktop = await registerDesktop(
        endpointId,
        `clamp-token-${String(i).padStart(22, "0")}`,
        `clamp-secret-${String(i).padStart(22, "0")}`,
        isolated.wsUrl,
      );
      desktop.ws.send(JSON.stringify({
        v: 2,
        type: "desktop_endpoint_revoke",
        endpoint_id: endpointId,
      }));
      assert.equal((await desktop.registered).endpoint_id, endpointId);
      await nextMessage(desktop.ws, "desktop_endpoint_revoked");
      await waitForClose(desktop.ws);
    }
    const over = await registerDesktop(
      "endpoint_clamp_over_capacity_x",
      "clamp-token-over-capacity-000000000x",
      "clamp-secret-over-capacity-00000000x",
      isolated.wsUrl,
    );
    over.ws.send(JSON.stringify({
      v: 2,
      type: "desktop_endpoint_revoke",
      endpoint_id: "endpoint_clamp_over_capacity_x",
    }));
    const refusal = await nextMessage(over.ws, "error");
    // Capacity exhaustion gets its own code: it is an expected operational
    // condition, not a persistence failure, and must stay distinguishable
    // from a broken disk.
    assert.equal(refusal.code, "revoked_endpoint_capacity_reached");
    await waitForClose(over.ws);

    const persisted = JSON.parse(await readFile(isolated.statePath, "utf8"));
    assert.equal(persisted.revoked_endpoints.length, 8);
    // The refused endpoint stays registered and usable; only the tombstone
    // is missing.
    const health = await (await fetch(`${isolated.httpUrl}/pinvou3/remote/healthz`)).json();
    assert.equal(health.endpoint_count, 1);
  } finally {
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
  let desktop;
  try {
    desktop = await registerDesktop(
      "endpoint_ingress_count",
      "ingress-count-token-0000000000000001",
      "ingress-count-secret-000000000000001",
      isolated.wsUrl,
    );
    const unsupported = nextMessage(desktop.ws, "error");
    desktop.ws.send(JSON.stringify({ v: 2, type: "unsupported-one" }));
    assert.equal((await unsupported).code, "unsupported_message_type");

    const limited = nextMessage(desktop.ws, "error");
    desktop.ws.send(JSON.stringify({ v: 2, type: "unsupported-two" }));
    assert.equal((await limited).code, "ingress_rate_limited");
    await waitForClose(desktop.ws);
  } finally {
    closeSocket(desktop?.ws);
    await stopRelay(isolated.child);
  }
});

test("rate limits websocket ingress by byte count", async () => {
  const isolated = await spawnRelay({
    WS_INGRESS_MESSAGE_LIMIT: "100",
    WS_INGRESS_BYTE_LIMIT: "1024",
  });
  let desktop;
  try {
    desktop = await registerDesktop(
      "endpoint_ingress_bytes",
      "ingress-bytes-token-0000000000000001",
      "ingress-bytes-secret-000000000000001",
      isolated.wsUrl,
    );
    const limited = nextMessage(desktop.ws, "error");
    desktop.ws.send(JSON.stringify({
      v: 2,
      type: "unsupported-large",
      padding: "x".repeat(1024),
    }));
    assert.equal((await limited).code, "ingress_rate_limited");
    await waitForClose(desktop.ws);
  } finally {
    closeSocket(desktop?.ws);
    await stopRelay(isolated.child);
  }
});

test("clamps payload and buffer floors so a legal 2 MiB response is forwarded", async () => {
  const isolated = await spawnRelay({
    MAX_PAYLOAD_BYTES: "1024",
    WS_MAX_BUFFERED_BYTES: "1024",
  });
  let desktop;
  let web;
  try {
    desktop = await registerDesktop(
      "endpoint_buffer_high_water",
      "buffer-token-0000000000000000000001",
      "buffer-secret-000000000000000000001",
      isolated.wsUrl,
    );
    const connected = nextMessage(desktop.ws, "web_client_connected");
    web = await joinWeb(
      "endpoint_buffer_high_water",
      "buffer-token-0000000000000000000001",
      {},
      isolated.wsUrl,
    );
    await connected;
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
    closeSocket(web?.ws);
    closeSocket(desktop?.ws);
    await stopRelay(isolated.child);
  }
});

test("caps concurrent websocket upgrades", async () => {
  const isolated = await spawnRelay({ MAX_WS_CONNECTIONS: "2", WS_CONNECT_LIMIT: "20" });
  let first;
  let second;
  try {
    first = await openSocket(isolated.wsUrl);
    second = await openSocket(isolated.wsUrl);
    assert.equal(await rejectedUpgrade(isolated.wsUrl), 503);
  } finally {
    closeSocket(first);
    closeSocket(second);
    await stopRelay(isolated.child);
  }
});

test("rate limits websocket upgrades per client", async () => {
  const isolated = await spawnRelay({ WS_CONNECT_LIMIT: "1", WS_CONNECT_WINDOW_MS: "60000" });
  let first;
  try {
    first = await openSocket(isolated.wsUrl);
    assert.equal(await rejectedUpgrade(isolated.wsUrl), 429);
  } finally {
    closeSocket(first);
    await stopRelay(isolated.child);
  }
});

test("caps endpoint creation while allowing an existing endpoint to reconnect", async () => {
  const isolated = await spawnRelay({ MAX_ENDPOINTS: "1", ENDPOINT_CREATE_LIMIT: "10" });
  let first;
  try {
    first = await registerDesktop(
      "endpoint_capacity_one",
      "capacity-token-one-0000000000000001",
      "capacity-secret-one-000000000000001",
      isolated.wsUrl,
    );
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
    closeSocket(first?.ws);
    await stopRelay(isolated.child);
  }
});

test("rate limits new endpoint creation per client", async () => {
  const isolated = await spawnRelay({ ENDPOINT_CREATE_LIMIT: "1", ENDPOINT_CREATE_WINDOW_MS: "60000" });
  let first;
  try {
    first = await registerDesktop(
      "endpoint_rate_one",
      "rate-token-one-00000000000000000001",
      "rate-secret-one-000000000000000001",
      isolated.wsUrl,
    );
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
    closeSocket(first?.ws);
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

test("rejects a duplicate version key before JSON.parse collapses it", async (t) => {
  const ws = await openSocket();
  t.after(() => closeSocket(ws));
  // Last-wins parsing would turn each of these into a v2 frame; the wire-level
  // guard must reject them as bad_json instead, whether the repeated key is
  // spelled plainly, escaped, or sits in a nested object.
  for (const frame of [
    '{"v":1,"v":2,"type":"web_client_join"}',
    '{"\\u0076":1,"v":2,"type":"web_client_join"}',
    '{"v":2,"type":"web_client_join","args":[{"v":1,"v":2}]}',
  ]) {
    const error = nextMessage(ws, "error");
    ws.send(frame);
    assert.equal((await error).code, "bad_json", frame);
  }
});

test("invalid or partially invalid proxy IP configuration fails startup instead of opening the allowlist", async () => {
  for (const variable of ["PINVOU_REMOTE_ALLOWED_PROXY_IPS", "PINVOU_REMOTE_TRUSTED_PROXY_IPS"]) {
    // The last value is a valid address padded past the IP text cap: length is
    // checked before trimming, so it must be rejected like any other garbage.
    for (const value of ["not-an-ip", "127.0.0.1,not-an-ip", ",,,", `1.2.3.4${" ".repeat(140)}`]) {
      const isolated = await spawnRelay({ [variable]: value }, { awaitListening: false });
      try {
        await waitForExit(isolated.child, 10000);
        assert.notEqual(isolated.child.exitCode, 0);
        assert.equal(
          isolated.errors().trim(),
          `[relay-error] proxy_ip_configuration_invalid: ${variable}`,
        );
        assert.doesNotMatch(isolated.output(), /pinvou remote relay listening/u);
      } finally {
        await stopRelay(isolated.child);
      }
    }
  }
});

test("survives a websocket protocol violation that emits an error event", async () => {
  const isolated = await spawnRelay({});
  let ws;
  try {
    ws = await openSocket(isolated.wsUrl);
    // A frame over maxPayload makes ws emit "error" on the socket; without a
    // listener the unhandled event crashed the whole relay process.
    ws.send("x".repeat(4 * 1024 * 1024 + 8));
    await waitForClose(ws);
    // The containment must stay observable: one fixed line, no frame content.
    assert.match(
      isolated.errors(),
      /\[relay-error\] websocket_protocol_violation code=WS_ERR_UNSUPPORTED_MESSAGE_LENGTH/u,
    );
    const follower = await openSocket(isolated.wsUrl);
    closeSocket(follower);
    assert.equal(isolated.child.exitCode, null);
  } finally {
    closeSocket(ws);
    await stopRelay(isolated.child);
  }
});

test("rejects malformed Host headers instead of crashing", async () => {
  const isolated = await spawnRelay({});
  try {
    const httpBanner = await rawRequest(
      isolated.port,
      "GET /pinvou3/remote/healthz HTTP/1.1\r\n"
      + "Host: ::1:99999\r\n"
      + "Connection: close\r\n\r\n",
    );
    assert.match(httpBanner, /400 Bad Request/u);
    assert.match(httpBanner, /bad request/u);
    const upgradeBanner = await rawRequest(
      isolated.port,
      "GET /pinvou3/remote/ws HTTP/1.1\r\n"
      + "Host: ::1:99999\r\n"
      + "Upgrade: websocket\r\nConnection: Upgrade\r\n"
      + "Sec-WebSocket-Key: AAAAAAAAAAAAAAAAAAAAAA==\r\n"
      + "Sec-WebSocket-Version: 13\r\n\r\n",
    );
    assert.match(upgradeBanner, /400 Bad Request/u);
    assert.match(upgradeBanner, /malformed host header/u);
    const follower = await openSocket(isolated.wsUrl);
    closeSocket(follower);
    assert.equal(isolated.child.exitCode, null);
  } finally {
    await stopRelay(isolated.child);
  }
});

test("attributes trusted-proxy connections to the rightmost forwarded address", async () => {
  const isolated = await spawnRelay({
    WS_CONNECT_LIMIT: "2",
    WS_CONNECT_WINDOW_MS: "60000",
    PINVOU_REMOTE_TRUSTED_PROXY_IPS: "127.0.0.1",
  });
  let first;
  let second;
  let own;
  let mapped;
  try {
    // A garbage mapped prefix and a valid item followed by a junk final item
    // both normalize to nothing: the connection must collapse onto the peer
    // address instead of attacker-chosen junk.
    first = await openSocket(isolated.wsUrl, { headers: { "x-forwarded-for": "::ffff:not-an-ip" } });
    second = await openSocket(isolated.wsUrl, { headers: { "x-forwarded-for": `1.2.3.4, ${"a".repeat(200)}` } });
    // The peer bucket is now full, so a connection with no XFF is rejected...
    assert.equal(await rejectedUpgrade(isolated.wsUrl), 429);
    // ...and so is one whose header exceeds the forwarded-header cap, even
    // though its final item is a perfectly valid address.
    assert.equal(await rejectedUpgrade(isolated.wsUrl, {
      headers: { "x-forwarded-for": `${"a".repeat(4200)}, 1.2.3.4` },
    }), 429);
    // ...while a valid forwarded address still gets its own bucket, which the
    // IPv4-mapped spelling of the same address shares.
    own = await openSocket(isolated.wsUrl, { headers: { "x-forwarded-for": "198.51.100.7" } });
    mapped = await openSocket(isolated.wsUrl, { headers: { "x-forwarded-for": "::ffff:198.51.100.7" } });
    assert.equal(await rejectedUpgrade(isolated.wsUrl, {
      headers: { "x-forwarded-for": "198.51.100.7" },
    }), 429);
  } finally {
    closeSocket(first);
    closeSocket(second);
    closeSocket(own);
    closeSocket(mapped);
    await stopRelay(isolated.child);
  }
});

test("matches proxy and forwarded addresses case-insensitively", async () => {
  // A valid-but-non-canonical spelling must still trust the proxy: IPv6 text
  // case carries no meaning, and a silently dead trust entry collapses every
  // client behind the proxy into one shared bucket.
  const isolated = await spawnRelay({
    WS_CONNECT_LIMIT: "1",
    PINVOU_REMOTE_TRUSTED_PROXY_IPS: "::FFFF:127.0.0.1",
  });
  let first;
  let second;
  try {
    first = await openSocket(isolated.wsUrl, { headers: { "x-forwarded-for": "198.51.100.9" } });
    assert.equal(await rejectedUpgrade(isolated.wsUrl, {
      headers: { "x-forwarded-for": "198.51.100.9" },
    }), 429);
    second = await openSocket(isolated.wsUrl);
  } finally {
    closeSocket(first);
    closeSocket(second);
    await stopRelay(isolated.child);
  }
});

test("folds case in forwarded address text before bucketing", async () => {
  // Case folding must cover header-derived text, not only configured
  // entries: otherwise one uppercase-spelled forwarded address opens a
  // second bucket for the address its lowercase spelling already filled.
  const isolated = await spawnRelay({
    WS_CONNECT_LIMIT: "1",
    PINVOU_REMOTE_TRUSTED_PROXY_IPS: "127.0.0.1",
  });
  let first;
  try {
    first = await openSocket(isolated.wsUrl, { headers: { "x-forwarded-for": "2001:DB8::25" } });
    assert.equal(await rejectedUpgrade(isolated.wsUrl, {
      headers: { "x-forwarded-for": "2001:db8::25" },
    }), 429);
  } finally {
    closeSocket(first);
    await stopRelay(isolated.child);
  }
});

test("the forwarded-header cap is exact at 4096 characters", async () => {
  const isolated = await spawnRelay({
    WS_CONNECT_LIMIT: "1",
    PINVOU_REMOTE_TRUSTED_PROXY_IPS: "127.0.0.1",
  });
  let plain;
  let atCap;
  try {
    plain = await openSocket(isolated.wsUrl);
    // Exactly at the cap the final item still attributes.
    const atCapHeader = `${"a".repeat(4087)}, 1.2.3.4`;
    assert.equal(atCapHeader.length, 4096);
    atCap = await openSocket(isolated.wsUrl, { headers: { "x-forwarded-for": atCapHeader } });
    assert.equal(await rejectedUpgrade(isolated.wsUrl, {
      headers: { "x-forwarded-for": atCapHeader },
    }), 429);
    // One character past the cap collapses onto the peer, whose bucket is
    // already full: the request must be refused even though its final item
    // names a fresh address.
    const overCapHeader = `${"a".repeat(4088)}, 1.2.3.5`;
    assert.equal(overCapHeader.length, 4097);
    assert.equal(await rejectedUpgrade(isolated.wsUrl, {
      headers: { "x-forwarded-for": overCapHeader },
    }), 429);
  } finally {
    closeSocket(plain);
    closeSocket(atCap);
    await stopRelay(isolated.child);
  }
});

test("shuts down with exit code 0 on repeated SIGTERM and on SIGINT", async () => {
  for (const [signal, repeats] of [["SIGTERM", 2], ["SIGINT", 3]]) {
    const isolated = await spawnRelay({});
    let ws;
    try {
      ws = await openSocket(isolated.wsUrl);
      // Repeated delivery must still end in exit 0, never in death by
      // signal: the guard makes shutdown idempotent and the handler stays
      // registered.
      for (let i = 0; i < repeats; i += 1) isolated.child.kill(signal);
      await waitForExit(isolated.child, 5000);
      assert.equal(isolated.child.exitCode, 0);
      assert.equal(isolated.child.signalCode, null);
      assert.doesNotMatch(isolated.output(), /listening on /u);
    } finally {
      closeSocket(ws);
      await stopRelay(isolated.child);
    }
  }
});

test("configuration errors do not echo the offending value", async () => {
  // Both parseOriginSet failure shapes: a parseable origin with a forbidden
  // scheme, and text that fails URL construction outright. The failure is one
  // fixed [relay-error] line: no stack, no offending value.
  const cases = [
    ["https://good.example, ftp://evil.example", "invalid browser WebSocket origin"],
    ["not a url", "invalid PINVOU_REMOTE_ALLOWED_WEB_ORIGINS entry"],
  ];
  for (const [value, message] of cases) {
    const isolated = await spawnRelay(
      { PINVOU_REMOTE_ALLOWED_WEB_ORIGINS: value },
      { awaitListening: false },
    );
    try {
      await waitForExit(isolated.child, 10000);
      assert.notEqual(isolated.child.exitCode, 0);
      assert.equal(isolated.errors().trim(), `[relay-error] ${message}`);
      assert.ok(!isolated.errors().includes(value), "offending origin echoed into stderr");
    } finally {
      await stopRelay(isolated.child);
    }
  }
});

test("an invalid PORT reports the fixed line instead of a stack", async () => {
  // listen() validates the port synchronously, so an invalid value dies
  // inside the call with a RangeError whose stack prints the module path.
  for (const value of ["abc", "99999", "-1", "1.5", "0"]) {
    const isolated = await spawnRelay({ PORT: value }, { awaitListening: false });
    try {
      await waitForExit(isolated.child, 10000);
      assert.notEqual(isolated.child.exitCode, 0);
      assert.equal(
        isolated.errors().trim(),
        "[relay-error] invalid PORT: use an integer from 1 to 65535",
      );
    } finally {
      await stopRelay(isolated.child);
    }
  }
});

test("relay state load failures surface a fixed message without paths", async (t) => {
  if (process.platform === "win32") {
    t.skip("Windows maps stat through a regular file to ENOENT, not ENOTDIR");
    return;
  }
  // A regular file where a directory is expected makes stat() fail with
  // ENOTDIR, whose message and `path` property both carry the full state path;
  // only a fixed code plus the errno name may reach stderr.
  const blocker = join(testRoot, `state-blocker-${Date.now()}`);
  await writeFile(blocker, "not-a-directory");
  const statePath = join(blocker, "relay-state.json");
  const isolated = await spawnRelay(
    { PINVOU_REMOTE_STATE_PATH: statePath },
    { awaitListening: false },
  );
  try {
    await waitForExit(isolated.child, 10000);
    assert.notEqual(isolated.child.exitCode, 0);
    assert.equal(isolated.errors().trim(), "[relay-error] relay_state_load_failed code=ENOTDIR");
  } finally {
    await stopRelay(isolated.child);
  }
});

test("a state above the clamped capacity loads and refuses only new revocations", async () => {
  // The tombstone clamp ties MAX_REVOKED_ENDPOINTS to MAX_RELAY_STATE_BYTES.
  // A state file accumulated above the clamp must still load — the size
  // limit is the real bound, every loaded tombstone stays enforced, and
  // refusing to start would brick the relay on a valid file. Only new
  // revocations are refused, with the capacity code instead of a persistence
  // failure.
  const statePath = join(testRoot, `clamped-state-${Date.now()}.json`);
  const entries = [];
  for (let i = 0; i < 9; i += 1) {
    entries.push({ endpoint_id: `clamp${i}`, revoked_at: "2026-01-01T00:00:00Z" });
  }
  await writeFile(statePath, JSON.stringify({ version: 1, revoked_endpoints: entries }));
  const isolated = await spawnRelay({
    PINVOU_REMOTE_STATE_PATH: statePath,
    MAX_RELAY_STATE_BYTES: "2048",
    ENDPOINT_CREATE_LIMIT: "100",
  });
  try {
    assert.match(isolated.output(), /revoked_endpoints=9/u);
    const desktop = await registerDesktop(
      "endpoint_clamped_capacity_x",
      "clamped-token-0000000000000000000x",
      "clamped-secret-000000000000000000x",
      isolated.wsUrl,
    );
    desktop.ws.send(JSON.stringify({
      v: 2,
      type: "desktop_endpoint_revoke",
      endpoint_id: "endpoint_clamped_capacity_x",
    }));
    const refusal = await nextMessage(desktop.ws, "error");
    assert.equal(refusal.code, "revoked_endpoint_capacity_reached");
    await waitForClose(desktop.ws);
    const persisted = JSON.parse(await readFile(statePath, "utf8"));
    assert.equal(persisted.revoked_endpoints.length, 9);
  } finally {
    await stopRelay(isolated.child);
  }
});

test("loads a state larger than today's default capacity and refuses only new revocations", async () => {
  // The upgrade shape of the same rule: an older relay could legitimately
  // persist more tombstones than today's derivation allows (65,536 at the
  // defaults; real tombstones are ~96-213 bytes, the old capacity 100,000).
  // The relay must start and keep every loaded tombstone enforced.
  const entries = Array.from({ length: 70_000 }, (_, i) => ({
    endpoint_id: `tomb_${String(i).padStart(8, "0")}_xxxx`,
    revoked_at: "2026-01-01T00:00:00.000Z",
  }));
  const statePath = join(testRoot, `state-over-capacity-${Date.now()}.json`);
  await writeFile(statePath, JSON.stringify({ version: 1, revoked_endpoints: entries }));
  const isolated = await spawnRelay({ PINVOU_REMOTE_STATE_PATH: statePath });
  try {
    assert.match(isolated.output(), /revoked_endpoints=70000/u);
    const health = await (await fetch(`${isolated.httpUrl}/pinvou3/remote/healthz`)).json();
    assert.equal(health.endpoint_count, 0);
    // A new revocation is refused by the write-side capacity with its own
    // code — not as a persistence failure — and the loaded file is untouched.
    const desktop = await registerDesktop(
      "endpoint_over_capacity_xxxx",
      "capacity-token-000000000000000000x",
      "capacity-secret-00000000000000000x",
      isolated.wsUrl,
    );
    desktop.ws.send(JSON.stringify({
      v: 2,
      type: "desktop_endpoint_revoke",
      endpoint_id: "endpoint_over_capacity_xxxx",
    }));
    const refusal = await nextMessage(desktop.ws, "error");
    assert.equal(refusal.code, "revoked_endpoint_capacity_reached");
    await waitForClose(desktop.ws);
    const persisted = JSON.parse(await readFile(statePath, "utf8"));
    assert.equal(persisted.revoked_endpoints.length, 70_000);
  } finally {
    await stopRelay(isolated.child);
  }
});

test("a fifo at the state path fails startup instead of blocking the read", async (t) => {
  if (process.platform === "win32") {
    t.skip("Windows has no mkfifo");
    return;
  }
  // The regular-file check must precede readFile(): a FIFO at the state path
  // would otherwise block startup forever with no diagnostic and no signal
  // handlers registered yet.
  const fifoPath = join(testRoot, `relay-state-fifo-${Date.now()}`);
  await new Promise((resolve, reject) => {
    execFile("mkfifo", [fifoPath], (error) => (error ? reject(error) : resolve()));
  });
  const isolated = await spawnRelay(
    { PINVOU_REMOTE_STATE_PATH: fifoPath },
    { awaitListening: false },
  );
  try {
    await waitForExit(isolated.child, 10000);
    assert.notEqual(isolated.child.exitCode, 0);
    assert.equal(
      isolated.errors().trim(),
      "[relay-error] relay state path is not a regular file",
    );
  } finally {
    await stopRelay(isolated.child);
  }
});

test("accepts frames nested exactly to the wire depth cap", async (t) => {
  const ws = await openSocket();
  t.after(() => closeSocket(ws));
  const error = nextMessage(ws, "error");
  // Well inside the cap, with a scalar at the bottom. It must clear the wire
  // guard and reach the protocol layer, which rejects it only for lacking
  // authentication.
  ws.send(`{"v":2,"payload":${"[".repeat(127)}0${"]".repeat(127)}}`);
  assert.equal((await error).code, "authentication_required");
  // The cap counts enclosing containers, so a container as the deepest value
  // is accepted one level deeper than a scalar would be. A fresh socket: the
  // first rejection already closed this one.
  const ws2 = await openSocket();
  t.after(() => closeSocket(ws2));
  const inner = nextMessage(ws2, "error");
  ws2.send(`{"v":2,"payload":${"[".repeat(128)}${"]".repeat(128)}}`);
  assert.equal((await inner).code, "authentication_required");
});

test("scans strings with escaped quotes and backslashes without false rejects", async (t) => {
  const ws = await openSocket();
  t.after(() => closeSocket(ws));
  const error = nextMessage(ws, "error");
  // Backslash parity in the scanner's string skipping: escapes inside string
  // values must scan as plain string content and reach the protocol layer,
  // never a false duplicate-key or malformed verdict.
  ws.send('{"v":2,"payload":["say \\"hi\\"", "a\\\\b", "\u0076"]}');
  assert.equal((await error).code, "authentication_required");
});

test("scans a many-key frame in bounded time", async (t) => {
  const ws = await openSocket();
  t.after(() => closeSocket(ws));
  // The scanner's escape probe must stay bounded by the key text. An
  // unbounded search to the end of the document made every key pay a scan of
  // the whole remaining frame, so one unauthenticated many-key frame stalled
  // the event loop for seconds; this frame scans in milliseconds when
  // bounded and for many seconds when not.
  const parts = new Array(220000);
  for (let i = 0; i < parts.length; i += 1) parts[i] = `"k${String(i).padStart(9, "0")}":1`;
  const reply = nextMessage(ws, "error", 5000);
  const started = performance.now();
  ws.send(`{${parts.join(",")}}`);
  assert.equal((await reply).code, "unsupported_protocol_version");
  assert(performance.now() - started < 2000, `scan took ${performance.now() - started} ms`);
});

test("a blank trusted proxy list is empty while an unset one inherits the allowlist", async () => {
  const spawnWithPeerBucketFull = async (env) => {
    const isolated = await spawnRelay({
      WS_CONNECT_LIMIT: "2",
      WS_CONNECT_WINDOW_MS: "60000",
      PINVOU_REMOTE_ALLOWED_PROXY_IPS: "127.0.0.1",
      ...env,
    });
    const sockets = [];
    try {
      sockets.push(await openSocket(isolated.wsUrl));
      sockets.push(await openSocket(isolated.wsUrl));
      return { isolated, sockets };
    } catch (error) {
      sockets.forEach(closeSocket);
      await stopRelay(isolated.child);
      throw error;
    }
  };
  // With the loopback peer's bucket full, a forwarded address opens a fresh
  // bucket only if the peer is a trusted proxy.
  const forwarded = { headers: { "x-forwarded-for": "198.51.100.7" } };
  const inherited = await spawnWithPeerBucketFull({});
  try {
    const own = await openSocket(inherited.isolated.wsUrl, forwarded);
    closeSocket(own);
  } finally {
    inherited.sockets.forEach(closeSocket);
    await stopRelay(inherited.isolated.child);
  }
  for (const value of ["", "   "]) {
    const blank = await spawnWithPeerBucketFull({ PINVOU_REMOTE_TRUSTED_PROXY_IPS: value });
    try {
      assert.equal(await rejectedUpgrade(blank.isolated.wsUrl, forwarded), 429);
    } finally {
      blank.sockets.forEach(closeSocket);
      await stopRelay(blank.isolated.child);
    }
  }
});

test("serves 503 instead of crashing when a web asset becomes unreadable", async (t) => {
  if (process.platform === "win32") {
    t.skip("file modes do not restrict read access on Windows");
    return;
  }
  if (process.getuid?.() === 0) {
    t.skip("file modes do not restrict root");
    return;
  }
  const unreadableWebDir = join(testRoot, `web-unreadable-${Date.now()}`);
  await mkdir(unreadableWebDir, { recursive: true });
  const indexPath = join(unreadableWebDir, "index.html");
  await writeFile(indexPath, "<html></html>");
  await chmod(indexPath, 0o000);
  const isolated = await spawnRelay({ PINVOU_REMOTE_WEB_DIR: unreadableWebDir });
  try {
    // stat() still succeeds on a mode-000 file, so only the read fails: the
    // same shape as a redeploy replacing files under a request.
    const response = await fetch(`${isolated.httpUrl}/pinvou3/remote/`);
    assert.equal(response.status, 503);
    assert.equal(await response.text(), "WebUI build is not available");
    // HEAD reads too, so its status can never disagree with the GET.
    const head = await fetch(`${isolated.httpUrl}/pinvou3/remote/`, { method: "HEAD" });
    assert.equal(head.status, 503);
    const follower = await openSocket(isolated.wsUrl);
    closeSocket(follower);
    assert.equal(isolated.child.exitCode, null);
  } finally {
    await chmod(indexPath, 0o600).catch(() => {});
    await stopRelay(isolated.child);
  }
});

test("serves 503 for the site root when the web build is missing entirely", async () => {
  const emptyWebDir = join(testRoot, `web-empty-${Date.now()}`);
  await mkdir(emptyWebDir, { recursive: true });
  const isolated = await spawnRelay({ PINVOU_REMOTE_WEB_DIR: emptyWebDir });
  try {
    // The site entry is the build: a missing index.html is "build is not
    // available" (503), not an ordinary 404. A missing asset with an
    // extension stays a plain 404 (a stale reference to a replaced build),
    // and an extensionless SPA route falls back to the same missing entry.
    const root = await fetch(`${isolated.httpUrl}/pinvou3/remote/`);
    assert.equal(root.status, 503);
    assert.equal(await root.text(), "WebUI build is not available");
    // HEAD reads the same site entry, so an uptime probe cannot disagree
    // with the GET that follows it.
    const rootHead = await fetch(`${isolated.httpUrl}/pinvou3/remote/`, { method: "HEAD" });
    assert.equal(rootHead.status, 503);
    assert.equal(
      (await fetch(`${isolated.httpUrl}/pinvou3/remote/staleAsset-12345678.js`)).status,
      404,
    );
    assert.equal(
      (await fetch(`${isolated.httpUrl}/pinvou3/remote/some/route`)).status,
      503,
    );
  } finally {
    await stopRelay(isolated.child);
  }
});

test("matches expanded and mapped ip spellings canonically", async () => {
  // The trust entry is spelled in fully expanded form; the loopback peer
  // reports ::1. Trust must hold so the forwarded connection opens a private
  // bucket, and the plain connection must then consume the peer bucket.
  const isolated = await spawnRelay({
    PINVOU_REMOTE_HOST: "::",
    WS_CONNECT_LIMIT: "1",
    PINVOU_REMOTE_TRUSTED_PROXY_IPS: "0:0:0:0:0:0:0:1",
  });
  const loopbackUrl = `ws://[::1]:${isolated.port}/pinvou3/remote/ws`;
  let first;
  let second;
  try {
    first = await openSocket(loopbackUrl, { headers: { "x-forwarded-for": "198.51.100.9" } });
    assert.equal(await rejectedUpgrade(loopbackUrl, {
      headers: { "x-forwarded-for": "198.51.100.9" },
    }), 429);
    second = await openSocket(loopbackUrl);
  } finally {
    closeSocket(first);
    closeSocket(second);
    await stopRelay(isolated.child);
  }
});

test("a forwarded ipv6 item shares its canonical bucket", async () => {
  const isolated = await spawnRelay({
    WS_CONNECT_LIMIT: "1",
    PINVOU_REMOTE_TRUSTED_PROXY_IPS: "127.0.0.1",
  });
  let first;
  try {
    first = await openSocket(isolated.wsUrl, { headers: { "x-forwarded-for": "2001:db8::1" } });
    // The fully expanded, uppercase spelling of the same address must land in
    // the one bucket instead of opening a fresh one.
    assert.equal(await rejectedUpgrade(isolated.wsUrl, {
      headers: { "x-forwarded-for": "2001:0DB8:0000:0000:0000:0000:0000:0001" },
    }), 429);
  } finally {
    closeSocket(first);
    await stopRelay(isolated.child);
  }
});

test("zone id entries start on every supported runtime", async () => {
  // The zone suffix is split off and validated here, so acceptance no longer
  // depends on the runtime's own zone-id support.
  const isolated = await spawnRelay({
    PINVOU_REMOTE_ALLOWED_PROXY_IPS: "FE80::1%wlp9s0",
  });
  try {
    const health = await (await fetch(`${isolated.httpUrl}/pinvou3/remote/healthz`)).json();
    assert.equal(health.endpoint_count, 0);
  } finally {
    await stopRelay(isolated.child);
  }
});

test("consults X-Real-IP only when X-Forwarded-For is entirely absent", async () => {
  const isolated = await spawnRelay({
    WS_CONNECT_LIMIT: "1",
    WS_CONNECT_WINDOW_MS: "60000",
    PINVOU_REMOTE_TRUSTED_PROXY_IPS: "127.0.0.1",
  });
  let realIp;
  let peer;
  try {
    // With no forwarded header at all, X-Real-IP chooses the bucket.
    realIp = await openSocket(isolated.wsUrl, { headers: { "x-real-ip": "198.51.100.7" } });
    assert.equal(await rejectedUpgrade(isolated.wsUrl, {
      headers: { "x-real-ip": "198.51.100.7" },
    }), 429);
    // A present-but-unusable forwarded header outranks X-Real-IP: the
    // connection falls back to the peer bucket, and a second connection with
    // the same unusable header must not open a bucket for its X-Real-IP.
    peer = await openSocket(isolated.wsUrl, {
      headers: { "x-forwarded-for": "not-an-ip", "x-real-ip": "203.0.113.9" },
    });
    assert.equal(await rejectedUpgrade(isolated.wsUrl, {
      headers: { "x-forwarded-for": "", "x-real-ip": "203.0.113.9" },
    }), 429);
    assert.equal(await rejectedUpgrade(isolated.wsUrl), 429);
  } finally {
    closeSocket(realIp);
    closeSocket(peer);
    await stopRelay(isolated.child);
  }
});

test("state validation failures name their own reason", async () => {
  const cases = [
    ["definitely not json", "relay_state_load_failed: invalid JSON"],
    [`{"version":1,"revoked_endpoints":[{"endpoint_id":"ab","revoked_at":"2026-01-01T00:00:00.000Z"}]}`,
      "relay_state_load_failed: malformed revoked endpoint"],
    [`{"version":1,"revoked_endpoints":"not-an-array"}`,
      "relay_state_load_failed: unsupported or malformed state"],
  ];
  for (const [contents, message] of cases) {
    const statePath = join(testRoot, `state-shape-${Date.now()}-${Math.random().toString(16).slice(2)}.json`);
    await writeFile(statePath, contents);
    const isolated = await spawnRelay(
      { PINVOU_REMOTE_STATE_PATH: statePath },
      { awaitListening: false },
    );
    try {
      await waitForExit(isolated.child, 10000);
      assert.notEqual(isolated.child.exitCode, 0);
      assert.equal(isolated.errors().trim(), `[relay-error] ${message}`);
    } finally {
      await stopRelay(isolated.child);
    }
  }
});

test("relay state exceeding the configured maximum names the limit", async () => {
  const statePath = join(testRoot, `oversize-state-${Date.now()}.json`);
  await writeFile(statePath, "0".repeat(2048));
  const isolated = await spawnRelay(
    { PINVOU_REMOTE_STATE_PATH: statePath, MAX_RELAY_STATE_BYTES: "1024" },
    { awaitListening: false },
  );
  try {
    await waitForExit(isolated.child, 10000);
    assert.notEqual(isolated.child.exitCode, 0);
    assert.equal(
      isolated.errors().trim(),
      "[relay-error] relay state exceeds MAX_RELAY_STATE_BYTES",
    );
  } finally {
    await stopRelay(isolated.child);
  }
});

test("a failed bind reports a fixed error without the bind address", async () => {
  const first = await spawnRelay({});
  try {
    const second = await spawnRelay({ PORT: String(first.port) }, { awaitListening: false });
    try {
      await waitForExit(second.child, 10000);
      assert.notEqual(second.child.exitCode, 0);
      assert.equal(second.errors().trim(), "[relay-error] listen_failed code=EADDRINUSE");
      assert.ok(!second.errors().includes(String(first.port)), "bind address echoed into stderr");
    } finally {
      await stopRelay(second.child);
    }
  } finally {
    await stopRelay(first.child);
  }
});

test("rejects frames nested beyond the wire depth cap as bad_json", async (t) => {
  const ws = await openSocket();
  t.after(() => closeSocket(ws));
  const error = nextMessage(ws, "error");
  // The wire guard caps nesting at 128 levels. JSON.parse alone would accept
  // this frame and fail later with a less specific rejection.
  ws.send(`{"payload":${"[".repeat(200)}${"]".repeat(200)}}`);
  assert.equal((await error).code, "bad_json");
  // One container past the scalar boundary in the accepts test.
  const inner = nextMessage(ws, "error");
  ws.send(`{"v":2,"payload":${"[".repeat(129)}${"]".repeat(129)}}`);
  assert.equal((await inner).code, "bad_json");
});
