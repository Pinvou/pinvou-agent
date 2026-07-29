// 用户消息的展示文本由 bridge 统一拼装:text + "\n\n" + "📎 " + names.join(" · ")
// (纯附件消息则整条就是一行 "📎 …")。历史消息也持久化为该格式,因此渲染层按
// 同一约定拆回,附件才能脱离正文以独立气泡展示,新旧消息表现一致。
export function splitAttachmentLine(text) {
  const raw = String(text == null ? '' : text);
  if (raw.startsWith('📎 ') && raw.indexOf('\n') < 0) {
    return { text: '', attachments: parseNames(raw.slice(3)) };
  }
  const sep = '\n\n📎 ';
  const at = raw.lastIndexOf(sep);
  if (at >= 0 && raw.indexOf('\n', at + sep.length) < 0) {
    const attachments = parseNames(raw.slice(at + sep.length));
    if (attachments.length) return { text: raw.slice(0, at), attachments };
  }
  return { text: raw, attachments: [] };
}

export function sessionTitlePresentation(title, attachmentNames = []) {
  const parsed = splitAttachmentLine(title);
  if (!parsed.attachments.length) {
    return { text: String(title == null ? '' : title), attachments: [] };
  }
  const completeNames = attachmentNames
    .map(name => String(name || '').trim())
    .filter(Boolean);
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
  return line
    .split(' · ')
    .map(function (name) { return name.trim(); })
    .filter(Boolean);
}
