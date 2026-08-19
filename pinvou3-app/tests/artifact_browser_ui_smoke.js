#!/usr/bin/env node
const fs = require('fs');
const os = require('os');
const path = require('path');

const puppeteer = require('puppeteer-core');

const CHROME = process.env.CHROME || [
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
  '/usr/bin/google-chrome',
  '/usr/bin/google-chrome-stable',
  '/snap/bin/chromium',
].find(candidate => fs.existsSync(candidate));

if (!CHROME) {
  console.error('SKIP: Chromium is required for the artifact browser smoke test');
  process.exit(2);
}

const maliciousHtml = `<!doctype html><html><head>
  <meta http-equiv="refresh" content="0;url=http://127.0.0.1:54321/meta-leak">
  <script>parent.postMessage({type:'pinvou:artifact-preview:open-external',url:'https://example.com/auto'},'*');location.href='http://127.0.0.1:54321/script-leak';</script>
</head><body>
  <img src="http://127.0.0.1:54321/image-leak" alt="remote">
  <form action="http://127.0.0.1:54321/form-leak"><input name="password"><button>登录</button></form>
  <a id="hash-one" href="#section-one">第一节</a>
  <a id="hash-two" href="#section-two">第二节</a>
  <a id="safe-link" href="https://example.com/docs?q=1">查看来源</a>
  <section id="section-one">章节一</section><section id="section-two">章节二</section>
  <svg id="inline-chart" viewBox="0 0 120 40" aria-label="趋势图"><path d="M0 36 L30 28 L60 30 L90 12 L120 4" fill="none" stroke="#2878ff" stroke-width="4"/><circle cx="90" cy="12" r="4" fill="#2878ff"/></svg>
  <svg><image href="http://127.0.0.1:54321/svg-image-leak" width="1" height="1"/></svg>
  <h1>交付物安全预览</h1>
  <div id="animated" style="animation:spin 1s linear infinite"></div><style>@keyframes spin{to{transform:rotate(1turn)}}</style>
</body></html>`;

const mockScript = `
  window.__ARTIFACT_OPENED__ = [];
  window.__ARTIFACT_SYSTEM_OPENED__ = [];
  window.__ARTIFACT_TEXT_READS__ = 0;
  window.__ARTIFACT_IMAGE_READS__ = 0;
  window.__ARTIFACT_VISUAL_RENDERS__ = 0;
  window.__ARTIFACT_HTML__ = new TextDecoder().decode(Uint8Array.from(atob(${JSON.stringify(Buffer.from(maliciousHtml).toString('base64'))}), function(char){ return char.charCodeAt(0); }));
  window.TauriBridge = {
    available: true,
    artifacts: {
      artifactInfo: async function(path) {
        if (String(path).includes('slow.html')) await new Promise(function(resolve){ setTimeout(resolve, 220); });
        var ext = String(path || '').split('.').pop().toLowerCase();
        var kind = ext === 'html' ? 'html' : ext === 'md' ? 'md' : ext === 'txt' ? 'text' : ext === 'png' ? 'image' : ext === 'pdf' ? 'pdf' : ext === 'docx' ? 'docx' : 'other';
        var size = String(path).includes('oversized.png') ? 25_000_001 : String(path).includes('oversized.pdf') ? 50 * 1024 * 1024 + 1 : String(path).includes('oversized') ? 10 * 1024 * 1024 + 1 : 2048;
        return { exists: true, kind: kind, size: size, modified: 1 };
      },
      readArtifactText: async function(path) {
        window.__ARTIFACT_TEXT_READS__ += 1;
        return String(path).endsWith('.md') ? '<h1>Markdown 报告</h1><p><a href="https://example.com/md">来源</a></p>' : window.__ARTIFACT_HTML__;
      },
      readArtifactImageB64: async function() {
        window.__ARTIFACT_IMAGE_READS__ += 1;
        return 'data:image/gif;base64,R0lGODlhAQABAAD/ACwAAAAAAQABAAACADs=';
      },
      readArtifactThumbnail: async function() { return null; },
      renderArtifactVisual: async function(path) {
        window.__ARTIFACT_VISUAL_RENDERS__ += 1;
        if (String(path).endsWith('.pdf')) return { mode: 'images', html: null, images: ['data:image/gif;base64,R0lGODlhAQABAAD/ACwAAAAAAQABAAACADs='], warning: null };
        return { mode: 'html', html: '<h1>Office 报告</h1><img src="http://127.0.0.1:54321/office-leak">', images: [], warning: null };
      },
      openArtifactExternal: async function(path, sessionId) { window.__ARTIFACT_SYSTEM_OPENED__.push({ path: path, sessionId: sessionId }); },
      openUserExternalUrl: async function(url) { window.__ARTIFACT_OPENED__.push(url); }
    },
    rendering: { renderMarkdown: function(value) { return String(value || ''); } }
  };
`;

const harnessModule = String.raw`
  import React, { useEffect, useState } from 'react';
  import { createRoot } from 'react-dom/client';
  import { ArtifactCard } from '/features/tools/tool-common.jsx';
  import { ArtifactBrowser } from '/features/artifacts/FilePreviewModal.jsx';
  import { ScaledHtmlPreview } from '/features/settings/SettingsView.jsx';
  import { dict } from '/shared/i18n.js';

  function Harness() {
    const [path, setPath] = useState('/home/tester/report.html');
    const [browser, setBrowser] = useState(null);
    const [showScaledFocusFixture, setShowScaledFocusFixture] = useState(false);
    useEffect(() => {
      window.__SET_ARTIFACT_PATH__ = nextPath => { setBrowser(null); setPath(nextPath); };
      window.__SHOW_SCALED_FOCUS_FIXTURE__ = setShowScaledFocusFixture;
    }, []);
    const item = { path, sessionId: 'session-artifact-smoke', title: path.split('/').pop() };
    return React.createElement(
      'main',
      { style: { minHeight:'100vh', boxSizing:'border-box', padding:'180px 64px', background:'#eef2f7' } },
      React.createElement('div', { style: { width:'560px' } }, React.createElement(ArtifactCard, { item, theme:'light', t:dict.zh, onOpen:setBrowser })),
      browser ? React.createElement(ArtifactBrowser, { ...browser, theme:'light', t:dict.zh, onClose:() => setBrowser(null) }) : null,
      showScaledFocusFixture ? React.createElement(
        'section',
        { id:'scaled-focus-fixture', style:{ width:'560px', height:'260px', marginTop:'24px' } },
        React.createElement('button', { id:'scaled-before' }, 'before'),
        React.createElement(ScaledHtmlPreview, { html:'<a id="scaled-only-link" href="https://example.com/scaled">link</a>' }),
        React.createElement('button', { id:'scaled-after' }, 'after'),
      ) : null,
    );
  }
  createRoot(document.getElementById('root')).render(React.createElement(Harness));
`;

const harnessHtml = `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><script>${mockScript}</script><script src="/vendor/tailwind.js"></script><script>tailwind.config={darkMode:'class'};</script></head><body style="margin:0"><div id="root"></div><script type="module" src="/@id/virtual:artifact-browser-harness.jsx"></script></body></html>`;

function near(actual, expected, tolerance = 1.5) {
  return Math.abs(actual - expected) <= tolerance;
}

(async () => {
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-artifact-browser-'));
  const shots = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-artifact-browser-shots-'));
  let vite;
  let browser;
  try {
    const [{ createServer }, { default: react }] = await Promise.all([
      import('vite'),
      import('@vitejs/plugin-react'),
    ]);
    const virtualPlugin = {
      name: 'pinvou-artifact-browser-harness',
      enforce: 'pre',
      resolveId(id) {
        return id === 'virtual:artifact-browser-harness.jsx' ? '\0virtual:artifact-browser-harness.jsx' : null;
      },
      load(id) {
        return id === '\0virtual:artifact-browser-harness.jsx' ? harnessModule : null;
      },
      configureServer(server) {
        server.middlewares.use(async (request, response, next) => {
          if (!String(request.url || '').startsWith('/artifact-browser-harness.html')) return next();
          const html = await server.transformIndexHtml(request.url, harnessHtml);
          response.statusCode = 200;
          response.setHeader('content-type', 'text/html; charset=utf-8');
          response.end(html);
        });
      },
    };
    vite = await createServer({
      root: path.resolve(__dirname, '../src'),
      configFile: false,
      logLevel: 'error',
      plugins: [react(), virtualPlugin],
      server: { host: '127.0.0.1', port: 0, strictPort: false },
    });
    await vite.listen();
    console.log('artifact-browser-ui: vite ready');
    const address = vite.httpServer.address();
    const url = `http://127.0.0.1:${address.port}/artifact-browser-harness.html`;

    if (process.env.PINVOU_ARTIFACT_BROWSER_HARNESS_ONLY === '1') {
      console.log(`ARTIFACT_BROWSER_HARNESS_URL=${url}`);
      await new Promise(resolve => {
        process.once('SIGINT', resolve);
        process.once('SIGTERM', resolve);
      });
      return;
    }

    browser = await puppeteer.launch({
      executablePath: CHROME,
      headless: 'new',
      userDataDir: profile,
      args: ['--no-sandbox', '--no-first-run', '--no-default-browser-check'],
    });
    const page = await browser.newPage();
    page.setDefaultTimeout(8000);
    await page.setViewport({ width: 1440, height: 900, deviceScaleFactor: 1 });
    const errors = [];
    const forbiddenRequests = [];
    page.on('pageerror', error => errors.push(error.stack || error.message));
    page.on('request', request => {
      if (/127\.0\.0\.1:54321/.test(request.url())) forbiddenRequests.push(request.url());
    });
    await page.goto(url, { waitUntil: 'domcontentloaded' });
    console.log('artifact-browser-ui: harness loaded');
    await page.waitForSelector('[data-testid="artifact-deliverable-card"]');
    console.log('artifact-browser-ui: source card ready');

    const beforeShot = path.join(shots, 'artifact-before.png');
    const midShot = path.join(shots, 'artifact-mid.png');
    const finalShot = path.join(shots, 'artifact-final.png');
    await page.screenshot({ path: beforeShot });
    const sourceRect = await page.$eval('[data-testid="artifact-deliverable-card"]', element => {
      const value = element.getBoundingClientRect();
      return { left: value.left, top: value.top, width: value.width, height: value.height };
    });
    await page.click('[data-testid="artifact-deliverable-card"]');
    await page.waitForSelector('.artifact-browser-root.is-open .artifact-browser-window');
    console.log('artifact-browser-ui: motion state', await page.evaluate(() => {
      const node = document.querySelector('.artifact-browser-window');
      return {
        reduced: matchMedia('(prefers-reduced-motion: reduce)').matches,
        className: document.querySelector('[data-testid="artifact-browser-root"]')?.className,
        origin: node?.dataset.artifactOriginTransform,
        computedTransform: node ? getComputedStyle(node).transform : '',
        transitionProperty: node ? getComputedStyle(node).transitionProperty : '',
        transitionDuration: node ? getComputedStyle(node).transitionDuration : '',
        animations: node?.getAnimations().map(animation => ({ type: animation.constructor.name, property: animation.transitionProperty, state: animation.playState, time: animation.currentTime })),
      };
    }));
    await page.waitForFunction(() => {
      const node = document.querySelector('.artifact-browser-window');
      return node && node.getAnimations().some(animation => animation.transitionProperty === 'transform');
    });

    await page.evaluate(() => {
      const clone = document.querySelector('.artifact-browser-launch-clone');
      for (const animation of clone.getAnimations({ subtree: true })) {
        animation.pause();
        animation.currentTime = 0;
      }
    });
    const cloneRect = await page.$eval('.artifact-browser-launch-clone', element => {
      const value = element.getBoundingClientRect();
      return { left: value.left, top: value.top, width: value.width, height: value.height };
    });
    if (!near(cloneRect.left, sourceRect.left) || !near(cloneRect.top, sourceRect.top) || !near(cloneRect.width, sourceRect.width) || !near(cloneRect.height, sourceRect.height)) {
      throw new Error(`source clone mismatch: ${JSON.stringify(cloneRect)}`);
    }

    const samples = [];
    for (const time of [0, 115, 230, 345, 460]) {
      const sample = await page.evaluate(async currentTime => {
        const node = document.querySelector('.artifact-browser-window');
        const root = document.querySelector('[data-testid="artifact-browser-root"]');
        for (const animation of root.getAnimations({ subtree: true })) {
          const timing = animation.effect?.getTiming?.() || {};
          const duration = Number(timing.duration) || currentTime;
          const delay = Math.max(0, Number(timing.delay) || 0);
          animation.pause();
          animation.currentTime = Math.min(currentTime, delay + duration);
        }
        await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
        const value = node.getBoundingClientRect();
        return { left: value.left, top: value.top, width: value.width, height: value.height };
      }, time);
      samples.push(sample);
      if (time === 230) await page.screenshot({ path: midShot });
    }
    await page.screenshot({ path: finalShot });

    const first = samples[0];
    if (![first.left, first.top, first.width, first.height].every((value, index) => near(value, [sourceRect.left, sourceRect.top, sourceRect.width, sourceRect.height][index]))) {
      throw new Error(`launch geometry mismatch: source=${JSON.stringify(sourceRect)} first=${JSON.stringify(first)}`);
    }
    for (let index = 1; index < samples.length; index += 1) {
      if (samples[index].width + 0.5 < samples[index - 1].width || samples[index].height + 0.5 < samples[index - 1].height) {
        throw new Error(`non-monotonic launch geometry: ${JSON.stringify(samples)}`);
      }
    }
    const frameHandle = await page.$('[data-testid="artifact-browser-html-frame"]');
    const frame = await frameHandle.contentFrame();
    const isolated = await frame.evaluate(() => ({
      rawAutoScript: [...document.scripts].some(script => script.textContent.includes('example.com/auto')),
      remoteImage: Boolean(document.querySelector('img[src^="http"]')),
      refresh: Boolean(document.querySelector('meta[http-equiv="refresh" i]')),
      form: Boolean(document.querySelector('form')),
      inlineSvg: Boolean(document.querySelector('#inline-chart path')),
      remoteSvgImage: Boolean(document.querySelector('svg image[href^="http"]')),
      externalTarget: document.querySelector('[data-pinvou-external-url]')?.getAttribute('data-pinvou-external-url') || '',
      nonceScripts: [...document.scripts].every(script => Boolean(script.nonce)),
    }));
    if (isolated.rawAutoScript || isolated.remoteImage || isolated.remoteSvgImage || isolated.refresh || isolated.form || !isolated.inlineSvg || !isolated.nonceScripts) {
      throw new Error(`isolated preview failed: ${JSON.stringify(isolated)}`);
    }
    if (isolated.externalTarget !== 'https://example.com/docs?q=1') throw new Error(`external link was not normalized: ${isolated.externalTarget}`);
    if (forbiddenRequests.length) throw new Error(`artifact leaked network requests: ${JSON.stringify(forbiddenRequests)}`);
    if ((await page.evaluate(() => window.__ARTIFACT_OPENED__.length)) !== 0) throw new Error('artifact opened a link without confirmation');

    await frame.focus('#hash-one');
    await page.keyboard.press('Tab');
    const iframeFocusProgressed = await frame.evaluate(() => document.activeElement?.id === 'hash-two');
    if (!iframeFocusProgressed) throw new Error('iframe focus boundary skipped an internal anchor');
    await frame.focus('#safe-link');
    await page.keyboard.press('Tab');
    await page.waitForFunction(() => document.activeElement?.matches('.artifact-browser-chrome button'));
    await page.focus('[data-testid="artifact-browser-html-frame"]');

    await frame.click('#safe-link');
    await page.waitForSelector('.artifact-browser-link-confirm');
    if ((await page.evaluate(() => window.__ARTIFACT_OPENED__.length)) !== 0) throw new Error('link request bypassed host confirmation');
    await page.click('.artifact-browser-link-confirm .artifact-browser-action:not(.is-primary)');
    await page.waitForSelector('.artifact-browser-link-confirm', { hidden: true });
    await page.waitForFunction(() => document.activeElement?.matches('[data-testid="artifact-browser-html-frame"]'));
    await frame.click('#safe-link');
    await page.waitForSelector('.artifact-browser-link-confirm');
    await page.click('.artifact-browser-link-confirm .is-primary');
    await page.waitForFunction(() => window.__ARTIFACT_OPENED__.length === 1);
    await page.waitForFunction(() => document.activeElement?.matches('[data-testid="artifact-browser-html-frame"]'));

    await page.click('.artifact-browser-action.is-close');
    await page.waitForSelector('[data-testid="artifact-browser-root"]', { hidden: true });

    await page.click('[data-testid="artifact-deliverable-card"]');
    await page.waitForSelector('[data-testid="artifact-browser-root"] .artifact-browser-action.is-close');
    await page.$eval('.artifact-browser-action.is-close', element => element.click());
    await page.waitForSelector('[data-testid="artifact-browser-root"]', { hidden: true });
    await new Promise(resolve => setTimeout(resolve, 650));
    if (await page.$('[data-testid="artifact-browser-root"]')) throw new Error('a fast close was overwritten by the opening animation');

    await page.click('[data-testid="artifact-deliverable-card"]');
    await page.waitForSelector('.artifact-browser-root.is-open .artifact-browser-action.is-close');
    await new Promise(resolve => setTimeout(resolve, 90));
    const closeStart = await page.$eval('.artifact-browser-window', element => {
      const value = element.getBoundingClientRect();
      return { width: value.width, height: value.height };
    });
    const closeBefore = await page.$eval('.artifact-browser-window', element => {
      const value = element.getBoundingClientRect();
      return { left: value.left, top: value.top, width: value.width, height: value.height };
    });
    const closeSync = await page.$eval('.artifact-browser-action.is-close', element => {
      element.click();
      const value = document.querySelector('.artifact-browser-window').getBoundingClientRect();
      return { left: value.left, top: value.top, width: value.width, height: value.height };
    });
    for (const key of ['left', 'top', 'width', 'height']) {
      if (!near(closeSync[key], closeBefore[key])) throw new Error(`mid-flight close jumped at ${key}: ${JSON.stringify({ closeBefore, closeSync })}`);
    }
    await new Promise(resolve => setTimeout(resolve, 150));
    const closeMid = await page.$eval('.artifact-browser-window', element => {
      const value = element.getBoundingClientRect();
      return { width: value.width, height: value.height };
    });
    if (!(closeMid.width < closeStart.width && closeMid.height < closeStart.height)) {
      throw new Error(`mid-flight close did not reverse toward the source card: start=${JSON.stringify(closeStart)} mid=${JSON.stringify(closeMid)}`);
    }
    await page.waitForSelector('[data-testid="artifact-browser-root"]', { hidden: true });

    await page.click('[data-testid="artifact-deliverable-card"]');
    await page.waitForSelector('[data-testid="artifact-browser-html-frame"]');
    if (await page.$('[data-testid="artifact-browser-chrome"] .artifact-browser-action.is-primary')) {
      throw new Error('desktop HTML preview exposed the legacy raw-window escape hatch');
    }
    await page.focus('[data-testid="artifact-browser-html-frame"]');
    await page.keyboard.press('Escape');
    await page.waitForSelector('[data-testid="artifact-browser-root"]', { hidden: true });
    const focusRestored = await page.$eval('[data-testid="artifact-deliverable-card"]', element => document.activeElement === element);
    if (!focusRestored) throw new Error('closing the browser did not restore focus to the source card');

    await page.evaluate(() => window.__SET_ARTIFACT_PATH__('/home/tester/slow.html'));
    await page.waitForFunction(() => document.body.innerText.includes('slow.html'));
    await page.click('[data-testid="artifact-deliverable-card"]');
    await page.waitForSelector('[data-testid="artifact-browser-root"]');
    if (await page.$('[data-testid="artifact-browser-chrome"] .artifact-browser-action.is-primary')) {
      throw new Error('loading HTML briefly exposed the legacy raw-window action');
    }
    await page.$eval('.artifact-browser-action.is-close', element => element.click());
    await page.waitForSelector('[data-testid="artifact-browser-root"]', { hidden: true });

    const chromeMetrics = [];
    for (const artifactPath of ['/home/tester/report.html', '/home/tester/report.md', '/home/tester/report.txt', '/home/tester/report.docx', '/home/tester/report.pdf', '/home/tester/report.png']) {
      await page.evaluate(nextPath => window.__SET_ARTIFACT_PATH__(nextPath), artifactPath);
      await page.waitForFunction(nextPath => document.body.innerText.includes(nextPath.split('/').pop()), {}, artifactPath);
      await page.click('[data-testid="artifact-deliverable-card"]');
      await page.waitForFunction(() => {
        const root = document.querySelector('.artifact-browser-root.is-settled');
        const renderer = document.querySelector('[data-testid="artifact-browser-html-frame"],.artifact-browser-preview-pad,.artifact-browser-pages,.artifact-browser-image-stage');
        return Boolean(root && renderer);
      });
      if (artifactPath.endsWith('.md')) {
        await page.click('[data-testid="artifact-browser-chrome"] .artifact-browser-action.is-primary');
        await page.waitForFunction(() => window.__ARTIFACT_SYSTEM_OPENED__.length === 1);
      }
      const formatMetric = await page.$eval('[data-testid="artifact-browser-chrome"]', element => {
        const style = getComputedStyle(element);
        const rect = element.getBoundingClientRect();
        const viewport = document.querySelector('[data-testid="artifact-browser-viewport"]')?.getBoundingClientRect();
        const renderer = document.querySelector('[data-testid="artifact-browser-html-frame"],.artifact-browser-preview-pad,.artifact-browser-pages,.artifact-browser-image-stage')?.getBoundingClientRect();
        return {
          height: Math.round(rect.height),
          background: style.backgroundColor,
          viewportHeight: Math.round(viewport?.height || 0),
          rendererHeight: Math.round(renderer?.height || 0),
        };
      });
      chromeMetrics.push({ ...formatMetric, path: artifactPath });
      await page.click('.artifact-browser-action.is-close');
      await page.waitForSelector('[data-testid="artifact-browser-root"]', { hidden: true });
    }
    if (new Set(chromeMetrics.map(({ height, background, viewportHeight }) => JSON.stringify({ height, background, viewportHeight }))).size !== 1) {
      throw new Error(`format chrome drifted: ${JSON.stringify(chromeMetrics)}`);
    }
    if (chromeMetrics.some(item => item.rendererHeight < item.viewportHeight || (!item.path.endsWith('.pdf') && item.viewportHeight !== item.rendererHeight))) {
      throw new Error(`format renderer does not fill the viewport: ${JSON.stringify(chromeMetrics)}`);
    }
    const systemOpen = await page.evaluate(() => window.__ARTIFACT_SYSTEM_OPENED__);
    if (JSON.stringify(systemOpen) !== JSON.stringify([{ path: '/home/tester/report.md', sessionId: 'session-artifact-smoke' }])) {
      throw new Error(`system open lost artifact ownership: ${JSON.stringify(systemOpen)}`);
    }

    const textReadsBeforeOversized = await page.evaluate(() => window.__ARTIFACT_TEXT_READS__);
    await page.evaluate(() => window.__SET_ARTIFACT_PATH__('/home/tester/oversized.html'));
    await page.waitForFunction(() => document.body.innerText.includes('oversized.html'));
    await page.click('[data-testid="artifact-deliverable-card"]');
    await page.waitForFunction(() => document.body.innerText.includes('文件过大，未载入内置预览'));
    if (await page.$('[data-testid="artifact-browser-html-frame"]')) {
      throw new Error('oversized text unexpectedly mounted an embedded renderer');
    }
    if ((await page.evaluate(() => window.__ARTIFACT_TEXT_READS__)) !== textReadsBeforeOversized) {
      throw new Error('oversized text crossed the frontend size gate');
    }
    await page.$eval('.artifact-browser-action.is-close', element => element.click());
    await page.waitForSelector('[data-testid="artifact-browser-root"]', { hidden: true });

    const imageReadsBeforeOversized = await page.evaluate(() => window.__ARTIFACT_IMAGE_READS__);
    await page.evaluate(() => window.__SET_ARTIFACT_PATH__('/home/tester/oversized.png'));
    await page.waitForFunction(() => document.body.innerText.includes('oversized.png'));
    await page.click('[data-testid="artifact-deliverable-card"]');
    await page.waitForFunction(() => document.body.innerText.includes('文件过大，未载入内置预览'));
    if ((await page.evaluate(() => window.__ARTIFACT_IMAGE_READS__)) !== imageReadsBeforeOversized) {
      throw new Error('oversized image crossed the frontend size gate');
    }
    await page.$eval('.artifact-browser-action.is-close', element => element.click());
    await page.waitForSelector('[data-testid="artifact-browser-root"]', { hidden: true });

    const visualRendersBeforeOversized = await page.evaluate(() => window.__ARTIFACT_VISUAL_RENDERS__);
    await page.evaluate(() => window.__SET_ARTIFACT_PATH__('/home/tester/oversized.pdf'));
    await page.waitForFunction(() => document.body.innerText.includes('oversized.pdf'));
    await page.click('[data-testid="artifact-deliverable-card"]');
    await page.waitForFunction(() => document.body.innerText.includes('文件过大，未载入内置预览'));
    if ((await page.evaluate(() => window.__ARTIFACT_VISUAL_RENDERS__)) !== visualRendersBeforeOversized) {
      throw new Error('oversized document crossed the frontend size gate');
    }
    await page.$eval('.artifact-browser-action.is-close', element => element.click());
    await page.waitForSelector('[data-testid="artifact-browser-root"]', { hidden: true });

    await page.evaluate(() => window.__SHOW_SCALED_FOCUS_FIXTURE__(true));
    const scaledFrameHandle = await page.waitForSelector('#scaled-focus-fixture [data-testid="artifact-html-preview-frame"]');
    const scaledFrame = await scaledFrameHandle.contentFrame();
    await scaledFrame.waitForSelector('#scaled-only-link');
    await scaledFrame.focus('#scaled-only-link');
    await page.keyboard.press('Tab');
    await page.waitForFunction(() => document.activeElement?.id === 'scaled-after');
    await page.evaluate(() => window.__SHOW_SCALED_FOCUS_FIXTURE__(false));
    await page.waitForSelector('#scaled-focus-fixture', { hidden: true });

    await page.emulateMediaFeatures([{ name: 'prefers-reduced-motion', value: 'reduce' }]);
    await page.evaluate(() => window.__SET_ARTIFACT_PATH__('/home/tester/reduced.html'));
    await page.waitForFunction(() => document.body.innerText.includes('reduced.html'));
    await page.click('[data-testid="artifact-deliverable-card"]');
    await page.waitForSelector('.artifact-browser-root.is-open');
    await page.waitForSelector('[data-testid="artifact-browser-html-frame"]');
    const reduced = await page.evaluate(() => ({
      animations: document.querySelector('[data-testid="artifact-browser-root"]').getAnimations({ subtree: true }).length,
      cloneDisplay: getComputedStyle(document.querySelector('.artifact-browser-launch-clone')).display,
    }));
    const reducedFrame = await (await page.$('[data-testid="artifact-browser-html-frame"]')).contentFrame();
    reduced.frameAnimations = await reducedFrame.evaluate(() => document.getAnimations().length);
    if (reduced.animations !== 0 || reduced.frameAnimations !== 0 || reduced.cloneDisplay !== 'none') throw new Error(`reduced motion failed: ${JSON.stringify(reduced)}`);

    if (errors.length) throw new Error(`page errors: ${errors.join('\n')}`);
    console.log(`ARTIFACT_BROWSER_UI_OK motion_samples=${JSON.stringify(samples)} formats=6 security=isolated+confirmed focus=restored reduced_motion=pass screenshots=${shots}`);
  } finally {
    if (browser) await browser.close();
    if (vite) await vite.close();
    fs.rmSync(profile, { recursive: true, force: true });
  }
})().catch(error => {
  console.error(error.stack || error);
  process.exit(1);
});
