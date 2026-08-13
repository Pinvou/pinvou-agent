#!/usr/bin/env node
// QuestionChoiceCard 整行可点的真实 DOM 冒烟（评审 P2-1）：
//   1) 点击选项文本/描述区域 → 恰好选中一次（label 隐式激活不可靠，onClick 兜底 preventDefault）；
//   2) 点击圆点本身 → 交原生 onChange，不双触发；
//   3) 多选 toggle 恰好一次（重复点击文本会切换，不会因双触发回到原态）；
//   4) 提交后锁定卡用 restoredAnswers 还原已选答案。
// 模式对齐 code_viewer_modal_smoke.mjs：vite mpa + puppeteer 加载 fixtures。
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

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-choice-card-smoke-'));
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
  const url = `http://127.0.0.1:${address.port}/tests/fixtures/question_choice_card_smoke.html`;

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

  // 按选项文本定位 label；返回 label 与内部 input 的 box，便于分别点击文本/圆点。
  const locate = (labelText) => page.evaluate((text) => {
    const labels = Array.from(document.querySelectorAll('label'));
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
    // 文本/描述区在圆点右侧：取 label 右半部分中心（避开 input）。
    const x = (loc.input.x + loc.input.width + loc.label.x + loc.label.width) / 2;
    const y = loc.label.y + loc.label.height / 2;
    await page.mouse.click(x, y);
    return loc;
  };
  const clickDot = async (labelText) => {
    const loc = await locate(labelText);
    assert(loc, `未找到选项 "${labelText}"`, { labelText });
    await page.mouse.click(loc.input.x + loc.input.width / 2, loc.input.y + loc.input.height / 2);
    return loc;
  };
  const checkedState = () => page.evaluate(() => Array.from(document.querySelectorAll('fieldset input')).map((i) => ({
    name: i.name,
    checked: i.checked,
    type: i.type,
  })));

  // ── 单选：点击文本恰好选中一次 ─────────────────────────────────
  const before = await checkedState();
  assert(before.filter((i) => i.checked).length === 0, '初始无选中', before);
  await clickTextArea('Python');
  const afterText = await checkedState();
  const pythonRadio = afterText.find((i) => i.name.endsWith('-q-lang-choice'));
  assert(pythonRadio && pythonRadio.checked === true, '点击文本后单选恰好选中（不得被原生隐式激活双触发取消）', afterText);
  assert(afterText.filter((i) => i.type === 'radio' && i.checked).length === 1, '单选只选中一项', afterText);

  // 切换到另一个选项：文本点击 Go，Python 取消、Go 选中。
  await clickTextArea('Go');
  const afterSwitch = await checkedState();
  // name 是 `${cardId}-${question.id}-choice`，不含选项；按 DOM 顺序取单选的两个 radio。
  const radios = afterSwitch.filter((i) => i.name.endsWith('-q-lang-choice'));
  assert(radios.length === 2 && radios[0].checked === false && radios[1].checked === true,
    '文本点击 Go 后单选切换到第二项', afterSwitch);

  // ── 多选：点击文本 toggle 恰好一次 ─────────────────────────────
  await clickTextArea('前端');
  const multiOn = await checkedState();
  const frontendCheck = multiOn.find((i) => i.type === 'checkbox' && i.name.endsWith('-q-skill-choice'));
  assert(frontendCheck && frontendCheck.checked === true, '多选点击文本后选中', multiOn);
  await clickTextArea('前端');
  const multiOff = await checkedState();
  const frontendCheck2 = multiOff.find((i) => i.type === 'checkbox' && i.name.endsWith('-q-skill-choice'));
  assert(frontendCheck2 && frontendCheck2.checked === false,
    '多选重复点击文本恰好 toggle 一次（若 onClick+原生 onChange 双触发会回到选中）', multiOff);
  await clickTextArea('后端');
  await clickTextArea('运维');
  const multiFinal = await checkedState();
  const checks = multiFinal.filter((i) => i.type === 'checkbox' && i.checked);
  assert(checks.length === 2, '多选可同时勾选多项', multiFinal);

  // ── 点击圆点本身：走原生 onChange，单选仍恰好一次 ──────────────
  await clickDot('Python');
  const afterDot = await checkedState();
  const radios2 = afterDot.filter((i) => i.name.endsWith('-q-lang-choice'));
  assert(radios2.length === 2 && radios2[0].checked === true && radios2[1].checked === false,
    '点击圆点本身经原生 onChange 选中', afterDot);

  // ── 提交：__submits 记录答案；锁定卡还原已选 ───────────────────
  await page.evaluate(() => {
    const btn = Array.from(document.querySelectorAll('button')).find((el) => el.textContent.includes('提交'));
    if (btn) btn.click();
  });
  await page.waitForFunction(() => document.body.innerText.includes('已提交（锁定）'), { timeout: 5000 });
  const submits = await page.evaluate(() => window.__submits);
  assert(submits.length === 1, '提交恰好触发一次', submits);
  const langGroup = submits[0].find((g) => g.questionId === 'q-lang');
  const skillGroup = submits[0].find((g) => g.questionId === 'q-skill');
  assert(langGroup && langGroup.answers.some((a) => a.label === 'Python'), '单选答案 Python 提交', submits);
  const skillLabels = skillGroup ? skillGroup.answers.map((a) => a.label).sort() : [];
  assert(JSON.stringify(skillLabels) === JSON.stringify(['后端', '运维']), '多选答案 后端+运维 提交', submits);

  // 锁定卡渲染 initialAnswers 选中态（resolved 卡高亮恢复）。
  const lockedChecked = await page.evaluate(() => {
    const cards = Array.from(document.querySelectorAll('fieldset'));
    return cards.map((f) => Array.from(f.querySelectorAll('input')).filter((i) => i.checked).length);
  });
  assert(lockedChecked.some((n) => n > 0), '锁定卡 restoredAnswers 还原选中态', lockedChecked);

  // ── 评审 P2：其他值 == 预设 value 时还原为“其他”而非预设 ─────────
  const otherCollision = await page.evaluate(() => {
    const card = document.querySelector('[data-testid="other-collision-card"]');
    const textInputs = Array.from(card.querySelectorAll('input[type="text"]'));
    const radios = Array.from(card.querySelectorAll('input[type="radio"]'));
    return {
      otherValue: textInputs.length ? textInputs[0].value : null,
      checkedCount: radios.filter((r) => r.checked).length,
    };
  });
  assert(otherCollision.otherValue === 'A', '其他值 == 预设 value 时应还原为“其他”输入而非预设选项', otherCollision);
  assert(otherCollision.checkedCount === 0, '预设选项 A 不得被误选中（应还原为“其他”）', otherCollision);

  assert(pageErrors.length === 0, '浏览器运行时异常', pageErrors);

  console.log('question_choice_card_smoke: ok');
} catch (error) {
  console.error('FAIL:', error?.stack || error);
  process.exitCode = 1;
} finally {
  if (browser) await browser.close();
  if (vite) await vite.close();
  fs.rmSync(profile, { recursive: true, force: true });
}
