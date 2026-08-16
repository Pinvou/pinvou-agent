# 第三方组件声明 — 飞书官方域技能(lark-*)

本目录下的 `lark-*` 技能(SKILL.md + references/)同步自飞书官方开源仓库
**larksuite/cli**(https://github.com/larksuite/cli),按 **MIT License** 分发。

```
MIT License

Copyright (c) larksuite/cli contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

收录的域:lark-shared(鉴权总则,必备)、lark-calendar、lark-doc、lark-drive、
lark-sheets、lark-im、lark-task、lark-wiki、lark-base。

更新方式:按与 `connectors.lock.json` 钉扎一致的 lark-cli tag 同步上游
`skills/<域>`(v1.0.87 起上游 tag 均含完整 skills/),保留本声明。

## Pinvou3 本地修改登记

以下修改为 pinvou3 在上游 skill 基础上的本地分叉。**下次上游 sync 时需逐条重放。**

### 同步记录(2026-08-16 → v1.0.87)

- CLI 钉扎 1.0.65 → 1.0.87;上次导入基线实为上游 main `ba51d487`(2026-06-26,
  早于 v1.0.65 tag),本次以 v1.0.87 tag 为新基线。后续 sync 以「与 lock 钉扎同名
  tag」为基线三方合并。
- **`--api-version v2` 已在 lark-cli 1.0.87 移除**(v2 成为唯一 API,flag 仅静默
  兼容):全部文件的命令示例、参数表、CRITICAL 提示与 frontmatter cliHelp 均已
  去除该参数。
- 跟随上游结构变化删除:lark-doc/references/style/(上游并入 genres/ 体系与新
  create-workflow)、lark-calendar-agenda/freebusy.md(并入 SKILL.md)、
  lark-drive-comments-guide.md(拆分为 comment-* 七篇)、
  lark-sheets-core-operations.md(上游 2026-07-13 重构)。
- 上游新增域文件(genres/ 28 篇、doc-script/xml-extended-blocks、base 的
  data-analysis/app 系列、calendar 的 meeting/recurring/schedule-* 等)已带入,
  并对其中引用未收录域处统一应用「未随包收录 + CLI 命令直给」口径。
- `--api-version v2` 之外,2026-07-25/07-26 两批登记全部在 v1.0.87 文本上重放;
  上游已等价解决的:lark-sheets 的 `set +H` 修正、lark-task 的 `+subscribe-event`
  (1.0.87 实测仍无该命令,维持删除)。lark-im/references/lark-im-scopes.md 为
  本地新增文件,保留并按 1.0.87 口径(`missing_scopes`、`files.create`)更新。

### 真实性审查补录(2026-08-16,同轮次复审)

同步后对照 lark-cli 1.0.87 二进制逐命令实测复审,除上述重放外修正:

- **lark-shared**:SKILL.md 补 frontmatter 三件套(description 防误用前缀 +
  `metadata.requires.bins` + `cliHelp`),与其余 8 域对齐;
  `references/lark-wiki-token-routing.md` 的 slides 行由「暂不支持」改为
  「技能未随包收录 + `lark-cli slides` 直给」(1.0.87 slides 域有完整编辑
  命令,mindnote 行同口径补直给)。
- **lark-calendar**:SKILL.md 4 处 lark-vc 口径由「本环境未提供/如实告知不
  支持」统一为「技能未随包收录 + `vc +search` 等直连」(实测 1.0.87 vc 域
  命令完整,原文会让模型拒绝实际可完成的请求);`+search-event` 默认页大小
  修正为 20(实测)。
- **lark-doc**:fetch.md 自指小节名与错位标题修正;4 个 media/resource 文档
  的 `../lark-shared` 链接显示文本与目标对齐。
- **lark-sheets**:SKILL.md 与 read-data.md 的 scripts 分发口径由上游的
  「只随仓库版/二进制内嵌版不含」修正为「随品悟应用内置分发」(品悟 bundle
  实际携带 scripts/ 且物化时整目录释放)。
- frontmatter 的 `requires.skills`(lark-doc)与 `siblings`(lark-sheets)键名
  不一致:引擎(CodeWhale)只消费 name/description,两者均无实际作用,保持
  上游原样,下次 sync 顺其自然。

### 提示词事实修正与去重(2026-07-25)

- **lark-shared**:description 改为中文统一风格;删除两处逐字重复(device-code 展示规则、更新提示规则);split-flow 步骤内的二维码展示规则去重;修正语病。
- **lark-base**:删除与快速路由表重复的「保留 Reference」整节(dashboard-block-get-data 链接并入路由表);删除内部重复规则 5 处(查询统计、写入前置、批量 200/1254291、form-submit)。
- **lark-doc**:description 压缩至引擎 280 字符截断上限内;消除对未收录 skill 的断链引用(lark-note → 声明妙记转写页暂不支持;lark-whiteboard → 改为 SVG/Mermaid 直插路径,references 内同步修正);`resource-*` 命令统一补 `+` 前缀;裸 `auth login` 改为按 lark-shared split-flow 处理。
- **lark-calendar**:description 压缩至 280 内;lark-vc 断链改为「本环境未提供该能力」声明;压缩与 lark-shared 重复的身份示例;删除重复日程规则重复半句。
- **lark-drive**:description 压缩至 280 内;lark-markdown 断链改为 download/本地编辑/upload 组合路径(含 references/lark-drive-upload.md);import 分流规则三处重复合并为一条映射。
- **lark-sheets**:description 压缩至 280 内;删除错误的 `set +H` 建议(Linux sh/dash 下为非法选项),改为单引号包裹方案;示例 `--sheet-name "Sheet1"` 改为占位符。
- **lark-wiki**:description 压缩至 280 内;删除与「成员管理硬限制」重复的决策条。
- **lark-im**:`Read 工具` 改为实际工具名 `read_file`;身份映射段去重;`--download-resources` 段去重;删除无对应文档的 Card Messages 孤儿段;权限表下沉至 `references/lark-im-scopes.md`(新增)。
- **lark-task**:删除 shipped lark-cli 中不存在的 `+subscribe-event` 表行及其 reference 文件;lark-minutes 断链改为直接写 `lark-cli minutes +todo`;时间格式改为 `YYYY-MM-DD HH:MM:SS`;补 `lark-cli whoami` 提示。
- **references 级修正**:lark-doc-whiteboard.md / lark-doc-update.md / lark-doc-xml.md / style/lark-doc-create-workflow.md / lark-drive-comment-location.md 中对未收录 lark-whiteboard 的引用全部改为「未随包收录 + CLI 命令直给」口径。
- **references 级修正(2026-07-25 复审补漏)**:lark-doc-create.md / lark-doc-update.md 的 `block_token` 说明、style/lark-doc-style.md 的已有画板编辑指引,lark-whiteboard 引用补「未随包收录 + CLI 直给」;lark-wiki-token-routing.md 的 slides 行改为「lark-slides 未收录,暂不支持」声明;lark-base-cell-value.md / lark-base-view-set-filter.md / lark-drive-search.md 的 lark-contact 提及补「未随包收录」声明并统一为 `lark-cli contact +search-user` 直给。

### 评审修复补漏(2026-07-26)

- **`Read 工具` 同类残留修正**:lark-task / lark-wiki / lark-drive / lark-sheets 的 SKILL.md 与 lark-doc 的 SKILL.md、references/lark-doc-create.md、references/lark-doc-update.md 中残留的 `Read 工具` 统一改为实际工具名 `read_file`(此前仅修了 lark-im)。
- **lark-doc**:description 补回压缩时丢失的 doubao 路由句(doubao.com 的 /docx/ 或 /wiki/ URL 也走本 skill),与 lark-sheets / lark-wiki / lark-drive 压缩版口径一致,description 仍控制在引擎 280 字符上限内。
