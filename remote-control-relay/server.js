import http from "node:http";
import crypto from "node:crypto";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { WebSocketServer } from "ws";

const __dirname = dirname(fileURLToPath(import.meta.url));
const PORT = Number(process.env.PORT || 8787);
const ROOM_TTL_MS = 10 * 60 * 1000;
const rooms = new Map();

function send(ws, value) {
  if (ws && ws.readyState === ws.OPEN) ws.send(JSON.stringify(value));
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

function loadMobilePage() {
  return readFile(join(__dirname, "web", "index.html"), "utf8");
}


function roomSummary(room) {
  return {
    room_id: room.room_id,
    session_id: room.session_id,
    paired: room.paired,
    closed: room.closed,
    desktop_open: !!room.desktop && room.desktop.readyState === room.desktop.OPEN,
    mobile_open: !!room.mobile && room.mobile.readyState === room.mobile.OPEN,
    expires_at: new Date(room.expires_at).toISOString(),
    audit: room.audit,
  };
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url || "/", `http://${req.headers.host || "127.0.0.1"}`);
  if (url.pathname === "/healthz") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ ok: true, rooms: [...rooms.values()].map(roomSummary) }));
    return;
  }
  if (url.pathname.startsWith("/r/")) {
    const roomId = url.pathname.split("/").filter(Boolean).pop();
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

const wss = new WebSocketServer({ server, path: "/ws" });

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
      let expiresAt = Date.parse(msg.expires_at || "");
      if (!msg.room_id || !msg.session_id || !msg.pairing_token || !expiresAt) {
        send(ws, { type: "error", message: "bad desktop_register" });
        return;
      }
      if (expiresAt <= Date.now()) {
        expiresAt = Date.now() + ROOM_TTL_MS;
      }
      const old = rooms.get(msg.room_id);
      const existingMobile =
        old && old.mobile && old.mobile.readyState === old.mobile.OPEN ? old.mobile : null;
      const wasPaired = Boolean(old?.paired && existingMobile);
      if (old) {
        try { old.desktop?.close(); } catch {}
      }
      const room = {
        room_id: msg.room_id,
        session_id: msg.session_id,
        desktop: ws,
        mobile: existingMobile,
        pairing_token_hash: tokenHash(msg.pairing_token),
        desktop_secret_hash: tokenHash(msg.desktop_secret),
        expires_at: expiresAt,
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
          payload: { type: "request_snapshot", payload: {} },
        });
      }
      return;
    }

    if (msg.type === "mobile_join") {
      const room = rooms.get(msg.room_id);
      if (!room || room.closed || Date.now() > room.expires_at) {
        console.log("[relay] mobile_join rejected", msg.room_id, "room expired or not found");
        send(ws, { type: "error", message: "room expired or not found" });
        return;
      }
      if (tokenHash(msg.token) !== room.pairing_token_hash) {
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
        payload: { type: "request_snapshot", payload: {} },
      });
      return;
    }

    const room = rooms.get(ws.roomId);
    if (!room || room.closed) return;

    if (ws.role === "desktop") {
      if (msg.type === "desktop_disconnect") {
        room.closed = true;
        audit(room, "desktop_disconnect");
        send(room.mobile, { type: "room_closed" });
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
      send(room.mobile, { type: "room_closed" });
      try { room.mobile?.close(); } catch {}
      rooms.delete(room.room_id);
    }
  });
});

setInterval(() => {
  const now = Date.now();
  for (const [id, room] of rooms) {
    if (!room.closed && !room.paired && now > room.expires_at) {
      room.closed = true;
      send(room.desktop, { type: "error", message: "room expired", room_id: id });
      try { room.desktop?.close(); } catch {}
      rooms.delete(id);
    }
  }
}, 30_000).unref();

server.listen(PORT, "0.0.0.0", () => {
  console.log(`pinvou remote relay listening on http://127.0.0.1:${PORT}`);
});
