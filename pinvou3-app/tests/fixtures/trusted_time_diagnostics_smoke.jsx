import React from 'react';
import { flushSync } from 'react-dom';
import { createRoot } from 'react-dom/client';
import TrustedTimeDiagnostics from '../../src/features/updater/TrustedTimeDiagnostics.jsx';
import { dictEn } from '../../src/shared/i18n/en.js';
import { dictJa } from '../../src/shared/i18n/ja.js';
import { dictZh } from '../../src/shared/i18n/zh.js';

// Test-only semantic adapters. These promises are DATA controls, not host/SCM,
// trusted-clock, network-control-service or cleanup authority.
const copies = { en: dictEn.trustedTime, ja: dictJa.trustedTime, zh: dictZh.trustedTime };
const calls = [];
function adapter(name) {
  const call = kind => new Promise((resolve, reject) => {
    calls.push({ name, kind, resolve, reject, settled: false });
  });
  return {
    inspectTrustedTime: () => call('read'),
    requestTrustedTimeRefresh: () => call('refresh'),
  };
}
const adapters = { A: adapter('A'), B: adapter('B') };
const root = createRoot(document.querySelector('#root'));
let current = { locale: 'en', adapter: 'A', supported: true, mounted: false };
const frames = [];
function snapshot() {
  const section = document.querySelector('section');
  return {
    text: section?.textContent || '',
    status: section?.querySelector('[role="status"]')?.textContent || '',
    alert: section?.querySelector('[role="alert"]')?.textContent || '',
    busy: section?.getAttribute('aria-busy') || null,
    buttons: section ? [...section.querySelectorAll('button')].map(button => ({
      text: button.textContent, disabled: button.disabled,
    })) : [],
  };
}
// Observe real DOM commits, including late async publications. render() also
// captures synchronously, so a replaced source cannot flash old state between
// a flushSync commit and the passive effect's next local read.
new MutationObserver(() => frames.push(snapshot())).observe(document.querySelector('#root'), {
  subtree: true, childList: true, characterData: true, attributes: true,
});
window.__timeDiagnostics = {
  render(patch) {
    current = { ...current, ...patch };
    document.documentElement.lang = current.locale;
    flushSync(() => root.render(current.mounted ? (
      <TrustedTimeDiagnostics copy={copies[current.locale]} supported={current.supported}
        updater={adapters[current.adapter]} />
    ) : null));
    const value = snapshot();
    frames.push(value);
    return value;
  },
  calls: () => calls.map(({ name, kind, settled }) => ({ name, kind, settled })),
  settle(name, kind, index, value, rejected = false) {
    const entry = calls.filter(call => call.name === name && call.kind === kind)[index];
    if (!entry || entry.settled) throw new Error('Missing or already settled fixture call');
    entry.settled = true;
    if (rejected) entry.reject(value); else entry.resolve(value);
  },
  snapshot,
  copies,
  frames: () => [...frames],
  clearFrames: () => { frames.length = 0; },
};
