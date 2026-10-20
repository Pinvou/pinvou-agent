// UI 词典核心:zh 全量内嵌(启动即用),en/ja 按语言惰性 chunk(见 i18n/ 目录)。
// 浏览器入口在首帧前 ensureLanguage(<初始语言>);切换语言先 ensureLanguage 再改状态。
// Node contract tests and any consumer needing all three languages import
// '../../tests/helpers/i18n-all.js' (aggregate shim, statically pulling in all three).
// 维护约定:en/ja 覆盖 zh 的全部叶子 key(parity 断言在 ui_language_coverage;en/ja
// 比 zh 多出的 settings 相关键是历史遗留);ja 以 en 兜底(i18n/ja.js 内 spread);
// settings 详情文案已并入各语言文件(原 settings-i18n.js)。
// Startup-budget (round-41 merge with #586): zh joins en/ja behind the
// dynamic loaders — the eager 152 kB dictionary pushed the main entry
// past the 500 kB budget. The dict fills via ensureLanguage (the same
// async flow en/ja already ride); a read before load resolves through
// the language gate's fallback (createLatestLanguageGate).
const dict = {};

// UI 语言 ↔ 后端 UserPrefs language(BCP 47 tag)映射。加语言时三处同步:
// 这里 / dict / Rust prefs.rs Language 枚举
const LANG_TO_TAG = { zh: 'zh-Hans', en: 'en', ja: 'ja' };
const TAG_TO_LANG = { 'zh-Hans': 'zh', 'en': 'en', 'ja': 'ja' };
function languageFromLocaleTags(localeTags, fallback = 'en') {
  const locales = Array.isArray(localeTags) ? localeTags : [localeTags];
  const locale = locales.find((value) => typeof value === 'string' && value.trim());
  if (!locale) return fallback;
  const primary = locale.trim().split(/[-_.@:]/, 1)[0].toLowerCase();
  if (primary === 'zh') return 'zh';
  if (primary === 'ja') return 'ja';
  if (primary === 'en') return 'en';
  // 当前只提供中、英、日；系统首选语言不受支持时使用英文。
  return 'en';
}
// 首帧系统语言探测:主窗口与各辅助窗口(桌宠/阅读器/分离窗口)在落盘
// settings 到达前共用;之后仍以 get_settings/bs.settings 的显式配置为准。
function initialSystemLanguage() {
  if (typeof navigator === 'undefined') return 'en';
  return languageFromLocaleTags(
    navigator.languages?.length ? navigator.languages : navigator.language,
  );
}
const SEARCH_KEY_PROVIDERS = ['metaso', 'bocha', 'baidu', 'tavily'];

// Backend "new chat" sentinel title set. The backend (store.rs's
// NEW_CHAT_TITLE) always persists the fixed zh sentinel「新对话」; the
// trilingual set is a display-layer superset — at render time any sentinel
// maps to the current language's "new chat" copy (covers sessions created
// before the sentinels were unified). The set is independent of language
// dictionary load progress and cannot be derived lazily from the dict
// (zh-primary users never load the en/ja chunks).
export const DEFAULT_CHAT_TITLES = new Set(['新对话', 'New chat', '新しいチャット']);

// F12-3：默认标题哨兵判定与集合同源导出（main.jsx 侧副本因 #484 冻结暂保留，
// 对话管理页等模块侧消费方统一走这里，避免逐文件重实现）。
export const isDefaultChatTitle = (title) => DEFAULT_CHAT_TITLES.has(title);

// 惰性语言词典装载。模式对齐 shared/syntax-highlighter.js 的 LAZY_LANGUAGE_LOADERS:
// 载入表冻结、在途去重、失败清挂起(下次触发可重试)。ja chunk 静态依赖 en chunk
// (兜底 spread 在模块内完成),由打包器拆成共享 chunk,无需在此处理顺序。
const LAZY_DICT_LOADERS = Object.freeze({
  zh: () => import('./i18n/zh.js'),
  en: () => import('./i18n/en.js'),
  ja: () => import('./i18n/ja.js'),
});
const lazyDictPending = new Map();
// 返回 Promise<boolean> 且永不 reject:true=词典就绪(dict[lang] 可用);
// false=不支持的语言或本次装载失败(失败不落词典、清挂起,下次触发可重试)。
// 已加载语言同步路径仍返回 Promise,调用方(main.jsx 首帧引导)统一 .then 链。
export function ensureLanguage(lang) {
  // hasOwnProperty.call 防止 'constructor'/'toString' 等原型链同名键命中
  // (Safari 14 无 Object.hasOwn,与 syntax-highlighter 同款写法)。
  // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 is the floor and Object.hasOwn is unavailable; this call is already in safe form
  if (Object.prototype.hasOwnProperty.call(dict, lang)) return Promise.resolve(true);
  // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 is the floor and Object.hasOwn is unavailable; this call is already in safe form
  const loader = Object.prototype.hasOwnProperty.call(LAZY_DICT_LOADERS, lang)
    ? LAZY_DICT_LOADERS[lang]
    : null;
  if (!loader) return Promise.resolve(false);
  let pending = lazyDictPending.get(lang);
  if (!pending) {
    pending = loader()
      .then((m) => {
        dict[lang] = m.dictZh || m.dictEn || m.dictJa;
        return true;
      })
      .catch(() => false)
      .finally(() => { lazyDictPending.delete(lang); });
    lazyDictPending.set(lang, pending);
  }
  return pending;
}

// 语言切换「最新选择胜出」门:切换是先装载后落地状态(见 ensureLanguage 注释),
// 而 ja chunk 静态依赖 en chunk,先选 ja 再选 en 时较新的 en 请求可能先完成
// (共享依赖先行就位),慢的旧 ja continuation 会覆盖状态/持久化/广播,回退到
// 用户未选择的语言。序号守卫保证只有发起时的最新选择允许落地;ensure 参数
// 仅供测试注入受控装载,生产路径恒为 ensureLanguage。
export function createLatestLanguageGate(ensure = ensureLanguage) {
  let seq = 0;
  return function switchToLatest(lang, apply) {
    const ticket = ++seq;
    ensure(lang)
      .then((ok) => {
        if (ok && ticket === seq) apply();
      })
      .catch(() => {});
  };
}

// zh-fallback readers (pet/persona/tool-common/reader/timeline): tolerate
// the lazy-load window — the boot sequence (ensureLanguageWithZhFallback)
// fills the preferred language, and zh when that chunk fails; node-side
// component tests may render before any load, hence the {} tail.
export function zhDict() {
  return dict.zh || {};
}

// Boot language sequence shared by every entry (main/pet/reader): load the
// preferred language; if its chunk cannot load (offline web, stale hashed
// chunk after a redeploy), load zh so the render-path leg
// `dict[language] || zhDict()` degrades to Chinese copy instead of
// undefined. Returns the ready language ('en'|'ja'|'zh'), or null when even
// zh failed — nothing renderable is left, so callers (the three entries,
// via shared/entry-boot.js) must paint the static failure/retry screen
// instead of mounting dictionary-dependent components (round-49 M2).
export async function ensureLanguageWithZhFallback(lang) {
  if (await ensureLanguage(lang)) return lang;
  if (await ensureLanguage('zh')) return 'zh';
  return null;
}

export { dict, LANG_TO_TAG, TAG_TO_LANG, languageFromLocaleTags, initialSystemLanguage, SEARCH_KEY_PROVIDERS };
