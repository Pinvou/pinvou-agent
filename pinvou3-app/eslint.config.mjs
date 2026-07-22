import { defineConfig } from 'eslint/config';

const browserGlobals = Object.fromEntries([
  'Blob', 'CustomEvent', 'DOMParser', 'Event', 'FileReader', 'Image',
  'IntersectionObserver', 'ResizeObserver', 'URL', 'URLSearchParams',
  'cancelAnimationFrame', 'clearInterval', 'clearTimeout', 'console', 'crypto', 'document', 'fetch',
  'localStorage', 'navigator', 'performance', 'requestAnimationFrame',
  'sessionStorage', 'setInterval', 'setTimeout', 'structuredClone', 'window',
].map(name => [name, 'readonly']));

export default defineConfig([
  {
    files: [
      'src/app/main.jsx',
      'src/app/pet-main.jsx',
      'src/components/**/*.{js,jsx}',
      'src/features/**/*.{js,jsx}',
      'src/hooks/**/*.{js,jsx}',
      'src/shared/**/*.{js,jsx}',
    ],
    languageOptions: {
      ecmaVersion: 'latest',
      sourceType: 'module',
      parserOptions: { ecmaFeatures: { jsx: true } },
      globals: browserGlobals,
    },
    rules: {
      'no-undef': 'error',
    },
  },
]);
