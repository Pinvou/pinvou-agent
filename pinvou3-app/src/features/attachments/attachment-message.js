// 用户消息的展示文本由 bridge 统一拼装为 `正文\n\n📎 <JSON 文件名数组>`。
// JSON 数组能无损表达合法文件名中的分隔符与空格；旧版 `name · name` 记录仍兼容。
export function formatAttachmentDisplayText(text, attachmentNames = []) {
  const body = String(text == null ? '' : text);
  const names = attachmentNames
    .map(name => String(name == null ? '' : name))
    .filter(Boolean);
  if (!names.length) return body;
  const attachmentLine = `📎 ${JSON.stringify(names)}`;
  return body.trim() ? `${body}\n\n${attachmentLine}` : attachmentLine;
}

export function splitAttachmentLine(text) {
  const raw = String(text == null ? '' : text);
  if (raw.startsWith('📎 ') && !raw.includes('\n')) {
    return { text: '', attachments: parseNames(raw.slice(3)) };
  }
  const sep = '\n\n📎 ';
  const at = raw.lastIndexOf(sep);
  if (at >= 0 && !raw.includes('\n', at + sep.length)) {
    const attachments = parseNames(raw.slice(at + sep.length));
    if (attachments.length) return { text: raw.slice(0, at), attachments };
  }
  return { text: raw, attachments: [] };
}

export function sessionTitlePresentation(title, attachmentNames = []) {
  const raw = String(title == null ? '' : title);
  const parsed = splitAttachmentLine(title);
  const completeNames = attachmentNames
    .map(name => String(name == null ? '' : name))
    .filter(Boolean);
  if (!parsed.attachments.length) {
    const markerAt = raw.startsWith('📎 ') ? 0 : raw.lastIndexOf('\n\n📎 ');
    if (completeNames.length && markerAt >= 0) {
      return {
        text: markerAt === 0 ? '' : raw.slice(0, markerAt).trim(),
        attachments: completeNames,
      };
    }
    return { text: raw, attachments: [] };
  }
  return {
    text: parsed.text.trim(),
    attachments: completeNames.length ? completeNames : parsed.attachments,
  };
}

export function sessionTitlePlainText(presentation) {
  const text = String(presentation?.text || '').trim();
  const attachments = presentation?.attachments || [];
  return [text, ...attachments].filter(Boolean).join(' ');
}

function parseNames(line) {
  if (line.trimStart().startsWith('[')) {
    try {
      const names = JSON.parse(line);
      if (Array.isArray(names) && names.every(name => typeof name === 'string')) {
        return names.filter(Boolean);
      }
    } catch { /* non-JSON lines fall back to the legacy format */ }
  }
  // Compatibility for transcripts written before JSON attachment markers.
  return line
    .split(' · ')
    .map(function (name) { return name.trim(); })
    .filter(Boolean);
}
