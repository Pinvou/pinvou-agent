import http from "node:http";
import crypto from "node:crypto";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { WebSocketServer } from "ws";

const __dirname = dirname(fileURLToPath(import.meta.url));
const PORT = Number(process.env.PORT || 8787);
const PUBLIC_BASE_PATH = normalizeBasePath(process.env.PINVOU_REMOTE_PUBLIC_BASE_PATH || "/pinvou3/remote");
const rooms = new Map();

function send(ws, value) {
  if (ws && ws.readyState === ws.OPEN) ws.send(JSON.stringify(value));
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


function healthSummary() {
  const values = [...rooms.values()];
  return {
    ok: true,
    room_count: values.length,
    paired_count: values.filter((room) => room.paired).length,
    desktop_open_count: values.filter((room) => room.desktop && room.desktop.readyState === room.desktop.OPEN).length,
    mobile_open_count: values.filter((room) => room.mobile && room.mobile.readyState === room.mobile.OPEN).length,
  };
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url || "/", `http://${req.headers.host || "127.0.0.1"}`);
  const routePath = stripPublicBasePath(url.pathname);
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

const wss = new WebSocketServer({ noServer: true });

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
  ws.role = "unknown";
  ws.roomId = null;

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
        audit: old?.audit || {
          created_at: new Date().toISOString(),
          connected_at: null,
          disconnected_at: null,
          event_count: 0,
        },
      };
      rooms.set(room.room_id, room);
      ws.role = "desktop";
      ws.roomId = room.room_id;
      audit(room, "desktop_register");
      send(ws, { type: "room_registered", room_id: room.room_id });
      if (existingMobile) {
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
      ws.role = "mobile";
      ws.roomId = room.room_id;
      audit(room, "mobile_connected", { connected_at: new Date().toISOString() });
      console.log("[relay] mobile_connected", room.room_id);
      send(ws, { type: "mobile_joined", room_id: room.room_id, session_id: room.session_id });
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
        room.closed = true;
        audit(room, "desktop_disconnect", { reason });
        send(room.mobile, { type: "room_closed", reason });
        try { room.mobile?.close(); } catch {}
        try { room.desktop?.close(); } catch {}
        rooms.delete(room.room_id);
        return;
      }
      if (typeof msg.session_id === "string" && msg.session_id) {
        room.session_id = msg.session_id;
      }
      audit(room, `desktop:${msg.type || "event"}`);
      send(room.mobile, msg);
      return;
    }

    if (ws.role === "mobile") {
      const payload = msg.type === "mobile_action" ? msg.payload : msg;
      audit(room, `mobile:${payload?.type || "action"}`);
      send(room.desktop, {
        type: "mobile_action",
        room_id: room.room_id,
        session_id: room.session_id,
        payload,
      });
    }
  });

  ws.on("close", () => {
    const room = rooms.get(ws.roomId);
    if (!room) return;
    if (ws.role === "mobile" && room.mobile === ws) {
      room.mobile = null;
      audit(room, "mobile_disconnected", { disconnected_at: new Date().toISOString() });
      send(room.desktop, { type: "mobile_disconnected", room_id: room.room_id });
    }
    if (ws.role === "desktop" && room.desktop === ws) {
      room.closed = true;
      audit(room, "desktop_disconnected");
      send(room.mobile, { type: "room_closed", reason: "desktop_disconnected" });
      try { room.mobile?.close(); } catch {}
      rooms.delete(room.room_id);
    }
  });
});

server.listen(PORT, "0.0.0.0", () => {
  console.log(`pinvou remote relay listening on http://127.0.0.1:${PORT}`);
});
