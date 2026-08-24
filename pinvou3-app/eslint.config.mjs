import { defineConfig } from 'eslint/config';
import compat from 'eslint-plugin-compat';

const browserGlobals = Object.fromEntries([
  'Blob', 'CustomEvent', 'DOMParser', 'Event', 'FileReader', 'Image', 'TextDecoder', 'TextEncoder',
  'IntersectionObserver', 'ResizeObserver', 'URL', 'URLSearchParams',
  'WebSocket', 'atob', 'btoa', 'cancelAnimationFrame', 'clearInterval', 'clearTimeout', 'console', 'crypto', 'document', 'fetch',
  'localStorage', 'navigator', 'performance', 'requestAnimationFrame',
  'queueMicrotask', 'sessionStorage', 'setInterval', 'setTimeout', 'structuredClone', 'window',
  'DOMPurify', 'marked',
].map(name => [name, 'readonly']));

export default defineConfig([
  {
    files: [
      'src/app/**/*.{js,jsx}',
      'src/components/**/*.{js,jsx}',
      'src/features/**/*.{js,jsx}',
      'src/hooks/**/*.{js,jsx}',
      'src/platform/**/*.{js,jsx}',
      'src/shared/**/*.{js,jsx}',
    ],
    languageOptions: {
      ecmaVersion: 'latest',
      sourceType: 'module',
      parserOptions: { ecmaFeatures: { jsx: true } },
      globals: browserGlobals,
    },
    plugins: { compat },
    rules: {
      'no-undef': 'error',
      // Webview compatibility gate: flags APIs outside .browserslistrc
      // (Safari 14 / iOS 14 / Chromium 107 / Firefox 115).
      'compat/compat': 'error',
    },
  },
]);
