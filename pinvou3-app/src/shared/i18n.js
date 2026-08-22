// UI 词典核心:zh 全量内嵌(启动即用),en/ja 按语言惰性 chunk(见 i18n/ 目录)。
// 浏览器入口在首帧前 ensureLanguage(<初始语言>);切换语言先 ensureLanguage 再改状态。
// Node 契约测试与需要三语全量的场合 import './i18n-all.js'(聚合 shim,静态引全三语)。
// 维护约定:三语 key 集合保持一致(测试 ui_language_coverage 断言);ja 以 en 兜底
// (i18n/ja.js 内 spread);settings 详情文案已并入各语言文件(原 settings-i18n.js)。
import { dictZh } from './i18n/zh.js';

const dict = { zh: dictZh };

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

// 惰性语言词典装载。模式对齐 shared/syntax-highlighter.js 的 LAZY_LANGUAGE_LOADERS:
// 载入表冻结、在途去重、失败清挂起(下次触发可重试)。ja chunk 静态依赖 en chunk
// (兜底 spread 在模块内完成),由打包器拆成共享 chunk,无需在此处理顺序。
const LAZY_DICT_LOADERS = Object.freeze({
  en: () => import('./i18n/en.js'),
  ja: () => import('./i18n/ja.js'),
});
const lazyDictPending = new Map();
// 返回 Promise<boolean>:true=词典就绪(dict[lang] 可用);false=不支持的语言。
// 已加载语言同步路径仍返回 Promise,调用方(main.jsx 首帧引导)统一 .then 链。
export function ensureLanguage(lang) {
  if (dict[lang]) return Promise.resolve(true);
  const loader = LAZY_DICT_LOADERS[lang];
  if (!loader) return Promise.resolve(false);
  let pending = lazyDictPending.get(lang);
  if (!pending) {
    pending = loader().then((m) => {
      dict[lang] = lang === 'ja' ? m.dictJa : m.dictEn;
      return true;
    });
    pending.catch(() => {}).finally(() => { lazyDictPending.delete(lang); });
  }
  return pending;
}

if (typeof window !== 'undefined') window.__PINVOU_SHARED_I18N__ = dict;

export { dict, LANG_TO_TAG, TAG_TO_LANG, languageFromLocaleTags, initialSystemLanguage, SEARCH_KEY_PROVIDERS };
