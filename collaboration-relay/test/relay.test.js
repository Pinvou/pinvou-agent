import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { WebSocket } from "ws";

process.env.PORT = "0";
process.env.PINVOU_COLLAB_PROJECT_TOKEN = "test-token";
process.env.PINVOU_COLLAB_PUBLIC_BASE_PATH = "/pinvou3/collaboration-test";
process.env.PINVOU_COLLAB_STATE_FILE = "";

const { server } = await import("../server.js");

before(async () => {
  if (server.listening) return;
  await new Promise((resolve) => server.once("listening", resolve));
});

function serverPort() {
  return server.address().port;
}

function waitOpen(ws) {
  return new Promise((resolve, reject) => {
    ws.once("open", resolve);
    ws.once("error", reject);
  });
}

function waitMessage(ws, predicate = () => true) {
  return new Promise((resolve) => {
    const onMessage = (raw) => {
      const msg = JSON.parse(String(raw));
      if (!predicate(msg)) return;
      ws.off("message", onMessage);
      resolve(msg);
    };
    ws.on("message", onMessage);
  });
}

function waitClose(ws) {
  return new Promise((resolve) => {
    if (ws.readyState === ws.CLOSED) {
      resolve();
      return;
    }
    ws.once("close", resolve);
  });
}

function request(pathname) {
  return new Promise((resolve, reject) => {
    http.get(`http://127.0.0.1:${serverPort()}${pathname}`, (res) => {
      let body = "";
      res.setEncoding("utf8");
      res.on("data", (chunk) => { body += chunk; });
      res.on("end", () => resolve({ statusCode: res.statusCode, headers: res.headers, body }));
    }).on("error", reject);
  });
}

async function registerPeer(peerId, displayName) {
  const ws = new WebSocket(`ws://127.0.0.1:${serverPort()}/pinvou3/collaboration-test/ws`);
  await waitOpen(ws);
  ws.send(JSON.stringify({
    v: 1,
    type: "peer_register",
    peer_id: peerId,
    project_id: "pinvou",
    project_token: "test-token",
    payload: { display_name: displayName, device_name: `${displayName}-PC` },
  }));
  const registered = await waitMessage(ws, (msg) => msg.type === "peer_registered");
  assert.equal(registered.to_peer_id, peerId);
  return ws;
}

after(() => {
  server.close();
});

test("serves the HTTPS invite landing page under the public base path", async () => {
  const res = await request("/pinvou3/collaboration-test/join?token=pinv1_demo");
  assert.equal(res.statusCode, 200);
  assert.match(res.headers["content-type"], /text\/html/);
  assert.match(res.body, /加入 Pinvou 协作/);
  assert.match(res.body, /pinvou:\/\/join\?token=pinv1_demo/);
  assert.match(res.body, /复制邀请口令/);
});

test("forwards task_create to an online peer and relays accept status", async () => {
  const a = await registerPeer("pv_peer_a", "许雨婷");
  const bOnline = waitMessage(a, (msg) => msg.type === "peer_status_changed" && msg.payload.peer_id === "pv_peer_b");
  const b = await registerPeer("pv_peer_b", "张三");
  await bOnline;

  a.send(JSON.stringify({
    v: 1,
    id: "msg_task_create",
    type: "task_create",
    from_peer_id: "pv_peer_a",
    to_peer_id: "pv_peer_b",
    project_id: "pinvou",
    payload: {
      task_id: "pct_1",
      title: "检查构建失败",
      instruction: "看一下 Tauri 配置",
      task_context: {
        share_mode: "full_task_session",
        session: { id: "session_a", message_count: 2 },
        messages: [{ role: "user", content: [{ type: "text", text: "背景" }] }],
        artifacts: [{ basename: "report.md", byte_size: 12 }],
      },
    },
  }));
  const incoming = await waitMessage(b, (msg) => msg.type === "task_create");
  assert.equal(incoming.payload.task_id, "pct_1");
  assert.equal(incoming.from_peer_id, "pv_peer_a");
  assert.equal(incoming.payload.task_context.share_mode, "full_task_session");
  assert.equal(incoming.payload.task_context.artifacts[0].basename, "report.md");

  b.send(JSON.stringify({
    v: 1,
    id: "msg_task_accept",
    type: "task_accept",
    from_peer_id: "pv_peer_b",
    to_peer_id: "pv_peer_a",
    project_id: "pinvou",
    payload: { task_id: "pct_1" },
  }));
  const accepted = await waitMessage(a, (msg) => msg.type === "task_accept");
  assert.equal(accepted.payload.task_id, "pct_1");

  a.close();
  b.close();
});

test("rejects registration without the project token", async () => {
  const ws = new WebSocket(`ws://127.0.0.1:${serverPort()}/pinvou3/collaboration-test/ws`);
  await waitOpen(ws);
  ws.send(JSON.stringify({
    v: 1,
    type: "peer_register",
    peer_id: "pv_peer_bad",
    project_id: "pinvou",
    project_token: "wrong",
    payload: { display_name: "Bad" },
  }));
  const error = await waitMessage(ws, (msg) => msg.type === "error");
  assert.equal(error.payload.code, "unauthorized");
  ws.close();
});

test("reports peer_offline instead of pretending delivery succeeded", async () => {
  const a = await registerPeer("pv_peer_sender", "发送方");
  a.send(JSON.stringify({
    v: 1,
    id: "msg_task_create_offline",
    type: "task_create",
    from_peer_id: "pv_peer_sender",
    to_peer_id: "pv_peer_missing",
    project_id: "pinvou",
    payload: { task_id: "pct_offline", title: "离线任务" },
  }));
  const failed = await waitMessage(a, (msg) => msg.type === "task_delivery_failed");
  assert.equal(failed.payload.reason, "peer_offline");
  assert.equal(failed.payload.task_id, "pct_offline");
  a.close();
});

test("queues task_create for a registered offline peer and delivers after reconnect", async () => {
  const a = await registerPeer("pv_peer_queue_sender", "发送方2");
  const b = await registerPeer("pv_peer_queue_target", "离线同事");
  b.close();
  await waitClose(b);

  a.send(JSON.stringify({
    v: 1,
    id: "msg_task_create_queued",
    type: "task_create",
    from_peer_id: "pv_peer_queue_sender",
    to_peer_id: "pv_peer_queue_target",
    project_id: "pinvou",
    payload: { task_id: "pct_queued", title: "离线可送达任务" },
  }));
  const pending = await waitMessage(a, (msg) => msg.type === "task_delivery_pending");
  assert.equal(pending.payload.task_id, "pct_queued");
  assert.equal(pending.payload.reason, "peer_offline");

  const reconnected = new WebSocket(`ws://127.0.0.1:${serverPort()}/pinvou3/collaboration-test/ws`);
  await waitOpen(reconnected);
  const incomingPromise = waitMessage(reconnected, (msg) => msg.type === "task_create");
  reconnected.send(JSON.stringify({
    v: 1,
    type: "peer_register",
    peer_id: "pv_peer_queue_target",
    project_id: "pinvou",
    project_token: "test-token",
    payload: { display_name: "离线同事", device_name: "离线同事-PC" },
  }));
  await waitMessage(reconnected, (msg) => msg.type === "peer_registered");
  const incoming = await incomingPromise;
  assert.equal(incoming.payload.task_id, "pct_queued");
  assert.equal(incoming.to_peer_id, "pv_peer_queue_target");

  a.close();
  reconnected.close();
});
