import DOMPurify from 'dompurify';
// eslint-disable-next-line import-x/namespace -- marked's ESM source uses ES2022 class fields, unparseable at this config's ES2021 floor (same as src/shared/markdown-renderer.js); the runtime bundler handles it, no impact
import { marked } from 'marked';
// 危险标签抹平与禁列表复用主渲染器的单一来源(本文件的旧副本漏了 u 标志)。
import {
  MARKDOWN_FORBID_ATTR,
  MARKDOWN_FORBID_TAGS,
  neutralizeRawDangerousTags,
} from '../../shared/markdown-renderer.js';

marked.setOptions({ gfm: true, breaks: true });

export function renderPetMarkdown(text) {
  const html = neutralizeRawDangerousTags(marked.parse(String(text || '')));
  return DOMPurify.sanitize(html, {
    FORBID_TAGS: MARKDOWN_FORBID_TAGS,
    FORBID_ATTR: MARKDOWN_FORBID_ATTR,
  });
}
