import http from "node:http";
import crypto from "node:crypto";
import { chmod, mkdir, open, readFile, rename, rm, stat } from "node:fs/promises";
import { dirname, extname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { WebSocketServer } from "ws";
import { createTelemetryService } from "./telemetry-service.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const MIB = 1024 * 1024;
const RELAY_STATE_VERSION = 1;
const PORT = Number(process.env.PORT || 8787);
const PUBLIC_BASE_PATH = normalizeBasePath(
  process.env.PINVOU_REMOTE_PUBLIC_BASE_PATH || "/pinvou3/remote",
);
const WEB_ROOT = resolve(process.env.PINVOU_REMOTE_WEB_DIR || resolve(__dirname, "web", "dist"));
const HEARTBEAT_INTERVAL_MS = Math.max(
  5000,
  Number(process.env.HEARTBEAT_INTERVAL_MS || 15_000),
);
// The v2 RPC contract permits up to 1 MiB of JSON args. Base64-heavy requests
// and normal envelope overhead need a Relay ceiling comfortably above that.
const MAX_PAYLOAD_BYTES = boundedInteger(
  process.env.MAX_PAYLOAD_BYTES,
  4 * MIB,
  4 * MIB,
  16 * MIB,
);
const MAX_ENDPOINTS = boundedInteger(
  process.env.MAX_ENDPOINTS ?? process.env.MAX_ROOMS,
  2000,
  1,
  100_000,
);
const ENDPOINT_CREATE_LIMIT = boundedInteger(
  process.env.ENDPOINT_CREATE_LIMIT ?? process.env.ROOM_CREATE_LIMIT,
  20,
  1,
  10_000,
);
const ENDPOINT_CREATE_WINDOW_MS = boundedInteger(
  process.env.ENDPOINT_CREATE_WINDOW_MS ?? process.env.ROOM_CREATE_WINDOW_MS,
  60_000,
  1000,
  60 * 60_000,
);
const MAX_WS_CONNECTIONS = boundedInteger(
  process.env.MAX_WS_CONNECTIONS,
  MAX_ENDPOINTS * 2 + 1000,
  2,
  250_000,
);
const WS_CONNECT_LIMIT = boundedInteger(process.env.WS_CONNECT_LIMIT, 120, 1, 100_000);
const WS_CONNECT_WINDOW_MS = boundedInteger(
  process.env.WS_CONNECT_WINDOW_MS,
  60_000,
  1000,
  60 * 60_000,
);
const WS_AUTH_TIMEOUT_MS = boundedInteger(
  process.env.WS_AUTH_TIMEOUT_MS,
  10_000,
  1000,
  60_000,
);
const ENDPOINT_OFFLINE_TTL_MS = boundedInteger(
  process.env.ENDPOINT_OFFLINE_TTL_MS,
  24 * 60 * 60_000,
  1000,
  365 * 24 * 60 * 60_000,
);
const WS_INGRESS_WINDOW_MS = boundedInteger(
  process.env.WS_INGRESS_WINDOW_MS,
  60_000,
  1000,
  60 * 60_000,
);
const WS_INGRESS_MESSAGE_LIMIT = boundedInteger(
  process.env.WS_INGRESS_MESSAGE_LIMIT,
  12_000,
  1,
  1_000_000,
);
const WS_INGRESS_BYTE_LIMIT = boundedInteger(
  process.env.WS_INGRESS_BYTE_LIMIT,
  512 * MIB,
  1024,
  16 * 1024 * MIB,
);
// Never configure the outbound high-water mark below one legal inbound frame:
// doing so would accept a request/response and then terminate its destination
// solely because of contradictory limits.
const WS_MAX_BUFFERED_BYTES = Math.max(
  MAX_PAYLOAD_BYTES,
  boundedInteger(
    process.env.WS_MAX_BUFFERED_BYTES,
    8 * MIB,
    512,
    256 * MIB,
  ),
);
const MAX_REVOKED_ENDPOINTS = boundedInteger(
  process.env.MAX_REVOKED_ENDPOINTS,
  100_000,
  1,
  1_000_000,
);
const MAX_RELAY_STATE_BYTES = boundedInteger(
  process.env.MAX_RELAY_STATE_BYTES,
  16 * MIB,
  1024,
  256 * MIB,
);
const RELAY_STATE_PATH = resolve(
  process.env.PINVOU_REMOTE_STATE_PATH || resolve(__dirname, "data", "relay-state.json"),
);
const ALLOWED_WEB_ORIGINS = parseOriginSet(process.env.PINVOU_REMOTE_ALLOWED_WEB_ORIGINS);
const ALLOWED_PROXY_IPS = parseIpSet(process.env.PINVOU_REMOTE_ALLOWED_PROXY_IPS);
const TRUSTED_PROXY_IPS = parseIpSet(
  process.env.PINVOU_REMOTE_TRUSTED_PROXY_IPS
    || process.env.PINVOU_REMOTE_ALLOWED_PROXY_IPS,
);

const WEB_TO_DESKTOP_TYPES = new Set([
  "rpc_request",
  "event_subscribe",
  "event_unsubscribe",
  "client_ready",
]);
const DESKTOP_TO_WEB_TYPES = new Set([
  "rpc_response",
  "event",
  "stream_reset",
  "desktop_snapshot",
]);
const MIME_TYPES = new Map([
  [".css", "text/css; charset=utf-8"],
  [".gif", "image/gif"],
  [".html", "text/html; charset=utf-8"],
  [".ico", "image/x-icon"],
  [".jpeg", "image/jpeg"],
  [".jpg", "image/jpeg"],
  [".js", "text/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".map", "application/json; charset=utf-8"],
  [".mjs", "text/javascript; charset=utf-8"],
  [".png", "image/png"],
  [".svg", "image/svg+xml"],
  [".txt", "text/plain; charset=utf-8"],
  [".wasm", "application/wasm"],
  [".webp", "image/webp"],
  [".woff", "font/woff"],
  [".woff2", "font/woff2"],
]);

const endpoints = new Map();
const revokedEndpoints = new Map();
const endpointCreationBuckets = new Map();
const wsConnectionBuckets = new Map();
const telemetry = createTelemetryService();
let relayStateMutation = Promise.resolve();

await loadRelayState();

function send(ws, value) {
  if (!ws || ws.readyState !== ws.OPEN || ws.policyClosing) return false;
  const encoded = JSON.stringify(value);
  const bytes = Buffer.byteLength(encoded);
  if (bytes > WS_MAX_BUFFERED_BYTES
    || ws.bufferedAmount + bytes > WS_MAX_BUFFERED_BYTES) {
    ws.policyClosing = true;
    clearTimeout(ws.authTimer);
    ws.authTimer = null;
    try { ws.terminate(); } catch {}
    return false;
  }
  try {
    ws.send(encoded, (error) => {
      if (!error) return;
      ws.policyClosing = true;
      try { ws.terminate(); } catch {}
    });
    return true;
  } catch {
    ws.policyClosing = true;
    try { ws.terminate(); } catch {}
    return false;
  }
}

function closeSocket(ws, code = 1000, reason = "") {
  if (!ws) return;
  ws.policyClosing = true;
  try { ws.close(code, String(reason).slice(0, 123)); } catch {}
}

function boundedInteger(raw, fallback, min, max) {
  const value = Number(raw);
  if (!Number.isFinite(value)) return fallback;
  return Math.min(max, Math.max(min, Math.floor(value)));
}

function normalizeIp(value) {
  const ip = String(value || "").trim();
  return ip.startsWith("::ffff:") ? ip.slice(7) : ip;
}

function parseIpSet(value) {
  return new Set(String(value || "")
    .split(",")
    .map(normalizeIp)
    .filter(Boolean));
}

function parseOriginSet(value) {
  const origins = new Set();
  for (const item of String(value || "").split(",").map((part) => part.trim()).filter(Boolean)) {
    let parsed;
    try { parsed = new URL(item); } catch {
      throw new Error(`invalid PINVOU_REMOTE_ALLOWED_WEB_ORIGINS entry: ${item}`);
    }
    if (!/^https?:$/.test(parsed.protocol) || parsed.origin === "null") {
      throw new Error(`invalid browser WebSocket origin: ${item}`);
    }
    origins.add(parsed.origin.toLowerCase());
  }
  return origins;
}

function browserOriginAllowed(req) {
  const raw = req.headers.origin;
  // Native desktop clients do not send Origin and remain eligible. This is a
  // browser CSWSH defense, not a replacement for the deployment enrollment gate.
  if (raw === undefined) return true;
  if (typeof raw !== "string" || raw.includes(",")) return false;
  let parsed;
  try { parsed = new URL(raw); } catch { return false; }
  if (!/^https?:$/.test(parsed.protocol) || parsed.origin === "null") return false;
  const origin = parsed.origin.toLowerCase();
  if (ALLOWED_WEB_ORIGINS.has(origin)) return true;
  const requestHost = String(req.headers.host || "").trim().toLowerCase();
  return Boolean(requestHost) && parsed.host.toLowerCase() === requestHost;
}

function isLoopback(ip) {
  return ip === "127.0.0.1" || ip === "::1";
}

function peerIp(req) {
  return normalizeIp(req.socket?.remoteAddress);
}

function sourceAllowed(req) {
  if (ALLOWED_PROXY_IPS.size === 0) return true;
  const peer = peerIp(req);
  return isLoopback(peer) || ALLOWED_PROXY_IPS.has(peer);
}

function clientIp(req) {
  const peer = peerIp(req);
  if (!TRUSTED_PROXY_IPS.has(peer)) return peer || "unknown";
  const forwarded = String(req.headers["x-forwarded-for"] || "")
    .split(",")
    .map(normalizeIp)
    .filter(Boolean);
  return forwarded.at(-1) || normalizeIp(req.headers["x-real-ip"]) || peer || "unknown";
}

function consumeRateLimit(buckets, ip, limit, windowMs, now = Date.now()) {
  const bucket = buckets.get(ip);
  if (!bucket || now - bucket.started_at >= windowMs) {
    buckets.set(ip, { started_at: now, count: 1 });
    return true;
  }
  if (bucket.count >= limit) return false;
  bucket.count += 1;
  return true;
}

function consumeEndpointCreation(ip, now = Date.now()) {
  return consumeRateLimit(
    endpointCreationBuckets,
    ip,
    ENDPOINT_CREATE_LIMIT,
    ENDPOINT_CREATE_WINDOW_MS,
    now,
  );
}

function consumeWsConnection(ip, now = Date.now()) {
  return consumeRateLimit(
    wsConnectionBuckets,
    ip,
    WS_CONNECT_LIMIT,
    WS_CONNECT_WINDOW_MS,
    now,
  );
}

function rawMessageBytes(raw) {
  if (typeof raw === "string") return Buffer.byteLength(raw);
  if (raw && Number.isSafeInteger(raw.byteLength)) return raw.byteLength;
  if (raw && Number.isSafeInteger(raw.length)) return raw.length;
  return Buffer.byteLength(String(raw || ""));
}

function consumeIngress(ws, bytes, now = Date.now()) {
  if (!ws.ingressBucket || now - ws.ingressBucket.started_at >= WS_INGRESS_WINDOW_MS) {
    ws.ingressBucket = { started_at: now, messages: 0, bytes: 0 };
  }
  const nextMessages = ws.ingressBucket.messages + 1;
  const nextBytes = ws.ingressBucket.bytes + bytes;
  if (nextMessages > WS_INGRESS_MESSAGE_LIMIT || nextBytes > WS_INGRESS_BYTE_LIMIT) {
    return false;
  }
  ws.ingressBucket.messages = nextMessages;
  ws.ingressBucket.bytes = nextBytes;
  return true;
}

function rejectSocket(ws, code, message) {
  send(ws, { v: 2, type: "error", code, message });
  closeSocket(ws, 1008, message);
}

function rejectUpgrade(socket, status, message) {
  const body = `${message}\n`;
  socket.write(
    `HTTP/1.1 ${status}\r\n`
    + "Connection: close\r\n"
    + "Content-Type: text/plain; charset=utf-8\r\n"
    + `Content-Length: ${Buffer.byteLength(body)}\r\n`
    + "\r\n"
    + body,
  );
  socket.destroy();
}

function authenticateSocket(ws, role, endpointId, leaseId = null) {
  clearTimeout(ws.authTimer);
  ws.authTimer = null;
  ws.role = role;
  ws.endpointId = endpointId;
  ws.leaseId = leaseId;
}

function normalizeBasePath(value) {
  let raw = String(value || "").trim();
  if (!raw) return "";
  try {
    if (/^https?:\/\//i.test(raw)) raw = new URL(raw).pathname;
  } catch {}
  raw = raw.replace(/\/+$/, "");
  if (!raw || raw === "/") return "";
  return raw.startsWith("/") ? raw : `/${raw}`;
}

function isWithinPublicBasePath(pathname) {
  if (!PUBLIC_BASE_PATH) return true;
  return pathname === PUBLIC_BASE_PATH || pathname.startsWith(`${PUBLIC_BASE_PATH}/`);
}

function stripPublicBasePath(pathname) {
  if (!PUBLIC_BASE_PATH) return pathname;
  if (pathname === PUBLIC_BASE_PATH) return "/";
  if (pathname.startsWith(`${PUBLIC_BASE_PATH}/`)) {
    return pathname.slice(PUBLIC_BASE_PATH.length) || "/";
  }
  return pathname;
}

function audit(endpoint, event, patch = {}) {
  endpoint.audit.event_count += 1;
  endpoint.audit.last_event = event;
  endpoint.audit.last_at = new Date().toISOString();
  Object.assign(endpoint.audit, patch);
}

function tokenHash(token) {
  return crypto.createHash("sha256").update(String(token || "")).digest("hex");
}

function tokenHashMatches(token, expectedHash) {
  if (!expectedHash) return false;
  const actual = Buffer.from(tokenHash(token), "hex");
  const expected = Buffer.from(expectedHash, "hex");
  return actual.length === expected.length && crypto.timingSafeEqual(actual, expected);
}

function boundedCredential(value) {
  return typeof value === "string" && value.length > 0 && value.length <= 1024
    ? value
    : null;
}

function hasLegacyTokenAlias(message) {
  return Object.hasOwn(message, "token") || Object.hasOwn(message, "pairing_token");
}

function validEndpointId(value) {
  return typeof value === "string"
    && value.length >= 4
    && value.length <= 128
    && /^[A-Za-z0-9_-]+$/.test(value);
}

function relayStateContents(entries) {
  const revoked = [...entries]
    .map(([endpointId, revokedAt]) => ({
      endpoint_id: endpointId,
      revoked_at: revokedAt,
    }))
    .sort((left, right) => left.endpoint_id.localeCompare(right.endpoint_id));
  const contents = `${JSON.stringify({
    version: RELAY_STATE_VERSION,
    revoked_endpoints: revoked,
  }, null, 2)}\n`;
  if (Buffer.byteLength(contents) > MAX_RELAY_STATE_BYTES) {
    throw new Error("relay state exceeds MAX_RELAY_STATE_BYTES");
  }
  return contents;
}

async function loadRelayState() {
  let contents;
  try {
    const metadata = await stat(RELAY_STATE_PATH);
    if (!metadata.isFile()) throw new Error("relay state path is not a regular file");
    if (metadata.size > MAX_RELAY_STATE_BYTES) {
      throw new Error("relay state exceeds MAX_RELAY_STATE_BYTES");
    }
    contents = await readFile(RELAY_STATE_PATH, "utf8");
    if (Buffer.byteLength(contents) > MAX_RELAY_STATE_BYTES) {
      throw new Error("relay state exceeds MAX_RELAY_STATE_BYTES");
    }
  } catch (error) {
    if (error?.code === "ENOENT") return;
    throw new Error(`failed to load relay state: ${error.message}`, { cause: error });
  }

  let state;
  try { state = JSON.parse(contents); } catch (error) {
    throw new Error("failed to load relay state: invalid JSON", { cause: error });
  }
  if (!state || typeof state !== "object" || Array.isArray(state)
    || state.version !== RELAY_STATE_VERSION
    || !Array.isArray(state.revoked_endpoints)
    || state.revoked_endpoints.length > MAX_REVOKED_ENDPOINTS) {
    throw new Error("failed to load relay state: unsupported or malformed state");
  }

  for (const entry of state.revoked_endpoints) {
    const revokedAt = typeof entry?.revoked_at === "string" ? entry.revoked_at : "";
    if (!entry || typeof entry !== "object" || Array.isArray(entry)
      || !validEndpointId(entry.endpoint_id)
      || !Number.isFinite(Date.parse(revokedAt))
      || revokedEndpoints.has(entry.endpoint_id)) {
      throw new Error("failed to load relay state: malformed revoked endpoint");
    }
    revokedEndpoints.set(entry.endpoint_id, revokedAt);
  }
}

async function syncDirectory(path) {
  let directory;
  try {
    directory = await open(path, "r");
    await directory.sync();
  } catch (error) {
    // Directory fsync is not supported on every platform. The file itself is
    // always fsynced before rename; supported hosts get the stronger guarantee.
    if (!["EACCES", "EINVAL", "EISDIR", "ENOTSUP", "EPERM"].includes(error?.code)) {
      throw error;
    }
  } finally {
    try { await directory?.close(); } catch {}
  }
}

async function atomicWriteRelayState(nextState) {
  const contents = relayStateContents(nextState);
  const stateDirectory = dirname(RELAY_STATE_PATH);
  await mkdir(stateDirectory, { recursive: true });
  const temporaryPath = `${RELAY_STATE_PATH}.${process.pid}.${crypto.randomBytes(8).toString("hex")}.tmp`;
  let temporaryFile;
  try {
    temporaryFile = await open(temporaryPath, "wx", 0o600);
    await temporaryFile.writeFile(contents, "utf8");
    await temporaryFile.sync();
    await temporaryFile.close();
    temporaryFile = null;
    await rename(temporaryPath, RELAY_STATE_PATH);
    try { await chmod(RELAY_STATE_PATH, 0o600); } catch {}
    await syncDirectory(stateDirectory);
  } catch (error) {
    try { await temporaryFile?.close(); } catch {}
    try { await rm(temporaryPath, { force: true }); } catch {}
    throw error;
  }
}

function persistRevocation(endpointId) {
  const operation = relayStateMutation.then(async () => {
    if (revokedEndpoints.has(endpointId)) return revokedEndpoints.get(endpointId);
    if (revokedEndpoints.size >= MAX_REVOKED_ENDPOINTS) {
      throw new Error("revoked endpoint capacity reached");
    }
    const revokedAt = new Date().toISOString();
    const nextState = new Map(revokedEndpoints);
    nextState.set(endpointId, revokedAt);
    await atomicWriteRelayState(nextState);
    revokedEndpoints.set(endpointId, revokedAt);
    return revokedAt;
  });
  relayStateMutation = operation.catch(() => {});
  return operation;
}

function randomLeaseId() {
  return `lease_${crypto.randomBytes(18).toString("base64url")}`;
}

function withoutCredentials(message) {
  const {
    access_token: _accessToken,
    token: _token,
    pairing_token: _pairingToken,
    desktop_secret: _desktopSecret,
    ...safe
  } = message;
  return safe;
}

function socketOpen(ws) {
  return Boolean(ws && ws.readyState === ws.OPEN);
}

function makeAudit() {
  return {
    created_at: new Date().toISOString(),
    connected_at: null,
    disconnected_at: null,
    event_count: 0,
  };
}

function desktopStatus(endpoint, status) {
  send(endpoint.web, {
    v: 2,
    type: "desktop_connection_state",
    endpoint_id: endpoint.endpoint_id,
    lease_id: endpoint.lease_id,
    status,
  });
}

function notifyDesktopOfWeb(endpoint) {
  if (!socketOpen(endpoint.desktop) || !socketOpen(endpoint.web)) return;
  send(endpoint.desktop, {
    v: 2,
    type: "web_client_connected",
    endpoint_id: endpoint.endpoint_id,
    lease_id: endpoint.lease_id,
    stream_epoch: endpoint.web.streamEpoch || null,
    after_seq: endpoint.web.afterSeq,
  });
}

function revokeEndpoint(endpoint, reason = "revoked", requester = null) {
  if (!endpoint || endpoints.get(endpoint.endpoint_id) !== endpoint) return;
  endpoints.delete(endpoint.endpoint_id);
  audit(endpoint, "endpoint_revoked", { reason });
  send(requester, {
    v: 2,
    type: "desktop_endpoint_revoked",
    endpoint_id: endpoint.endpoint_id,
    reason,
  });
  send(endpoint.web, {
    v: 2,
    type: "endpoint_revoked",
    endpoint_id: endpoint.endpoint_id,
    reason,
  });
  if (endpoint.desktop !== requester) {
    send(endpoint.desktop, {
      v: 2,
      type: "desktop_endpoint_revoked",
      endpoint_id: endpoint.endpoint_id,
      reason,
    });
  }
  closeSocket(endpoint.web, 4003, "endpoint revoked");
  if (endpoint.desktop !== requester) closeSocket(endpoint.desktop, 4003, "endpoint revoked");
  endpoint.web = null;
  endpoint.desktop = null;
  endpoint.lease_id = null;
}

function expireEndpoint(endpoint, reason = "desktop_offline_ttl_expired") {
  if (!endpoint || endpoint.revoking || endpoints.get(endpoint.endpoint_id) !== endpoint) return;
  endpoints.delete(endpoint.endpoint_id);
  audit(endpoint, "endpoint_expired", { reason });
  send(endpoint.web, {
    v: 2,
    type: "endpoint_revoked",
    endpoint_id: endpoint.endpoint_id,
    reason,
  });
  send(endpoint.desktop, {
    v: 2,
    type: "desktop_endpoint_revoked",
    endpoint_id: endpoint.endpoint_id,
    reason,
  });
  closeSocket(endpoint.web, 4003, "endpoint expired");
  closeSocket(endpoint.desktop, 4003, "endpoint expired");
  endpoint.web = null;
  endpoint.desktop = null;
  endpoint.lease_id = null;
}

function pruneExpiredEndpoints(now = Date.now()) {
  for (const endpoint of endpoints.values()) {
    if (endpoint.revoking || socketOpen(endpoint.desktop)
      || !Number.isFinite(endpoint.desktop_offline_since)) continue;
    if (now - endpoint.desktop_offline_since >= ENDPOINT_OFFLINE_TTL_MS) {
      expireEndpoint(endpoint);
    }
  }
}

function healthSummary() {
  pruneExpiredEndpoints();
  const values = [...endpoints.values()];
  return {
    ok: true,
    endpoint_count: values.length,
    // Keep the aggregate-only key consumed by the current deployment probe.
    room_count: values.length,
    connected_endpoint_count: values.filter((endpoint) => socketOpen(endpoint.desktop)).length,
    desktop_open_count: values.filter((endpoint) => socketOpen(endpoint.desktop)).length,
    desktop_offline_count: values.filter((endpoint) => !socketOpen(endpoint.desktop)).length,
    web_client_open_count: values.filter((endpoint) => socketOpen(endpoint.web)).length,
    ws_connection_count: wss.clients.size,
    unauthenticated_connection_count: [...wss.clients]
      .filter((ws) => ws.role === "unknown").length,
  };
}

function webHeaders(contentType, cacheControl) {
  return {
    "content-type": contentType,
    "cache-control": cacheControl,
    "content-security-policy": "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; media-src 'self' blob:; connect-src 'self' ws: wss:; object-src 'none'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
    "cross-origin-opener-policy": "same-origin",
    "referrer-policy": "no-referrer",
    "x-content-type-options": "nosniff",
    "x-frame-options": "DENY",
  };
}

function safeWebPath(routePath) {
  let decoded;
  try { decoded = decodeURIComponent(routePath); } catch { return null; }
  if (decoded.includes("\0")) return null;
  const requested = decoded === "/" ? "index.html" : decoded.replace(/^\/+/, "");
  const candidate = resolve(WEB_ROOT, requested);
  const relation = relative(WEB_ROOT, candidate);
  if (relation.startsWith("..") || relation.includes(":") || relation.startsWith("/")) return null;
  return candidate;
}

async function regularFile(path) {
  try { return (await stat(path)).isFile(); } catch { return false; }
}

async function serveWebUi(req, res, routePath) {
  if (req.method !== "GET" && req.method !== "HEAD") {
    res.writeHead(405, { allow: "GET, HEAD" });
    res.end();
    return;
  }

  let path = safeWebPath(routePath);
  if (!path) {
    res.writeHead(400, { "content-type": "text/plain; charset=utf-8" });
    res.end("bad path");
    return;
  }
  if (!(await regularFile(path))) {
    if (extname(path)) {
      res.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
      res.end("not found");
      return;
    }
    path = resolve(WEB_ROOT, "index.html");
  }
  if (!(await regularFile(path))) {
    res.writeHead(503, { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" });
    res.end("WebUI build is not available");
    return;
  }

  const extension = extname(path).toLowerCase();
  const webRelativePath = relative(WEB_ROOT, path).replaceAll("\\", "/");
  const contentHashedAsset = /^assets\/(?:.*\/)?[^/]+-[A-Za-z0-9_-]{8,}\.[^/]+$/.test(webRelativePath);
  const cacheControl = extension === ".html"
    ? "no-store"
    : contentHashedAsset
      ? "public, max-age=31536000, immutable"
      : "no-cache";
  const body = req.method === "HEAD" ? null : await readFile(path);
  res.writeHead(200, webHeaders(MIME_TYPES.get(extension) || "application/octet-stream", cacheControl));
  res.end(body);
}

const server = http.createServer(async (req, res) => {
  if (!sourceAllowed(req)) {
    res.writeHead(403, { "content-type": "text/plain; charset=utf-8" });
    res.end("forbidden");
    return;
  }
  const url = new URL(req.url || "/", `http://${req.headers.host || "127.0.0.1"}`);
  const routePath = stripPublicBasePath(url.pathname);
  if (await telemetry.handleHttp(req, res, routePath, { clientIp: clientIp(req) })) return;
  if (isWithinPublicBasePath(url.pathname) && routePath === "/healthz") {
    res.writeHead(200, {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
    });
    res.end(JSON.stringify(healthSummary()));
    return;
  }
  if (isWithinPublicBasePath(url.pathname)) {
    await serveWebUi(req, res, routePath);
    return;
  }
  res.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
  res.end("not found");
});

const wss = new WebSocketServer({ noServer: true, maxPayload: MAX_PAYLOAD_BYTES });

server.on("upgrade", (req, socket, head) => {
  if (!sourceAllowed(req)) {
    socket.write("HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n");
    socket.destroy();
    return;
  }
  const url = new URL(req.url || "/", `http://${req.headers.host || "127.0.0.1"}`);
  if (!isWithinPublicBasePath(url.pathname) || stripPublicBasePath(url.pathname) !== "/ws") {
    socket.destroy();
    return;
  }
  if (!browserOriginAllowed(req)) {
    rejectUpgrade(socket, "403 Forbidden", "websocket origin forbidden");
    return;
  }
  if (wss.clients.size >= MAX_WS_CONNECTIONS) {
    rejectUpgrade(socket, "503 Service Unavailable", "websocket capacity reached");
    return;
  }
  if (!consumeWsConnection(clientIp(req))) {
    rejectUpgrade(socket, "429 Too Many Requests", "websocket connection rate limited");
    return;
  }
  wss.handleUpgrade(req, socket, head, (ws) => {
    wss.emit("connection", ws, req);
  });
});

function registerDesktop(ws, msg) {
  const endpointId = msg.endpoint_id;
  const accessToken = boundedCredential(msg.access_token);
  const desktopSecret = boundedCredential(msg.desktop_secret);
  if (ws.role !== "unknown" || msg.v !== 2 || !validEndpointId(endpointId)
    || !accessToken || !desktopSecret) {
    rejectSocket(ws, "bad_desktop_endpoint_register", "bad desktop endpoint registration");
    return;
  }

  pruneExpiredEndpoints();
  if (revokedEndpoints.has(endpointId)) {
    rejectSocket(ws, "endpoint_revoked", "endpoint was permanently revoked");
    return;
  }

  let endpoint = endpoints.get(endpointId);
  if (!endpoint) {
    if (endpoints.size >= MAX_ENDPOINTS) {
      rejectSocket(ws, "endpoint_capacity_reached", "endpoint capacity reached");
      return;
    }
    if (!consumeEndpointCreation(ws.clientIp)) {
      rejectSocket(ws, "endpoint_creation_rate_limited", "endpoint creation rate limited");
      return;
    }
    endpoint = {
      endpoint_id: endpointId,
      access_token_hash: tokenHash(accessToken),
      desktop_secret_hash: tokenHash(desktopSecret),
      desktop: null,
      web: null,
      lease_id: null,
      desktop_offline_since: null,
      revoking: false,
      audit: makeAudit(),
    };
    endpoints.set(endpointId, endpoint);
  } else {
    if (!tokenHashMatches(desktopSecret, endpoint.desktop_secret_hash)) {
      audit(endpoint, "desktop_register_rejected", { reason: "invalid_desktop_secret" });
      rejectSocket(ws, "invalid_desktop_secret", "invalid desktop secret");
      return;
    }
    if (!tokenHashMatches(accessToken, endpoint.access_token_hash)) {
      audit(endpoint, "desktop_register_rejected", { reason: "invalid_access_token" });
      rejectSocket(ws, "invalid_access_token", "invalid access token");
      return;
    }
    if (socketOpen(endpoint.desktop) && endpoint.desktop !== ws) {
      send(endpoint.desktop, {
        v: 2,
        type: "desktop_endpoint_replaced",
        endpoint_id: endpointId,
      });
      closeSocket(endpoint.desktop, 4001, "desktop endpoint replaced");
    }
  }

  endpoint.desktop = ws;
  endpoint.desktop_offline_since = null;
  authenticateSocket(ws, "desktop", endpointId);
  audit(endpoint, "desktop_endpoint_registered", { connected_at: new Date().toISOString() });
  send(ws, {
    v: 2,
    type: "desktop_endpoint_registered",
    endpoint_id: endpointId,
    web_client_connected: socketOpen(endpoint.web),
    lease_id: socketOpen(endpoint.web) ? endpoint.lease_id : null,
  });
  if (socketOpen(endpoint.web)) {
    desktopStatus(endpoint, "connected");
    notifyDesktopOfWeb(endpoint);
  }
}

function joinWebClient(ws, msg) {
  const endpointId = msg.endpoint_id;
  const accessToken = boundedCredential(msg.access_token);
  if (ws.role !== "unknown" || msg.v !== 2 || !validEndpointId(endpointId) || !accessToken) {
    rejectSocket(ws, "bad_web_client_join", "bad web client join");
    return;
  }
  pruneExpiredEndpoints();
  const endpoint = endpoints.get(endpointId);
  if (!endpoint) {
    rejectSocket(ws, "endpoint_not_found", "endpoint not found");
    return;
  }
  if (!tokenHashMatches(accessToken, endpoint.access_token_hash)) {
    audit(endpoint, "web_join_rejected", { reason: "invalid_token" });
    rejectSocket(ws, "invalid_token", "invalid token");
    return;
  }

  const leaseId = randomLeaseId();
  const previousWeb = socketOpen(endpoint.web) ? endpoint.web : null;
  if (previousWeb) {
    send(previousWeb, {
      v: 2,
      type: "endpoint_replaced",
      endpoint_id: endpointId,
      lease_id: previousWeb.leaseId,
    });
  }

  ws.streamEpoch = typeof msg.stream_epoch === "string" ? msg.stream_epoch.slice(0, 256) : null;
  ws.afterSeq = Math.max(0, Number.isSafeInteger(msg.after_seq) ? msg.after_seq : 0);
  endpoint.web = ws;
  endpoint.lease_id = leaseId;
  authenticateSocket(ws, "web", endpointId, leaseId);
  audit(endpoint, "web_client_joined", { connected_at: new Date().toISOString() });
  send(ws, {
    v: 2,
    type: "web_client_joined",
    endpoint_id: endpointId,
    lease_id: leaseId,
    desktop_connected: socketOpen(endpoint.desktop),
  });
  notifyDesktopOfWeb(endpoint);

  if (previousWeb) closeSocket(previousWeb, 4001, "endpoint replaced");
}

async function revokeFromSocket(ws, msg) {
  const endpointId = msg.endpoint_id || ws.endpointId;
  const desktopSecret = boundedCredential(msg.desktop_secret);
  const endpoint = validEndpointId(endpointId) ? endpoints.get(endpointId) : null;
  if (!endpoint) {
    rejectSocket(ws, "endpoint_not_found", "endpoint not found");
    return;
  }
  if ((ws.role !== "desktop" || ws.endpointId !== endpointId)
    && !tokenHashMatches(desktopSecret, endpoint.desktop_secret_hash)) {
    rejectSocket(ws, "invalid_desktop_secret", "invalid desktop secret");
    return;
  }
  if (desktopSecret && !tokenHashMatches(desktopSecret, endpoint.desktop_secret_hash)) {
    rejectSocket(ws, "invalid_desktop_secret", "invalid desktop secret");
    return;
  }
  if (endpoint.revoking) {
    rejectSocket(ws, "revocation_in_progress", "endpoint revocation is already in progress");
    return;
  }
  endpoint.revoking = true;
  ws.revokePending = true;
  clearTimeout(ws.authTimer);
  ws.authTimer = null;
  try {
    await persistRevocation(endpointId);
  } catch (error) {
    endpoint.revoking = false;
    ws.revokePending = false;
    console.error(`failed to persist revocation for ${endpointId}: ${error.message}`);
    rejectSocket(ws, "revoke_persistence_failed", "failed to persist endpoint revocation");
    return;
  }
  revokeEndpoint(endpoint, msg.reason || "revoked", ws);
  closeSocket(ws, 1000, "endpoint revoked");
}

function forwardAuthenticatedMessage(ws, msg) {
  const endpoint = endpoints.get(ws.endpointId);
  if (!endpoint) {
    rejectSocket(ws, "endpoint_not_found", "endpoint not found");
    return;
  }

  if (ws.role === "web") {
    if (endpoint.web !== ws || endpoint.lease_id !== ws.leaseId) {
      rejectSocket(ws, "stale_lease", "stale web client lease");
      return;
    }
    if (!WEB_TO_DESKTOP_TYPES.has(msg.type)) {
      send(ws, {
        v: 2,
        type: "error",
        code: "unsupported_message_type",
        message: `unsupported web message type: ${String(msg.type || "")}`,
      });
      return;
    }
    if (!socketOpen(endpoint.desktop)) {
      send(ws, {
        v: 2,
        type: "error",
        code: "desktop_offline",
        message: "desktop is offline",
      });
      return;
    }
    audit(endpoint, `web:${msg.type}`);
    send(endpoint.desktop, {
      ...withoutCredentials(msg),
      v: 2,
      endpoint_id: endpoint.endpoint_id,
      lease_id: endpoint.lease_id,
    });
    return;
  }

  if (ws.role === "desktop") {
    if (endpoint.desktop !== ws) {
      rejectSocket(ws, "desktop_endpoint_replaced", "desktop endpoint was replaced");
      return;
    }
    if (!DESKTOP_TO_WEB_TYPES.has(msg.type)) {
      send(ws, {
        v: 2,
        type: "error",
        code: "unsupported_message_type",
        message: `unsupported desktop message type: ${String(msg.type || "")}`,
      });
      return;
    }
    if (!socketOpen(endpoint.web)) return;
    if (msg.lease_id && msg.lease_id !== endpoint.lease_id) {
      audit(endpoint, `desktop:${msg.type}:stale_lease`);
      return;
    }
    audit(endpoint, `desktop:${msg.type}`);
    send(endpoint.web, {
      ...withoutCredentials(msg),
      v: 2,
      endpoint_id: endpoint.endpoint_id,
      lease_id: endpoint.lease_id,
    });
  }
}

wss.on("connection", (ws, req) => {
  ws.role = "unknown";
  ws.endpointId = null;
  ws.leaseId = null;
  ws.clientIp = clientIp(req);
  ws.isAlive = true;
  ws.policyClosing = false;
  ws.revokePending = false;
  ws.ingressBucket = { started_at: Date.now(), messages: 0, bytes: 0 };
  ws.authTimer = setTimeout(() => {
    if (ws.role === "unknown") {
      rejectSocket(ws, "authentication_timeout", "websocket authentication timeout");
    }
  }, WS_AUTH_TIMEOUT_MS);
  ws.authTimer.unref?.();
  ws.on("pong", () => { ws.isAlive = true; });

  ws.on("message", (raw, isBinary) => {
    if (ws.policyClosing || ws.revokePending) return;
    if (!consumeIngress(ws, rawMessageBytes(raw))) {
      rejectSocket(ws, "ingress_rate_limited", "websocket ingress rate limit exceeded");
      return;
    }
    if (isBinary) {
      send(ws, { v: 2, type: "error", code: "binary_unsupported", message: "binary messages are unsupported" });
      return;
    }
    let msg;
    try {
      msg = JSON.parse(String(raw));
    } catch {
      send(ws, { v: 2, type: "error", code: "bad_json", message: "bad json" });
      return;
    }
    if (!msg || typeof msg !== "object" || Array.isArray(msg)) {
      send(ws, { v: 2, type: "error", code: "bad_message", message: "bad message" });
      return;
    }
    if (msg.v !== 2) {
      rejectSocket(
        ws,
        "unsupported_protocol_version",
        "protocol version 2 is required",
      );
      return;
    }
    if (hasLegacyTokenAlias(msg)) {
      rejectSocket(
        ws,
        "legacy_token_alias_unsupported",
        "legacy token aliases are unsupported",
      );
      return;
    }

    if (msg.type === "desktop_endpoint_register") {
      registerDesktop(ws, msg);
      return;
    }
    if (msg.type === "web_client_join") {
      joinWebClient(ws, msg);
      return;
    }
    if (msg.type === "desktop_endpoint_revoke") {
      void revokeFromSocket(ws, msg).catch((error) => {
        console.error(`unexpected endpoint revoke failure: ${error.message}`);
        rejectSocket(ws, "revoke_failed", "endpoint revocation failed");
      });
      return;
    }
    if (ws.role === "unknown") {
      rejectSocket(ws, "authentication_required", "authenticate first");
      return;
    }
    forwardAuthenticatedMessage(ws, msg);
  });

  ws.on("close", () => {
    clearTimeout(ws.authTimer);
    ws.authTimer = null;
    const endpoint = endpoints.get(ws.endpointId);
    if (!endpoint) return;
    if (ws.role === "web" && endpoint.web === ws) {
      const leaseId = endpoint.lease_id;
      endpoint.web = null;
      endpoint.lease_id = null;
      audit(endpoint, "web_client_disconnected", { disconnected_at: new Date().toISOString() });
      send(endpoint.desktop, {
        v: 2,
        type: "web_client_disconnected",
        endpoint_id: endpoint.endpoint_id,
        lease_id: leaseId,
      });
    }
    if (ws.role === "desktop" && endpoint.desktop === ws) {
      endpoint.desktop = null;
      endpoint.desktop_offline_since = Date.now();
      audit(endpoint, "desktop_disconnected", { disconnected_at: new Date().toISOString() });
      desktopStatus(endpoint, "offline");
    }
  });
});

const heartbeatTimer = setInterval(() => {
  pruneExpiredEndpoints();
  const endpointBucketExpiry = Date.now() - ENDPOINT_CREATE_WINDOW_MS;
  for (const [ip, bucket] of endpointCreationBuckets) {
    if (bucket.started_at <= endpointBucketExpiry) endpointCreationBuckets.delete(ip);
  }
  const wsBucketExpiry = Date.now() - WS_CONNECT_WINDOW_MS;
  for (const [ip, bucket] of wsConnectionBuckets) {
    if (bucket.started_at <= wsBucketExpiry) wsConnectionBuckets.delete(ip);
  }
  for (const ws of wss.clients) {
    if (ws.isAlive === false) {
      ws.terminate();
      continue;
    }
    ws.isAlive = false;
    try { ws.ping(); } catch { ws.terminate(); }
  }
}, HEARTBEAT_INTERVAL_MS);
heartbeatTimer.unref();

server.listen(PORT, "0.0.0.0", () => {
  console.log(
    `pinvou remote relay listening on http://127.0.0.1:${PORT}`
    + ` (protocol=v2, max_endpoints=${MAX_ENDPOINTS}`
    + `, max_ws_connections=${MAX_WS_CONNECTIONS}`
    + `, ws_connect_limit=${WS_CONNECT_LIMIT}/${WS_CONNECT_WINDOW_MS}ms`
    + `, ws_auth_timeout=${WS_AUTH_TIMEOUT_MS}ms`
    + `, ws_ingress_limit=${WS_INGRESS_MESSAGE_LIMIT}msg/${WS_INGRESS_BYTE_LIMIT}B/${WS_INGRESS_WINDOW_MS}ms`
    + `, ws_buffer_high_water=${WS_MAX_BUFFERED_BYTES}B`
    + `, endpoint_create_limit=${ENDPOINT_CREATE_LIMIT}/${ENDPOINT_CREATE_WINDOW_MS}ms`
    + `, endpoint_offline_ttl=${ENDPOINT_OFFLINE_TTL_MS}ms`
    + `, revoked_endpoints=${revokedEndpoints.size}`
    + `, max_payload=${MAX_PAYLOAD_BYTES})`,
  );
});
