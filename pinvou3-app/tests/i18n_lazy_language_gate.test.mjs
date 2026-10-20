// ensureLanguage 惰性装载门的契约测试:在途去重、失败可重试、永不 reject、
// 未知/原型链语言返回 false、已装载语言同步命中。门历史上出过「去重不存在
// (lazyDictPending.set 从未调用)」的真实缺陷且当时无任何测试拦截,本文件钉住
// 全部对外契约,防止回归。
//
// 运行环境是 Node:门内部走 LAZY_DICT_LOADERS 的动态 import('./i18n/en.js'),
// Node 下该模块可直接解析,失败分支通过临时注册的 loader 注入(见各用例)。
import assert from 'node:assert/strict';
import test from 'node:test';

const i18nUrl = new URL('../src/shared/i18n.js', import.meta.url).href;

// 每个用例重新 import 一份干净的 i18n 模块(带唯一 query 绕过模块缓存),
// 保证 lazyDictPending/dict 的状态不跨用例泄漏。
async function freshGate() {
  const mod = await import(`${i18nUrl}?case=${Math.random().toString(36).slice(2)}`);
  return mod;
}

test('zh 装载后同步路径返回 true(启动预算拆分后走惰性装载)', async () => {
  const { ensureLanguage, dict } = await freshGate();
  // zh 自启动预算拆分(round-41)起与 en/ja 一致走惰性装载——装载完成后
  // 同步命中(第二次 ensureLanguage 立即 resolve,dict.zh 就位)。
  const first = await ensureLanguage('zh');
  assert.equal(first, true);
  assert.equal(typeof dict.zh, 'object');
  const second = await ensureLanguage('zh');
  assert.equal(second, true);
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
  const enUrl = new URL('../src/shared/i18n/en.js', import.meta.url).href;
  source = source.replace(
    "en: () => import('./i18n/en.js'),",
    `en: () => globalThis.__stubEn(),`,
  );
  source = source.replace(
    "import { dictZh } from './i18n/zh.js';",
    () => `import { dictZh } from '${enUrl.replace('/en.js', '/zh.js')}';`,
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
    await new Promise((resolve) => { setTimeout(resolve, 20); });
    throw new Error('slow stub failure');
  };
  const dedupUrl = 'data:text/javascript;charset=utf-8,' + encodeURIComponent(source + '\n// dedup-case');
  const { ensureLanguage: freshEnsure } = await import(dedupUrl);
  const results = await Promise.all([freshEnsure('en'), freshEnsure('en'), freshEnsure('en')]);
  assert.deepEqual(results, [false, false, false]);
  assert.equal(slowAttempts, 1, '在途并发应共享同一次装载(去重)');

  delete globalThis.__stubEn;
});

// Round-47 M6: 生产入口的调用点钉——helper 在隔离态被钉还不够,三个入口
// (main/pet-main/reader-main)必须真的经由 ensureLanguageWithZhFallback
// 引导;退回裸 ensureLanguage(...) 正是 round-46 M1 修掉的白屏类。
test('引导兜底:三个入口都经 ensureLanguageWithZhFallback 引导', async () => {
  const fs = await import('node:fs');
  const read = (p) => fs.readFileSync(new URL(p, import.meta.url), 'utf8');
  for (const entry of [
    '../src/app/main.jsx',
    '../src/app/pet-main.jsx',
    '../src/app/reader-main.jsx',
  ]) {
    const source = read(entry);
    assert.match(
      source,
      /ensureLanguageWithZhFallback\(/,
      `${entry} 必须经引导兜底装载语言`,
    );
    assert.doesNotMatch(
      source,
      /(?<!With)ensureLanguage\((?:__storedLang \|\| __initialLang|initialSystemLanguage\(\))\)/,
      `${entry} 不得退回裸 ensureLanguage 引导`,
    );
  }
});

// Round-49 M2 (Jensen round-49 P2): 入口双失败回归——helper 返回 null 时,
// 三个入口必须挂词典无关的静态失败/重试态而不是照常挂载(词典依赖组件会
// 在首帧 TypeError)。先钉接线:入口必须经 mountWhenLanguageReady 引导,
// 不得退回裸 `.then(() => render)` 形状;再行为级驱动双失败分支。
test('入口双失败接线:三个入口经引导门,不得退回裸 .then 挂载', async () => {
  const fs = await import('node:fs');
  const read = (p) => fs.readFileSync(new URL(p, import.meta.url), 'utf8');
  for (const entry of [
    '../src/app/main.jsx',
    '../src/app/pet-main.jsx',
    '../src/app/reader-main.jsx',
  ]) {
    const source = read(entry);
    assert.match(
      source,
      /mountWhenLanguageReady\(\{/,
      `${entry} 必须经 shared/entry-boot.js 的引导门挂载(round-49 M2)`,
    );
    assert.doesNotMatch(
      source,
      /ensureLanguageWithZhFallback\([^)]*\)\s*\.then\(/,
      `${entry} 不得绕过引导门直接 .then 挂载(null 判定会被丢弃)`,
    );
  }
});

test('入口双失败行为:null 判定挂失败/重试态且不挂载,Retry 成功后才 mount', async () => {
  const bootUrl = new URL('../src/shared/entry-boot.js', import.meta.url).href;
  const { mountWhenLanguageReady } = await import(
    `${bootUrl}?case=${Math.random().toString(36).slice(2)}`
  );

  // 最小 DOM 桩:记录绘制效果,暴露 Retry 监听器供测试驱动。
  function fakeDoc() {
    const listeners = new Map();
    const makeEl = (tag) => ({
      tag,
      type: undefined,
      textContent: '',
      attributes: {},
      dataset: {},
      children: [],
      setAttribute(key, value) { this.attributes[key] = value; },
      addEventListener(kind, fn) { listeners.set(kind, fn); },
      append(...els) { this.children.push(...els); },
    });
    const host = makeEl('#root');
    return {
      host,
      listeners,
      createElement: (tag) => makeEl(tag),
      querySelector: (sel) => (sel === '#root' ? host : null),
      clickRetry() {
        const fn = listeners.get('click');
        if (fn) fn();
      },
    };
  }

  let attempts = 0;
  let mounted = 0;
  const doc = fakeDoc();
  // 注意用 undefined 表示"继续双失败":null ?? fallback 会把 null 吞掉。
  const verdicts = [null, null, 'en'];
  const boot = mountWhenLanguageReady({
    load: () => {
      attempts += 1;
      const verdict = verdicts.shift();
      return Promise.resolve(verdict === undefined ? 'en' : verdict);
    },
    mount: () => { mounted += 1; },
    doc,
  });

  // 第一轮:双失败(null)——不得挂载,须挂静态失败态(硬编码英文文案 +
  // Retry 按钮,词典不可用态下不存在可用 i18n 文案)。
  await new Promise((resolve) => { setTimeout(resolve, 0); });
  assert.equal(attempts, 1);
  assert.equal(mounted, 0, '双失败判定不得挂载词典依赖组件');
  const box = doc.host.children[0];
  assert.ok(box, '双失败判定必须绘制失败态');
  assert.equal(box.dataset.bootFailure, 'true');
  const button = box.children[2];
  assert.equal(button.textContent, 'Retry');

  // 第二轮:Retry 再次双失败——仍是失败态,依然不挂载。
  doc.clickRetry();
  await new Promise((resolve) => { setTimeout(resolve, 0); });
  assert.equal(attempts, 2);
  assert.equal(mounted, 0, '重试再失败仍不得挂载');
  assert.equal(doc.host.children[0].dataset.bootFailure, 'true', '失败态应重绘');

  // 第三轮:Retry 后装载成功——恰挂载一次,且失败态被清除。
  doc.clickRetry();
  await new Promise((resolve) => { setTimeout(resolve, 0); });
  assert.equal(attempts, 3);
  assert.equal(mounted, 1, '装载成功后应恰好挂载一次');
  assert.equal(doc.host.textContent, '', '挂载前应清掉失败态 DOM');

  // boot 返回值可被测试/入口直接驱动;挂载后再次引导是 no-op(round-50 P2
  // 的挂载独占语义):不重复挂载,也不再绘失败屏。
  const direct = await boot();
  assert.equal(direct, false, '挂载后的引导尝试必须是 no-op');
  assert.equal(mounted, 1, '挂载态独占根,不二次挂载');
  assert.equal(doc.host.dataset.bootFailure, undefined, '挂载后不得重绘失败屏');
});

// Round-50 P2 (Jensen): 并发 Retry 乱序回归——Retry 在装载期间可点击,较旧的
// 完成不得触碰根:旧 null 不得清掉已挂载的应用重绘失败屏,旧成功不得二次
// 挂载。顺序用受控 deferred 精确编排(Jensen 的探针序列)。
test('入口并发 Retry 乱序:陈旧完成不触碰根,最新尝试独占', async () => {
  const bootUrl = new URL('../src/shared/entry-boot.js', import.meta.url).href;
  const { mountWhenLanguageReady } = await import(
    `${bootUrl}?case=${Math.random().toString(36).slice(2)}`
  );

  // 假 DOM(round-51 M1 加固):textContent='' 必须与真实 DOM 同语义地清空
  // 子节点,否则"陈旧 null 重绘"断言在幸存旧盒上假绿;paints 记录每次
  // 失败屏绘制(每次 paint 恰向 #root append 一个盒),突变掉票号守卫后
  // 陈旧重绘会使计数变 2。
  function fakeDoc() {
    const listeners = new Map();
    const makeEl = (tag) => {
      const el = {
        tag,
        type: undefined,
        dataset: {},
        attributes: {},
        children: [],
        setAttribute(key, value) { this.attributes[key] = value; },
        addEventListener(kind, fn) { listeners.set(kind, fn); },
        append(...els) { for (const child of els) this.children.push(child); },
      };
      let text = '';
      Object.defineProperty(el, 'textContent', {
        get: () => text,
        set: (value) => {
          text = value;
          if (value === '') el.children = [];
        },
      });
      return el;
    };
    const host = makeEl('#root');
    const paints = [];
    const hostAppend = host.append.bind(host);
    host.append = (...els) => {
      paints.push(els[0]);
      hostAppend(...els);
    };
    return {
      host,
      paints,
      listeners,
      createElement: (tag) => makeEl(tag),
      querySelector: (sel) => (sel === '#root' ? host : null),
      clickRetry() {
        const fn = listeners.get('click');
        if (fn) fn();
      },
    };
  }

  function deferred() {
    let resolve;
    const promise = new Promise((res) => { resolve = res; });
    return { promise, settle: (verdict) => resolve(verdict) };
  }
  const tick = () => new Promise((resolve) => { setTimeout(resolve, 0); });

  async function drive(verdicts) {
    const doc = fakeDoc();
    const pending = [];
    let mounted = 0;
    let attemptIndex = 0;
    const boot = mountWhenLanguageReady({
      load: () => {
        const attempt = deferred();
        pending.push(attempt);
        void verdicts[attemptIndex];
        attemptIndex += 1;
        return attempt.promise;
      },
      mount: () => { mounted += 1; },
      doc,
    });
    return {
      doc, pending, boot,
      get mounted() { return mounted; },
      settle(index, verdict) { pending[index].settle(verdict); },
      get paints() { return doc.paints.length; },
    };
  }

  // ── 腿 1: 陈旧 null 在最新尝试在途时落地——不得重绘(突变判别腿:
  // 判定脚本 gen1=null, gen2=null, gen3='en'; gen2 在 gen3 在途时先落)。──
  {
    const run = await drive([null, null, 'en']);
    await tick();
    run.settle(0, null);
    await tick();
    assert.equal(run.mounted, 0);
    assert.equal(run.paints, 1, '初始双失败恰绘一次失败屏');
    const firstBox = run.doc.host.children[0];
    assert.equal(firstBox.dataset.bootFailure, 'true');

    run.doc.clickRetry();
    run.doc.clickRetry();
    await tick();
    assert.equal(run.pending.length, 3, 'gen1..gen3 各占一次装载(gen1 已放行但仍在册)');
    run.settle(1, null); // gen2: 陈旧 null,gen3 仍在途
    await tick();
    assert.equal(run.mounted, 0, '陈旧 null 完成不得挂载');
    assert.equal(
      run.paints, 1,
      '陈旧 null 不得重绘失败屏(票号守卫被突变掉后此处变 2,即回归信号)',
    );
    assert.equal(
      run.doc.host.children[0], firstBox,
      '根上仍是初始失败盒(陈旧重绘会先清空 children 再放新盒)',
    );

    run.settle(2, 'en'); // gen3: 最新成功 → 恰一次挂载,根清空
    await tick();
    assert.equal(run.mounted, 1, '最新成功尝试恰挂载一次');
    assert.equal(run.doc.host.textContent, '');
  }

  // ── 腿 2: 陈旧成功在最新 null 在途时落地——不得挂载(突变判别腿:
  // 判定脚本 gen1=null, gen2='en', gen3=null; gen2 的成功在 gen3 在途时
  // 先落,陈旧成功挂载会把应用盖在最新判定"仍是失败"之上)。──
  {
    const run = await drive([null, 'en', null]);
    await tick();
    run.settle(0, null);
    await tick();
    assert.equal(run.paints, 1);

    run.doc.clickRetry();
    run.doc.clickRetry();
    await tick();
    run.settle(1, 'en'); // gen2: 陈旧成功,gen3(null)仍在途
    await tick();
    assert.equal(run.mounted, 0, '陈旧成功完成不得挂载(最新判定尚未落地)');
    assert.equal(run.paints, 1, '不得触碰失败屏');
    assert.equal(
      run.doc.host.children[0].dataset.bootFailure, 'true',
      '失败屏必须原样保留',
    );

    run.settle(2, null); // gen3: 最新判定仍失败 → 允许重绘
    await tick();
    assert.equal(run.mounted, 0);
    assert.equal(run.paints, 2, '最新判定的失败重绘是合法行为');
    assert.equal(run.doc.host.children[0].dataset.bootFailure, 'true');
  }

  // ── 腿 3: 最新成功之后落陈旧成功——挂载态独占,不得二次挂载。──
  {
    const run = await drive([null, 'en', 'en']);
    await tick();
    run.settle(0, null);
    await tick();
    run.doc.clickRetry();
    run.doc.clickRetry();
    await tick();
    run.settle(2, 'en'); // gen3: 最新成功先落
    await tick();
    assert.equal(run.mounted, 1);
    run.settle(1, 'en'); // gen2: 陈旧成功后落
    await tick();
    assert.equal(run.mounted, 1, '陈旧成功不得二次挂载');
    assert.equal(run.doc.host.children.length, 0, '挂载态下根上无失败盒');
  }
});

// 引导兜底(round-46 M1):入口的降级装载链。旧 `dict[language] || dict.zh`
// 是死腿——zh 只在语言=zh 时才装载,en/ja 用户 chunk 失败时该腿永远空,首帧
// t.uiMainApp 解引用 undefined 即白屏。ensureLanguageWithZhFallback 让 zh 在
// 首选语言失败时真实装载,渲染腿 dict[language] || zhDict() 因此活。
test('引导兜底:首选语言 chunk 失败时降级装载 zh,渲染腿以中文兜底', async () => {
  const fs = await import('node:fs');
  const zhUrl = new URL('../src/shared/i18n/zh.js', import.meta.url).href;
  const inject = (enStub, zhStub) => {
    let source = fs.readFileSync(new URL('../src/shared/i18n.js', import.meta.url), 'utf8');
    // 函数形式的替换值:动态字符串直接作为 replacement 会被
    // unicorn/no-unsafe-string-replacement 拦下($ 模式风险)。
    if (enStub) source = source.replace("en: () => import('./i18n/en.js'),", () => `en: () => ${enStub},`);
    if (zhStub) source = source.replace("zh: () => import('./i18n/zh.js'),", () => `zh: () => ${zhStub},`);
    source = source.replace("import { dictZh } from './i18n/zh.js';", () => `import { dictZh } from '${zhUrl}';`);
    return 'data:text/javascript;charset=utf-8,' + encodeURIComponent(source + `\n// fallback-${Math.random().toString(36).slice(2)}`);
  };

  // en 恒败,zh 走真实装载(data: URL 实例内的惰性装载器须指绝对路径)。
  const good = await import(inject("Promise.reject(new Error('stub: chunk unreachable'))", `import('${zhUrl}')`));
  const ready = await good.ensureLanguageWithZhFallback('en');
  assert.equal(ready, 'zh', 'en chunk 失败应降级装载 zh');
  assert.ok(!good.dict.en, '失败的 en 不得装箱');
  assert.ok(good.zhDict().newChat, 'zh 兜底装载后 zhDict() 非空');
  assert.equal((good.dict.en || good.zhDict()).newChat, '新对话', '渲染腿降级到中文词条而非 undefined');
  assert.equal(await good.ensureLanguageWithZhFallback('zh'), 'zh', '首选即 zh 时直接命中');

  // 双失败:en 与 zh 都不可达,返回 null(无可渲染词典,白屏是诚实的终态)。
  const dead = await import(inject("Promise.reject(new Error('stub: en unreachable'))", "Promise.reject(new Error('stub: zh unreachable'))"));
  assert.equal(await dead.ensureLanguageWithZhFallback('en'), null, 'zh 也失败时应返回 null 而非 reject');
});

test('en/ja 惰性 chunk 在浏览器口径下不进首屏(静态扫描 i18n.js 不 import en/ja)', async () => {
  const fs = await import('node:fs');
  const source = fs.readFileSync(new URL('../src/shared/i18n.js', import.meta.url), 'utf8');
  // 核心模块只允许动态 import 语言文件;出现静态 import 即拆分失效。
  const staticImport = source.match(/^import\s+[^'"]*['"][^'"]*i18n\/(en|ja)\.js['"]/m);
  assert.equal(staticImport, null, 'i18n.js 不得静态 import en/ja(会钉进共享 chunk)');
});

// 语言切换「最新选择胜出」门的乱序回归:ja chunk 静态依赖 en chunk,先选 ja
// 再选 en 时较新的 en 请求可能先完成(共享依赖先行就位),慢的旧 ja continuation
// 若无守卫会覆盖状态/持久化/广播,回退到用户未选择的语言(同步时代是最后
// 选择胜出,惰性化后必须保持该语义)。
test('切换乱序:后发起的较新选择胜出,慢的旧请求不得覆盖', async () => {
  const { createLatestLanguageGate } = await freshGate();
  const applied = [];
  // 受控装载时钟:en 快(5ms)、ja 慢(30ms),模拟真实依赖顺序。
  const ensure = (lang) => new Promise((resolve) => {
    setTimeout(() => resolve(true), lang === 'ja' ? 30 : 5);
  });
  const switchTo = createLatestLanguageGate(ensure);
  switchTo('ja', () => applied.push('ja'));
  switchTo('en', () => applied.push('en'));
  await new Promise((resolve) => { setTimeout(resolve, 100); });
  assert.deepEqual(applied, ['en'], '只应落地较新的 en 选择,慢的旧 ja continuation 必须被拦下');
});

test('切换门:成功落地恰一次;装载失败不落地且可重试', async () => {
  const { createLatestLanguageGate } = await freshGate();
  const applied = [];
  let jaAttempts = 0;
  const ensure = (lang) => {
    if (lang !== 'ja') return Promise.resolve(true);
    jaAttempts += 1;
    return jaAttempts === 1 ? Promise.resolve(false) : Promise.resolve(true);
  };
  const switchTo = createLatestLanguageGate(ensure);
  switchTo('ja', () => applied.push('ja-fail'));
  await new Promise((resolve) => { setTimeout(resolve, 0); });
  assert.deepEqual(applied, [], '装载失败(false)不得落地');
  // 失败后重试(无更新选择在途)应正常落地;顺序发起的选择各自落地。
  switchTo('ja', () => applied.push('ja-retry'));
  await new Promise((resolve) => { setTimeout(resolve, 0); });
  assert.deepEqual(applied, ['ja-retry'], '失败清挂起后重试应落地');
  switchTo('zh', () => applied.push('zh'));
  await new Promise((resolve) => { setTimeout(resolve, 0); });
  assert.deepEqual(applied, ['ja-retry', 'zh'], '顺序发起的选择按发起顺序落地');
});

test('切换门:ensure 抛异常不得炸出未处理 rejection', async () => {
  const { createLatestLanguageGate } = await freshGate();
  const applied = [];
  const switchTo = createLatestLanguageGate(() => Promise.reject(new Error('boom')));
  switchTo('en', () => applied.push('en'));
  await new Promise((resolve) => { setTimeout(resolve, 0); });
  assert.deepEqual(applied, [], '拒绝的装载不得落地');
});
