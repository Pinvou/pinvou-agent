// 三语聚合 shim:Node 契约测试与需要全语言的场合静态 import 本文件。
// 浏览器入口不得 import 本文件(会把三语全部钉进主 chunk)——用 i18n.js 的
// ensureLanguage 按需装载。三语 key 集合一致性与 ja←en 兜底见 ui_language_coverage 测试。
import { dict } from './i18n.js';
import { dictEn } from './i18n/en.js';
import { dictJa } from './i18n/ja.js';

dict.en = dictEn;
dict.ja = dictJa;

export { dict, LANG_TO_TAG, TAG_TO_LANG, languageFromLocaleTags, initialSystemLanguage, SEARCH_KEY_PROVIDERS } from './i18n.js';
