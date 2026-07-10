import { createServer } from "node:http";
import { createReadStream, existsSync, statSync } from "node:fs";
import { dirname, extname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const host = process.env.PINVOU3_UI_DEV_HOST || "127.0.0.1";
const port = Number(process.env.PINVOU3_UI_DEV_PORT || 1420);
const scriptDir = dirname(fileURLToPath(import.meta.url));
const root = resolve(scriptDir, "../../src");

const mime = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".webp": "image/webp",
  ".ico": "image/x-icon",
  ".wasm": "application/wasm",
};

function send(res, status, body, type = "text/plain; charset=utf-8") {
  res.writeHead(status, {
    "content-type": type,
    "cache-control": "no-store",
    "access-control-allow-origin": "*",
  });
  res.end(body);
}

const server = createServer((req, res) => {
  let url;
  try {
    url = new URL(req.url || "/", `http://${host}:${port}`);
  } catch {
    return send(res, 400, "Bad request");
  }
  let pathname;
  try {
    pathname = decodeURIComponent(url.pathname);
  } catch {
    return send(res, 400, "Bad request");
  }
  const candidate = resolve(root, `.${pathname === "/" ? "/index.html" : pathname}`);
  const rel = relative(root, candidate);
  if (rel.startsWith("..") || isAbsolute(rel)) return send(res, 403, "Forbidden");
  const file = existsSync(candidate) && statSync(candidate).isFile() ? candidate : join(root, "index.html");
  if (!existsSync(file)) return send(res, 404, "Not found");
  res.writeHead(200, {
    "content-type": mime[extname(file).toLowerCase()] || "application/octet-stream",
    "cache-control": "no-store",
    "access-control-allow-origin": "*",
  });
  createReadStream(file).pipe(res);
});

server.listen(port, host, () => {
  console.log(`[pinvou3-ui-dev] serving ${root} at http://${host}:${port}`);
});
