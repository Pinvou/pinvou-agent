import React from 'react';
import { createRoot } from 'react-dom/client';
import '../../src/styles/base.css';
import { ToolOutput } from '../../src/features/tools/tool-renderers.jsx';

const replacementLines = [];
for (let line = 1; line <= 18; line += 1) {
  replacementLines.push(`-old line ${line}`, `+new line ${line}`);
}

const diffOutput = [
  '--- a/src/example.js',
  '+++ b/src/example.js',
  '@@ -1,18 +1,18 @@',
  ...replacementLines,
  '',
  'Replaced 18 occurrences in src/example.js',
  '<diagnostics file="src/example.js">',
  '  ERROR [7:3] simulated diagnostic',
  '</diagnostics>',
].join('\n');

const writeDiffOutput = [
  '--- a/notes/new file.md',
  '+++ b/notes/new file.md',
  '@@ -0,0 +1,2 @@',
  '+first line',
  '+second line',
  '',
  'Created notes/new file.md (23 bytes)',
].join('\n');

// append_file 新版输出:unified diff(尾部 context + 追加行) + 字节摘要末尾行。
const appendDiffOutput = [
  '--- a/notes/deck.html',
  '+++ b/notes/deck.html',
  '@@ -1 +1,2 @@',
  ' <html>',
  '+</html>',
  '',
  'Appended 8 bytes to notes/deck.html (7 -> 15 bytes)',
].join('\n');

// 旧 session 落盘的 append_file 输出:纯字节摘要,无 diff,走 appendBytes 兜底。
const appendLegacyOutput = 'Appended 8 bytes to notes/deck.html (7 -> 15 bytes)';

const t = {
  receiptNote: '输出已截断',
  receiptEmpty: '无输出',
  appendBytes: (created, appended, before, after) =>
    `${created ? '创建并追加' : '追加'} ${appended} 字节（${before} → ${after} 字节）`,
};

const Fixture = () => (
  <main className="mx-auto grid max-w-[900px] gap-6">
    <section data-testid="edit-output">
      <ToolOutput
        item={{ name: 'edit_file', output: diffOutput, success: true }}
        isDark={false}
        t={t}
      />
    </section>
    <section data-testid="fallback-output">
      <ToolOutput
        item={{ name: 'edit_file', output: 'Replaced 0 occurrences', success: true }}
        isDark={false}
        t={t}
      />
    </section>
    <section data-testid="write-output" className="dark rounded-lg bg-[#131314] p-3">
      <ToolOutput
        item={{ name: 'write_file', output: writeDiffOutput, success: true }}
        isDark
        t={t}
      />
    </section>
    <section data-testid="append-output" className="dark rounded-lg bg-[#131314] p-3">
      <ToolOutput
        item={{ name: 'append_file', output: appendDiffOutput, success: true }}
        isDark
        t={t}
      />
    </section>
    <section data-testid="append-legacy-output">
      <ToolOutput
        item={{ name: 'append_file', output: appendLegacyOutput, success: true }}
        isDark={false}
        t={t}
      />
    </section>
  </main>
);

createRoot(document.getElementById('root')).render(<Fixture />);
