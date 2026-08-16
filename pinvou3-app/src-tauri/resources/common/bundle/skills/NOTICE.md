# 第三方组件声明 — 飞书官方域技能(lark-*)

本目录下的 `lark-*` 技能(SKILL.md + references/)同步自飞书官方开源仓库
**larksuite/cli**(https://github.com/larksuite/cli),按 **MIT License** 分发。

```
MIT License

Copyright (c) 2026 Lark Technologies Pte. Ltd.

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
  (对账注 2026-08-16:经复核,v1.0.87 上游 lark-base/references/ 仍含
  lark-base-view-set-filter.md,且与品悟当前文件逐字节一致、内文已无 lark-contact
  提及——该文件的旧登记在 v1.0.87 文本上无需重放(上游重写已消化);
  cell-value 与 drive-search 两处仍有效,已在 v1.0.87 文本上重放,可对照上游
  tag diff 验证。)

- **评审修复补漏(2026-07-26)** 详目:`Read 工具` 同类残留修正见下;lark-doc description 补回 doubao 路由句等。以下为该轮明细,保留备查。

### 本地工具依赖审查补录(2026-08-16,第四轮)

品悟为三端应用(macOS/Linux/Windows),Windows 不保证本地 `jq` 可用;lark-cli 全局
`--jq` 实测对 API/shortcut 命令可用(管理命令如 `auth status` 不支持)。以下修改
改为 CLI 内置 `--jq` 或模型直接读取,下次上游 sync 需重放:

- **lark-im/references/lark-im-chat-create.md / lark-im-chat-search.md**:示例中
  `--format json | jq -r '.data...'` 命令替换链 3 处改为 `--jq` 直出 + 从输出
  读取 ID 的两步流(消除本地 jq 与 shell `$( )` 依赖)。
- **lark-im/references/lark-im-chat-list.md**:Scenario 3 的 `while`/`echo|jq -r`
  分页循环改为 `--jq '{chat_ids,has_more,page_token}'` 单次投影 + 逐页传
  `--page-token` 的说明。
- 其余命中判定不动:`lark-im-card-action-reply.md` 的 `--jq`(本就是 CLI 内置
  flag);lark-base data-analysis-sop 的本地 `jq -s`(已自带「本地 jq 不可用时改
  `--jq-records`」逃生口,分析工作流属可选重工具路径);lark-drive-status 的
  `--jq`;visual-design/lark-doc-md 的 base64 为数据编码语义非 shell 工具依赖。

### 第三轮盲区审查补录(2026-08-16,补登记)

以下第三轮(c9c6afb3)改动此前未登记,2026-08-16 NOTICE 对账补录(依据上游
v1.0.87 diff 实测,均需在下次 sync 重放):

- **lark-base/references/lark-base-workflow-schema.md** 两处笔误:operator 列表
  中 `containsAll` 前多余的斜杠(`/ /containsAll`→`containsAll`);
  `receive_scene` 枚举行 `"Chat"` 改为实际枚举值 `"chat"`。

### 第四轮 references 级补登记(2026-08-16)

第四轮(77c19912)在 SKILL.md 之外还改了以下 references,上节登记仅覆盖部分,
现补全(依据上游 v1.0.87 diff 实测):

- **裸 `auth login` 导正(7 处)**:上游 `可提示用户先完成 lark-cli auth login`
  统一改为「按 [`../../lark-shared/SKILL.md`] 的按需授权流程
  (`auth login --scope ...`)完成用户身份登录」——涉及 lark-drive 五篇
  (upload.md / create-folder.md / task-result.md / import.md 的 `permission_grant
  status=skipped` 提示,及 search.md 的 `--mine` 取不到 open_id 报错提示)、
  lark-im/references/lark-im-chat-identity.md(owner 转移需 owner 本人 UAT 授权)、
  lark-wiki/references/lark-wiki-node-create.md(bot 建节点后授权提示)。
- **假命令修正(3 处,上游同款 bug 建议回馈)**:lark-drive 三个 workflow 文档
  的 `sheets +read`/`+find`(1.0.87 实测不存在)改为 `sheets +cells-get`/
  `+cells-search`——comment-location.md 单元格读取示例、
  workflow-topic-move-collector.md 与 -resolve-verify.md 的 CONTENT_VERIFY 命令
  族两表;resolve-verify 同步去除 `docs +fetch --api-version v2` 残留参数。
- **正文断言矛盾修正(2 处)**:lark-drive-files-list.md 的「不要使用不存在的
  `--folder-token` flag」改为「typed flag `--folder-token` 实际存在(--help 可
  见),本 workflow 统一用 `--params` 传参避免与 shortcut 语义混淆」,并合并
  错误用法表中重复的 `--page-all` 行;
  lark-drive-workflow-permission-governance.md 两处「Drive folder 不支持
  `+inspect`」改为「`+inspect` 支持 folder URL / `--type folder`(如
  `/drive/folder/<token>`),也可直接从 URL 路径解析」。
- **未收录域口径补漏(4 处)**:lark-calendar-meeting.md 末尾补注「vc/note/
  minutes 对应 skill 未随包收录,以上为 CLI 命令直连用法」并去
  `--api-version v2`;lark-doc/references/genres/email.md 两处 `lark-mail` 断链
  改为「`lark-cli mail` 命令直连(未随包收录,先 `lark-cli mail --help`)」;
  lark-im-card-action-reply.md 头部 `../../lark-event/SKILL.md` 断链改为
  「lark-event 未随包收录,先 `lark-cli event --help` 查真实 flags」;
  lark-drive-update-title.md 的 `lark-apps` 断链改为「`lark-cli apps` 命令处理
  (lark-apps 未随包收录)」。
- **lark-drive-upload.md 快速决策**:`lark-markdown` 断链改为「本环境无
  lark-markdown 技能;download → 本地编辑 → upload(覆盖传 `--file-token`)」
  组合路径(该文件 `permission_grant` 行的裸 auth login 修正已列上条)。

### 评审修复补漏(2026-07-26)

- **`Read 工具` 同类残留修正**:lark-task / lark-wiki / lark-drive / lark-sheets 的 SKILL.md 与 lark-doc 的 SKILL.md、references/lark-doc-create.md、references/lark-doc-update.md 中残留的 `Read 工具` 统一改为实际工具名 `read_file`(此前仅修了 lark-im)。

(对账注 2026-08-16:本节及 2026-07-25 各条中的 `read_file` 登记已被后续 CodeWhale
v0.9.5 升级(PR #231)再次统一为 `File(action="read")`,当前 9 域文件实际即此口径。
下次 sync 重放时,所有「读取工具名适配」一律写 `File(action="read")`,不要再写
`read_file`——它已是引擎退役名。同理,lark-doc-fetch.md 等文件中的读取指引以
`File(action="read")` 为准。)
- **lark-doc**:description 补回压缩时丢失的 doubao 路由句(doubao.com 的 /docx/ 或 /wiki/ URL 也走本 skill),与 lark-sheets / lark-wiki / lark-drive 压缩版口径一致,description 仍控制在引擎 280 字符上限内。
