import { cpSync, existsSync, mkdirSync, readdirSync } from 'node:fs';
import { extname, join, resolve } from 'node:path';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

const sourceRoot = resolve(import.meta.dirname, 'src');
const staticExtensions = new Set([
  '.avif', '.gif', '.ico', '.jpeg', '.jpg', '.png', '.svg', '.webp',
]);
const staticScripts = new Set([
  'features/personas/personas-i18n.js',
  'features/updater/update-notice-logic.js',
  'platform/tauri/bridge.js',
  'vendor/marked.min.js',
  'vendor/purify.min.js',
  'vendor/tailwind.js',
]);

function copyRuntimeAssets() {
  let outputRoot;
  return {
    name: 'pinvou-copy-runtime-assets',
    apply: 'build',
    configResolved(config) {
      outputRoot = resolve(config.root, config.build.outDir);
    },
    closeBundle() {
      const visit = (dir) => {
        for (const entry of readdirSync(dir, { withFileTypes: true })) {
          const source = join(dir, entry.name);
          if (entry.isDirectory()) {
            visit(source);
            continue;
          }
          const relative = source.slice(sourceRoot.length + 1).replaceAll('\\', '/');
          if (!staticExtensions.has(extname(entry.name).toLowerCase()) && !staticScripts.has(relative)) continue;
          const target = join(outputRoot, relative);
          mkdirSync(resolve(target, '..'), { recursive: true });
          cpSync(source, target);
        }
      };
      if (existsSync(sourceRoot)) visit(sourceRoot);
    },
  };
}

export default defineConfig({
  root: 'src',
  publicDir: false,
  server: {
    host: process.env.PINVOU3_UI_DEV_HOST || '127.0.0.1',
    port: Number(process.env.PINVOU3_UI_DEV_PORT || 1420),
    strictPort: true,
  },
  plugins: [react(), copyRuntimeAssets()],
  build: {
    outDir: '../dist',
    emptyOutDir: true,
    rolldownOptions: {
      input: {
        main: resolve(sourceRoot, 'index.html'),
        pet: resolve(sourceRoot, 'pet.html'),
      },
    },
  },
});
