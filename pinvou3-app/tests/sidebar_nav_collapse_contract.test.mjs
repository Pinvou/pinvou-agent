import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const main = readFileSync(new URL('../src/app/main.jsx', import.meta.url), 'utf8');

// The primary-nav collapse is a global manual toggle: nothing auto-collapses it,
// no mode switch (entering or leaving code mode included) resets it, and both
// toggle rows are reachable in every mode on the desktop sidebar. The phone
// drawer (compact shell) is the one deliberate exception — it always keeps the
// full nav so the task list keeps its vertical room (web-ui.smoke drawer-height
// contract). This pins the removal of the old code-mode auto-collapse; main.jsx
// has no DOM harness, so the invariant is pinned on the state sources in the
// style of code_mode_exit_contract.test.mjs.

// 1. The state persists and starts expanded: absent storage reads as false.
assert.match(
  main,
  /const \[sidebarNavCollapsed, setSidebarNavCollapsed\] = useState\(\(\) => \{/,
  'nav collapse state must read its default from persisted storage',
);
assert.ok(
  main.includes("localStorage.getItem('pinvou_sidebar_nav_collapsed')"),
  'nav collapse choice must persist via pinvou_sidebar_nav_collapsed',
);

// 2. No mode transition resets the state: the old exit-effect shape must stay gone.
assert.ok(!main.includes('codeNavExpanded'), 'legacy code-scoped collapse state must not come back');
assert.ok(
  !/if \(!codeModeOn\) setSidebarNavCollapsed/.test(main),
  'exiting code mode must not reset the nav collapse state',
);
assert.ok(!main.includes('codeStyleActive'), 'code-style-only nav gating must stay removed');

// 3. Both rows are gated on the open desktop sidebar alone — never on code mode
//    or the code style — and persist the manual choice, so the toggle exists in
//    work mode as well and is never mode-driven.
assert.match(
  main,
  /\{isSidebarOpen && !isCompactShell && sidebarNavCollapsed \? \(/,
  'expand row must render from the global state alone',
);
assert.match(
  main,
  /onClick=\{\(\) => setSidebarNavCollapsedPersisted\(false\)\}/,
  'expand row must persist the manual choice',
);
assert.match(
  main,
  /data-testid="sidebar-primary-nav-collapse"/,
  'collapse row must stay identified for the smoke coverage',
);
assert.match(
  main,
  /onClick=\{\(\) => setSidebarNavCollapsedPersisted\(true\)\}/,
  'collapse row must persist the manual choice',
);
assert.match(
  main,
  /\{isSidebarOpen && !isCompactShell && \(\s*<button\s*type="button"\s*data-testid="sidebar-primary-nav-collapse"/,
  'collapse row must stay gated on the open desktop sidebar only',
);

console.log('sidebar nav collapse contract tests passed');
