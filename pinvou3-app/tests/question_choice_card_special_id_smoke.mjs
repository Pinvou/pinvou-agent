#!/usr/bin/env node
// QuestionChoiceCard 特殊 question id 的 DOM 冒烟（评审第三轮 P1）：
//   constructor/toString/__proto__ 是后端仅校验非空即可通过的合法输入；卡片状态必须用
//   无原型对象，否则：
//     - 未选择就被判为已回答（other/selected 读命中 Object.prototype）；
//     - 提交时伪造“其他答案”；
//     - __proto__ 历史答案赋值触发 setter，无法形成 own property，重挂载丢选中态。
// 断言：
//   1) 未选择时提交按钮禁用（特殊 id 不得被误判为已回答）；
//   2) 选择三个选项后提交按钮启用，payload 的 questionId/answerKey 与答案完整、无伪造 other；
//   3) 锁定卡用 initialAnswers（含 __proto__）还原选中态。
// 模式对齐 question_choice_card_smoke.mjs：vite mpa + puppeteer 加载 fixtures。
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import react from '@vitejs/plugin-react';
import puppeteer from 'puppeteer-core';
import { createServer } from 'vite';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const chrome = process.env.CHROME || [
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  '/snap/bin/chromium',
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
  '/usr/bin/google-chrome',
  '/usr/bin/google-chrome-stable',
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
].find((candidate) => fs.existsSync(candidate));

if (!chrome) {
  console.error('SKIP: 未找到 Chrome/Edge，可通过 CHROME 指定');
  process.exit(2);
}

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-choice-card-special-id-smoke-'));
let browser;
let vite;

function assert(condition, message, detail) {
  if (!condition) throw new Error(`${message}${detail ? `: ${JSON.stringify(detail)}` : ''}`);
}

try {
  vite = await createServer({
    root: appRoot,
    configFile: false,
    appType: 'mpa',
    logLevel: 'error',
    plugins: [react()],
    server: { host: '127.0.0.1', port: 0, strictPort: false },
  });
  await vite.listen();
  const address = vite.httpServer.address();
  const url = `http://127.0.0.1:${address.port}/tests/fixtures/question_choice_card_special_id_smoke.html`;

  browser = await puppeteer.launch({
    executablePath: chrome,
    headless: 'new',
    userDataDir: profile,
    args: ['--no-sandbox', '--disable-gpu', '--no-first-run', '--no-default-browser-check'],
  });
  const page = await browser.newPage();
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  await page.setViewport({ width: 1200, height: 900, deviceScaleFactor: 1 });
  await page.goto(url, { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('fieldset input[type="radio"]', { timeout: 10000 });

  const locate = (labelText) => page.evaluate((text) => {
    const labels = Array.from(document.querySelectorAll('[data-testid="special-card"] label'));
    const label = labels.find((el) => el.innerText.includes(text) && el.querySelector('input'));
    if (!label) return null;
    const input = label.querySelector('input');
    const lr = label.getBoundingClientRect();
    const ir = input.getBoundingClientRect();
    return {
      label: { x: lr.x, y: lr.y, width: lr.width, height: lr.height },
      input: { x: ir.x, y: ir.y, width: ir.width, height: ir.height },
      type: input.type,
      checked: input.checked,
    };
  }, labelText);

  const clickTextArea = async (labelText) => {
    const loc = await locate(labelText);
    assert(loc, `未找到选项 "${labelText}"`, { labelText });
    const x = (loc.input.x + loc.input.width + loc.label.x + loc.label.width) / 2;
    const y = loc.label.y + loc.label.height / 2;
    await page.mouse.click(x, y);
    return loc;
  };

  const submitButtonDisabled = () => page.evaluate(() => {
    const card = document.querySelector('[data-testid="special-card"]');
    const btn = Array.from(card.querySelectorAll('button')).find((el) => el.textContent.includes('提交'));
    return btn ? btn.disabled : null;
  });

  // ── 未选择：提交按钮必须禁用（特殊 id 不得被误判为已回答）──────
  const initiallyDisabled = await submitButtonDisabled();
  assert(initiallyDisabled === true, '未选择时提交按钮应禁用（constructor/toString/__proto__ 不得命中原型链被判为已回答）', { initiallyDisabled });

  // ── 选择三个选项后提交 ────────────────────────────────────────
  await clickTextArea('A');
  await clickTextArea('C');
  await clickTextArea('X');
  const enabled = await submitButtonDisabled();
  assert(enabled === false, '选择全部选项后提交按钮应启用', { enabled });

  await page.evaluate(() => {
    const card = document.querySelector('[data-testid="special-card"]');
    const btn = Array.from(card.querySelectorAll('button')).find((el) => el.textContent.includes('提交'));
    if (btn) btn.click();
  });
  await page.waitForFunction(() => document.body.innerText.includes('已提交（锁定）'), { timeout: 5000 });

  const submits = await page.evaluate(() => window.__submits);
  assert(submits.length === 1, '提交恰好触发一次', submits);
  const groupOf = (id) => submits[0].find((g) => g.questionId === id);
  for (const id of ['constructor', 'toString', '__proto__']) {
    const group = groupOf(id);
    assert(group, `payload 应包含 questionId "${id}"`, submits);
    assert(group.answerKey === id, `answerKey 应等于 "${id}"`, group.answerKey);
    assert(group.answers.length === 1, `"${id}" 恰好一条答案`, group.answers);
    assert(group.answers[0].other === false, `"${id}" 不得伪造 other 答案`, group.answers[0]);
  }
  assert(groupOf('constructor').answers[0].label === 'A', 'constructor 答案 label', groupOf('constructor').answers[0].label);
  assert(groupOf('toString').answers[0].label === 'C', 'toString 答案 label', groupOf('toString').answers[0].label);
  assert(groupOf('__proto__').answers[0].label === 'X', '__proto__ 答案 label', groupOf('__proto__').answers[0].label);

  // ── 锁定卡用 initialAnswers（含 __proto__）还原选中态 ──────────
  const lockedChecked = await page.evaluate(() => {
    const card = document.querySelector('[data-testid="locked-special-card"]');
    const input = Array.from(card.querySelectorAll('input[type="radio"]'))
      .find((el) => el.name.endsWith('__proto__-choice'));
    return input ? input.checked : null;
  });
  assert(lockedChecked === true, '锁定卡应还原 __proto__ 历史答案选中态（不得因 __proto__ setter 丢失 own property）', { lockedChecked });

  assert(pageErrors.length === 0, '浏览器运行时异常', pageErrors);

  console.log('question_choice_card_special_id_smoke: ok');
} catch (error) {
  console.error('FAIL:', error?.stack || error);
  process.exitCode = 1;
} finally {
  if (browser) await browser.close();
  if (vite) await vite.close();
  fs.rmSync(profile, { recursive: true, force: true });
}
