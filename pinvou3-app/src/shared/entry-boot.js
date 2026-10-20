// 入口引导门(round-49 M2 / Jensen round-49 P2):词典就绪才允许挂载。
// ensureLanguageWithZhFallback 返回 null 表示首选语言与 zh chunk 双双装载失败
// (web 上可达:已缓存的 bundle 引用重部署后已剪除的哈希 chunk)。此时挂载
// 词典依赖组件会在渲染中途 TypeError(main 的无条件 RightDockHost 表达式、
// 桌宠的 petCopy.sendFailed 依赖)——三个生产入口必须经由本门,让双失败
// 分支只有一处实现、一份行为测试。
// 失败态文案刻意硬编码英文:该状态下词典定义上不可用,不存在可用的 i18n
// 文案;这是词典无关的静态失败/重试态,不是常规 UI 文案。字符串刻意收紧:
// 本模块随主入口 bundle 计入启动预算(504 kB 上限)。
// doc 可注入:node 侧回归测试用它驱动假 DOM,验证绘制效果与 Retry 闭包。

export function paintBootFailure({ retry, doc = document }) {
  const host = doc.querySelector('#root');
  if (!host) return;
  host.textContent = '';
  const box = doc.createElement('div');
  box.dataset.bootFailure = 'true';
  box.setAttribute(
    'style',
    'display:flex;flex-direction:column;align-items:center;justify-content:center;height:100vh',
  );
  const title = doc.createElement('div');
  title.textContent = 'Failed to load';
  const hint = doc.createElement('div');
  hint.setAttribute('style', 'font-size:13px;opacity:0.7;');
  hint.textContent = 'Language data failed to load. Check your connection and retry.';
  const button = doc.createElement('button');
  button.type = 'button';
  button.textContent = 'Retry';
  button.setAttribute('style', 'padding:6px 18px;cursor:pointer;');
  button.addEventListener('click', retry);
  box.append(title, hint, button);
  host.append(box);
}

// load 解析为就绪语言(truthy)才执行 mount;null 判定挂静态失败/重试态,
// Retry 按钮重新走完整引导(而不是复用失败结果)。
// 并发乱序防护(round-50 P2):Retry 在装载期间仍可点击,每次 boot 递增代数,
// 只有最新一次尝试可以触碰根——否则旧 null 完成会在新尝试成功挂载后清掉
// 应用重绘失败屏,旧成功也会二次挂载(装载完成后不再绘失败屏,挂载态独占
// 根)。挂载前必须清掉失败态 DOM:React 18 的 createRoot 不保证移除容器
// 既有子节点。返回 boot 供测试直接驱动。
export function mountWhenLanguageReady({ load, mount, doc = document }) {
  let generation = 0;
  let mounted = false;
  const boot = () => {
    const ticket = ++generation;
    return load().then((ready) => {
      if (ticket !== generation || mounted) return false;
      if (ready) {
        mounted = true;
        const host = doc.querySelector('#root');
        if (host) host.textContent = '';
        mount();
        return true;
      }
      paintBootFailure({ retry: boot, doc });
      return false;
    });
  };
  boot();
  return boot;
}
