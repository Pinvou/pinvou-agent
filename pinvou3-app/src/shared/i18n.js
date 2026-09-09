// UI 词典核心:zh 全量内嵌(启动即用),en 按语言惰性 chunk(见 i18n/ 目录)。
// 浏览器入口在首帧前 ensureLanguage(<初始语言>);切换语言先 ensureLanguage 再改状态。
// Node 契约测试与需要全量词典的场合 import './i18n-all.js'(聚合 shim)。
// 维护约定:en 覆盖 zh 的全部叶子 key(parity 断言在 ui_language_coverage);
// settings 详情文案已并入各语言文件(原 settings-i18n.js)。
import { dictZh } from './i18n/zh.js';

const dict = { zh: dictZh };

// UI 语言 ↔ 后端 UserPrefs language(BCP 47 tag)映射。加语言时三处同步:
// 这里 / dict / Rust prefs.rs Language 枚举
const LANG_TO_TAG = { zh: 'zh-Hans', en: 'en' };
const TAG_TO_LANG = { 'zh-Hans': 'zh', 'en': 'en' };
function languageFromLocaleTags(localeTags, fallback = 'en') {
  const locales = Array.isArray(localeTags) ? localeTags : [localeTags];
  const locale = locales.find((value) => typeof value === 'string' && value.trim());
  if (!locale) return fallback;
  const primary = locale.trim().split(/[-_.@:]/, 1)[0].toLowerCase();
  if (primary === 'zh') return 'zh';
  if (primary === 'en') return 'en';
  // 当前只提供中文和英文；系统首选语言不受支持时使用英文。
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

// 后端中英文「新会话」兜底标题（platform/{tauri,web}/bridge.js BT_TABLE 的
// newChatFallbackTitle 同款字面量,后端按创建时的 UI 语言落盘其一)。显示层
// 把任意一种哨兵映射成当前语言的「新对话」文案;集合与语言词典装载进度无关,
// 不能从 dict 惰性派生（zh 主用户不会装载 en chunk）。
export const DEFAULT_CHAT_TITLES = new Set(['新对话', 'New chat']);

// 惰性语言词典装载。模式对齐 shared/syntax-highlighter.js 的 LAZY_LANGUAGE_LOADERS:
// 载入表冻结、在途去重、失败清挂起(下次触发可重试)。
const LAZY_DICT_LOADERS = Object.freeze({
  en: () => import('./i18n/en.js'),
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
        dict[lang] = m.dictEn;
        return true;
      })
      .catch(() => false)
      .finally(() => { lazyDictPending.delete(lang); });
    lazyDictPending.set(lang, pending);
  }
  return pending;
}

// 语言切换「最新选择胜出」门:切换是先装载后落地状态(见 ensureLanguage 注释),
// 慢的旧装载 continuation 可能覆盖状态/持久化/广播,回退到用户未选择的语言。
// 序号守卫保证只有发起时的最新选择允许落地;ensure 参数
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

if (typeof window !== 'undefined') window.__PINVOU_SHARED_I18N__ = dict;

export { dict, LANG_TO_TAG, TAG_TO_LANG, languageFromLocaleTags, initialSystemLanguage, SEARCH_KEY_PROVIDERS };
