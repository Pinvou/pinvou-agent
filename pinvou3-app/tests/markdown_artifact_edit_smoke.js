#!/usr/bin/env node
/**
 * Markdown artifact direct-edit smoke test.
 *
 * Runs against the built UI with a mock Tauri backend. It verifies the core
 * automated path: open a Markdown artifact, edit rendered content, auto-save
 * Markdown, select text, and prefill the composer for AI edit without sending.
 */
const fs = require('fs');
const os = require('os');
const path = require('path');
const { startUiTestServer } = require('./ui_test_server');

function loadPuppeteer() {
  try { return require('puppeteer-core'); } catch (_) { /* fall through */ }
  const npx = path.join(os.homedir(), '.npm', '_npx');
  if (fs.existsSync(npx)) {
    for (const d of fs.readdirSync(npx)) {
      const p = path.join(npx, d, 'node_modules', 'puppeteer-core');
      if (fs.existsSync(p)) {
        try { return require(p); } catch (_) { /* next */ }
      }
    }
  }
  console.error('SKIP: puppeteer-core not found');
  process.exit(2);
}

function findChrome() {
  const candidates = [
    process.env.CHROME,
    '/snap/bin/chromium',
    '/usr/bin/chromium',
    '/usr/bin/chromium-browser',
    '/usr/bin/google-chrome',
    '/usr/bin/google-chrome-stable',
    path.join(process.env.ProgramFiles || '', 'Google/Chrome/Application/chrome.exe'),
    path.join(process.env['ProgramFiles(x86)'] || '', 'Google/Chrome/Application/chrome.exe'),
    path.join(process.env.LOCALAPPDATA || '', 'Google/Chrome/Application/chrome.exe'),
    path.join(process.env.ProgramFiles || '', 'Microsoft/Edge/Application/msedge.exe'),
    path.join(process.env['ProgramFiles(x86)'] || '', 'Microsoft/Edge/Application/msedge.exe'),
  ].filter(Boolean);
  return candidates.find((p) => fs.existsSync(p));
}

const puppeteer = loadPuppeteer();
const CHROME = findChrome();
if (!CHROME) {
  console.error('SKIP: chromium/chrome/edge not found; set CHROME=/path/to/browser');
  process.exit(2);
}

const ARTIFACT_PATH = '/tmp/pinvou3/sessions/s-md/artifacts/meeting.md';
const LONG_USER_PROMPT = [
  '要求修改下面这段 Markdown 产物内容。',
  '',
  '路径：',
  'C:\\Users\\123\\.pinvou3\\sessions\\woxhexe j3yjd0\\workspace\\hello-world.md',
  '',
  '选中文本：',
  '```markdown',
  '1994 年 8 月 5 日',
  '```',
  '',
  '修改要求：',
  '把日期改得更自然一点，同时保留原文里的中文说明。',
].join('\n');
const INITIAL_MD = [
  '# 会议纪要',
  '',
  '这是一段正文，需要被直接编辑。',
  '',
  'literal ``` fence',
  '',
  '| 名称 | 状态 | 备注 |',
  '| --- | --- | --- |',
  '| Alpha | 进行中 | 保留 |',
  '| Beta | 待处理 | 可编辑 |',
].join('\n');

function injectSource() {
  return `(function(){
    try { localStorage.setItem('pinvou_artifactW', '650'); } catch (_) {}
    window.__TAURI_EVENT_HANDLERS__ = {};
    window.__MD_WRITES__ = [];
    window.__MD_SENT__ = [];
    window.__MD_READ_TEXT__ = ${JSON.stringify(INITIAL_MD)};
    window.__MD_EXISTS__ = true;
    window.__MD_MTIME__ = 1;
    var SESSIONS = [{id:'s-md',title:'Markdown编辑测试',created_at:1,updated_at:9}];
    var CONV = { 's-md': {
      metadata:{id:'s-md',title:'Markdown编辑测试'},
      artifacts:[{path:${JSON.stringify(ARTIFACT_PATH)},basename:'meeting.md'}],
      messages:[{role:'user',content:[{type:'text',text:${JSON.stringify(LONG_USER_PROMPT)}}]}]
    }};
    function invoke(cmd,args){
      switch(cmd){
        case 'get_settings': return Promise.resolve({theme:'liquid-light',language:'zh-Hans'});
        case 'get_effective_model_config': return Promise.resolve({model:'test',base_url:'http://127.0.0.1',api_key_set:false});
        case 'list_sessions': return Promise.resolve(SESSIONS);
        case 'load_session': return Promise.resolve(CONV[args && args.id] || CONV['s-md']);
        case 'get_super_permission_status': return Promise.resolve(false);
        case 'list_personas': return Promise.resolve([]);
        case 'get_backend_status': return Promise.resolve({online:true,ok:true,status:'online'});
        case 'check_for_update': return Promise.resolve({available:false});
        case 'find_resumable_run': return Promise.resolve(null);
        case 'check_dependencies': return Promise.resolve([]);
        case 'list_marketplace_tools': return Promise.resolve([]);
        case 'get_mode_state': return Promise.resolve({mode:'yolo',plan_phase:'none'});
        case 'get_active_persona': return Promise.resolve(null);
        case 'list_workflows': return Promise.resolve([]);
        case 'list_workspace_files': return Promise.resolve([]);
        case 'get_session_persona_events': return Promise.resolve([]);
        case 'get_session_pinvou_reviews': return Promise.resolve([]);
        case 'artifact_info': return Promise.resolve({exists:window.__MD_EXISTS__ !== false,kind:'md',size:String(window.__MD_READ_TEXT__ || '').length,modified:window.__MD_MTIME__ || 1});
        case 'read_artifact_text': return Promise.resolve(window.__MD_READ_TEXT__);
        case 'write_artifact_text':
          if (window.__MD_EXISTS__ === false) return Promise.reject(new Error('not a file'));
          window.__MD_WRITES__.push({path:args.path,content:args.content,at:Date.now()});
          return Promise.resolve();
        case 'save_session_artifacts': return Promise.resolve();
        case 'render_artifact_visual': return Promise.resolve({mode:'unsupported'});
        case 'send_message':
        case 'send_chat':
          window.__MD_SENT__.push({cmd:cmd,args:args});
          return Promise.resolve();
        default: return Promise.resolve(null);
      }
    }
    window.__TAURI__ = {core:{invoke:invoke},event:{listen:function(name,handler){
      var handlers = window.__TAURI_EVENT_HANDLERS__[name] || (window.__TAURI_EVENT_HANDLERS__[name] = []);
      handlers.push(handler);
      return Promise.resolve(function(){ var i = handlers.indexOf(handler); if (i >= 0) handlers.splice(i,1); });
    }},window:{getCurrentWindow:function(){return {minimize(){},maximize(){},close(){},toggleMaximize(){},isMaximized(){return Promise.resolve(false);},onResized(){return Promise.resolve(function(){});},startDragging(){}};}},dialog:{open:function(){return Promise.resolve(null);}}};
  })();`;
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function clickText(page, text) {
  return page.evaluate((needle) => {
    let els = [...document.querySelectorAll('button,div,span,a')]
      .filter((el) => (el.textContent || '').trim() === needle);
    if (!els.length) {
      els = [...document.querySelectorAll('button,div,span,a')]
        .filter((el) => (el.textContent || '').trim().includes(needle));
    }
    const el = els[els.length - 1];
    if (!el) return false;
    el.scrollIntoView({ block: 'center', inline: 'center' });
    el.click();
    return true;
  }, text);
}

async function expandSidebar(page) {
  return page.evaluate(() => {
    const b = document.querySelector('[title*="侧边栏"],[title*="展开"]');
    if (!b) return false;
    b.click();
    return true;
  });
}

async function mouseDownText(page, text) {
  return page.evaluate((needle) => {
    const els = [...document.querySelectorAll('button,div,span,a')]
      .filter((el) => (el.textContent || '').trim() === needle);
    const el = els[els.length - 1];
    if (!el) return false;
    el.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true, view: window }));
    return true;
  }, text);
}

async function rec(results, name, pass, detail) {
  results.push({ name, pass, detail });
  console.log(`${pass ? 'PASS' : 'FAIL'} ${name}${detail ? ' ' + detail : ''}`);
}

(async () => {
  const { url } = await startUiTestServer();
  const results = [];
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-md-edit-'));
  const browser = await puppeteer.launch({
    executablePath: CHROME,
    headless: 'new',
    args: ['--no-sandbox', '--disable-gpu', '--no-first-run', '--no-default-browser-check'],
    userDataDir: profile,
  });
  const page = await browser.newPage();
  const pageErrors = [];
  page.on('pageerror', (e) => pageErrors.push(e.message));
  page.on('dialog', async (dialog) => {
    if (dialog.type() === 'confirm') await dialog.accept();
    else await dialog.dismiss();
  });
  await page.evaluateOnNewDocument(injectSource());
  await page.setViewport({ width: 1000, height: 1000, deviceScaleFactor: 1 });
  await page.goto(url, { waitUntil: 'networkidle0' });
  await page.waitForFunction(() => window.TauriBridge && document.body, { timeout: 20000 });
  await sleep(1500);

  await expandSidebar(page);
  await sleep(300);
  await clickText(page, 'Markdown编辑测试');
  await sleep(1500);
  await clickText(page, '产物与代码');
  await sleep(500);
  await clickText(page, 'meeting.md');
  await sleep(900);

  const userBubbleLayout = await page.evaluate(() => {
    const candidates = [...document.querySelectorAll('div')]
      .filter((el) => {
        const text = el.textContent || '';
        return text.includes('C:\\Users\\123\\.pinvou3') && text.includes('修改要求');
      })
      .map((el) => {
        const rect = el.getBoundingClientRect();
        return { el, rect, area: rect.width * rect.height };
      })
      .filter((item) => item.rect.width > 0 && item.rect.height > 0)
      .sort((a, b) => a.area - b.area);
    const bubble =
      (candidates.find((item) => String(item.el.className || '').includes('bg-[#D3E3FD]')) || {}).el ||
      (candidates.find((item) => String(item.el.className || '').includes('bg-[#004A77]')) || {}).el ||
      (candidates[0] && candidates[0].el);
    const scroll = bubble && bubble.closest('.custom-scrollbar') || document.querySelector('.custom-scrollbar');
    if (!bubble || !scroll) return { found: !!bubble, hasScroll: !!scroll };
    const bubbleRect = bubble.getBoundingClientRect();
    const scrollRect = scroll.getBoundingClientRect();
    return {
      found: true,
      hasScroll: true,
      bubbleLeft: bubbleRect.left,
      bubbleRight: bubbleRect.right,
      scrollLeft: scrollRect.left,
      scrollRight: scrollRect.right,
      bubbleWidth: bubbleRect.width,
      scrollWidth: scrollRect.width,
    };
  });
  await rec(
    results,
    'user bubble remains inside narrowed chat panel',
    userBubbleLayout.found &&
      userBubbleLayout.hasScroll &&
      userBubbleLayout.bubbleLeft >= userBubbleLayout.scrollLeft - 1 &&
      userBubbleLayout.bubbleRight <= userBubbleLayout.scrollRight + 1 &&
      userBubbleLayout.bubbleWidth <= userBubbleLayout.scrollWidth,
    JSON.stringify(userBubbleLayout),
  );

  const opened = await page.evaluate(() => {
    const editable = document.querySelector('[contenteditable="true"]');
    return {
      hasEditable: !!editable,
      hasPreviewSwitch: document.body.innerText.includes('编辑模式') || document.body.innerText.includes('源码编辑'),
      text: editable ? editable.innerText : '',
    };
  });
  await rec(results, 'opens markdown artifact in direct editable preview', opened.hasEditable && !opened.hasPreviewSwitch && opened.text.includes('会议纪要'), JSON.stringify(opened));
  if (!opened.hasEditable) {
    await browser.close();
    process.exit(1);
  }

  await page.evaluate(() => {
    const editable = document.querySelector('[contenteditable="true"]');
    const p = [...editable.querySelectorAll('p')].find((el) => el.textContent.includes('这是一段正文'));
    p.textContent = '这是一段正文，已经被自动化修改。';
    editable.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: '自动化修改' }));
  });
  await sleep(1300);
  const saved = await page.evaluate(() => {
    const writes = window.__MD_WRITES__ || [];
    const last = writes[writes.length - 1] || {};
    return {
      count: writes.length,
      path: last.path,
      content: last.content || '',
      bodyText: document.body.innerText,
    };
  });
  await rec(
    results,
    'auto-saves edited content as markdown',
    saved.count >= 1 &&
      saved.path === ARTIFACT_PATH &&
      saved.content.includes('已经被自动化修改') &&
      saved.content.includes('| Alpha |') &&
      !saved.content.includes('<h1'),
    JSON.stringify({ count: saved.count, path: saved.path, content: saved.content.slice(0, 160) }),
  );

  await page.evaluate(() => {
    const editable = document.querySelector('[contenteditable="true"]');
    const p = [...editable.querySelectorAll('p')].find((el) => el.textContent.includes('已经被自动化修改'));
    const range = document.createRange();
    range.selectNodeContents(p);
    range.collapse(false);
    const sel = window.getSelection();
    sel.removeAllRanges();
    sel.addRange(range);
    const event = new Event('paste', { bubbles: true, cancelable: true });
    Object.defineProperty(event, 'clipboardData', {
      value: {
        getData(type) {
          if (type === 'text/plain') return ' PASTED plain text';
          if (type === 'text/html') return '<img src=x onerror=alert(1)><script>alert(1)</script>';
          return '';
        },
      },
    });
    editable.dispatchEvent(event);
  });
  await sleep(1300);
  const pasted = await page.evaluate(() => {
    const writes = window.__MD_WRITES__ || [];
    const last = writes[writes.length - 1] || {};
    return { count: writes.length, content: last.content || '' };
  });
  await rec(
    results,
    'paste sanitizes rich html to plain markdown text',
    pasted.count >= 2 &&
      pasted.content.includes('PASTED') &&
      pasted.content.includes('plain text') &&
      !pasted.content.includes('<script>') &&
      !pasted.content.includes('onerror'),
    JSON.stringify({ count: pasted.count, content: pasted.content.slice(0, 200) }),
  );

  await page.evaluate(() => {
    const editable = document.querySelector('[contenteditable="true"]');
    const walker = document.createTreeWalker(editable, NodeFilter.SHOW_TEXT);
    var node = null;
    while ((node = walker.nextNode())) {
      if (node.nodeValue.includes('literal ``` fence')) {
        const start = node.nodeValue.indexOf('literal');
        const range = document.createRange();
        range.setStart(node, start);
        range.setEnd(node, start + 'literal ``` fence'.length);
        const sel = window.getSelection();
        sel.removeAllRanges();
        sel.addRange(range);
        document.dispatchEvent(new Event('selectionchange'));
        break;
      }
    }
  });
  await sleep(500);
  const aiButtonVisible = await page.evaluate(() => document.body.innerText.includes('AI 编辑'));
  await rec(results, 'shows AI edit button for non-empty selection', aiButtonVisible);

  await mouseDownText(page, 'AI 编辑');
  await sleep(300);
  await page.mouse.click(20, 20);
  await sleep(300);
  const dismissed = await page.evaluate(() => ({
    hasInput: !!document.querySelector('#md-selection-ai-input'),
    hasSelection: String(window.getSelection && window.getSelection() || '').length > 0,
  }));
  await rec(
    results,
    'clicking outside dismisses AI input and clears selection',
    !dismissed.hasInput && !dismissed.hasSelection,
    JSON.stringify(dismissed),
  );

  await page.evaluate(() => {
    const editable = document.querySelector('[contenteditable="true"]');
    const walker = document.createTreeWalker(editable, NodeFilter.SHOW_TEXT);
    var node = null;
    while ((node = walker.nextNode())) {
      if (node.nodeValue.includes('literal ``` fence')) {
        const start = node.nodeValue.indexOf('literal');
        const range = document.createRange();
        range.setStart(node, start);
        range.setEnd(node, start + 'literal ``` fence'.length);
        const sel = window.getSelection();
        sel.removeAllRanges();
        sel.addRange(range);
        document.dispatchEvent(new Event('selectionchange'));
        break;
      }
    }
  });
  await sleep(300);
  await mouseDownText(page, 'AI 编辑');
  await sleep(300);
  await page.type('#md-selection-ai-input', '把它改成中文项目名');
  await page.keyboard.press('Enter');
  await sleep(600);
  const prefill = await page.evaluate(() => {
    const st = window.TauriBridge.state.get('chat');
    return {
      prefill: st.composerPrefill && st.composerPrefill.text,
      sent: window.__MD_SENT__.length,
      writes: window.__MD_WRITES__.length,
    };
  });
  await rec(
    results,
    'AI edit confirmation prefills composer without sending',
    !!prefill.prefill &&
      prefill.prefill.includes(ARTIFACT_PATH) &&
      prefill.prefill.includes('literal ``` fence') &&
      prefill.prefill.includes('````markdown') &&
      prefill.prefill.includes('把它改成中文项目名') &&
      prefill.sent === 0 &&
      prefill.writes >= 1,
    JSON.stringify({ sent: prefill.sent, writes: prefill.writes, prefill: (prefill.prefill || '').slice(0, 180) }),
  );

  await page.evaluate((path) => {
    window.__MD_MTIME__ += 1;
    window.__MD_READ_TEXT__ = [
      '# 会议纪要',
      '',
      '外部更新后的内容已经写入磁盘。',
      '',
      '| 名称 | 状态 | 备注 |',
      '| --- | --- | --- |',
      '| Gamma | 已完成 | 自动刷新 |',
    ].join('\n');
    (window.__TAURI_EVENT_HANDLERS__['artifact:disk'] || []).forEach((handler) => {
      handler({ payload: { path, event: 'modified', session_id: 's-md' } });
    });
  }, ARTIFACT_PATH);
  await sleep(900);
  const reloaded = await page.evaluate(() => {
    const editable = document.querySelector('[contenteditable="true"]');
    return {
      text: editable ? editable.innerText : '',
      stateText: document.body.innerText,
      mtime: window.__MD_MTIME__,
    };
  });
  await rec(
    results,
    'reloads open markdown preview when current artifact changes on disk',
    reloaded.text.includes('外部更新后的内容已经写入磁盘') &&
      reloaded.text.includes('Gamma') &&
      !reloaded.text.includes('已经被自动化修改。 PASTED'),
    JSON.stringify({ mtime: reloaded.mtime, text: reloaded.text.slice(0, 180) }),
  );

  await page.evaluate(() => {
    const editable = document.querySelector('[contenteditable="true"]');
    const p = [...editable.querySelectorAll('p')].find((el) => el.textContent.includes('外部更新后的内容'));
    p.textContent = '本地未保存内容不能被外部刷新覆盖。';
    editable.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: '本地未保存内容' }));
  });
  await sleep(150);
  await page.evaluate((path) => {
    window.__MD_MTIME__ += 1;
    window.__MD_READ_TEXT__ = '# 会议纪要\n\n外部冲突内容不应该覆盖本地草稿。';
    (window.__TAURI_EVENT_HANDLERS__['artifact:disk'] || []).forEach((handler) => {
      handler({ payload: { path, event: 'modified', session_id: 's-md' } });
    });
  }, ARTIFACT_PATH);
  await sleep(500);
  const dirtyProtected = await page.evaluate(() => {
    const editable = document.querySelector('[contenteditable="true"]');
    return {
      text: editable ? editable.innerText : '',
      bodyText: document.body.innerText,
      writes: window.__MD_WRITES__.length,
    };
  });
  await rec(
    results,
    'does not overwrite dirty markdown draft on external artifact change',
    dirtyProtected.text.includes('本地未保存内容不能被外部刷新覆盖') &&
      !dirtyProtected.text.includes('外部冲突内容不应该覆盖本地草稿') &&
      dirtyProtected.bodyText.includes('文件已在外部更新'),
    JSON.stringify({ writes: dirtyProtected.writes, text: dirtyProtected.text.slice(0, 160) }),
  );

  await page.evaluate((path) => {
    window.__MD_EXISTS__ = false;
    window.__MD_MTIME__ += 1;
    (window.__TAURI_EVENT_HANDLERS__['artifact:disk'] || []).forEach((handler) => {
      handler({ payload: { path, event: 'removed', session_id: 's-md' } });
    });
  }, ARTIFACT_PATH);
  // 超过 1 秒自动保存窗口，模拟后端因文件已删除而拒绝写回；草稿仍必须留在编辑器中。
  await sleep(700);
  const removed = await page.evaluate(() => ({
    hasEditable: !!document.querySelector('[contenteditable="true"]'),
    text: document.querySelector('[contenteditable="true"]')?.innerText || '',
    bodyText: document.body.innerText,
  }));
  await rec(
    results,
    'keeps dirty markdown draft when current artifact is removed',
    removed.hasEditable &&
      removed.text.includes('本地未保存内容不能被外部刷新覆盖') &&
      removed.bodyText.includes('文件已在外部删除'),
    JSON.stringify({ hasEditable: removed.hasEditable, text: removed.text.slice(0, 160) }),
  );

  if (pageErrors.length) {
    await rec(results, 'no browser page errors', false, pageErrors.slice(0, 3).join(' | '));
  }

  await browser.close();
  const failed = results.filter((r) => !r.pass).length;
  console.log(failed ? `\\n${failed}/${results.length} FAILED` : `\\nALL ${results.length} PASS`);
  process.exit(failed ? 1 : 0);
})().catch((e) => {
  console.error('FATAL', e && e.stack ? e.stack : e);
  process.exit(1);
});
