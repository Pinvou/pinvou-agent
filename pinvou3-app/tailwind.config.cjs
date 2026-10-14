/** @type {import('tailwindcss').Config} */
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
