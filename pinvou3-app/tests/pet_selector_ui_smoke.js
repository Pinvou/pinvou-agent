#!/usr/bin/env node
const fs = require('fs');
const os = require('os');
const path = require('path');
const { startUiTestServer } = require('./ui_test_server');

function loadPuppeteer() {
  try { return require('puppeteer-core'); } catch (_) { /* fall through */ }
  const npx = path.join(os.homedir(), '.npm', '_npx');
  if (fs.existsSync(npx)) {
    for (const directory of fs.readdirSync(npx)) {
      const candidate = path.join(npx, directory, 'node_modules', 'puppeteer-core');
      if (fs.existsSync(candidate)) {
        try { return require(candidate); } catch (_) { /* try next */ }
      }
    }
  }
  throw new Error('找不到 puppeteer-core');
}

const puppeteer = loadPuppeteer();
const CHROME = process.env.CHROME || [
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  '/usr/bin/chromium',
  '/usr/bin/google-chrome',
].find((candidate) => fs.existsSync(candidate));

if (!CHROME) throw new Error('未找到 Chrome/Edge，可通过 CHROME 指定');

function tauriMockSource({ failImages = false } = {}) {
  return `(function () {
    var handlers = Object.create(null);
    var selectedPet = 'lingling';
    function emit(name, payload) {
      return Promise.all((handlers[name] || []).slice().map(function (handler) {
        return handler({ payload: payload });
      }));
    }
    function invoke(command, args) {
      switch (command) {
        case 'get_settings': return Promise.resolve({ theme: 'liquid-light', language: 'zh-Hans', pet: { enabled: true } });
        case 'get_selected_pet': return Promise.resolve(selectedPet);
        case 'set_selected_pet':
          if (args && args.id === 'langlang') return Promise.reject(new Error('simulated persistence failure'));
          selectedPet = args.id;
          return emit('pet:selected_changed', { selected_pet: selectedPet });
        case 'get_effective_model_config': return Promise.resolve(null);
        case 'list_sessions': return Promise.resolve([]);
        case 'get_super_permission_status': return Promise.resolve(false);
        case 'list_personas': return Promise.resolve([]);
        case 'get_backend_status': return Promise.resolve({ online: true, vllm_online: true });
        case 'check_for_update': return Promise.resolve({ available: false });
        case 'find_resumable_run': return Promise.resolve(null);
        case 'check_dependencies': return Promise.resolve([]);
        case 'list_marketplace_tools': return Promise.resolve([]);
        case 'list_scheduled_tasks': return Promise.resolve([]);
        case 'get_pet_scale': return Promise.resolve(0.5);
        default: return Promise.resolve(null);
      }
    }
    window.__PET_TEST__ = { emit: emit, getSelectedPet: function () { return selectedPet; } };
    window.__TAURI__ = {
      core: { invoke: invoke },
      event: {
        listen: function (name, handler) {
          (handlers[name] || (handlers[name] = [])).push(handler);
          return Promise.resolve(function () {
            var index = handlers[name].indexOf(handler);
            if (index >= 0) handlers[name].splice(index, 1);
          });
        },
        emit: emit
      },
      window: {
        getCurrentWindow: function () {
          return {
            close: function () {},
            isMaximized: function () { return Promise.resolve(false); },
            maximize: function () {},
            minimize: function () {},
            onMoved: function () { return Promise.resolve(function () {}); },
            onResized: function () { return Promise.resolve(function () {}); },
            innerPosition: function () { return Promise.resolve({ x: 0, y: 0 }); },
            innerSize: function () { return Promise.resolve({ width: 200, height: 200 }); },
            outerPosition: function () { return Promise.resolve({ x: 0, y: 0 }); },
            setPosition: function () { return Promise.resolve(); },
            startDragging: function () {},
            toggleMaximize: function () {}
          };
        }
      },
      dialog: { open: function () { return Promise.resolve(null); } }
    };
    ${failImages ? `window.Image = class BrokenImage {
      set src(value) { this.currentSrc = value; }
      decode() { return Promise.reject(new Error('simulated image decode failure')); }
    };` : ''}
  })();`;
}

function sharedTauriMockSource({ failTargetAtlas = false } = {}) {
  return `(function () {
    var handlers = Object.create(null);
    var failNextDecode = false;
    function dispatch(name, payload) {
      if (${failTargetAtlas} && name === 'pet:selected_changed'
        && payload && payload.selected_pet === 'ace-taffy') {
        failNextDecode = true;
      }
      return Promise.all((handlers[name] || []).slice().map(function (handler) {
        return handler({ payload: payload });
      }));
    }
    window.__PET_SHARED_DISPATCH__ = dispatch;
    window.__TAURI__ = {
      core: {
        invoke: function (command, args) {
          return window.__PET_SHARED_INVOKE__(command, args || {});
        }
      },
      event: {
        listen: function (name, handler) {
          (handlers[name] || (handlers[name] = [])).push(handler);
          return Promise.resolve(function () {
            var index = handlers[name].indexOf(handler);
            if (index >= 0) handlers[name].splice(index, 1);
          });
        },
        emit: dispatch
      },
      window: {
        getCurrentWindow: function () {
          return {
            close: function () {},
            isMaximized: function () { return Promise.resolve(false); },
            maximize: function () {},
            minimize: function () {},
            onMoved: function () { return Promise.resolve(function () {}); },
            onResized: function () { return Promise.resolve(function () {}); },
            innerPosition: function () { return Promise.resolve({ x: 0, y: 0 }); },
            innerSize: function () { return Promise.resolve({ width: 200, height: 200 }); },
            outerPosition: function () { return Promise.resolve({ x: 0, y: 0 }); },
            setPosition: function () { return Promise.resolve(); },
            startDragging: function () {},
            toggleMaximize: function () {}
          };
        }
      },
      dialog: { open: function () { return Promise.resolve(null); } }
    };
    if (${failTargetAtlas}) {
      window.Image = class ControlledImage {
        set src(value) { this.currentSrc = value; }
        decode() {
          if (failNextDecode) {
            failNextDecode = false;
            return Promise.reject(new Error('simulated target atlas decode failure'));
          }
          return Promise.resolve(this.currentSrc);
        }
      };
    }
  })();`;
}

async function waitUntil(predicate, message, timeoutMs = 20000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 40));
  }
  throw new Error(message);
}

async function waitForCards(page) {
  await page.waitForFunction(() => {
    const cards = [...document.querySelectorAll('[data-pet-id]')];
    return cards.length === 3 && cards.every((card) => !card.querySelector('.pet-card-main').disabled);
  }, { timeout: 20000 });
}

async function main() {
  const { url } = await startUiTestServer();
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-pet-selector-'));
  const browser = await puppeteer.launch({
    executablePath: CHROME,
    headless: 'new',
    args: ['--no-sandbox', '--disable-gpu', '--no-first-run'],
    userDataDir: profile,
  });

  try {
    const page = await browser.newPage();
    const pageErrors = [];
    page.on('pageerror', (error) => pageErrors.push(error.message));
    await page.evaluateOnNewDocument(tauriMockSource());
    await page.setViewport({ width: 1280, height: 900, deviceScaleFactor: 1 });
    await page.goto(`${url}/index.html`, { waitUntil: 'networkidle0' });
    await page.waitForSelector('[data-testid="app-root"]', { timeout: 20000 });
    await page.click('button[title="设置"]');
    await waitForCards(page);

    const layout = await page.evaluate(() => {
      const track = document.querySelector('.pet-card-track');
      const trackRect = track.getBoundingClientRect();
      return {
        hasToggle: !!document.querySelector('[data-pet-selector-toggle="true"]'),
        trackClientWidth: track.clientWidth,
        trackScrollWidth: track.scrollWidth,
        trackRight: trackRect.right,
        cards: [...document.querySelectorAll('[data-pet-id]')].map((card) => {
          const rect = card.getBoundingClientRect();
          return { id: card.dataset.petId, left: rect.left, right: rect.right, top: rect.top, width: rect.width };
        }),
      };
    });
    if (layout.hasToggle) throw new Error('桌宠选择器不应再展示“更换”展开按钮');
    if (layout.trackScrollWidth > layout.trackClientWidth + 1) {
      throw new Error(`桌宠选择器不应出现横向滚动条: ${JSON.stringify(layout)}`);
    }
    if (layout.cards.some((card) => card.right > layout.trackRight + 1 || card.width > 165)) {
      throw new Error(`桌宠卡片未完整收紧展示: ${JSON.stringify(layout)}`);
    }
    const cards = layout.cards;
    if (cards.map(({ id }) => id).join(',') !== 'lingling,langlang,ace-taffy') {
      throw new Error(`宠物顺序错误: ${JSON.stringify(layout)}`);
    }
    if (!cards.every((card) => Math.abs(card.top - cards[0].top) < 1)
      || !(cards[0].left < cards[1].left && cards[1].left < cards[2].left)) {
      throw new Error(`三卡不是横向布局: ${JSON.stringify(layout)}`);
    }

    await page.hover('[data-pet-id="ace-taffy"] .pet-card-main');
    await page.waitForSelector('[data-pet-id="ace-taffy"] .pet-card-sprite');
    const hoverGeometry = await page.evaluate(() => {
      const track = document.querySelector('.pet-card-track');
      const card = document.querySelector('[data-pet-id="ace-taffy"]');
      const trackRect = track.getBoundingClientRect();
      const cardRect = card.getBoundingClientRect();
      return {
        cardTop: cardRect.top,
        paddingTop: getComputedStyle(track).paddingTop,
        trackTop: trackRect.top,
      };
    });
    if (hoverGeometry.paddingTop !== '4px' || hoverGeometry.cardTop < hoverGeometry.trackTop + 1) {
      throw new Error(`悬浮卡片顶部仍被滚动容器裁切: ${JSON.stringify(hoverGeometry)}`);
    }
    const contentGeometry = await page.$eval('[data-pet-id="ace-taffy"] .pet-card-figure', (figure) => {
      const figureStyle = getComputedStyle(figure);
      const name = figure.parentElement.querySelector('.pet-card-name');
      const description = figure.parentElement.querySelector('.pet-card-desc');
      const card = figure.closest('.pet-card');
      const figureRect = figure.getBoundingClientRect();
      const nameRect = name.getBoundingClientRect();
      const descriptionRect = description.getBoundingClientRect();
      const cardRect = card.getBoundingClientRect();
      return {
        figureBackground: figureStyle.backgroundImage,
        figureHeight: Number.parseFloat(figureStyle.height),
        figureBottom: figureRect.bottom,
        nameTop: nameRect.top,
        nameWeight: Number.parseInt(getComputedStyle(name).fontWeight, 10),
        descriptionBottom: descriptionRect.bottom,
        cardBottom: cardRect.bottom,
        cardHeight: cardRect.height,
      };
    });
    const bottomWhitespace = contentGeometry.cardBottom - contentGeometry.descriptionBottom;
    if (contentGeometry.figureBackground !== 'none'
      || contentGeometry.figureHeight !== 62
      || contentGeometry.cardHeight < 132
      || contentGeometry.cardHeight > 150
      || contentGeometry.nameTop < contentGeometry.figureBottom + 4
      || contentGeometry.nameWeight < 600
      || bottomWhitespace < 8
      || bottomWhitespace > 28) {
      throw new Error(`角色卡内容布局错误: ${JSON.stringify(contentGeometry)}`);
    }
    await page.waitForSelector('[data-pet-id="ace-taffy"] .pet-card-sprite', { timeout: 1000 });
    const previewBefore = await page.$eval('[data-pet-id="ace-taffy"] .pet-card-sprite', (sprite) => ({
      position: getComputedStyle(sprite).backgroundPosition,
      size: getComputedStyle(sprite).backgroundSize,
      layoutHeight: Number.parseFloat(getComputedStyle(sprite).height),
      visualWidth: sprite.getBoundingClientRect().width,
      visualHeight: sprite.getBoundingClientRect().height,
      transform: getComputedStyle(sprite).transform,
      figureWidth: sprite.closest('.pet-card-figure').getBoundingClientRect().width,
      figureHeight: sprite.closest('.pet-card-figure').getBoundingClientRect().height,
    }));
    await new Promise((resolve) => setTimeout(resolve, 350));
    const previewAfter = await page.$eval('[data-pet-id="ace-taffy"] .pet-card-sprite', (sprite) => (
      getComputedStyle(sprite).backgroundPosition
    ));
    // Chromium serializes the implicit vertical `auto` as either `<width>` or
    // `<width> auto`, depending on the engine build.
    if (!previewBefore.size.startsWith('460.8px') || Math.abs(previewBefore.layoutHeight - 62.4) > 0.5) {
      throw new Error(`Ace Taffy 预览尺寸错误: ${JSON.stringify(previewBefore)}`);
    }
    if (previewBefore.visualWidth < 60 || previewBefore.visualHeight < 68
      || previewBefore.visualWidth > previewBefore.figureWidth + 8
      || previewBefore.visualHeight > previewBefore.figureHeight + 10
      || previewBefore.transform === 'none') {
      throw new Error(`悬浮角色尺寸未匹配紧凑卡片: ${JSON.stringify(previewBefore)}`);
    }
    if (previewBefore.position === previewAfter) {
      throw new Error(`Ace Taffy 悬浮动画未推进: ${previewBefore.position}`);
    }
    await page.waitForFunction(() => {
      const sprite = document.querySelector('[data-pet-id="ace-taffy"] .pet-card-sprite');
      if (!sprite) return false;
      window.__PET_PREVIEW_POSITIONS__ = window.__PET_PREVIEW_POSITIONS__ || [];
      const position = getComputedStyle(sprite).backgroundPosition;
      if (!window.__PET_PREVIEW_POSITIONS__.includes(position)) window.__PET_PREVIEW_POSITIONS__.push(position);
      return window.__PET_PREVIEW_POSITIONS__.length >= 3;
    }, { timeout: 3000 });

    const names = await page.$$eval('[data-pet-id] .pet-card-name', (elements) => (
      elements.map((element) => element.textContent.trim())
    ));
    if (names.join(',') !== '灵灵,浪浪,Ace Taffy') {
      throw new Error(`宠物显示名错误: ${JSON.stringify(names)}`);
    }

    await page.click('[data-pet-id="ace-taffy"] .pet-card-main');
    await page.waitForFunction(() => document.querySelector('[data-pet-id="ace-taffy"]')
      .classList.contains('pet-card--selected'));
    const selectedAfterSuccess = await page.evaluate(() => window.__PET_TEST__.getSelectedPet());
    if (selectedAfterSuccess !== 'ace-taffy') throw new Error('点击未切换到 Ace Taffy');

    await page.click('[data-pet-id="langlang"] .pet-card-main');
    await new Promise((resolve) => setTimeout(resolve, 150));
    const rollback = await page.evaluate(() => ({
      selected: window.__PET_TEST__.getSelectedPet(),
      aceSelected: document.querySelector('[data-pet-id="ace-taffy"]').classList.contains('pet-card--selected'),
      langlangSelected: document.querySelector('[data-pet-id="langlang"]').classList.contains('pet-card--selected'),
    }));
    if (rollback.selected !== 'ace-taffy' || !rollback.aceSelected || rollback.langlangSelected) {
      throw new Error(`失败回滚错误: ${JSON.stringify(rollback)}`);
    }
    if (pageErrors.length) throw new Error(`设置页 pageerror: ${pageErrors.join(' | ')}`);

    const fallbackPage = await browser.newPage();
    await fallbackPage.evaluateOnNewDocument(tauriMockSource({ failImages: true }));
    await fallbackPage.setViewport({ width: 320, height: 320, deviceScaleFactor: 1 });
    await fallbackPage.goto(`${url}/pet.html`, { waitUntil: 'networkidle0' });
    await fallbackPage.waitForSelector('[data-pet-activation-failed="true"]', { visible: true, timeout: 20000 });
    const fallback = await fallbackPage.$eval('[data-pet-activation-failed="true"]', (element) => {
      const rect = element.getBoundingClientRect();
      return { text: element.textContent, width: rect.width, height: rect.height };
    });
    if (!fallback.text.includes('公仔加载失败') || fallback.width < 100 || fallback.height < 80) {
      throw new Error(`加载失败兜底不可见: ${JSON.stringify(fallback)}`);
    }
    await fallbackPage.click('[data-pet-activation-failed="true"]');

    // End-to-end rollback across the two real React windows:
    // settings persists/broadcasts Ace Taffy, the pet window fails its atlas
    // decode, then requests the old Lingling ID and settings follows that event.
    let sharedSelectedPet = 'lingling';
    const sharedHistory = [];
    const sharedPages = [];
    const sharedDispatchErrors = [];
    let broadcastChain = Promise.resolve();
    const dispatchToSharedPages = async (payload) => {
      await Promise.all(sharedPages.map((sharedPage) => sharedPage.evaluate((eventPayload) => (
        window.__PET_SHARED_DISPATCH__('pet:selected_changed', eventPayload)
      ), payload)));
    };
    const sharedInvoke = async (command, args) => {
      switch (command) {
        case 'get_settings':
          return { theme: 'liquid-light', language: 'zh-Hans', pet: { enabled: true } };
        case 'get_selected_pet':
          return sharedSelectedPet;
        case 'set_selected_pet': {
          sharedSelectedPet = args.id;
          sharedHistory.push(args.id);
          const payload = { selected_pet: args.id };
          broadcastChain = broadcastChain
            .then(() => dispatchToSharedPages(payload))
            .catch((error) => sharedDispatchErrors.push(error.message));
          return null;
        }
        case 'get_effective_model_config': return null;
        case 'list_sessions': return [];
        case 'get_super_permission_status': return false;
        case 'list_personas': return [];
        case 'get_backend_status': return { online: true, vllm_online: true };
        case 'check_for_update': return { available: false };
        case 'find_resumable_run': return null;
        case 'check_dependencies': return [];
        case 'list_marketplace_tools': return [];
        case 'list_scheduled_tasks': return [];
        case 'get_pet_scale': return 0.5;
        default: return null;
      }
    };

    const petPage = await browser.newPage();
    const settingsPage = await browser.newPage();
    sharedPages.push(petPage, settingsPage);
    const sharedPageErrors = [];
    petPage.on('pageerror', (error) => sharedPageErrors.push(`pet: ${error.message}`));
    settingsPage.on('pageerror', (error) => sharedPageErrors.push(`settings: ${error.message}`));
    await petPage.exposeFunction('__PET_SHARED_INVOKE__', sharedInvoke);
    await settingsPage.exposeFunction('__PET_SHARED_INVOKE__', sharedInvoke);
    await petPage.evaluateOnNewDocument(sharedTauriMockSource({ failTargetAtlas: true }));
    await settingsPage.evaluateOnNewDocument(sharedTauriMockSource());
    await petPage.setViewport({ width: 640, height: 640, deviceScaleFactor: 1 });
    await settingsPage.setViewport({ width: 1280, height: 900, deviceScaleFactor: 1 });
    await petPage.goto(`${url}/pet.html`, { waitUntil: 'networkidle0' });
    await petPage.waitForSelector('.pet-sprite', { timeout: 20000 });
    const initialPetImage = await petPage.$eval('.pet-sprite', (element) => (
      getComputedStyle(element).backgroundImage
    ));

    await settingsPage.goto(`${url}/index.html`, { waitUntil: 'networkidle0' });
    await settingsPage.waitForSelector('[data-testid="app-root"]', { timeout: 20000 });
    await settingsPage.click('button[title="设置"]');
    await waitForCards(settingsPage);
    await settingsPage.click('[data-pet-id="ace-taffy"] .pet-card-main');

    await waitUntil(
      () => sharedHistory.length >= 2,
      `跨窗口回滚未完成，命令历史: ${JSON.stringify(sharedHistory)}`,
    );
    await broadcastChain;
    if (sharedHistory.join(',') !== 'ace-taffy,lingling' || sharedSelectedPet !== 'lingling') {
      throw new Error(`跨窗口回滚命令错误: ${JSON.stringify({ sharedHistory, sharedSelectedPet })}`);
    }
    await settingsPage.waitForFunction(() => (
      document.querySelector('[data-pet-id="lingling"]').classList.contains('pet-card--selected')
      && !document.querySelector('[data-pet-id="ace-taffy"]').classList.contains('pet-card--selected')
    ), { timeout: 10000 });
    const finalPetImage = await petPage.$eval('.pet-sprite', (element) => (
      getComputedStyle(element).backgroundImage
    ));
    if (finalPetImage !== initialPetImage) {
      throw new Error(`桌宠失败后未保留旧图集: ${JSON.stringify({ initialPetImage, finalPetImage })}`);
    }
    if (sharedDispatchErrors.length || sharedPageErrors.length) {
      throw new Error(`跨窗口测试异常: ${[...sharedDispatchErrors, ...sharedPageErrors].join(' | ')}`);
    }

    console.log('pet selector Chromium tests passed');
  } finally {
    await browser.close();
    fs.rmSync(profile, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error('FAIL:', error && error.stack ? error.stack : error);
  process.exit(1);
});
