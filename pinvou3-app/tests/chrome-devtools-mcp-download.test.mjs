import assert from "node:assert/strict";
// A fake ClientRequest needs the EventEmitter-compatible `once`/`emit` API used by node:https.
import { EventEmitter } from "node:events";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { Readable } from "node:stream";
import test from "node:test";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const {
  downloadHttpsOnce,
  downloadWithRetries,
} = require("../scripts/tauri/chrome-devtools-mcp.js");

// import.meta.dirname needs Node >= 20.11; resolve the script path portably instead.
const SCRIPT_PATH = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "scripts",
  "tauri",
  "chrome-devtools-mcp.js",
);

function fakeHttpsGet(routes, requests) {
  return (url, options, onResponse) => {
    const request = new EventEmitter();
    request.setTimeout = () => request;
    request.destroy = (error) => request.emit("error", error);
    queueMicrotask(() => {
      const key = url.toString();
      requests.push({ url: key, options });
      const configuredRoute = routes.get(key);
      if (!configuredRoute) {
        request.emit("error", new Error(`unexpected request: ${key}`));
        return;
      }
      const route = typeof configuredRoute === "function" ? configuredRoute() : configuredRoute;
      const response = route.response || Readable.from(route.body ? [route.body] : []);
      response.statusCode = route.statusCode;
      response.headers = route.headers || {};
      onResponse(response);
    });
    return request;
  };
}

test("vendor download uses Node HTTPS and follows HTTPS redirects", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-cdmcp-download-"));
  const destination = path.join(root, "package.tgz");
  const requests = [];
  const get = fakeHttpsGet(
    new Map([
      [
        "https://registry.example.test/package.tgz",
        { statusCode: 302, headers: { location: "/objects/package.tgz" } },
      ],
      [
        "https://registry.example.test/objects/package.tgz",
        { statusCode: 200, body: Buffer.from("pinned tarball") },
      ],
    ]),
    requests,
  );
  try {
    await downloadHttpsOnce("https://registry.example.test/package.tgz", destination, { get });
    assert.equal(fs.readFileSync(destination, "utf8"), "pinned tarball");
    assert.deepEqual(
      requests.map(({ url }) => url),
      [
        "https://registry.example.test/package.tgz",
        "https://registry.example.test/objects/package.tgz",
      ],
    );
    assert.equal(requests[0].options.headers.Accept, "application/octet-stream");
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("vendor download rejects redirects that leave HTTPS", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-cdmcp-download-"));
  const destination = path.join(root, "package.tgz");
  const get = fakeHttpsGet(
    new Map([
      [
        "https://registry.example.test/package.tgz",
        {
          statusCode: 302,
          headers: { location: "http://mirror.example.test/package.tgz" },
        },
      ],
    ]),
    [],
  );
  try {
    await assert.rejects(
      downloadHttpsOnce("https://registry.example.test/package.tgz", destination, { get }),
      /redirect to http:/,
    );
    assert.equal(fs.existsSync(destination), false);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("vendor download retries without retaining a partial tarball", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-cdmcp-download-"));
  const destination = path.join(root, "package.tgz");
  const sleeps = [];
  let attempts = 0;
  try {
    await downloadWithRetries("https://registry.example.test/package.tgz", destination, {
      attempts: 3,
      retryDelayMs: 7,
      sleep: async (milliseconds) => {
        sleeps.push(milliseconds);
      },
      download: async (_url, file) => {
        attempts += 1;
        if (attempts === 1) {
          fs.writeFileSync(file, "partial");
          throw new Error("transient network failure");
        }
        assert.equal(fs.existsSync(file), false, "a retry must not reuse partial bytes");
        fs.writeFileSync(file, "complete");
      },
    });
    assert.equal(attempts, 2);
    assert.deepEqual(sleeps, [7]);
    assert.equal(fs.readFileSync(destination, "utf8"), "complete");
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

for (const discardedStatus of [302, 503]) {
  test(`vendor download retries when a discarded HTTP ${discardedStatus} response errors`, async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-cdmcp-download-"));
    const destination = path.join(root, "package.tgz");
    const requests = [];
    let discardedResponses = 0;
    const get = fakeHttpsGet(
      new Map([
        [
          "https://registry.example.test/package.tgz",
          () => {
            discardedResponses += 1;
            if (discardedResponses === 1) {
              const response = new Readable({
                read: () => response.destroy(
                  new Error(`discarded ${discardedStatus} response failed`),
                ),
              });
              return {
                statusCode: discardedStatus,
                headers: discardedStatus === 302 ? { location: "/objects/package.tgz" } : {},
                response,
              };
            }
            return discardedStatus === 302
              ? { statusCode: 302, headers: { location: "/objects/package.tgz" } }
              : { statusCode: 200, body: Buffer.from("recovered tarball") };
          },
        ],
        [
          "https://registry.example.test/objects/package.tgz",
          { statusCode: 200, body: Buffer.from("redirected tarball") },
        ],
      ]),
      requests,
    );
    const expectedBody = discardedStatus === 302 ? "redirected tarball" : "recovered tarball";
    try {
      await downloadWithRetries("https://registry.example.test/package.tgz", destination, {
        attempts: 2,
        retryDelayMs: 0,
        sleep: async () => {},
        download: (url, file) => downloadHttpsOnce(url, file, { get }),
      });
      assert.equal(fs.readFileSync(destination, "utf8"), expectedBody);
      assert.equal(discardedResponses, 2, "the discarded response error must reach the retry loop");
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });
}

test("vendor download enforces a wall-clock deadline on a stalled attempt", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-cdmcp-download-"));
  const destination = path.join(root, "package.tgz");
  let attempts = 0;
  // The response never ends and keeps the stream "active", so only the wall-clock
  // deadline can stop the attempt; request.setTimeout (inactivity) would never fire.
  const get = (url, options, onResponse) => {
    attempts += 1;
    const response = new Readable({ read: () => {} });
    response.statusCode = 200;
    response.headers = {};
    queueMicrotask(() => onResponse(response));
    return { once: () => {}, setTimeout: () => {} };
  };
  try {
    await assert.rejects(
      downloadWithRetries("https://registry.example.test/package.tgz", destination, {
        attempts: 2,
        retryDelayMs: 0,
        sleep: async () => {},
        download: (url, file) => downloadHttpsOnce(url, file, { get, timeoutMs: 25 }),
      }),
      /timed out/,
    );
    assert.equal(attempts, 2, "the deadline failure must reach the retry loop");
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("vendor download retry survives a locked partial tarball", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-cdmcp-download-"));
  const destination = path.join(root, "package.tgz");
  const cleanupFailures = [];
  let attempts = 0;
  try {
    await downloadWithRetries("https://registry.example.test/package.tgz", destination, {
      attempts: 3,
      retryDelayMs: 0,
      sleep: async () => {},
      removePartial: (file, options) => {
        if (attempts === 1) {
          // Simulate Windows antimalware briefly locking the fresh tarball.
          const error = new Error("EBUSY: resource busy or locked");
          cleanupFailures.push(error.message);
          throw error;
        }
        fs.rmSync(file, options);
      },
      download: async (_url, file) => {
        attempts += 1;
        fs.writeFileSync(file, attempts === 1 ? "partial" : "complete");
        if (attempts === 1) throw new Error("transient network failure");
      },
    });
    assert.equal(attempts, 2, "a failed cleanup must not abandon the remaining retries");
    assert.deepEqual(cleanupFailures, ["EBUSY: resource busy or locked"]);
    assert.equal(fs.readFileSync(destination, "utf8"), "complete");
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("vendor download rejects non-HTTPS targets before any request", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-cdmcp-download-"));
  const destination = path.join(root, "package.tgz");
  const requests = [];
  const get = fakeHttpsGet(new Map(), requests);
  try {
    await assert.rejects(
      downloadHttpsOnce("http://registry.example.test/package.tgz", destination, { get }),
      /Refusing non-HTTPS vendor download: http:/,
    );
    assert.equal(requests.length, 0, "no socket may be opened for a plain-HTTP target");
    assert.equal(fs.existsSync(destination), false);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("vendor download fails cleanly on a non-2xx response", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-cdmcp-download-"));
  const destination = path.join(root, "package.tgz");
  const requests = [];
  const get = fakeHttpsGet(
    new Map([
      [
        "https://registry.example.test/package.tgz",
        { statusCode: 404, body: Buffer.from("not found") },
      ],
    ]),
    requests,
  );
  try {
    await assert.rejects(
      downloadHttpsOnce("https://registry.example.test/package.tgz", destination, { get }),
      /Vendor download failed with HTTP 404/,
    );
    assert.equal(fs.existsSync(destination), false);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("vendor download exceeds its redirect cap", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-cdmcp-download-"));
  const destination = path.join(root, "package.tgz");
  // With maxRedirects 2 the loop issues three requests (counts 0-2); the
  // third redirect response trips the cap, so /hop3 is never requested.
  const get = fakeHttpsGet(
    new Map([
      ["https://registry.example.test/package.tgz", { statusCode: 302, headers: { location: "/objects/package.tgz" } }],
      ["https://registry.example.test/objects/package.tgz", { statusCode: 302, headers: { location: "/hop2/package.tgz" } }],
      ["https://registry.example.test/hop2/package.tgz", { statusCode: 302, headers: { location: "/hop3/package.tgz" } }],
    ]),
    [],
  );
  try {
    await assert.rejects(
      downloadHttpsOnce("https://registry.example.test/package.tgz", destination, {
        get,
        maxRedirects: 2,
      }),
      /Vendor download exceeded 2 redirects/,
    );
    assert.equal(fs.existsSync(destination), false);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("vendor script has no external curl dependency", () => {
  const source = fs.readFileSync(SCRIPT_PATH, "utf8");
  assert.match(source, /require\("node:https"\)/);
  assert.doesNotMatch(source, /run\(\s*["']curl["']/);
});
