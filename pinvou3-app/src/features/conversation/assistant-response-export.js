import { renderMarkdown } from '../../shared/markdown-renderer.js';

export const ASSISTANT_EXPORT_FORMATS = Object.freeze(['md', 'html']);

function escapeHtmlAttribute(value) {
  return String(value || '')
    .replaceAll('&', '&amp;')
    .replaceAll('"', '&quot;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;');
}

function timestampPart(date) {
  const pad = value => String(value).padStart(2, '0');
  return [
    date.getFullYear(),
    pad(date.getMonth() + 1),
    pad(date.getDate()),
    '-',
    pad(date.getHours()),
    pad(date.getMinutes()),
    pad(date.getSeconds()),
  ].join('');
}

export function assistantExportFilename(format, date = new Date()) {
  const extension = ASSISTANT_EXPORT_FORMATS.includes(format) ? format : 'md';
  return `pinvou-response-${timestampPart(date)}.${extension}`;
}

export function buildAssistantResponseExport(markdown, format, options = {}) {
  const normalized = String(markdown || '').replaceAll(/\r\n?/g, '\n').trim();
  if (format === 'md') {
    return {
      content: normalized ? `${normalized}\n` : '',
      mimeType: 'text/markdown;charset=utf-8',
    };
  }
  if (format !== 'html') throw new Error(`Unsupported assistant export format: ${format}`);

  const title = options.title || 'Pinvou response';
  const body = renderMarkdown(normalized);
  return {
    content: `<!doctype html>
<html lang="${escapeHtmlAttribute(options.language || 'zh-CN')}">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline'">
  <title>${escapeHtmlAttribute(title)}</title>
  <style>
    :root { color-scheme: light dark; font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
    body { max-width: 880px; margin: 0 auto; padding: 40px 24px 64px; color: #1f1f1f; background: #fff; line-height: 1.7; overflow-wrap: anywhere; }
    h1, h2, h3, h4 { line-height: 1.3; margin: 1.4em 0 .6em; }
    a { color: #0b57d0; }
    img { max-width: 100%; height: auto; }
    blockquote { margin-left: 0; padding-left: 1em; border-left: 3px solid #c7c7c7; color: #5f6368; }
    table { width: 100%; border-collapse: collapse; display: block; overflow-x: auto; }
    th, td { padding: 8px 10px; border: 1px solid #d5d7da; text-align: left; }
    pre { padding: 14px 16px; border-radius: 10px; overflow-x: auto; background: #f3f4f6; }
    code { font-family: "SFMono-Regular", Consolas, "Liberation Mono", monospace; font-size: .92em; }
    :not(pre) > code { padding: .12em .35em; border-radius: 5px; background: #f0f1f2; }
    @media (prefers-color-scheme: dark) {
      body { color: #e8eaed; background: #202124; }
      a { color: #8ab4f8; }
      blockquote { color: #bdc1c6; border-left-color: #5f6368; }
      th, td { border-color: #5f6368; }
      pre, :not(pre) > code { background: #292a2d; }
    }
  </style>
</head>
<body>
${body}
</body>
</html>
`,
    mimeType: 'text/html;charset=utf-8',
  };
}
