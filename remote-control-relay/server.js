import http from "node:http";
import crypto from "node:crypto";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { WebSocketServer } from "ws";
import { createTelemetryService } from "./telemetry-service.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const MIB = 1024 * 1024;
const PORT = Number(process.env.PORT || 8787);
const PUBLIC_BASE_PATH = normalizeBasePath(process.env.PINVOU_REMOTE_PUBLIC_BASE_PATH || "/pinvou3/remote");
const DESKTOP_RECONNECT_GRACE_MS = Math.max(1000, Number(process.env.DESKTOP_RECONNECT_GRACE_MS || 15_000));
const HEARTBEAT_INTERVAL_MS = Math.max(5000, Number(process.env.HEARTBEAT_INTERVAL_MS || 15_000));
const MAX_PAYLOAD_BYTES = boundedInteger(process.env.MAX_PAYLOAD_BYTES, 4 * MIB, MIB, 16 * MIB);
const MAX_ROOMS = boundedInteger(process.env.MAX_ROOMS, 2000, 1, 100_000);
const ROOM_CREATE_LIMIT = boundedInteger(process.env.ROOM_CREATE_LIMIT, 20, 1, 10_000);
const ROOM_CREATE_WINDOW_MS = boundedInteger(process.env.ROOM_CREATE_WINDOW_MS, 60_000, 1000, 60 * 60_000);
const MAX_WS_CONNECTIONS = boundedInteger(
  process.env.MAX_WS_CONNECTIONS,
  MAX_ROOMS * 2 + 1000,
  2,
  250_000,
);
const WS_CONNECT_LIMIT = boundedInteger(process.env.WS_CONNECT_LIMIT, 120, 1, 100_000);
const WS_CONNECT_WINDOW_MS = boundedInteger(process.env.WS_CONNECT_WINDOW_MS, 60_000, 1000, 60 * 60_000);
const WS_AUTH_TIMEOUT_MS = boundedInteger(process.env.WS_AUTH_TIMEOUT_MS, 10_000, 1000, 60_000);
// mobile → desktop 上传分片的字节速率窗口(防 mobile 在慢网/抖动下把 desktop 写盘撑爆)。
// 与其它限额一样支持 env 覆盖,便于单测用小窗口触发限流。
const MOBILE_UPLOAD_WINDOW_BYTES = boundedInteger(
  process.env.MOBILE_UPLOAD_WINDOW_BYTES,
  100 * 1024 * 1024,
  1024,
  1024 * 1024 * 1024,
);
const MOBILE_UPLOAD_WINDOW_SECS = boundedInteger(process.env.MOBILE_UPLOAD_WINDOW_SECS, 5, 1, 3600);
const ALLOWED_PROXY_IPS = parseIpSet(process.env.PINVOU_REMOTE_ALLOWED_PROXY_IPS);
const TRUSTED_PROXY_IPS = parseIpSet(
  process.env.PINVOU_REMOTE_TRUSTED_PROXY_IPS || process.env.PINVOU_REMOTE_ALLOWED_PROXY_IPS,
);
const rooms = new Map();
const roomCreationBuckets = new Map();
const wsConnectionBuckets = new Map();
// 每个 mobile ws → { bytes, resetAt(ms epoch) };用 WeakMap 让 ws 被 GC 时窗口自动回收。
const mobileUploadRate = new WeakMap();
const telemetry = createTelemetryService();

function send(ws, value, onSent) {
  if (!ws || ws.readyState !== ws.OPEN) {
    onSent?.(new Error("websocket is not open"));
    return false;
  }
  try {
    ws.send(JSON.stringify(value), onSent);
    return true;
  } catch (error) {
    onSent?.(error);
    return false;
  }
}

function acknowledgeDownloadChunk(room, payload, error) {
  send(room.desktop, {
    type: "artifact_download_relay_ack",
    room_id: room.room_id,
    session_id: room.session_id,
    payload: {
      download_id: payload?.download_id || "",
      index: payload?.index,
      ok: !error,
      message: error ? String(error.message || error) : undefined,
    },
  });
}

// mobile → desktop 的 attach_file_chunk 转发结果(成功或失败)时,relay 主动向 mobile 回一份
// ack,让 mobile 端的 waitForRelayAck 立即 resolve(成功)或 reject(失败),而不是干等 60s 超时。
// 镜像 acknowledgeDownloadChunk 的结构,字段名换成 upload_id,事件名换成 attach_file_relay_ack。
// 方向与下载相反:下载 ack 回 desktop(发送方),上传 ack 回 mobile(发送方)。
function acknowledgeUploadChunk(room, payload, error) {
  send(room.mobile, {
    type: "attach_file_relay_ack",
    room_id: room.room_id,
    session_id: room.session_id,
    payload: {
      upload_id: payload?.upload_id || "",
      index: payload?.index,
      ok: !error,
      message: error ? String(error.message || error) : undefined,
    },
  });
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

function consumeRoomCreation(ip, now = Date.now()) {
  return consumeRateLimit(
    roomCreationBuckets,
    ip,
    ROOM_CREATE_LIMIT,
    ROOM_CREATE_WINDOW_MS,
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

// 固定窗口(tumbling window):单个 mobile ws 在 MOBILE_UPLOAD_WINDOW_SECS 窗口内累计超过
// MOBILE_UPLOAD_WINDOW_BYTES 时拒绝转发。窗口到期整体重置为 0(非逐事件滑动)。返回 true 表示
// 放行(已从预算中扣减 byteLen),false 表示限流(未扣减,调用方需自行向 mobile 发
// attach_file_rate_limited 通知)。
// 边界注意:窗口刚好翻转的瞬间最多放行 ~2× 预算(翻转前满载 + 翻转后再次满载);由
// MAX_PAYLOAD_BYTES(默认 4MiB)与桌面端 single-active-upload 不变量兜底,不影响保护目标。
function consumeMobileUploadBudget(ws, byteLen, now = Date.now()) {
  let entry = mobileUploadRate.get(ws);
  if (!entry || now >= entry.resetAt) {
    entry = { bytes: 0, resetAt: now + MOBILE_UPLOAD_WINDOW_SECS * 1000 };
    mobileUploadRate.set(ws, entry);
  }
  if (entry.bytes + byteLen > MOBILE_UPLOAD_WINDOW_BYTES) {
    return false;
  }
  entry.bytes += byteLen;
  return true;
}

function rejectSocket(ws, code, message) {
  send(ws, { type: "error", code, message });
  try { ws.close(1008, message); } catch {}
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

function authenticateSocket(ws, role, roomId) {
  clearTimeout(ws.authTimer);
  ws.authTimer = null;
  ws.role = role;
  ws.roomId = roomId;
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

function stripPublicBasePath(pathname) {
  if (!PUBLIC_BASE_PATH) return pathname;
  if (pathname === PUBLIC_BASE_PATH) return "/";
  if (pathname.startsWith(`${PUBLIC_BASE_PATH}/`)) {
    return pathname.slice(PUBLIC_BASE_PATH.length) || "/";
  }
  return pathname;
}

function audit(room, event, patch = {}) {
  room.audit.event_count += 1;
  room.audit.last_event = event;
  room.audit.last_at = new Date().toISOString();
  Object.assign(room.audit, patch);
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

function loadMobilePage() {
  return readFile(join(__dirname, "web", "index.html"), "utf8");
}

function clearDesktopReconnectTimer(room) {
  if (!room?.desktop_reconnect_timer) return;
  clearTimeout(room.desktop_reconnect_timer);
  room.desktop_reconnect_timer = null;
}

function closeRoom(room, reason) {
  if (!room || room.closed || rooms.get(room.room_id) !== room) return;
  clearDesktopReconnectTimer(room);
  room.closed = true;
  audit(room, "room_closed", { reason });
  send(room.mobile, { type: "room_closed", reason });
  try { room.mobile?.close(); } catch {}
  try { room.desktop?.close(); } catch {}
  rooms.delete(room.room_id);
}

function waitForDesktopReconnect(room) {
  clearDesktopReconnectTimer(room);
  room.desktop = null;
  audit(room, "desktop_reconnecting", { grace_ms: DESKTOP_RECONNECT_GRACE_MS });
  send(room.mobile, {
    type: "desktop_connection_state",
    status: "reconnecting",
    grace_ms: DESKTOP_RECONNECT_GRACE_MS,
  });
  room.desktop_reconnect_timer = setTimeout(() => {
    if (rooms.get(room.room_id) === room && !room.desktop) {
      closeRoom(room, "desktop_disconnected");
    }
  }, DESKTOP_RECONNECT_GRACE_MS);
  room.desktop_reconnect_timer.unref?.();
}


function healthSummary() {
  const values = [...rooms.values()];
  return {
    ok: true,
    room_count: values.length,
    paired_count: values.filter((room) => room.paired).length,
    desktop_open_count: values.filter((room) => room.desktop && room.desktop.readyState === room.desktop.OPEN).length,
    mobile_open_count: values.filter((room) => room.mobile && room.mobile.readyState === room.mobile.OPEN).length,
    ws_connection_count: wss.clients.size,
    unauthenticated_connection_count: [...wss.clients].filter((ws) => ws.role === "unknown").length,
  };
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
  if (routePath === "/healthz") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify(healthSummary()));
    return;
  }
  if (routePath.startsWith("/r/")) {
    const roomId = routePath.split("/").filter(Boolean).pop();
    const room = rooms.get(roomId);
    if (room) audit(room, "mobile_page");
    const html = await loadMobilePage();
    res.writeHead(200, {
      "content-type": "text/html; charset=utf-8",
      "cache-control": "no-store",
    });
    res.end(html);
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
  if (stripPublicBasePath(url.pathname) !== "/ws") {
    socket.destroy();
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

wss.on("connection", (ws, req) => {
  ws.role = "unknown";
  ws.roomId = null;
  ws.clientIp = clientIp(req);
  ws.isAlive = true;
  ws.authTimer = setTimeout(() => {
    if (ws.role === "unknown") {
      rejectSocket(ws, "authentication_timeout", "websocket authentication timeout");
    }
  }, WS_AUTH_TIMEOUT_MS);
  ws.authTimer.unref?.();
  ws.on("pong", () => { ws.isAlive = true; });

  ws.on("message", (raw) => {
    let msg;
    try {
      msg = JSON.parse(String(raw));
    } catch {
      send(ws, { type: "error", message: "bad json" });
      return;
    }

    if (msg.type === "desktop_register") {
      if (!msg.room_id || typeof msg.session_id !== "string" || !msg.pairing_token || !msg.desktop_secret) {
        send(ws, { type: "error", message: "bad desktop_register" });
        return;
      }
      const old = rooms.get(msg.room_id);
      if (!old && rooms.size >= MAX_ROOMS) {
        rejectSocket(ws, "room_capacity_reached", "room capacity reached");
        return;
      }
      if (!old && !consumeRoomCreation(ws.clientIp)) {
        rejectSocket(ws, "room_creation_rate_limited", "room creation rate limited");
        return;
      }
      const existingMobile =
        old && old.mobile && old.mobile.readyState === old.mobile.OPEN ? old.mobile : null;
      const wasPaired = Boolean(old?.paired && existingMobile);
      if (old) {
        if (!tokenHashMatches(msg.desktop_secret, old.desktop_secret_hash)) {
          audit(old, "desktop_register_rejected", { reason: "invalid_desktop_secret" });
          send(ws, { type: "error", message: "invalid desktop secret" });
          try { ws.close(); } catch {}
          return;
        }
        clearDesktopReconnectTimer(old);
        try { old.desktop?.close(); } catch {}
      }
      const room = {
        room_id: msg.room_id,
        session_id: msg.session_id,
        desktop: ws,
        mobile: existingMobile,
        pairing_token_hash: tokenHash(msg.pairing_token),
        desktop_secret_hash: tokenHash(msg.desktop_secret),
        paired: wasPaired,
        closed: false,
        desktop_reconnect_timer: null,
        audit: old?.audit || {
          created_at: new Date().toISOString(),
          connected_at: null,
          disconnected_at: null,
          event_count: 0,
        },
      };
      rooms.set(room.room_id, room);
      authenticateSocket(ws, "desktop", room.room_id);
      audit(room, "desktop_register");
      send(ws, { type: "room_registered", room_id: room.room_id });
      if (existingMobile) {
        send(existingMobile, { type: "desktop_connection_state", status: "connected" });
        send(ws, { type: "mobile_connected", room_id: room.room_id });
        send(ws, {
          type: "mobile_action",
          room_id: room.room_id,
          session_id: room.session_id,
          payload: { type: room.session_id ? "request_snapshot" : "request_session_list", payload: {} },
        });
      }
      return;
    }

    if (msg.type === "mobile_join") {
      const room = rooms.get(msg.room_id);
      if (!room || room.closed) {
        console.log("[relay] mobile_join rejected", msg.room_id, "room not found");
        send(ws, { type: "error", message: "room not found" });
        return;
      }
      if (!tokenHashMatches(msg.token, room.pairing_token_hash)) {
        console.log("[relay] mobile_join rejected", msg.room_id, "invalid token");
        send(ws, { type: "error", message: "invalid token" });
        return;
      }
      if (room.mobile && room.mobile.readyState === room.mobile.OPEN) {
        send(room.mobile, { type: "room_replaced" });
        try { room.mobile.close(); } catch {}
      }
      room.paired = true;
      room.mobile = ws;
      authenticateSocket(ws, "mobile", room.room_id);
      audit(room, "mobile_connected", { connected_at: new Date().toISOString() });
      console.log("[relay] mobile_connected", room.room_id);
      send(ws, { type: "mobile_joined", room_id: room.room_id, session_id: room.session_id });
      if (!room.desktop) {
        send(ws, {
          type: "desktop_connection_state",
          status: "reconnecting",
          grace_ms: DESKTOP_RECONNECT_GRACE_MS,
        });
      }
      send(room.desktop, { type: "mobile_connected", room_id: room.room_id });
      send(room.desktop, {
        type: "mobile_action",
        room_id: room.room_id,
        session_id: room.session_id,
        payload: { type: room.session_id ? "request_snapshot" : "request_session_list", payload: {} },
      });
      return;
    }

    const room = rooms.get(ws.roomId);
    if (!room || room.closed) return;

    if (ws.role === "desktop") {
      if (msg.type === "desktop_disconnect") {
        const reason = msg.payload?.reason || "stopped";
        audit(room, "desktop_disconnect", { reason });
        closeRoom(room, reason);
        return;
      }
      if (typeof msg.session_id === "string" && msg.session_id) {
        room.session_id = msg.session_id;
      }
      audit(room, `desktop:${msg.type || "event"}`);
      // 下载分片必须等 relay 真正把当前帧写出后才通知 desktop 继续下一块。
      // 这把桌面端的有界通道背压贯穿到 relay→mobile 的慢速链路，避免
      // ws.send 在慢网手机前无限累计几十 MiB 的 bufferedAmount。
      if (msg.type === "artifact_download_chunk") {
        const payload = msg.payload || {};
        send(room.mobile, msg, (error) => acknowledgeDownloadChunk(room, payload, error));
        return;
      }
      send(room.mobile, msg);
      return;
    }

    if (ws.role === "mobile") {
      const payload = msg.type === "mobile_action" ? msg.payload : msg;
      audit(room, `mobile:${payload?.type || "action"}`);
      if (payload?.type === "attach_file_chunk") {
        // 上传分片先做字节速率限流:mobile 网络抖动时避免把 desktop 写盘撑爆。
        // base64 长度 ≈ 4/3 原始字节,粗估即可,精确解码反而消耗 CPU。
        const approxBytes = Math.ceil((payload?.data_base64 || "").length * 3 / 4);
        if (!consumeMobileUploadBudget(ws, approxBytes)) {
          // 超出窗口:直接通知 mobile 限流并停止转发,desktop 不会收到这一块。
          send(ws, {
            type: "attach_file_rate_limited",
            room_id: room.room_id,
            session_id: room.session_id,
            payload: {
              upload_id: payload?.upload_id || "",
              retry_after_ms: 1000,
            },
          });
          return;
        }
        // 等到 desktop WS 真正把这一帧写完后再向 mobile 回 ack(成功/失败均回),
        // 把 desktop 有界通道的背压贯穿到 mobile 的下一块,并避免 mobile 干等 60s 超时。
        send(room.desktop, {
          type: "mobile_action",
          room_id: room.room_id,
          session_id: room.session_id,
          payload,
        }, (error) => acknowledgeUploadChunk(room, payload, error));
        return;
      }
      send(room.desktop, {
        type: "mobile_action",
        room_id: room.room_id,
        session_id: room.session_id,
        payload,
      });
    }
  });

  ws.on("close", () => {
    clearTimeout(ws.authTimer);
    ws.authTimer = null;
    const room = rooms.get(ws.roomId);
    if (!room) return;
    if (ws.role === "mobile" && room.mobile === ws) {
      room.mobile = null;
      audit(room, "mobile_disconnected", { disconnected_at: new Date().toISOString() });
      send(room.desktop, { type: "mobile_disconnected", room_id: room.room_id });
    }
    if (ws.role === "desktop" && room.desktop === ws) {
      audit(room, "desktop_disconnected");
      waitForDesktopReconnect(room);
    }
  });
});

const heartbeatTimer = setInterval(() => {
  const bucketExpiry = Date.now() - ROOM_CREATE_WINDOW_MS;
  for (const [ip, bucket] of roomCreationBuckets) {
    if (bucket.started_at <= bucketExpiry) roomCreationBuckets.delete(ip);
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
    + ` (max_rooms=${MAX_ROOMS}, max_ws_connections=${MAX_WS_CONNECTIONS}`
    + `, ws_connect_limit=${WS_CONNECT_LIMIT}/${WS_CONNECT_WINDOW_MS}ms`
    + `, ws_auth_timeout=${WS_AUTH_TIMEOUT_MS}ms`
    + `, room_create_limit=${ROOM_CREATE_LIMIT}/${ROOM_CREATE_WINDOW_MS}ms`
    + `, max_payload=${MAX_PAYLOAD_BYTES})`,
  );
});
