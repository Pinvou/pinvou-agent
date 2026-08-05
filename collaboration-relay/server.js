import http from "node:http";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { WebSocketServer } from "ws";

const PORT = Number(process.env.PORT || 8790);
const HOST = String(process.env.PINVOU_COLLAB_HOST || process.env.HOST || "0.0.0.0").trim() || "0.0.0.0";
const PUBLIC_BASE_PATH = normalizeBasePath(process.env.PINVOU_COLLAB_PUBLIC_BASE_PATH || "/pinvou3/collaboration");
// Local MVP demo default. Production deployments should always override this
// through PINVOU_COLLAB_PROJECT_TOKEN.
const PROJECT_TOKEN = String(process.env.PINVOU_COLLAB_PROJECT_TOKEN || "pinvou-task-mvp-token").trim();
const HEARTBEAT_INTERVAL_MS = boundedInteger(process.env.HEARTBEAT_INTERVAL_MS, 15_000, 5000, 60_000);
const WS_AUTH_TIMEOUT_MS = boundedInteger(process.env.WS_AUTH_TIMEOUT_MS, 10_000, 1000, 60_000);
const MAX_PAYLOAD_BYTES = boundedInteger(process.env.MAX_PAYLOAD_BYTES, 2 * 1024 * 1024, 64 * 1024, 2 * 1024 * 1024);
const STATE_FILE = String(process.env.PINVOU_COLLAB_STATE_FILE || "").trim();
const DOWNLOAD_URL = String(process.env.PINVOU_COLLAB_DOWNLOAD_URL || "https://pinvou.com").trim();

const peers = new Map();
const registry = new Map();
const pendingDeliveries = new Map();
const audit = {
  started_at: new Date().toISOString(),
  registered_count: 0,
  forwarded_count: 0,
  failed_delivery_count: 0,
  queued_delivery_count: 0,
};

function boundedInteger(raw, fallback, min, max) {
  const value = Number(raw);
  if (!Number.isFinite(value)) return fallback;
  return Math.min(max, Math.max(min, Math.floor(value)));
}

function normalizeBasePath(value) {
  let raw = String(value || "").trim();
  if (!raw) return "";
  raw = raw.replace(/\/+$/, "");
  if (!raw || raw === "/") return "";
  return raw.startsWith("/") ? raw : `/${raw}`;
}

function stripPublicBasePath(pathname) {
  if (!PUBLIC_BASE_PATH) return pathname;
  if (pathname === PUBLIC_BASE_PATH) return "/";
  if (pathname.startsWith(`${PUBLIC_BASE_PATH}/`)) {
    return pathname.slice(PUBLIC_BASE_PATH.length) || "/";
  }
  return pathname;
}

function escapeHtml(value) {
  return String(value || "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function inviteJoinPage(token) {
  const safeToken = escapeHtml(token);
  const deepLink = `pinvou://join?token=${encodeURIComponent(token)}`;
  const safeDeepLink = escapeHtml(deepLink);
  const safeDownloadUrl = escapeHtml(DOWNLOAD_URL || "https://pinvou.com");
  return `<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>加入 Pinvou 协作</title>
  <style>
    :root {
      color-scheme: light dark;
      --bg: #f5f5f7;
      --panel: rgba(255,255,255,.78);
      --text: #1d1d1f;
      --muted: #6e6e73;
      --line: rgba(0,0,0,.08);
      --blue: #007aff;
      --blue-hover: #006fe6;
      --shadow: 0 24px 70px rgba(15,23,42,.16);
    }
    @media (prefers-color-scheme: dark) {
      :root {
        --bg: #000;
        --panel: rgba(28,28,30,.82);
        --text: #f5f5f7;
        --muted: #a1a1a6;
        --line: rgba(255,255,255,.12);
        --shadow: 0 24px 70px rgba(0,0,0,.42);
      }
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      min-height: 100vh;
      display: grid;
      place-items: center;
      padding: 24px;
      background: var(--bg);
      color: var(--text);
      font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", "Segoe UI", sans-serif;
      letter-spacing: 0;
    }
    main {
      width: min(520px, 100%);
      border: 1px solid var(--line);
      border-radius: 28px;
      background: var(--panel);
      box-shadow: var(--shadow);
      backdrop-filter: blur(24px);
      overflow: hidden;
    }
    .content { padding: 28px; }
    .icon {
      width: 58px;
      height: 58px;
      display: grid;
      place-items: center;
      border-radius: 19px;
      background: var(--blue);
      color: #fff;
      font-size: 28px;
      font-weight: 700;
      box-shadow: 0 14px 32px rgba(0,122,255,.32);
    }
    h1 {
      margin: 22px 0 8px;
      font-size: 28px;
      line-height: 1.15;
      letter-spacing: 0;
    }
    p {
      margin: 0;
      color: var(--muted);
      font-size: 15px;
      line-height: 1.55;
    }
    .token {
      margin-top: 18px;
      padding: 12px;
      border: 1px solid var(--line);
      border-radius: 16px;
      background: rgba(118,118,128,.10);
      color: var(--muted);
      font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
      font-size: 12px;
      overflow-wrap: anywhere;
    }
    .actions {
      display: grid;
      gap: 10px;
      padding: 16px;
      border-top: 1px solid var(--line);
      background: rgba(118,118,128,.06);
    }
    a, button {
      width: 100%;
      height: 46px;
      border: 0;
      border-radius: 16px;
      display: inline-flex;
      align-items: center;
      justify-content: center;
      text-decoration: none;
      font: inherit;
      font-size: 15px;
      font-weight: 700;
      cursor: pointer;
    }
    .primary { background: var(--blue); color: #fff; }
    .primary:hover { background: var(--blue-hover); }
    .secondary {
      background: rgba(118,118,128,.14);
      color: var(--text);
    }
    .hint {
      margin-top: 12px;
      text-align: center;
      font-size: 12px;
      color: var(--muted);
    }
  </style>
</head>
<body>
  <main>
    <div class="content">
      <div class="icon">P</div>
      <h1>加入 Pinvou 协作</h1>
      <p>如果本机已安装支持协作功能的新版 Pinvou，可以直接打开客户端加入。未安装或无法唤起时，请复制邀请口令，在 Pinvou 工作台的“加入邀请”里粘贴。</p>
      <div class="token" id="token">${safeToken || "缺少邀请 token"}</div>
      <div class="hint" id="copy-hint"></div>
    </div>
    <div class="actions">
      <a class="primary" href="${safeDeepLink}" id="open-pinvou">打开 Pinvou 加入协作</a>
      <button class="secondary" type="button" id="copy-token">复制邀请口令</button>
      <a class="secondary" href="${safeDownloadUrl}">下载或升级 Pinvou</a>
    </div>
  </main>
  <script>
    const token = ${JSON.stringify(token)};
    const hint = document.getElementById('copy-hint');
    document.getElementById('copy-token').addEventListener('click', async () => {
      try {
        await navigator.clipboard.writeText(token);
        hint.textContent = '已复制，请到 Pinvou 工作台的“加入邀请”中粘贴。';
      } catch {
        hint.textContent = '复制失败，请手动选择上方口令复制。';
      }
    });
  </script>
</body>
</html>`;
}

function peerKey(projectId, peerId) {
  return `${projectId}:${peerId}`;
}

function safeString(value, max = 256) {
  return String(value || "").trim().slice(0, max);
}

function normalizeCapabilities(value) {
  return Array.isArray(value)
    ? value.slice(0, 20).map((item) => safeString(item, 64)).filter(Boolean)
    : [];
}

function sameToken(value) {
  if (!PROJECT_TOKEN) return false;
  const actual = Buffer.from(String(value || ""));
  const expected = Buffer.from(PROJECT_TOKEN);
  return actual.length === expected.length && crypto.timingSafeEqual(actual, expected);
}

function send(ws, value) {
  if (!ws || ws.readyState !== ws.OPEN) return false;
  ws.send(JSON.stringify(value));
  return true;
}

function envelope(type, patch = {}) {
  return {
    v: 1,
    id: `msg_${crypto.randomBytes(10).toString("hex")}`,
    type,
    ts: new Date().toISOString(),
    ...patch,
  };
}

function publicPeer(peer) {
  const online = peers.has(peerKey(peer.project_id, peer.peer_id));
  return {
    peer_id: peer.peer_id,
    project_id: peer.project_id,
    display_name: peer.display_name,
    device_name: peer.device_name,
    capabilities: peer.capabilities,
    description: peer.description,
    app_version: peer.app_version,
    status: online ? "online" : "offline",
    last_seen_at: peer.last_seen_at,
  };
}

function peerList(projectId) {
  return [...registry.values()]
    .filter((peer) => peer.project_id === projectId)
    .map(publicPeer);
}

function loadState() {
  if (!STATE_FILE) return;
  try {
    if (!fs.existsSync(STATE_FILE)) return;
    const raw = JSON.parse(fs.readFileSync(STATE_FILE, "utf8"));
    for (const item of Array.isArray(raw.registry) ? raw.registry : []) {
      const projectId = safeString(item.project_id, 96);
      const peerId = safeString(item.peer_id, 128);
      if (!projectId || !peerId) continue;
      registry.set(peerKey(projectId, peerId), {
        project_id: projectId,
        peer_id: peerId,
        display_name: safeString(item.display_name || peerId, 96),
        device_name: safeString(item.device_name, 128),
        capabilities: normalizeCapabilities(item.capabilities),
        description: safeString(item.description, 256),
        app_version: safeString(item.app_version, 32),
        last_seen_at: safeString(item.last_seen_at, 64) || new Date().toISOString(),
      });
    }
    for (const [key, items] of Object.entries(raw.pending_deliveries || {})) {
      if (Array.isArray(items) && items.length > 0) pendingDeliveries.set(key, items.slice(-100));
    }
  } catch (error) {
    console.warn(`[pinvou collaboration relay] failed to load state: ${error.message}`);
  }
}

function saveState() {
  if (!STATE_FILE) return;
  try {
    fs.mkdirSync(path.dirname(STATE_FILE), { recursive: true });
    fs.writeFileSync(STATE_FILE, JSON.stringify({
      registry: [...registry.values()].map(publicPeer),
      pending_deliveries: Object.fromEntries(pendingDeliveries),
    }, null, 2));
  } catch (error) {
    console.warn(`[pinvou collaboration relay] failed to save state: ${error.message}`);
  }
}

function broadcastPeerStatus(projectId, changedPeer) {
  const event = envelope("peer_status_changed", {
    project_id: projectId,
    payload: publicPeer(changedPeer),
  });
  for (const peer of peers.values()) {
    if (peer.project_id === projectId && peer.ws !== changedPeer.ws) send(peer.ws, event);
  }
}

function closePeer(ws, reason = "disconnected") {
  if (!ws.peer_id || !ws.project_id) return;
  const key = peerKey(ws.project_id, ws.peer_id);
  const peer = peers.get(key);
  if (!peer || peer.ws !== ws) return;
  peers.delete(key);
  const registered = registry.get(key);
  if (registered) {
    registered.last_seen_at = new Date().toISOString();
    registry.set(key, registered);
    saveState();
  }
  const event = envelope("peer_status_changed", {
    project_id: peer.project_id,
    payload: { ...publicPeer(peer), status: "offline", reason },
  });
  for (const other of peers.values()) {
    if (other.project_id === peer.project_id) send(other.ws, event);
  }
}

function handlePeerRegister(ws, msg) {
  const projectId = safeString(msg.project_id || msg.projectId, 96);
  const peerId = safeString(msg.peer_id || msg.peerId, 128);
  if (!projectId || !peerId || !sameToken(msg.project_token || msg.projectToken)) {
    send(ws, envelope("error", { payload: { code: "unauthorized", message: "invalid collaboration project token" } }));
    try { ws.close(1008, "unauthorized"); } catch {}
    return;
  }
  const payload = msg.payload || {};
  const peer = {
    ws,
    project_id: projectId,
    peer_id: peerId,
    display_name: safeString(payload.display_name || payload.displayName || peerId, 96),
    device_name: safeString(payload.device_name || payload.deviceName || "", 128),
    capabilities: normalizeCapabilities(payload.capabilities),
    description: safeString(payload.description, 256),
    app_version: safeString(payload.app_version || payload.appVersion || "", 32),
    last_seen_at: new Date().toISOString(),
  };
  const old = peers.get(peerKey(projectId, peerId));
  if (old && old.ws !== ws) {
    send(old.ws, envelope("peer_replaced", { project_id: projectId, to_peer_id: peerId }));
    try { old.ws.close(1000, "peer replaced"); } catch {}
  }
  ws.project_id = projectId;
  ws.peer_id = peerId;
  peers.set(peerKey(projectId, peerId), peer);
  registry.set(peerKey(projectId, peerId), { ...peer, ws: undefined });
  saveState();
  audit.registered_count += 1;
  send(ws, envelope("peer_registered", {
    project_id: projectId,
    to_peer_id: peerId,
    payload: { self: publicPeer(peer), peers: peerList(projectId) },
  }));
  broadcastPeerStatus(projectId, peer);
  flushPending(projectId, peerId);
}

function handleTaskForward(ws, msg) {
  if (!ws.peer_id || !ws.project_id) {
    send(ws, envelope("error", { payload: { code: "not_registered", message: "peer must register first" } }));
    return;
  }
  const type = safeString(msg.type, 64);
  const allowed = new Set(["task_create", "task_ack", "task_accept", "task_reject", "task_cancel"]);
  if (!allowed.has(type)) return;
  const projectId = safeString(msg.project_id || msg.projectId, 96);
  const fromPeerId = safeString(msg.from_peer_id || msg.fromPeerId, 128);
  const toPeerId = safeString(msg.to_peer_id || msg.toPeerId, 128);
  if (projectId !== ws.project_id || fromPeerId !== ws.peer_id || !toPeerId) {
    send(ws, envelope("error", { payload: { code: "bad_envelope", message: "message envelope does not match registered peer" } }));
    return;
  }
  const targetKey = peerKey(projectId, toPeerId);
  const target = peers.get(targetKey);
  if (!target) {
    if (registry.has(targetKey)) {
      const queued = {
        v: 1,
        id: safeString(msg.id, 128) || `msg_${crypto.randomBytes(10).toString("hex")}`,
        type,
        from_peer_id: fromPeerId,
        to_peer_id: toPeerId,
        project_id: projectId,
        ts: msg.ts || new Date().toISOString(),
        payload: msg.payload || {},
      };
      queueDelivery(projectId, toPeerId, queued);
      send(ws, envelope("task_delivery_pending", {
        project_id: projectId,
        from_peer_id: toPeerId,
        to_peer_id: fromPeerId,
        payload: {
          task_id: msg.payload?.task_id || "",
          reason: "peer_offline",
          message: "target peer is offline; task queued for later delivery",
        },
      }));
      return;
    }
    audit.failed_delivery_count += 1;
    send(ws, envelope("task_delivery_failed", {
      project_id: projectId,
      from_peer_id: toPeerId,
      to_peer_id: fromPeerId,
      payload: {
        task_id: msg.payload?.task_id || "",
        reason: "peer_offline",
        message: "target peer is offline",
      },
    }));
    return;
  }
  const forwarded = {
    v: 1,
    id: safeString(msg.id, 128) || `msg_${crypto.randomBytes(10).toString("hex")}`,
    type,
    from_peer_id: fromPeerId,
    to_peer_id: toPeerId,
    project_id: projectId,
    ts: msg.ts || new Date().toISOString(),
    payload: msg.payload || {},
  };
  if (send(target.ws, forwarded)) {
    audit.forwarded_count += 1;
    return;
  }
  peers.delete(targetKey);
  if (registry.has(targetKey)) {
    queueDelivery(projectId, toPeerId, forwarded);
    send(ws, envelope("task_delivery_pending", {
      project_id: projectId,
      from_peer_id: toPeerId,
      to_peer_id: fromPeerId,
      payload: {
        task_id: msg.payload?.task_id || "",
        reason: "peer_offline",
        message: "target peer connection is closed; task queued for later delivery",
      },
    }));
  }
}

function queueDelivery(projectId, peerId, message) {
  const key = peerKey(projectId, peerId);
  const queue = pendingDeliveries.get(key) || [];
  queue.push(message);
  pendingDeliveries.set(key, queue.slice(-100));
  audit.queued_delivery_count += 1;
  saveState();
}

function flushPending(projectId, peerId) {
  const key = peerKey(projectId, peerId);
  const queue = pendingDeliveries.get(key);
  if (!queue || queue.length === 0) return;
  const target = peers.get(key);
  if (!target) return;
  const remaining = [];
  for (const message of queue) {
    if (send(target.ws, message)) {
      audit.forwarded_count += 1;
    } else {
      remaining.push(message);
    }
  }
  if (remaining.length > 0) {
    pendingDeliveries.set(key, remaining);
  } else {
    pendingDeliveries.delete(key);
  }
  saveState();
}

function healthSummary() {
  return {
    ok: true,
    peer_count: peers.size,
    registered_peer_count: registry.size,
    pending_delivery_count: [...pendingDeliveries.values()].reduce((sum, items) => sum + items.length, 0),
    project_count: new Set([...registry.values()].map((peer) => peer.project_id)).size,
    ws_connection_count: wss.clients.size,
    authenticated_connection_count: [...wss.clients].filter((ws) => ws.peer_id).length,
    audit,
  };
}

loadState();

const server = http.createServer((req, res) => {
  const url = new URL(req.url || "/", `http://${req.headers.host || "127.0.0.1"}`);
  const routePath = stripPublicBasePath(url.pathname);
  if (routePath === "/" || routePath === "/join") {
    const token = safeString(url.searchParams.get("token") || "", 4096);
    if (routePath === "/" && !token) {
      res.writeHead(200, { "content-type": "text/plain; charset=utf-8" });
      res.end("pinvou collaboration relay");
      return;
    }
    res.writeHead(token ? 200 : 400, {
      "content-type": "text/html; charset=utf-8",
      "cache-control": "no-store",
    });
    res.end(inviteJoinPage(token));
    return;
  }
  if (routePath === "/healthz") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify(healthSummary()));
    return;
  }
  res.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
  res.end("not found");
});

const wss = new WebSocketServer({ noServer: true, maxPayload: MAX_PAYLOAD_BYTES });

server.on("upgrade", (req, socket, head) => {
  const url = new URL(req.url || "/", `http://${req.headers.host || "127.0.0.1"}`);
  if (stripPublicBasePath(url.pathname) !== "/ws") {
    socket.destroy();
    return;
  }
  wss.handleUpgrade(req, socket, head, (ws) => {
    wss.emit("connection", ws, req);
  });
});

wss.on("connection", (ws) => {
  ws.isAlive = true;
  ws.peer_id = null;
  ws.project_id = null;
  ws.authTimer = setTimeout(() => {
    if (!ws.peer_id) {
      send(ws, envelope("error", { payload: { code: "authentication_timeout", message: "peer_register timeout" } }));
      try { ws.close(1008, "authentication timeout"); } catch {}
    }
  }, WS_AUTH_TIMEOUT_MS);
  ws.authTimer.unref?.();
  ws.on("pong", () => { ws.isAlive = true; });
  ws.on("message", (raw) => {
    let msg;
    try {
      msg = JSON.parse(String(raw));
    } catch {
      send(ws, envelope("error", { payload: { code: "bad_json", message: "bad json" } }));
      return;
    }
    if (msg.type === "peer_register") {
      clearTimeout(ws.authTimer);
      ws.authTimer = null;
      handlePeerRegister(ws, msg);
      return;
    }
    if (msg.type === "peer_list_request") {
      if (ws.peer_id) send(ws, envelope("peer_list", { project_id: ws.project_id, to_peer_id: ws.peer_id, payload: { peers: peerList(ws.project_id) } }));
      return;
    }
    handleTaskForward(ws, msg);
  });
  ws.on("close", () => {
    clearTimeout(ws.authTimer);
    ws.authTimer = null;
    closePeer(ws);
  });
});

const heartbeatTimer = setInterval(() => {
  for (const ws of wss.clients) {
    if (ws.isAlive === false) {
      closePeer(ws, "heartbeat_timeout");
      ws.terminate();
      continue;
    }
    ws.isAlive = false;
    try { ws.ping(); } catch { ws.terminate(); }
  }
}, HEARTBEAT_INTERVAL_MS);
heartbeatTimer.unref();

server.listen(PORT, HOST, () => {
  console.log(`pinvou collaboration relay listening on http://${HOST}:${PORT}${PUBLIC_BASE_PATH}`);
});

export { server, wss, peers, registry, pendingDeliveries };
