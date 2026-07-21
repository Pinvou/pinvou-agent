// 解析 unified diff 文本为结构化数据(无外部依赖,~150 行)。
//
// 后端 tools/diff_format.rs::make_unified_diff 用 Rust `similar` crate 生成
// 标准 unified diff(含 `--- a/...` / `+++ b/...` 头 + `@@ ... @@` hunk 头 +
// `+/-/ `(空格前缀的 context 行)。前端原本只按行着色,无行号、无并排、
// 无 hunk 结构。本模块把它解析成结构化 ParsedDiff,供 IdeDiffViewer 渲染。
//
// 容错原则:解析失败 / 截断的 receipt preview 都不能崩,降级为单列文本。

/**
 * @typedef {Object} DiffLine
 * @property {'add'|'del'|'context'|'meta'} kind
 * @property {string} text        行内容(不含 +/- 前缀)
 * @property {number|null} oldNo  旧文件行号(del/context 有,add/hunk/meta 为 null)
 * @property {number|null} newNo  新文件行号(add/context 有,del/hunk/meta 为 null)
 */

/**
 * @typedef {Object} DiffHunk
 * @property {string} header        完整 hunk 头文本,如 `@@ -1,3 +1,4 @@`
 * @property {number} oldStart      旧文件起始行号(从 1 起)
 * @property {number} oldCount      旧行数
 * @property {number} newStart      新文件起始行号
 * @property {number} newCount      新行数
 * @property {DiffLine[]} lines     该 hunk 的所有行(add/del/context)
 */

/**
 * @typedef {Object} DiffFile
 * @property {string|null} oldPath
 * @property {string|null} newPath
 * @property {DiffHunk[]} hunks
 */

/**
 * @typedef {Object} ParsedDiff
 * @property {boolean} ok          true = 成功解析出 hunks;false = 降级文本
 * @property {string|null} oldPath 顶层兼容字段:多文件场景下取首个文件的 oldPath(单文件最常见用法)
 * @property {string|null} newPath 顶层兼容字段:同上
 * @property {DiffHunk[]} hunks    顶层兼容字段:所有文件 hunks 的扁平拼接
 * @property {DiffFile[]} files    多文件分段(单文件场景下长度为 1,与顶层 oldPath/newPath/hunks 一致)
 * @property {string} summary      diff 文本末尾的非 diff 摘要行(如 "Replaced 1 occurrence in ...")
 * @property {string} trailingDiagnostics  LSP 诊断块(若后端 append),用于单独渲染
 * @property {string} raw          原始文本(降级时使用)
 * @property {string|null} omitReason  大文件被截断时的 "[diff omitted] ..." 原因
 */

const HUNK_RE = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;
const META_RE = /^(---|\+\+\+) /;

// 文件头路径提取:后端 similar::unified_diff().header() 直接输出真实路径,
// 不带时间戳,路径可含空格 —— 只按 Tab 剥 GNU diff 的 `\t<timestamp>` 后缀,
// 绝不按空格切(B3 修复:含空格文件名曾被截断成 `a/my`)。git 的 quoted path
// (`"a/my file.txt"`)剥首尾引号并还原常见转义。
function parseHeaderPath(raw) {
  let p = raw.split('\t')[0].trim();
  if (p.length >= 2 && p.startsWith('"') && p.endsWith('"')) {
    p = p.slice(1, -1).replace(/\\(["\\])/g, '$1');
  }
  return p;
}

/**
 * 把 unified diff 文本解析成结构化数据。
 * 容错:任何解析异常都返回 { ok: false, raw },不抛异常。
 */
function parseUnifiedDiff(text) {
  if (typeof text !== 'string') return { ok: false, raw: String(text ?? ''), hunks: [], files: [], summary: '', trailingDiagnostics: '', omitReason: null };
  // split('\n') 后,CRLF 输入每行末尾会残留一个 '\r'(只有 + 1 个前缀被剥,
  // 但 \r 还在 → 渲染末尾多一个不可见字符)。这里统一剥尾随 '\r'。
  const lines = text.split('\n').map((l) => l.replace(/\r$/, ''));

  // 多文件分段:H1 修复 —— 多文件 diff(`--- a/f1 / +++ b/f1 / @@ / --- a/f2 / +++ b/f2 / @@`)
  // 之前会覆写同一对 oldPath/newPath,把两个文件的 hunks 全混。改为按文件分段,
  // 遇到新的 `---`/`+++` 对且当前文件已含 hunk 时开新段。
  let pendingOldPath = null;
  let pendingNewPath = null;
  /** @type {DiffFile[]} */
  const files = [];
  /** @type {DiffFile|null} */
  let curFile = null;
  let cur = null;
  let oldNo = 0;
  let newNo = 0;
  let i_at_exit = lines.length;

  // 用 hunk header 声明的 oldCount/newCount 限制 hunk 体内行数,避免依赖行首前缀
  // 判定 hunk 结束(真空行 / tab context 行 / 截断 receipt 都会误判)。H3/M3 修复。
  let oldRemain = 0;
  let newRemain = 0;

  for (let i = 0; i < lines.length; i++) {
    const l = lines[i];
    // 文件头。B2 修复:hunk 声明行数未消费完时,`--- `/`+++ ` 开头的行是 hunk
    // 体内的 del/add 行(例如 SQL 注释 `-- old comment` 删除后形如
    // `--- old comment`),绝不能当文件头吞掉 —— 只在 hunk 已消费完
    // (或尚未进入 hunk)时才识别 META_RE。
    const inHunkBody = cur && (oldRemain > 0 || newRemain > 0);
    const meta = inHunkBody ? null : l.match(META_RE);
    if (meta) {
      // B3 修复:只按 Tab 剥 GNU 时间戳,含空格文件名保留完整(见 parseHeaderPath)。
      if (meta[1] === '---') pendingOldPath = parseHeaderPath(l.slice(4));
      else pendingNewPath = parseHeaderPath(l.slice(4));
      // 后端 make_unified_diff 一次只处理单文件,apply_patch / git diff 才会多文件;
      // 当当前文件已有 hunk 且新出现 --- 时,把 pending 落成新文件段。
      if (meta[1] === '---' && curFile && curFile.hunks.length > 0 && pendingOldPath) {
        curFile = { oldPath: null, newPath: null, hunks: [] };
        files.push(curFile);
        cur = null;
        oldRemain = 0;
        newRemain = 0;
      }
      continue;
    }
    // hunk 头
    const hm = l.match(HUNK_RE);
    if (hm) {
      // B1 修复:只有看到新的 ---/+++ 文件头(pending 非空)才分段;同文件的
      // 后续 hunk(中间无文件头)直接 append 到当前文件段,不拆成空文件头新段。
      if (!curFile || pendingOldPath != null || pendingNewPath != null) {
        if (!curFile || curFile.hunks.length > 0) {
          curFile = { oldPath: pendingOldPath, newPath: pendingNewPath, hunks: [] };
          files.push(curFile);
        } else {
          // curFile 是上方 --- 分支刚建的空段(尚无 hunk):落 pending 路径。
          curFile.oldPath = pendingOldPath;
          curFile.newPath = pendingNewPath;
        }
        pendingOldPath = null;
        pendingNewPath = null;
      }
      cur = {
        header: l,
        oldStart: parseInt(hm[1], 10),
        oldCount: hm[2] != null ? parseInt(hm[2], 10) : 1,
        newStart: parseInt(hm[3], 10),
        newCount: hm[4] != null ? parseInt(hm[4], 10) : 1,
        lines: [],
      };
      curFile.hunks.push(cur);
      oldNo = cur.oldStart;
      newNo = cur.newStart;
      // 声明行数计数(若 header 异常缺失 count,默认 1)。
      oldRemain = cur.oldCount;
      newRemain = cur.newCount;
      continue;
    }
    if (!cur) continue; // hunk 头之前的杂行跳过(空行 / 文件头已处理)
    // hunk 体
    // 用 hunk header 声明的 oldCount/newCount(oldRemain/newRemain)来判定 hunk
    // 是否还有未消费行。判定优先级:
    //   1. 显式前缀(+/-/ /\\):按对应 kind 计数(同时耗 old/new 或单边);
    //   2. hunk 体内**真空行**(0 字符)或 **tab 缩进行**(LLM/损坏 receipt 常见):
    //      只要 hunk 还有未消费行(oldRemain>0 || newRemain>0),就按 context 处理,
    //      而不是当作 hunk 结束(H3 / M3 修复);
    //   3. hunk 行已消费完且看到非 diff 行:当作 hunk 结束,尝试往前找下一个 hunk
    //      头继续解析,只有后续确实没有 hunk 头时才落 summary(H2 修复)。
    if (l.startsWith('+')) {
      cur.lines.push({ kind: 'add', text: l.slice(1), oldNo: null, newNo: newNo++ });
      newRemain = Math.max(0, newRemain - 1);
    } else if (l.startsWith('-')) {
      cur.lines.push({ kind: 'del', text: l.slice(1), oldNo: oldNo++, newNo: null });
      oldRemain = Math.max(0, oldRemain - 1);
    } else if (l.startsWith(' ')) {
      cur.lines.push({ kind: 'context', text: l.slice(1), oldNo: oldNo++, newNo: newNo++ });
      oldRemain = Math.max(0, oldRemain - 1);
      newRemain = Math.max(0, newRemain - 1);
    } else if (/^\\ No newline at end of file/.test(l)) {
      // 标记行,跳过(不显示行号)
      cur.lines.push({ kind: 'meta', text: l, oldNo: null, newNo: null });
    } else if ((l === '' || l.startsWith('\t')) && (oldRemain > 0 || newRemain > 0)) {
      // H3 / M3:真空行(0 字符)或 tab 缩进的 context 行,在 hunk 声明的行数
      // 计数未满时按 context 行处理(同时耗 old/new)。
      cur.lines.push({ kind: 'context', text: l.startsWith('\t') ? l.slice(1) : '', oldNo: oldNo++, newNo: newNo++ });
      oldRemain = Math.max(0, oldRemain - 1);
      newRemain = Math.max(0, newRemain - 1);
    } else {
      // 异常行(hunk 计数已满 / 真正的非 diff 行)。
      // H2 修复:不要直接 break + 把后续全切进 summary。先扫看后面是否还有
      // 合法 hunk 头(可能是夹在中间的思考行/截断续行);若有,跳过当前异常行
      // 继续解析后续 hunk;若无,这里才是真正的 summary 起点。
      let nextHunkIdx = -1;
      for (let j = i; j < lines.length; j++) {
        if (HUNK_RE.test(lines[j])) { nextHunkIdx = j; break; }
      }
      if (nextHunkIdx > i) {
        // 跳过 [i, nextHunkIdx) 的污染段;但保留这段当 summary 仅当后面真无 hunk。
        i = nextHunkIdx - 1; // for 循环 i++ 后落到 nextHunkIdx
        // 当前 hunk 结束(不再追加行);cur 保留,让下一轮 hunk 头分支新建。
        cur = null;
        continue;
      }
      i_at_exit = i;
      break;
    }
  }

  const hunks = files.flatMap((f) => f.hunks);
  if (hunks.length === 0) {
    // H4 / H5 修复:omit 检测从「解析前对所有行扫描」改成「解析后,确实没解出
    // 任何合法 hunk 时才考虑」。这样 hunk 体内出现的 `[diff omitted] ...`(LLM
    // 误输出 / 截断 receipt 续行)不会让整个真实 diff 被丢弃。
    const omitIdx = lines.findIndex((l) => /^\[diff omitted\]/.test(l));
    if (omitIdx >= 0) {
      // H5 修复:后端 write_file 大文件布局是
      //   `format!("{summary}\n[diff omitted] {path} is too large ...")`
      // summary 在前,[diff omitted] 在后。旧 parser 只保留 omitIdx 之后的文本,
      // 丢掉了 summary("Wrote N bytes")。现在 omitReason 取整段(含 summary),
      // 让用户能看到完整的写入反馈 + 截断原因。
      return {
        ok: false,
        raw: text,
        omitReason: lines.join('\n'),
        summary: lines.slice(0, omitIdx).join('\n').trim(),
        files: [],
        hunks: [],
        trailingDiagnostics: '',
      };
    }
    return { ok: false, raw: text, files: [], hunks: [], summary: '', trailingDiagnostics: '', omitReason: null };
  }

  // summary + diagnostics:lastHunkEnd = 解析循环退出时的位置(break 出或自然跑完)。
  // 不能反向扫首字符判定 —— LSP 诊断块里以空格开头的行(`  foo.py:1:1 ...`)
  // 会被误判为 context 行,导致 tail 切空。
  const lastHunkEnd = i_at_exit;
  const tail = lines.slice(lastHunkEnd).join('\n').trim();
  let summary = tail;
  let trailingDiagnostics = '';
  // 切出 LSP diagnostics 块。后端可能输出两套格式:
  //   (1) 文本 header:`Diagnostics:`/`诊断:`/`── diagnostics` 起的整段;
  //   (2) XML 块:`<diagnostics file="...">...</diagnostics>`
  //     (diagnostics.rs::render,见 DeepSeek-TUI/crates/tui/src/lsp/diagnostics.rs)。
  // XML 块优先匹配到闭合标签;无闭合标签(被截断/旧格式)时取到行尾。
  const dm = tail.match(/(?:\n|^)(Diagnostics[^\n]*|LSP Diagnostics[^\n]*|诊断[^\n]*|── diagnostics[^\n]*|--- diagnostics|<diagnostics[^\n]*>)\n([\s\S]*?(?:<\/diagnostics>|$))/i);
  if (dm) {
    const cut = tail.indexOf(dm[1]);
    summary = tail.slice(0, cut).replace(/\n+$/, '');
    trailingDiagnostics = `${dm[1]}\n${dm[2].trim()}`;
  }

  // 顶层 oldPath/newPath/hunks 保持向后兼容(单文件最常见用法):取首个文件路径。
  const oldPath = files[0] && files[0].oldPath;
  const newPath = files[0] && files[0].newPath;
  return { ok: true, oldPath, newPath, hunks, files, summary, trailingDiagnostics, raw: text };
}

/**
 * 把 ParsedDiff 折算成统计:{add, del, ctx}
 */
function diffStats(parsed) {
  let add = 0, del = 0, ctx = 0;
  for (const h of parsed.hunks) {
    for (const l of h.lines) {
      if (l.kind === 'add') add++;
      else if (l.kind === 'del') del++;
      else if (l.kind === 'context') ctx++;
    }
  }
  return { add, del, ctx };
}

export { parseUnifiedDiff, diffStats };
