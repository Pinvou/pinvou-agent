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

const t = {
  receiptNote: '输出已截断',
  receiptEmpty: '无输出',
};

const Fixture = () => (
  <main className="mx-auto grid max-w-[900px] gap-6">
    <section data-testid="edit-output">
      <ToolOutput
        item={{ name: 'File', args: { action: 'edit' }, output: diffOutput, success: true }}
        isDark={false}
        t={t}
      />
    </section>
    <section data-testid="fallback-output">
      <ToolOutput
        item={{ name: 'File', args: { action: 'edit' }, output: 'Replaced 0 occurrences', success: true }}
        isDark={false}
        t={t}
      />
    </section>
    <section data-testid="write-output" className="dark rounded-lg bg-[#131314] p-3">
      <ToolOutput
        item={{ name: 'File', args: { action: 'write' }, output: writeDiffOutput, success: true }}
        isDark
        t={t}
      />
    </section>
  </main>
);

createRoot(document.getElementById('root')).render(<Fixture />);
