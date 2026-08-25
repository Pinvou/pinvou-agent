// Static analysis baseline for the Pinvou frontend.
//
// Language floor: ES2021 (not "latest"). The binding webview is macOS 11's
// Safari 14.0 WKWebView (see .browserslistrc and vite.config.js build.target);
// ES2022 syntax (class fields, static blocks, top-level await) is a parse
// error there and blanks the whole chunk. scripts/audit-compat.mjs enforces
// the same floor on built output; this config enforces it at lint time so
// violations surface before a build.
//
// Tooling notes that drove rule selection:
//   - eslint-plugin-react@7 pins `settings.react.version` because `detect`
//     still calls the removed context.getFilename() API under ESLint 10.
//   - unicorn rules whose fixes emit Safari 15.4+ APIs are deliberately off:
//     no-array-sort / no-array-reverse (→ toSorted/toReversed),
//     prefer-array-last-methods / prefer-at (→ .at()), and prefer-await
//     (top-level await is unparseable at the Safari 14 baseline).
//     prefer-blob-reading-methods (→ blob.text()/arrayBuffer(), Safari 14.0.1+)
//     is off as well, and prefer-spread is disabled per-site where the operand
//     is a WebKit non-iterable DOM list (DOMRectList/FileList/DataTransferItemList).
import { defineConfig } from 'eslint/config';
import js from '@eslint/js';
import globals from 'globals';
import compat from 'eslint-plugin-compat';
import react from 'eslint-plugin-react';
import reactHooks from 'eslint-plugin-react-hooks';
import importX from 'eslint-plugin-import-x';
import jsdoc from 'eslint-plugin-jsdoc';
import unicorn from 'eslint-plugin-unicorn';
import sonarjs from 'eslint-plugin-sonarjs';
import nPlugin from 'eslint-plugin-n';

const srcFiles = [
  'src/app/**/*.{js,jsx}',
  'src/components/**/*.{js,jsx}',
  'src/features/**/*.{js,jsx}',
  'src/hooks/**/*.{js,jsx}',
  'src/platform/**/*.{js,jsx}',
  'src/shared/**/*.{js,jsx}',
];

export default defineConfig([
  {
    files: srcFiles,
    languageOptions: {
      ecmaVersion: 2021,
      sourceType: 'module',
      parserOptions: { ecmaFeatures: { jsx: true } },
      globals: { ...globals.browser, DOMPurify: 'readonly', marked: 'readonly' },
    },
    plugins: { compat, 'import-x': importX, jsdoc, unicorn, sonarjs },
    settings: { 'import-x/resolver': { node: { extensions: ['.js', '.jsx'] } } },
    rules: {
      ...js.configs.recommended.rules,

      // ---- correctness / bug class (core) ----
      'no-var': 'error',
      'prefer-const': 'error',
      eqeqeq: ['error', 'smart'],
      'no-eval': 'error',
      'no-implied-eval': 'error',
      'no-new-func': 'error',
      'no-script-url': 'error',
      'no-proto': 'error',
      'no-throw-literal': 'error',
      'no-return-await': 'error',
      'no-promise-executor-return': 'error',
      'no-async-promise-executor': 'error',
      'prefer-promise-reject-errors': 'error',
      'no-prototype-builtins': 'error',
      'no-shadow-restricted-names': 'error',
      'no-self-assign': 'error',
      'no-self-compare': 'error',
      'no-unmodified-loop-condition': 'error',
      'no-unreachable-loop': 'error',
      'no-unsafe-negation': 'error',
      'no-unsafe-optional-chaining': 'error',
      'no-unused-private-class-members': 'error',
      'no-constructor-return': 'error',
      'no-new-native-nonconstructor': 'error',
      'array-callback-return': ['error', { allowImplicit: true }],
      'no-implicit-globals': 'error',
      'no-caller': 'error',
      'no-useless-call': 'error',
      'no-labels': 'error',
      'no-multi-str': 'error',
      'no-iterator': 'error',
      'no-with': 'error',
      'no-octal': 'error',
      'symbol-description': 'error',
      'no-undef-init': 'error',
      'object-shorthand': ['error', 'properties'],
      'prefer-rest-params': 'error',
      'prefer-spread': 'error',
      'no-useless-rename': 'error',
      'no-useless-concat': 'error',
      'no-useless-catch': 'error',

      // ---- webview compatibility gate ----
      'compat/compat': 'error',

      // ---- import correctness ----
      ...importX.configs.recommended.rules,
      'import-x/no-duplicates': 'error',
      'import-x/no-self-import': 'error',
      'import-x/no-useless-path-segments': 'error',
      'import-x/no-anonymous-default-export': 'error',
      // no-cycle is delegated to madge (lint:cycles), which reports the full
      // cycle graph instead of one edge at a time.

      // ---- jsdoc: check what exists, don't demand new prose ----
      ...jsdoc.configs['recommended-typescript-flavor'].rules,
      'jsdoc/require-jsdoc': 'off', // retrofitting ~1.9k doc blocks is churn, not signal
      'jsdoc/require-param': 'off',
      'jsdoc/require-returns': 'off',
      'jsdoc/require-property-description': 'off',
      'jsdoc/informative-docs': 'off',

      // ---- unicorn: correctness/modernization subset ----
      // Rules whose fix output would violate the Safari 14 baseline are
      // intentionally absent (see header).
      'unicorn/no-useless-undefined': 'error',
      'unicorn/prefer-includes': 'error',
      'unicorn/prefer-includes-over-repeated-comparisons': 'error',
      'unicorn/prefer-spread': 'error',
      'unicorn/no-useless-fallback-in-spread': 'error',
      'unicorn/prefer-string-replace-all': 'error',
      'unicorn/no-unsafe-string-replacement': 'error',
      'unicorn/escape-case': 'error',
      'unicorn/prefer-number-properties': 'error',
      'unicorn/prefer-number-is-safe-integer': 'error',
      'unicorn/prefer-number-coercion': 'error',
      'unicorn/prefer-optional-catch-binding': 'error',
      'unicorn/no-negated-array-predicate': 'error',
      'unicorn/no-this-outside-of-class': 'error',
      'unicorn/no-undeclared-class-members': 'error',
      'unicorn/no-typeof-undefined': 'error',
      'unicorn/no-impossible-length-comparison': 'error',
      'unicorn/no-invalid-argument-count': 'error',
      'unicorn/no-new-array': 'error',
      'unicorn/new-for-builtins': 'error',
      'unicorn/no-await-expression-member': 'error',
      'unicorn/no-immediate-mutation': 'error',
      'unicorn/no-useless-promise-resolve-reject': 'error',
      'unicorn/no-return-array-push': 'error',
      'unicorn/no-lonely-if': 'error',
      'unicorn/no-duplicate-if-branches': 'error',
      'unicorn/no-unnecessary-nested-ternary': 'error',
      'unicorn/no-negated-condition': 'error',
      'unicorn/prefer-continue': 'error',
      'unicorn/prefer-else-if': 'error',
      'unicorn/prefer-code-point': 'error',
      'unicorn/prefer-string-raw': 'error',
      'unicorn/no-unnecessary-string-trim': 'error',
      'unicorn/no-useless-coercion': 'error',
      'unicorn/no-unnecessary-global-this': 'error',
      // Autofix yields blob.text()/blob.arrayBuffer() (Safari 14.0.1+, slightly above the 14.0.0 floor) and audit-compat does not cover these two member APIs; disabled.
      'unicorn/prefer-blob-reading-methods': 'off',
      'unicorn/prefer-dom-node-append': 'error',
      'unicorn/prefer-dom-node-remove': 'error',
      'unicorn/prefer-dom-node-text-content': 'error',
      'unicorn/prefer-dom-node-dataset': 'error',
      'unicorn/prefer-query-selector': 'error',
      'unicorn/require-array-sort-compare': 'error',
      'unicorn/throw-new-error': 'error',

      // ---- sonar: bug + dead-code class ----
      ...sonarjs.configs.recommended.rules,
      'sonarjs/no-nested-conditional': 'off', // JSX className ternaries are idiomatic
      'sonarjs/no-nested-functions': 'off', // callback-based DOM/React patterns
      'sonarjs/no-ignored-exceptions': 'off', // intentional silent catches; no-empty already guards
      'sonarjs/cognitive-complexity': ['error', 30], // Sonar's 15 default is noise for UI code; 30 catches the outliers
      'sonarjs/todo-tag': 'off',
      'sonarjs/prefer-single-boolean': 'off',
      'sonarjs/no-nested-switch': 'off',
      'sonarjs/no-identical-functions': 'warn',
      'sonarjs/no-duplicated-branches': 'error',
      'sonarjs/no-all-duplicated-branches': 'error',
    },
  },
  {
    files: srcFiles,
    plugins: { react },
    settings: { react: { version: '19.2' } },
    rules: {
      ...react.configs.flat.recommended.rules,
      ...react.configs.flat['jsx-runtime'].rules,
      // Pure-JS codebase: no PropTypes by design. Component prop contracts
      // are checked by tsc --checkJs (jsconfig.json) with JSDoc types.
      'react/prop-types': 'off',
    },
  },
  {
    files: srcFiles,
    plugins: { 'react-hooks': reactHooks },
    rules: {
      ...reactHooks.configs.flat['recommended-latest'].rules,
      'react-hooks/exhaustive-deps': 'error',
    },
  },
  // Node-side code (tests, build scripts) runs on Node 22 in CI, so its
  // language floor is the Node 22 feature set, not the webview ES2021 one.
  {
    files: ['tests/**/*.{js,mjs}', 'scripts/**/*.{js,mjs}', 'vite.config.js'],
    languageOptions: {
      ecmaVersion: 2024,
      sourceType: 'module',
      // Tests evaluate frontend sources in vm sandboxes and puppeteer pages,
      // so window/document appear as identifiers there too.
      globals: { ...globals.node, ...globals.browser },
    },
    plugins: { n: nPlugin, jsdoc, unicorn, sonarjs, 'import-x': importX },
    settings: {
      'import-x/resolver': { node: { extensions: ['.js', '.mjs'] } },
      // CI runs Node 22 (latest 22.x, which is >= 22.16 where
      // import.meta.dirname landed); without this the n plugin falls back to
      // its built-in >=16 floor and flags every modern builtin.
      n: { version: '>=22.16.0 <23' },
    },
    rules: {
      ...js.configs.recommended.rules,
      ...nPlugin.configs['flat/recommended-module'].rules,
      ...importX.configs.recommended.rules,
      // eslint.config.mjs imports the lint plugins themselves; import-x cannot
      // parse their dual ESM/CJS entries and reports phantom parse errors.
      // These two rules lack an ignore option, so scope them to code files.
      'no-var': 'error',
      'prefer-const': 'error',
      eqeqeq: ['error', 'smart'],
      'no-throw-literal': 'error',
      'no-promise-executor-return': 'error',
      'prefer-promise-reject-errors': 'error',
      'no-useless-catch': 'error',
      'object-shorthand': ['error', 'properties'],
      'unicorn/no-useless-undefined': 'error',
      'unicorn/prefer-includes': 'error',
      'unicorn/prefer-spread': 'error',
      'unicorn/no-unsafe-string-replacement': 'error',
      'unicorn/prefer-top-level-await': 'error',
      'unicorn/no-lonely-if': 'error',
      'unicorn/require-array-sort-compare': 'error',
      // Harness scripts deliberately exit with a code from any depth.
      'n/no-process-exit': 'off',
      // localStorage/navigator look like experimental Node globals to the n
      // plugin, but in tests they run inside puppeteer page.evaluate()
      // browser contexts where they are ordinary web APIs, not Node builtins.
      'n/no-unsupported-features/node-builtins': [
        'error',
        { ignores: ['localStorage', 'navigator'] },
      ],
      // Many test entries are run directly by URL in package.json scripts
      // without extensions in shebang form; hashbangs are intentional there.
      'n/hashbang': 'off',
    },
  },
  {
    ignores: ['dist/**', 'node_modules/**', 'src/vendor/**', 'src-tauri/**', 'coverage/**'],
  },
]);
