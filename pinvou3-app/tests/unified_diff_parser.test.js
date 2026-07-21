#!/usr/bin/env node
// 解析器单元测试:无外部依赖,纯函数行为校验。
// 运行: node tests/unified_diff_parser.test.js
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

// 源文件用 ES module export(运行时 React import,测试 vm 加载时剥 export)
const logicPath = path.join(__dirname, '..', 'src', 'features', 'tools', 'unified-diff-parser.js');
const code = fs.readFileSync(logicPath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '');
const ctx = { console };
vm.createContext(ctx);
vm.runInContext(`${code}\nthis.parseUnifiedDiff = parseUnifiedDiff;\nthis.diffStats = diffStats;`, ctx, {
  filename: logicPath,
});
const { parseUnifiedDiff, diffStats } = ctx;
// 跨 vm context 对象的 prototype 不同,deepStrictEqual 会失败;
// 拷贝成 plain 对象(主进程 Object.prototype)再比。
const stats = (parsed) => JSON.parse(JSON.stringify(diffStats(parsed)));

let pass = 0, fail = 0;
function test(name, fn) {
  try { fn(); pass++; console.log(`  ✓ ${name}`); }
  catch (e) { fail++; console.error(`  ✗ ${name}\n    ${e.message}`); }
}

// 1. 标准 edit_file 输出(单 hunk,1 del + 1 add)
test('parse edit_file single-line replacement', () => {
  const out = [
    '--- a/edit_me.txt',
    '+++ b/edit_me.txt',
    '@@ -1 +1 @@',
    '-hello world',
    '+hi world',
    'Replaced 1 occurrence in /tmp/edit_me.txt',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  assert.strictEqual(p.ok, true);
  assert.strictEqual(p.oldPath, 'a/edit_me.txt');
  assert.strictEqual(p.newPath, 'b/edit_me.txt');
  assert.strictEqual(p.hunks.length, 1);
  const h = p.hunks[0];
  assert.strictEqual(h.oldStart, 1);
  assert.strictEqual(h.oldCount, 1);
  assert.strictEqual(h.newStart, 1);
  assert.strictEqual(h.newCount, 1);
  assert.strictEqual(h.lines.length, 2);
  assert.strictEqual(h.lines[0].kind, 'del');
  assert.strictEqual(h.lines[0].text, 'hello world');
  assert.strictEqual(h.lines[0].oldNo, 1);
  assert.strictEqual(h.lines[0].newNo, null);
  assert.strictEqual(h.lines[1].kind, 'add');
  assert.strictEqual(h.lines[1].text, 'hi world');
  assert.strictEqual(h.lines[1].oldNo, null);
  assert.strictEqual(h.lines[1].newNo, 1);
  assert.strictEqual(p.summary, 'Replaced 1 occurrence in /tmp/edit_me.txt');
  assert.strictEqual(p.trailingDiagnostics, '');
  assert.deepStrictEqual(stats(p), { add: 1, del: 1, ctx: 0 });
});

// 2. write_file 新建文件(全 add,无 del)
test('parse write_file new file creation', () => {
  const out = [
    '--- a/output.txt',
    '+++ b/output.txt',
    '@@ -0,0 +1 @@',
    '+test content',
    'Created /tmp/output.txt (12 bytes)',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  assert.strictEqual(p.ok, true);
  assert.strictEqual(p.hunks[0].oldStart, 0);
  assert.strictEqual(p.hunks[0].oldCount, 0);
  assert.strictEqual(p.hunks[0].newStart, 1);
  assert.strictEqual(p.hunks[0].lines.length, 1);
  assert.strictEqual(p.hunks[0].lines[0].kind, 'add');
  assert.strictEqual(p.hunks[0].lines[0].newNo, 1);
  assert.strictEqual(p.summary, 'Created /tmp/output.txt (12 bytes)');
});

// 3. 多 hunk diff(context 行 + 行号递增)
test('parse multi-hunk diff with context lines', () => {
  const out = [
    '--- a/foo.js',
    '+++ b/foo.js',
    '@@ -1,3 +1,3 @@',
    ' line1',
    '-old2',
    '+new2',
    ' line3',
    '@@ -10,2 +10,2 @@',
    ' line10',
    '-old11',
    '+new11',
    'Replaced 2 occurrences in foo.js',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  assert.strictEqual(p.hunks.length, 2);
  // 第一 hunk: 1 context + 1 del + 1 add + 1 context
  assert.strictEqual(p.hunks[0].lines.length, 4);
  assert.strictEqual(p.hunks[0].lines[0].kind, 'context');
  assert.strictEqual(p.hunks[0].lines[0].oldNo, 1);
  assert.strictEqual(p.hunks[0].lines[0].newNo, 1);
  assert.strictEqual(p.hunks[0].lines[1].kind, 'del');
  assert.strictEqual(p.hunks[0].lines[1].oldNo, 2);
  assert.strictEqual(p.hunks[0].lines[2].kind, 'add');
  assert.strictEqual(p.hunks[0].lines[2].newNo, 2);
  assert.strictEqual(p.hunks[0].lines[3].kind, 'context');
  assert.strictEqual(p.hunks[0].lines[3].oldNo, 3);
  assert.strictEqual(p.hunks[0].lines[3].newNo, 3);
  // 第二 hunk: 行号从 10 续起
  assert.strictEqual(p.hunks[1].oldStart, 10);
  assert.strictEqual(p.hunks[1].lines[0].oldNo, 10);
  assert.deepStrictEqual(stats(p), { add: 2, del: 2, ctx: 3 });
});

// 4. LSP diagnostics 块单独切出
test('parse splits LSP diagnostics block from summary', () => {
  const out = [
    '--- a/x.py',
    '+++ b/x.py',
    '@@ -1 +1 @@',
    '-x = 1',
    '+x = 2',
    'Replaced 1 occurrence in x.py',
    'Diagnostics (1):',
    '  x.py:1:1 - undefined name "y" [pyflakes]',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  assert.ok(p.summary.includes('Replaced 1 occurrence'));
  assert.ok(!p.summary.includes('Diagnostics'));
  assert.ok(p.trailingDiagnostics.includes('Diagnostics (1):'));
  assert.ok(p.trailingDiagnostics.includes('undefined name'));
});

// 5. 大文件被截断("[diff omitted]")
test('parse handles [diff omitted] large file', () => {
  const out = '[diff omitted] /tmp/big.txt is too large for an inline write_file diff (old=50000 bytes, new=60000 bytes, limit=32768 bytes). Use read_file with line ranges to inspect it.';
  const p = parseUnifiedDiff(out);
  assert.strictEqual(p.ok, false);
  assert.ok(p.omitReason);
  assert.ok(p.omitReason.includes('too large'));
  assert.strictEqual(p.hunks.length, 0);
});

// 6. 非_diff 文本安全降级
test('parse returns ok=false for non-diff text', () => {
  const p = parseUnifiedDiff('Just some random text\nno diff markers');
  assert.strictEqual(p.ok, false);
  assert.strictEqual(p.raw, 'Just some random text\nno diff markers');
});

// 7. fuzzy 匹配摘要保留(后端在 summary 后缀 fuzzy 注解)
test('parse preserves fuzzy annotation in summary', () => {
  const out = [
    '--- a/x.txt',
    '+++ b/x.txt',
    '@@ -1 +1 @@',
    '-foo',
    '+bar',
    'Replaced 1 occurrence in x.txt (fuzzy punctuation match — typographic quotes/dashes normalized)',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  assert.ok(p.summary.includes('fuzzy punctuation match'));
});

// 8. "No newline at end of file" 标记行不报行号
test('parse handles "No newline at end of file" marker', () => {
  const out = [
    '--- a/x.txt',
    '+++ b/x.txt',
    '@@ -1 +1 @@',
    '-old',
    '\\ No newline at end of file',
    '+new',
    'Replaced 1 occurrence',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  const meta = p.hunks[0].lines.find((l) => l.kind === 'meta');
  assert.ok(meta);
  assert.strictEqual(meta.oldNo, null);
  assert.strictEqual(meta.newNo, null);
});

// 9. 截断的 receipt preview(只有部分 diff)不崩
test('parse does not crash on truncated preview', () => {
  const truncated = '--- a/x.txt\n+++ b/x.txt\n@@ -1 +1 @@\n-old\n+ne';
  const p = parseUnifiedDiff(truncated);
  // 截断的 add 行只到 "+ne" 也应该被解析,后续可能没 summary
  assert.strictEqual(p.ok, true);
  assert.strictEqual(p.hunks[0].lines.length, 2);
});

// 10. 空字符串 / 非字符串安全处理
test('parse safe on null/empty input', () => {
  assert.strictEqual(parseUnifiedDiff('').ok, false);
  assert.strictEqual(parseUnifiedDiff(null).ok, false);
  assert.strictEqual(parseUnifiedDiff(undefined).ok, false);
});

// 11. 后端真实输出 —— `format!("{diff}\n{summary}")` 会在 diff body 与 summary
// 间插一个真空行。parser 不应把它当成 context 行(否则会渲染出幽灵空行 +
// 末尾行号 off-by-one)。
test('parse does not treat blank line between diff and summary as context', () => {
  const out = [
    '--- a/foo.js',
    '+++ b/foo.js',
    '@@ -1 +1 @@',
    '-old',
    '+new',
    '',
    'Replaced 1 occurrence in foo.js',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  assert.strictEqual(p.ok, true);
  assert.strictEqual(p.hunks[0].lines.length, 2);
  assert.strictEqual(p.hunks[0].lines[0].kind, 'del');
  assert.strictEqual(p.hunks[0].lines[1].kind, 'add');
  // 不应多出一条 context 行
  assert.ok(!p.hunks[0].lines.some((l) => l.kind === 'context'));
  assert.strictEqual(p.summary, 'Replaced 1 occurrence in foo.js');
  assert.strictEqual(p.trailingDiagnostics, '');
  // 行号也不应 off-by-one:add 行还是 newNo=1
  assert.strictEqual(p.hunks[0].lines[1].newNo, 1);
  assert.deepStrictEqual(stats(p), { add: 1, del: 1, ctx: 0 });
});

// 12. 后端 diagnostics.rs::render 输出 `<diagnostics file="...">...</diagnostics>`
// XML 块。parser 应把整段切到 trailingDiagnostics,summary 里不应残留标签。
test('parse splits LSP <diagnostics> XML block from summary', () => {
  const out = [
    '--- a/x.rs',
    '+++ b/x.rs',
    '@@ -1 +1 @@',
    '-let x = 1',
    '+let x = 2',
    '',
    'Replaced 1 occurrence in x.rs',
    '<diagnostics file="crates/tui/src/x.rs">',
    '  ERROR [12:8] missing semicolon',
    '  ERROR [13:1] expected `,`, found `}`',
    '</diagnostics>',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  assert.ok(p.ok);
  assert.ok(p.summary.includes('Replaced 1 occurrence'));
  assert.ok(!p.summary.includes('<diagnostics'));
  assert.ok(p.trailingDiagnostics.includes('<diagnostics file="crates/tui/src/x.rs">'));
  assert.ok(p.trailingDiagnostics.includes('ERROR [12:8]'));
  assert.ok(p.trailingDiagnostics.includes('</diagnostics>'));
});

// 13. CRLF 输入:每行尾随 '\r' 必须被剥掉,否则单行末尾会多一个不可见 '\r'。
test('parse strips trailing CR from CRLF diff lines', () => {
  const input = '--- a/file\r\n+++ b/file\r\n@@ -1,1 +1,1 @@\r\n-old\r\n+new\r\n';
  const p = parseUnifiedDiff(input);
  assert.strictEqual(p.ok, true);
  assert.strictEqual(p.hunks.length, 1);
  // 每条 add/del 行的 text 都不应保留尾随 '\r'
  assert.strictEqual(
    p.hunks[0].lines.every((l) => !l.text.endsWith('\r')),
    true,
  );
  // 具体断言 del/add 内容
  const del = p.hunks[0].lines.find((l) => l.kind === 'del');
  const add = p.hunks[0].lines.find((l) => l.kind === 'add');
  assert.strictEqual(del.text, 'old');
  assert.strictEqual(add.text, 'new');
});

// 14. H1 多文件 diff:之前 parser 覆写同一对 oldPath/newPath,把两个文件的
// hunks 全混(只保留最后一个文件路径)。修复后应按文件分段。
test('parse multi-file diff into per-file segments (H1)', () => {
  const out = [
    '--- a/file1.txt',
    '+++ b/file1.txt',
    '@@ -1,2 +1,2 @@',
    '-old1',
    '+new1',
    ' line1ctx',
    '--- a/file2.txt',
    '+++ b/file2.txt',
    '@@ -1,2 +1,2 @@',
    ' line2ctx',
    '-old2',
    '+new2',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  assert.strictEqual(p.ok, true);
  // 必须有 files 数组,长度恰好为 2
  assert.ok(Array.isArray(p.files));
  assert.strictEqual(p.files.length, 2);
  // 每段独立保留路径
  assert.strictEqual(p.files[0].oldPath, 'a/file1.txt');
  assert.strictEqual(p.files[0].newPath, 'b/file1.txt');
  assert.strictEqual(p.files[1].oldPath, 'a/file2.txt');
  assert.strictEqual(p.files[1].newPath, 'b/file2.txt');
  // 每段独立保留 hunk 行(不能全混)
  assert.strictEqual(p.files[0].hunks.length, 1);
  assert.strictEqual(p.files[1].hunks.length, 1);
  assert.strictEqual(p.files[0].hunks[0].lines.length, 3);
  assert.strictEqual(p.files[1].hunks[0].lines.length, 3);
  // 顶层兼容:旧消费者读 p.oldPath/newPath 仍取首文件
  assert.strictEqual(p.oldPath, 'a/file1.txt');
  assert.strictEqual(p.newPath, 'b/file1.txt');
  // 顶层 hunks 应是所有段的扁平拼接(2 个 hunk)
  assert.strictEqual(p.hunks.length, 2);
  // 统计聚合仍正确
  assert.deepStrictEqual(stats(p), { add: 2, del: 2, ctx: 2 });
});

// 15. H2 i_at_exit 状态泄漏:hunk 之间夹一行非 diff 文本,旧 parser 会把后续
// hunk header / del / add 全切进 summary。修复后第二个 hunk 仍应被解析。
test('parse recovers subsequent hunk after a stray non-diff line (H2)', () => {
  const out = [
    '--- a/f',
    '+++ b/f',
    '@@ -1 +1 @@',
    '-a',
    '+b',
    'THIS_IS_A_BAD_LINE',
    '@@ -10 +10 @@',
    '-c',
    '+d',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  assert.strictEqual(p.ok, true);
  assert.strictEqual(p.hunks.length, 2);
  assert.strictEqual(p.hunks[0].lines.length, 2);
  assert.strictEqual(p.hunks[1].lines.length, 2);
  // 第二个 hunk 的内容不应被丢进 summary
  const del2 = p.hunks[1].lines.find((l) => l.kind === 'del');
  const add2 = p.hunks[1].lines.find((l) => l.kind === 'add');
  assert.strictEqual(del2.text, 'c');
  assert.strictEqual(add2.text, 'd');
  assert.strictEqual(del2.oldNo, 10);
  assert.strictEqual(add2.newNo, 10);
  // summary 不应包含 hunk header / diff 行
  assert.ok(!p.summary.includes('@@ -10 +10 @@'));
  assert.ok(!p.summary.includes('-c'));
  assert.ok(!p.summary.includes('+d'));
  // 污染行本身是否落到 summary 不做硬约束(允许被丢弃),关键是后续 hunk 不丢
});

// 16. H3 hunk 体内真空行(0 字符):GNU diff 规范用 " \n"(1 字符空格),但
// 损坏/截断/某些非标准工具可能输出 \n\n(0 字符)。hunk 声明的行数计数未满时
// 必须按 context 处理,不能 break 把后续行吞进 summary。
test('parse treats blank line inside hunk body as context while count>0 (H3)', () => {
  const out = [
    '--- a/f',
    '+++ b/f',
    '@@ -1,3 +1,3 @@',
    ' keep1',
    '',
    ' keep3',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  assert.strictEqual(p.ok, true);
  assert.strictEqual(p.hunks.length, 1);
  // 必须保留 3 行(2 个真实 context + 1 个空 context),不能只剩 1 行
  assert.strictEqual(p.hunks[0].lines.length, 3);
  assert.strictEqual(p.hunks[0].lines[0].kind, 'context');
  assert.strictEqual(p.hunks[0].lines[0].text, 'keep1');
  // 中间的真空行被当作空 context 行
  assert.strictEqual(p.hunks[0].lines[1].kind, 'context');
  assert.strictEqual(p.hunks[0].lines[1].text, '');
  assert.strictEqual(p.hunks[0].lines[2].kind, 'context');
  assert.strictEqual(p.hunks[0].lines[2].text, 'keep3');
  // 'keep3' 不应落到 summary 里
  assert.ok(!p.summary.includes('keep3'));
  assert.deepStrictEqual(stats(p), { add: 0, del: 0, ctx: 3 });
});

// 17. M3 tab 缩进 context 行:LLM 生成的 patch 有时用 tab 缩进 context 行
// (而非严格的单空格)。hunk 行数计数未满时应按 context 处理,而不是 break。
test('parse treats tab-indented line inside hunk as context (M3)', () => {
  const out = [
    '--- a/f.js',
    '+++ b/f.js',
    '@@ -1 +1 @@',
    '\tconst x = 1;',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  assert.strictEqual(p.ok, true);
  assert.strictEqual(p.hunks.length, 1);
  assert.strictEqual(p.hunks[0].lines.length, 1);
  assert.strictEqual(p.hunks[0].lines[0].kind, 'context');
  assert.strictEqual(p.hunks[0].lines[0].text, 'const x = 1;');
  // diff 行不应落到 summary 里
  assert.ok(!p.summary.includes('const x'));
});

// 18. H4 hunk 体内出现 [diff omitted] 行:旧 parser 解析前对所有行扫 omit,
// hunk 体内任何无前缀的 [diff omitted] 行让整个 diff 被丢弃。修复后:解析出
// 合法 hunk 时不再走 omit 分支。
test('parse does not treat [diff omitted] inside hunk as omit (H4)', () => {
  const out = [
    '--- a/f',
    '+++ b/f',
    '@@ -1 +1 @@',
    '-a',
    '[diff omitted] real summary',
    '+b',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  // 关键断言:ok=true,真实 diff 不被整体吞掉(不进入 omit 分支)
  assert.strictEqual(p.ok, true);
  assert.ok(!p.omitReason, 'must not enter omit branch when valid hunks exist');
  assert.strictEqual(p.hunks.length, 1);
  // del 行一定在
  const del = p.hunks[0].lines.find((l) => l.kind === 'del');
  assert.strictEqual(del.text, 'a');
  // hunk 体内的 [diff omitted] 干扰行不应作为 context 行混进 diff
  assert.ok(!p.hunks[0].lines.some((l) => l.text && l.text.includes('diff omitted')));
});

// 19. H5 write_file 大文件真实布局:后端输出 `format!("{summary}\n[diff omitted] ...")`,
// summary 在前(Wrote N bytes),[diff omitted] 在后。旧 parser omitReason 只保留
// [diff omitted] 之后的文本,丢掉 summary。修复后:omitReason 含整段,summary 单独保留。
test('parse preserves write_file summary in omit branch (H5)', () => {
  const out = [
    'Wrote 100000 bytes to /tmp/big.txt',
    '[diff omitted] /tmp/big.txt is too large for an inline write_file diff (old=50000 bytes, new=60000 bytes, limit=32768 bytes). Use read_file with line ranges to inspect it.',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  assert.strictEqual(p.ok, false);
  assert.ok(p.omitReason);
  // omitReason 必须含 summary 行
  assert.ok(p.omitReason.includes('Wrote 100000 bytes'));
  assert.ok(p.omitReason.includes('too large'));
  // summary 字段也单独保留 summary 行(供 DiffView 分别渲染)
  assert.strictEqual(p.summary, 'Wrote 100000 bytes to /tmp/big.txt');
  assert.strictEqual(p.hunks.length, 0);
});

// 20. M1 文件头时间戳剥离:GNU diff / git diff 的 `--- a/file.txt\t<timestamp>`
// 行带 tab 分隔的时间戳,parser 应只保留路径部分,不带时间戳脏数据。
test('parse strips timestamp from file header (M1)', () => {
  const out = [
    '--- a/file.txt\t2024-01-01 12:00:00.000000000 +0000',
    '+++ b/file.txt\t2024-01-01 12:00:01.000000000 +0000',
    '@@ -1 +1 @@',
    '-old',
    '+new',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  assert.strictEqual(p.ok, true);
  assert.strictEqual(p.oldPath, 'a/file.txt');
  assert.strictEqual(p.newPath, 'b/file.txt');
  // 路径里绝不能残留时间戳
  assert.ok(!p.oldPath.includes('2024'));
  assert.ok(!p.newPath.includes('2024'));
  assert.ok(!p.oldPath.includes('\t'));
});

// B1: 同文件多个 hunk(中间无新文件头)不得拆成多个文件段
test('parse keeps same-file multiple hunks in one file segment (B1)', () => {
  const out = [
    '--- a/foo.js',
    '+++ b/foo.js',
    '@@ -1,1 +1,1 @@',
    '-old1',
    '+new1',
    '@@ -10,1 +10,1 @@',
    '-old10',
    '+new10',
    'Replaced 2 occurrences in foo.js',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  assert.strictEqual(p.ok, true);
  assert.strictEqual(p.files.length, 1, '同文件多 hunk 不得拆段');
  assert.strictEqual(p.files[0].oldPath, 'a/foo.js');
  assert.strictEqual(p.files[0].newPath, 'b/foo.js');
  assert.strictEqual(p.files[0].hunks.length, 2);
  assert.strictEqual(p.hunks.length, 2);
  assert.deepStrictEqual(stats(p), { add: 2, del: 2, ctx: 0 });
  assert.strictEqual(p.summary, 'Replaced 2 occurrences in foo.js');
});

// B2: hunk 体内以 `-- ` / `++ ` 开头的内容(如 SQL 注释)不得被当文件头吞掉
test('parse does not treat --- / +++ lines inside hunk body as file headers (B2)', () => {
  const out = [
    '--- a/query.sql',
    '+++ b/query.sql',
    '@@ -1,2 +1,2 @@',
    ' SELECT 1;',
    '--- old SQL comment',
    '+++ new SQL comment',
    'Modified query.sql',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  assert.strictEqual(p.ok, true);
  assert.strictEqual(p.files.length, 1, 'hunk 体内 --- 不得触发分段');
  const h = p.hunks[0];
  assert.strictEqual(h.lines.length, 3);
  assert.strictEqual(h.lines[0].kind, 'context');
  assert.strictEqual(h.lines[1].kind, 'del');
  assert.strictEqual(h.lines[1].text, '-- old SQL comment');
  assert.strictEqual(h.lines[1].oldNo, 2);
  assert.strictEqual(h.lines[2].kind, 'add');
  assert.strictEqual(h.lines[2].text, '++ new SQL comment');
  assert.strictEqual(h.lines[2].newNo, 2);
  assert.deepStrictEqual(stats(p), { add: 1, del: 1, ctx: 1 });
});

// B3: 含空格的文件名不得被截断(后端 similar 直接输出真实路径,无时间戳)
test('parse preserves filename with spaces (B3)', () => {
  const out = [
    '--- a/my file.txt',
    '+++ b/my file.txt',
    '@@ -1 +1 @@',
    '-old',
    '+new',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  assert.strictEqual(p.ok, true);
  assert.strictEqual(p.oldPath, 'a/my file.txt');
  assert.strictEqual(p.newPath, 'b/my file.txt');
});

// B3 变体:git quoted path 剥引号
test('parse unquotes git quoted path (B3)', () => {
  const out = [
    '--- "a/my file.txt"',
    '+++ "b/my file.txt"',
    '@@ -1 +1 @@',
    '-old',
    '+new',
  ].join('\n');
  const p = parseUnifiedDiff(out);
  assert.strictEqual(p.ok, true);
  assert.strictEqual(p.oldPath, 'a/my file.txt');
  assert.strictEqual(p.newPath, 'b/my file.txt');
});

console.log(`\n${pass} passed, ${fail} failed`);
if (fail > 0) process.exit(1);
