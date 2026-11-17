export function normalizeRichTextLabel(text) {
  return text.replaceAll(/[*_~`\s]/g, '');
}

// Strip complete HTML tags in linear time for visible-label comparison. An
// unmatched '<' stays literal; no markup is attached to a document.
export function visibleMarkupLabel(text) {
  let result = '';
  let position = 0;
  while (position < text.length) {
    const start = text.indexOf('<', position);
    if (start < 0) break;
    const end = text.indexOf('>', start + 1);
    if (end < 0) break;
    result += text.slice(position, start);
    position = end + 1;
  }
  return result + text.slice(position);
}
