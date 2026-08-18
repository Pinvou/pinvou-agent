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

更新方式(新手操作手册):

1. 查钉扎版本:读
   `pinvou3-app/src-tauri/resources/platforms/<os>/<arch>/bundle/connectors/connectors.lock.json`
   (5 份,任一即可,版本字段一致)中 `name: "lark-cli"` 的 `version`(当前 1.0.87)。
2. 拉上游源:上游 tag 带 `v` 前缀,即
   `https://github.com/larksuite/cli/archive/refs/tags/v<version>.tar.gz`
   (当前 v1.0.87),解压后取其 `skills/<域>`(上游仓库共 27 个 lark-* 域,
   品悟只收录上述 9 域;其余域一律「未随包收录」,文档中提及须按「技能未随包
   收录 + CLI 命令直给」口径,不复制其目录)。
3. 以该 tag 为三方合并基线,按下文登记逐条重放本地修改后,保留本 NOTICE。

本目录下的 `visual-design/` 为品悟自研技能,不来自 lark-cli 上游,sync 时不涉及。

(对账注 2026-08-16:本 NOTICE 内文引用上游文件时,lark-shared 域内的
`references/lark-wiki-token-routing.md` 全称为
`lark-shared/references/lark-wiki-token-routing.md`;裸写 `references/…` 的
其他条目均属各自所属域,如 lark-doc 域的 `references/lark-doc-create.md`。)

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
  (状态注 2026-08-16:以上 4 项均为「已删,无需重放」,列出仅为解释旧登记
  条目中 style/、freebusy.md 等路径为何在本仓库不再存在。)
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
- **lark-task**:删除 shipped lark-cli 中不存在的 `+subscribe-event` 表行及其 reference 文件(reference 文件已删,无需重放;2026-08-17 勘误:经对 v1.0.87 上游 tarball 全量 grep,上游已无 subscribe-event 字样,表行删除项同样无需重放);lark-minutes 断链改为直接写 `lark-cli minutes +todo`;时间格式改为 `YYYY-MM-DD HH:MM:SS`;补 `lark-cli whoami` 提示。
- **references 级修正**:lark-doc-whiteboard.md / lark-doc-update.md / lark-doc-xml.md / style/lark-doc-create-workflow.md / lark-drive-comment-location.md 中对未收录 lark-whiteboard 的引用全部改为「未随包收录 + CLI 命令直给」口径。
  (状态注 2026-08-16:`style/lark-doc-create-workflow.md` 及其所在
  `lark-doc/references/style/` 目录已随 v1.0.87 同步删除——上游把 style/ 并入
  genres/ 体系,本条对其无需重放;whiteboard/update 两文件仍在,有效;
  2026-08-17 勘误:lark-doc-xml.md 已与 v1.0.87 上游逐字节一致(上游自行
  移除了 whiteboard 引用),该文件无需重放。)
- **references 级修正(2026-07-25 复审补漏)**:lark-doc-create.md / lark-doc-update.md 的 `block_token` 说明、style/lark-doc-style.md 的已有画板编辑指引,lark-whiteboard 引用补「未随包收录 + CLI 直给」;lark-wiki-token-routing.md 的 slides 行改为「lark-slides 未收录,暂不支持」声明;lark-base-cell-value.md / lark-base-view-set-filter.md / lark-drive-search.md 的 lark-contact 提及补「未随包收录」声明并统一为 `lark-cli contact +search-user` 直给。
  (状态注 2026-08-16:`style/lark-doc-style.md` 已随 v1.0.87 同步删除(style/
  并入 genres/),该子项无需重放;slides 行已被 2026-08-16 真实性审查补录
  改为「技能未随包收录 + `lark-cli slides` 直给」,以补录条为准。lark-base-view-set-filter.md 一条见下方对账注。)
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
  (`lark-drive-upload.md` 的 `permission_grant status=skipped` 提示、
  `lark-drive-create-folder.md`、`lark-drive-task-result.md`、
  `lark-drive-import.md` 三篇同类提示,及 `lark-drive-search.md` 的
  `--mine` 取不到 open_id 报错提示)、lark-im/references/lark-im-chat-identity.md(owner 转移需 owner 本人 UAT 授权)、
  lark-wiki/references/lark-wiki-node-create.md(bot 建节点后授权提示)。
  (重放结果 2026-08-16 实测 + 2026-08-17 勘误:lark-drive 四篇各 1 处命中;
  search.md 一处先前注「上游重写已消化」系误判——v1.0.87 tag 的
  lark-drive-search.md L125 `--mine` 行括注仍含「提示运行 lark-cli auth
  login」裸字样,重放时照常导正,勿跳过。)
- **别名命令改写为正式名(3 处)**:lark-drive 三个 workflow 文档的
  `sheets +read`/`+find` 改为 `sheets +cells-get`/`+cells-search`——
  `lark-drive-comment-location.md` 单元格读取示例、
  `lark-drive-workflow-topic-move-collector.md` 与
  `lark-drive-workflow-topic-move-collector-resolve-verify.md` 的 CONTENT_VERIFY 命令
  族两表;resolve-verify 同步去除 `docs +fetch --api-version v2` 残留参数。
  (第七轮复核勘误:`+read`/`+find` 在 1.0.87 是**隐藏别名**,`--help` 实测存在且
  分别转发 `+cells-get`/`+cells-search`——最初判「假命令」系探测方式误报,无上游
  bug 需回馈;改写为正式名仍值得保留(别名不在 `sheets --help` 快捷方式清单中,
  可见性差),故不回滚,后续 sync 若上游原文用回别名也无需再改。
  2026-08-17 第十一轮补注:lark-sheets/SKILL.md 场景速查表「查找/替换」行的
  「❌ 不存在」列曾把 `+find` 与真不存在的 `+cells-find`/`--query` 并列——
  与本条勘误矛盾,已改为「`+find` 是隐藏别名,正式名 `+cells-search`」口径,
  下次 sync 保持。)
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

(文件路径口径注 2026-08-16:本节裸写的 lark-im-card-action-reply.md 位于
`lark-im/references/`、lark-calendar-meeting.md 位于
`lark-calendar/references/`,其余裸写文件均位于 `lark-drive/references/`;
全称即加 `lark-<域>-` 前缀的文件名。)

### 评审修复补漏(2026-07-26)

- **`Read 工具` 同类残留修正**:lark-task / lark-wiki / lark-drive / lark-sheets 的 SKILL.md 与 lark-doc 的 SKILL.md、references/lark-doc-create.md、references/lark-doc-update.md 中残留的 `Read 工具` 统一改为实际工具名 `read_file`(此前仅修了 lark-im)。

(对账注 2026-08-16:本节及 2026-07-25 各条中的 `read_file` 登记已被后续 CodeWhale
v0.9.5 升级(PR #231)再次统一为 `File(action="read")`,当前 9 域文件实际即此口径。
下次 sync 重放时,所有「读取工具名适配」一律写 `File(action="read")`,不要再写
`read_file`——它已是引擎退役名。同理,lark-doc-fetch.md 等文件中的读取指引以
`File(action="read")` 为准。)
- **lark-doc**:description 补回压缩时丢失的 doubao 路由句(doubao.com 的 /docx/ 或 /wiki/ URL 也走本 skill),与 lark-sheets / lark-wiki / lark-drive 压缩版口径一致,description 仍控制在引擎 280 字符上限内。

## 第七轮脚本代码安全审计补录(2026-08-16)

对 lark-sheets/scripts/ 全部 6 个 Python 脚本做逐脚本安全审计(路径安全/注入面/
危险操作/数据外泄/异常退出码/资源/跨平台)。审计结论:全部只读封装,无文件写入、
无 shell=True、无 eval/exec/pickle、无网络直连(网络仅经 lark-cli)、错误统一
emit_error(JSON+exit 1)、无裸 except。修复 1 处,下次 sync 需重放:

- **lark-sheets/scripts/lark_sheet_read_cli.py**:`run_sheets()` 的
  `subprocess.run(..., text=True)` 补 `errors='replace'`——Windows GBK 控制台下
  lark-cli 输出含 emoji 时会抛未捕获 `UnicodeDecodeError`,导致调用方(3 个 CLI
  脚本)以 traceback 崩溃而非 emit_error 受控失败。其余 5 个脚本(lark_detect_subtables/
  lark_inspect_workbook/lark_profile_table/lark_sheet_range/sheets_df)无 subprocess
  文本解码或文件 I/O,无同类问题。

### 独立复审补登记(2026-08-16,PR 评审)

对照上游 v1.0.87 tag tarball 全量 diff 复审,以下既有本地分叉此前未登记,
下次 sync 需逐条重放:

- **`python` → `python3` 改写(5 处/3 文件,上游 v1.0.87 原文均为裸 `python`;
  品悟宿主环境无裸 `python`,实测 command not found;2026-08-17 计数勘误:原记
  「7 处」,实测 sop 1 + read-data 3 + write-cells 1 = 5 处)**:
  `lark-base/references/lark-base-data-analysis-sop.md`(uv run 示例 1 处)、
  `lark-sheets/references/lark-sheets-read-data.md`(3 处)、
  `lark-sheets/references/lark-sheets-write-cells.md`(1 处管道示例)。
- **lark-calendar/references/lark-calendar-create.md**:补闭合代码围栏
  ` ``` `(上游该文件仅 3 个围栏行,尾部代码块未闭合,会把后续正文吞进代码块;
  本地补成 4 个)。
- **lark-doc/SKILL.md**:复制文档路由由 `drive files copy` 改为
  `drive +copy`(并补「复制到知识库用 `wiki +node-copy`」分流)——上游命令
  实为 `+copy` 快捷方式(对照上游 Go 源 shortcuts/drive/drive_copy.go),
  `files copy` 是回退形态。同步重放本条时保留 `File(action="read")` 口径注。
- **lark-base/SKILL.md**:url-resolve 段补「Wiki URL(`.../wiki/<token>`)也可
  直接传给 `+url-resolve`(先解析 Wiki 节点再返回底层 base_token)」整句。
- **lark-wiki/SKILL.md**:新增「交接到底层文档/表格 Skill(obj_type 分流)」
  整节(obj_type + obj_token 解析后按 doc/sheet/bitable 分流的交接表)。
- **lark-shared/SKILL.md**:新增「临时文件一律写 `tmp/` 子目录」安全规则
  (过程文件不污染工作目录根的产出物列表)。
- **lark-contact 断链适配 3 处(PR #299 同步引入,2026-08-17 补登记)**:
  上游 v1.0.87 在 `lark-calendar/SKILL.md`(2 处,「常用其他域命令」区的
  搜索用户/通讯录行)与 `lark-task/SKILL.md`(1 处,负责人真实姓名注)原本
  指向未收录技能(`../lark-contact/SKILL.md` 断链/「详见 lark-contact」),
  本地改为「lark-contact skill 未随包收录,用 `lark-cli contact +search-user`
  直连」口径。`lark-base/references/lark-base-filter-condition.md:111` 的同类
  适配 1 处一并列入本条(filter-condition 为 v1.0.87 新文件,适配随首次同步带入)。
- **表格管道转义 7 处(第五/八轮审查引入,2026-08-17 补登记)**:
  `lark-base/references/lark-base-data-analysis-sop.md`(4 处,类型表
  `string\|null` 等)、`lark-doc/references/lark-doc-script.md`(2 处,
  `--as user\|bot` 与 `--format` 枚举)、`lark-im/references/card/components/
  checker.md`(1 处,`pc_display_rule:"always"\|"on_hover"`)——单元格代码跨度内的 `|` 须
  `\|` 转义,否则 GFM 渲染表格列破裂。(2026-08-17 勘误:checker.md 一条示例文
  原写作 `plain_text\|"lark_md"`,经核上游 v1.0.87 该单元格本就自带转义;实际
  未转义并修复的是 `pc_display_rule:"always"\|"on_hover"` 行,计数与文件无误,
  仅示例文指错单元格。)
- **lark-sheets 临时文件口径对齐(2026-08-17 评审修正)**:SKILL.md 两处
  「临时文件放系统临时目录」(脚本配合节)与「别把临时文件写进用户项目目录」
  (stdin/@file 陷阱条)按 lark-shared 的 `tmp/` 规则改写为「写 cwd 下 `tmp/`
  子目录」——上游「系统临时目录」是绝对路径,与 `@file` 仅接受 cwd 相对路径
  的约束冲突;重放时以本条为准,不回放上游原文。

### 第十轮独立复审补登记(2026-08-17,评审勘误)

对照上游 v1.0.87 tag 全量 diff 复审,以下既有本地分叉此前未登记或登记
有误,下次 sync 逐条重放:

- **lark-shared/SKILL.md「更新检查」节重写(第四轮 f2f7dfbc 引入,此前未登记)**:
  上游「始终使用 `lark-cli update` 更新」整段改成品悟钉扎口径——品悟内
  lark-cli 由应用按 `connectors.lock.json` 钉扎分发与升级,不要执行
  `lark-cli update` 自行更新(自行更新会脱离品悟校验,下次使用时被重装回
  钉扎版本);`_notice.update` 提示改为「新版本会随品悟应用更新自动就位」。
  本条是自更新禁令的正文载体,重放时必须保留,不得回退上游原文。
- **lark-im / lark-base 的 SKILL.md description 本地重写(2026-07-25 批次沿袭,
  2026-08-17 二次勘误:同批共 3 域漏登——im/base/task;其余 6 域已登记)**:
  均为「【何时用:仅当用户明确指向
  飞书…;泛指需求默认走本地工具】」防误用前缀 + 全文重组压缩(lark-im 280
  字符压线,lark-base 277,lark-task 245)。重放时保留前缀与压缩,不回退上游直译版。
- **lark-calendar/SKILL.md bot 空日历句(同步提交 3b66f343 引入;2026-08-17
  勘误定位:实际位于 L22「按日程归属选身份」区,非「常用其他域命令」区)**:
  「`--as bot` 查用户日程会拿到空结果(bot 只能访问自己的(空)日历,原理见
  lark-shared),查用户日程必须 `--as user`」——依据为 lark-shared/SKILL.md
  上游原文第 61 行同款口径,属对上游既有事实的收录补写。同处「压缩身份示例」
  的删除侧已登记,本条补登新增侧。
- **lark-task/SKILL.md 措辞分叉(1 处,品悟基线沿袭)**:「列取任务列表」
  (上游 v1.0.87 原文)在本地为「获取任务列表」,语义等价;下次 sync 跟随
  上游即可,无需重放(登记仅为对账完备)。
