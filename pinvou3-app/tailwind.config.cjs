/** @type {import('tailwindcss').Config} */
// tailwindcss stays on the 3.x line (`v3-lts` dist-tag, 3.4.19): v4's emitted
// CSS requires Safari 16.4+ (cascade layers, @property, color-mix), above the
// macOS 11 / Safari 14.0 WKWebView floor that tests/compat_audit.test.mjs and
// scripts/audit-compat.mjs enforce on the built output.
// tests/fixtures/**/*.html carries its own utility classes on fixture bodies
// (e.g. bg-[#f5f5f5] in the smoke pages). Unlike the removed Play runtime,
// which scanned the live DOM in the dev server, build-time compilation only
// emits classes present in a scanned file, so the fixtures need to be part
// of the content set or their pages silently lose styling.
module.exports = {
  content: [
    './src/**/*.{html,js,jsx,mjs}',
    './tests/fixtures/**/*.{html,jsx}',
  ],
  darkMode: 'class',
};
