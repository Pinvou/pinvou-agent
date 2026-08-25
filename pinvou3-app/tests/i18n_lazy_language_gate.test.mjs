// ensureLanguage 惰性装载门的契约测试:在途去重、失败可重试、永不 reject、
// 未知/原型链语言返回 false、已装载语言同步命中。门历史上出过「去重不存在
// (lazyDictPending.set 从未调用)」的真实缺陷且当时无任何测试拦截,本文件钉住
// 全部对外契约,防止回归。
//
// 运行环境是 Node:门内部走 LAZY_DICT_LOADERS 的动态 import('./i18n/en.js'),
// Node 下该模块可直接解析,失败分支通过临时注册的 loader 注入(见各用例)。
import assert from 'node:assert/strict';
import test from 'node:test';
import { pathToFileURL } from 'node:url';

const i18nUrl = new URL('../src/shared/i18n.js', import.meta.url).href;

// 每个用例重新 import 一份干净的 i18n 模块(带唯一 query 绕过模块缓存),
// 保证 lazyDictPending/dict 的状态不跨用例泄漏。
async function freshGate() {
  const mod = await import(`${i18nUrl}?case=${Math.random().toString(36).slice(2)}`);
  return mod;
}

test('已装载语言(zh 内嵌)同步路径返回 true', async () => {
  const { ensureLanguage, dict } = await freshGate();
  assert.equal(typeof dict.zh, 'object');
  const ok = await ensureLanguage('zh');
  assert.equal(ok, true);
});

test('未知语言与原型链键返回 false 且不抛错', async () => {
  const { ensureLanguage } = await freshGate();
  for (const lang of ['fr', '', 'constructor', 'toString', '__proto__', 'hasOwnProperty']) {
    const ok = await ensureLanguage(lang);
    assert.equal(ok, false, `ensureLanguage(${JSON.stringify(lang)}) 应返回 false`);
  }
});

test('成功装载:去重在途并发,词典一次装箱', async () => {
  const { ensureLanguage, dict } = await freshGate();
  // en 是真实的惰性 chunk:并发触发应共享同一次装载。
  const results = await Promise.all([ensureLanguage('en'), ensureLanguage('en'), ensureLanguage('en')]);
  assert.deepEqual(results, [true, true, true]);
  assert.ok(dict.en, '装载成功后 dict.en 应可用');
  assert.equal(dict.en.newChat, 'New chat');
  // 装载完成后回到同步命中路径。
  assert.equal(await ensureLanguage('en'), true);
});

test('成功装载:ja chunk 装箱(经 en 兜底 spread)', async () => {
  const { ensureLanguage, dict } = await freshGate();
  assert.ok(!dict.ja);
  const ok = await ensureLanguage('ja');
  assert.equal(ok, true); // Node 下 ja chunk 可解析,验证成功路径装箱
  assert.ok(dict.ja);
  assert.equal(dict.ja.newChat, '新しいチャット');
});

// 失败/重试分支需要 loader 可控失败,而 LAZY_DICT_LOADERS 冻结且键固定为
// en/ja。子进程里用 --import 钩子拦截动态 import 不值得;改为直接源码级
// 驱动:读取 i18n.js 源码,把 en loader 替换为受控桩后在 data: URL 里实例化,
// 验证 catch(false)/finally(清挂起)/重试三轮契约。
test('失败分支:源码注入桩 loader 验证 false + 清挂起 + 可重试', async () => {
  const fs = await import('node:fs');
  let source = fs.readFileSync(new URL('../src/shared/i18n.js', import.meta.url), 'utf8');
  assert.match(source, /const LAZY_DICT_LOADERS/, 'i18n.js 结构变化,注入点失效需更新本测试');
  // 把 en 的 loader 替换为受控桩:两次失败后成功;import 路径改为相对 data: URL 可达的绝对路径。
  const enUrl = pathToFileURL(new URL('../src/shared/i18n/en.js', import.meta.url).pathname).href;
  source = source.replace(
    "en: () => import('./i18n/en.js'),",
    `en: () => globalThis.__stubEn(),`,
  );
  source = source.replace(
    "import { dictZh } from './i18n/zh.js';",
    `import { dictZh } from '${enUrl.replace('/en.js', '/zh.js')}';`,
  );
  let attempts = 0;
  globalThis.__stubEn = async () => {
    attempts += 1;
    if (attempts < 3) throw new Error(`stub failure #${attempts}`);
    return import(enUrl);
  };
  const modUrl = 'data:text/javascript;charset=utf-8,' + encodeURIComponent(source);
  const { ensureLanguage, dict } = await import(modUrl);

  assert.equal(await ensureLanguage('en'), false, '第一次失败应 resolve false');
  assert.equal(attempts, 1);
  assert.ok(!dict.en, '失败不得装箱词典');
  assert.equal(await ensureLanguage('en'), false, '第二次仍失败,resolve false');
  assert.equal(attempts, 2, '清挂起后应真实重试(而不是复用失败结果)');
  assert.equal(await ensureLanguage('en'), true, '第三次成功');
  assert.equal(attempts, 3);
  assert.ok(dict.en, '成功后词典装箱');

  // 在途去重:并发三次失败只产生一次装载尝试。
  let slowAttempts = 0;
  globalThis.__stubEn = async () => {
    slowAttempts += 1;
    await new Promise((resolve) => setTimeout(resolve, 20));
    throw new Error('slow stub failure');
  };
  const dedupUrl = 'data:text/javascript;charset=utf-8,' + encodeURIComponent(source + '\n// dedup-case');
  const { ensureLanguage: freshEnsure } = await import(dedupUrl);
  const results = await Promise.all([freshEnsure('en'), freshEnsure('en'), freshEnsure('en')]);
  assert.deepEqual(results, [false, false, false]);
  assert.equal(slowAttempts, 1, '在途并发应共享同一次装载(去重)');

  delete globalThis.__stubEn;
});

test('en/ja 惰性 chunk 在浏览器口径下不进首屏(静态扫描 i18n.js 不 import en/ja)', async () => {
  const fs = await import('node:fs');
  const source = fs.readFileSync(new URL('../src/shared/i18n.js', import.meta.url), 'utf8');
  // 核心模块只允许动态 import 语言文件;出现静态 import 即拆分失效。
  const staticImport = source.match(/^import\s+[^'"]*['"][^'"]*i18n\/(en|ja)\.js['"]/m);
  assert.equal(staticImport, null, 'i18n.js 不得静态 import en/ja(会钉进共享 chunk)');
});
