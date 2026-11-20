import DOMPurify from 'dompurify';
import { marked } from 'marked';
// 危险标签抹平与禁列表复用主渲染器的单一来源(本文件的旧副本漏了 u 标志)。
import {
  SANITIZE_OPTIONS,
  neutralizeRawDangerousTags,
} from '../../shared/markdown-renderer.js';

marked.setOptions({ gfm: true, breaks: true });

export function renderPetMarkdown(text) {
  const html = neutralizeRawDangerousTags(marked.parse(String(text || '')));
  return DOMPurify.sanitize(html, SANITIZE_OPTIONS);
}
