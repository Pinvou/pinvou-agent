// 测试 tauri-bridge.js 的 renderMarkdown 危险标签抹平。
//
// 起因:Pinvou 表格 cell 里 LLM 写"在同一个 <script> 标签内"会让 marked 透传成真 HTML,
// 浏览器 HTML 解析器把 <script> 后的内容(包括 | 列分隔和后续 cell)全卷进 script 元素,
// DOMPurify 再整段剥掉 → 用户看到表格后几个 cell "空掉"。修法:marked.parse 之后用正则
// 把 script/style/iframe/object/embed/link/meta 标签 escape 成 &lt;...&gt;。
//
// 跑法:`node --test pinvou3-app/tests/render_markdown.test.js`。
// 改 renderMarkdown 后跑一次确认 5 个 case 仍 PASS。
//
// 注:本测试只验 marked + neutralize 两层(后者拷在这里),不验 DOMPurify。
// DOMPurify 在 node 里无 DOM 跑不了,真测试请在浏览器/Tauri webview。

var marked = require("../src/vendor/marked.min.js");

// === 与 tauri-bridge.js 中定义保持一致 ===
var DANGEROUS_TAGS_RE = /<(\/?(?:script|style|iframe|object|embed|link|meta)\b[^>]*)>/gi;
function neutralizeRawDangerousTags(html) {
  return html.replace(DANGEROUS_TAGS_RE, function (_, inner) {
    return "&lt;" + inner + "&gt;";
  });
}
function render(md) {
  marked.setOptions({ gfm: true, breaks: true, headerIds: false, mangle: false });
  return neutralizeRawDangerousTags(marked.parse(md || ""));
}

var pass = 0,
  fail = 0;
function check(name, cond, html) {
  if (cond) {
    pass++;
    console.log("✓ " + name);
  } else {
    fail++;
    console.log("✗ " + name);
    console.log("  got:", JSON.stringify(html));
  }
}

// Case 1: 复现 screenshot ——表格 cell 含裸 <script> 不能吃掉后续列
var h1 = render(
  [
    "| Finding | Severity | Status | User Decision |",
    "|---|---|---|---|",
    "| 两步写入的 JS 代码在同一个 <script> 标签内 | CRITICAL | RAISED | 待用户拍 |",
  ].join("\n"),
);
check(
  "table cell with raw <script> preserves后续 cells",
  h1.indexOf("CRITICAL") >= 0 &&
    h1.indexOf("RAISED") >= 0 &&
    h1.indexOf("&lt;script") >= 0 &&
    h1.indexOf("<script>") < 0,
  h1,
);

// Case 2: 反引号包的 `<script>` 不能被双重转义(应渲染成代码块里的字面量 <script>)
var h2 = render("`<script>` 标签");
check(
  "inline code `<script>` not double-escaped",
  h2.indexOf("&amp;lt;") < 0 && h2.indexOf("&lt;script&gt;") >= 0,
  h2,
);

// Case 3: 裸 <iframe> 也要被抹平
var h3 = render('before <iframe src="evil"></iframe> after');
check(
  "raw <iframe> neutralized",
  h3.indexOf("&lt;iframe") >= 0 &&
    h3.indexOf("&lt;/iframe") >= 0 &&
    h3.indexOf("<iframe") < 0,
  h3,
);

// Case 4: 合法的 <br> 等 inline HTML 不能被误伤
var h4 = render("line1<br>line2");
check("legitimate <br> survives", h4.indexOf("<br>") >= 0, h4);

// Case 5: marked 自己产出的结构标签(<h1>, <table>, <td>)不能被误伤
var h5 = render("# Title\n\n- bullet\n\n| a | b |\n|---|---|\n| 1 | 2 |");
check(
  "marked output structure preserved",
  /<h1[^>]*>/.test(h5) && /<table/.test(h5) && /<td>/.test(h5),
  h5,
);

console.log("\n" + pass + " passed, " + fail + " failed");
process.exit(fail === 0 ? 0 : 1);
