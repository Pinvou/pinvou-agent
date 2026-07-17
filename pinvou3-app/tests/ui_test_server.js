const { createReadStream, existsSync, statSync } = require('fs');
const { createServer } = require('http');
const { extname, isAbsolute, join, relative, resolve } = require('path');

// CI / local verification can point at an isolated Vite output when the normal
// dist directory is read-only or intentionally preserved.
const root = resolve(process.env.PINVOU3_UI_TEST_ROOT || resolve(__dirname, '../dist'));
const mime = {
  '.css': 'text/css; charset=utf-8',
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.jpg': 'image/jpeg',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.webp': 'image/webp',
};

function startUiTestServer() {
  if (!existsSync(join(root, 'index.html'))) {
    throw new Error('UI build not found; run `npm run build:ui` before browser smoke tests');
  }
  return new Promise((resolveReady, reject) => {
    const server = createServer((req, res) => {
      const pathname = decodeURIComponent(new URL(req.url || '/', 'http://127.0.0.1').pathname);
      const candidate = resolve(root, `.${pathname === '/' ? '/index.html' : pathname}`);
      const rel = relative(root, candidate);
      if (rel.startsWith('..') || isAbsolute(rel)) {
        res.writeHead(403).end('Forbidden');
        return;
      }
      const file = existsSync(candidate) && statSync(candidate).isFile() ? candidate : join(root, 'index.html');
      res.writeHead(200, {
        'content-type': mime[extname(file).toLowerCase()] || 'application/octet-stream',
        'cache-control': 'no-store',
      });
      createReadStream(file).pipe(res);
    });
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      server.unref();
      const address = server.address();
      resolveReady({ server, url: `http://127.0.0.1:${address.port}` });
    });
  });
}

module.exports = { startUiTestServer };
