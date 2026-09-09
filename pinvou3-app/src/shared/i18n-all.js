// 中英文聚合 shim:Node 契约测试与需要全语言的场合静态 import 本文件。
// 浏览器入口不得 import 本文件(会把英文钉进主 chunk)——用 i18n.js 的
// ensureLanguage 按需装载。英文对中文的叶子 key 覆盖由测试断言。
import { dict } from './i18n.js';
import { dictEn } from './i18n/en.js';

dict.en = dictEn;

// Only dict is re-exported: consumers of the other i18n.js exports
// (LANG_TO_TAG / TAG_TO_LANG / languageFromLocaleTags / initialSystemLanguage /
// SEARCH_KEY_PROVIDERS) import './i18n.js' directly; this aggregation shim no
// longer forwards them, avoiding unused re-exports.
export { dict } from './i18n.js';
