# 企业微信域技能来源声明

本目录下的 `wecomcli-*` 技能文档来自腾讯官方 **wecom-cli** 项目,按 **MIT 许可证**分发。

- 上游仓库:https://github.com/WecomTeam/wecom-cli (`skills/` 目录)
- 许可证:MIT(随上游)
- 用途:pinvou3 接入企业微信(`@wecom/cli`)连接器时,作为官方域技能由引擎
  `SkillRegistry` 渐进披露,教模型用 `wecom-cli <域> ...`。

> 该 NOTICE.md 本身不含 `SKILL.md`,不会被技能注册表当作技能加载(与 lark NOTICE 同)。

## MIT License

以下许可证文本取自上游仓库 `LICENSE` 文件(https://github.com/WecomTeam/wecom-cli/blob/main/LICENSE):

```
MIT License

Copyright (c) 2026 WeCom

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

## Pinvou3 本地修改登记

以下修改为 pinvou3 在上游 skill 基础上的本地分叉。**本地修改在下次上游 sync 时需重放。**

### 上游同步(2026-08-18,wecom-cli 1.1.0)

全量同步至上游 v1.1.0 发布提交 `cd0480e0`(npm `@wecom/cli@1.1.0` 同源)。
1.1.0 上游重构了命令模型(鉴权 `init`→`auth init`、`msg`→`message`、
`schedule`→`calendar`、入参 JSON 位置参数→命名 flags/`--json`/`--set`,
新增 disk/doc-manage/email/media/sheet/smartpage/shared 服务技能与 chat 顶层
服务域——chat 为 CLI 命令域,上游无对应技能目录,域探测见 wecom-smoke.sh),
上游 `skills/` 目录随之重排为 14 个技能,本地结构**整体跟随上游**,不再维持
0.1.9 时代的「sheet/smartpage 并入 wecomcli-doc」合并形态(该合并的结构性
前提——doc 服务吞并 sheet/smartpage——已随上游服务模型拆分而消失)。
旧目录 `wecomcli-msg`/`wecomcli-schedule` 从包内移除,存量用户解包目录由
`apply_wecom_skills` 门控清理(BUNDLE_VERSION 0.23)。

### 0.1.9 时代历史登记(已在 1.1.0 同步中自然吸收或失效,存档备查)

- **结构性修改(失效)**:wecomcli-doc 合并 sheet/smartpage——1.1.0 上游已按
  服务模型独立拆分,合并形态废弃。
- **提示词去重与精简(2026-07-25)(吸收)**:0.1.9 时代的 contact/doc/msg/
  schedule 四技能去重精简与 meeting 结构下沉,随旧目录移除;1.1.0 上游重写了
  全部技能文档,新一轮去重以 1.1.0 文本为基线另行评估。
- **上游同步(2026-07-25)(失效)**:todo 同步 `9d2aeaf`、smartsheet 同步
  `bae1cc3e`——两者的内容已包含在 1.1.0 重写版中。
- **真实性审查(2026-08-16)(吸收)**:0.1.9 二进制命令面核实与 frontmatter
  防误用前缀(「何时用:」开头、≤280 字符)作为品悟常设口径保留,本轮已在
  全部 14 个技能上重放;`File(action="read")` 引擎工具名口径、smartsheet
  `records.values` 双层嵌套 JSON 上游 bug 修正,在 1.1.0 新文本上复核重放。

### 本轮(1.1.0)品悟适配清单

1. **frontmatter description 防误用前缀**:14 个 SKILL.md 的 description 改写为
   「何时用:仅当用户明确指向企业微信…时使用;泛指…默认走本地工具」开头,全部
   ≤280 字符(上游 disk/doc/smartsheet 三个超限 description 压缩改写)。
2. **安装教学改品悟代管口径**:wecomcli-shared Step 1 的
   `npm install -g @wecom/cli` 自更新指引(会绕过品悟 lock 钉扎触发哈希不匹配
   重装循环)改写为「wecom-cli 由品悟代管、随应用更新;版本不足时在工具商店
   企业微信卡片重新点连接触发安装/升级」。
3. **引擎工具名口径**:上游「先 `read` 对应 references 文件」「用 `write` 工具
   写入」「用 Write 工具」「`grep 工具` 提取」等裸引擎工具名统一改为
   `File(action="read")` / `File(action="write")` 口径,覆盖 doc/doc-manage/
   meeting/sheet/smartpage/smartsheet/email 七个技能的 SKILL.md 与 references
   (含 doc-create.md 的 Write 写文件示例、smart-sheet-read.md 的 grep 提取与
   smartpage-edit.md 的回包文件读写);calendar/contact/todo/disk/media 上游
   原文无裸工具名,无需改动。
4. **python3 口径**:doc-create 等 references 的 `python build_docx.py` 示例统一
   `python3`(宿主环境无裸 python)。
5. **上游 bug 修正重放**:smartsheet `records.values` 双层嵌套 JSON 结构修正
   (若 1.1.0 上游原文仍为 `"values": {"values":{...}}` 则保持单层修正版)。
6. **shared 技能 bins frontmatter**:上游 wecomcli-shared 的 frontmatter 无
   `metadata.requires.bins`(其余 13 技能上游自带),本地按连接器技能包契约
   (connector_skills_contract 规则 6)补 `bins: ["wecom-cli"]`,使技能注册表
   能声明二进制依赖。

### 路由口径统一与文档缺陷修复(2026-08-27)

wecom sheet/smartsheet/smartpage 三技能文档审计修复,属本地适配层(在上文
「本轮品悟适配清单」第 1 条 description 口径基础上继续演进):

1. **泛指做表格口径统一为先消歧**:sheet 与 smartsheet 的 description 原本
   对「企微语境+泛指做表格」给出相反缺省(前者先消歧、后者默认本技能)。
   统一为先消歧确认——与 calendar/meeting 的「先追问」口径对称;两类表格
   产品能力不同,缺省错选代价高于一次追问;doc-manage 已按显式意图分流
   (新建在线表格→sheet、智能表格 CRUD→smartsheet),泛指场景不定缺省。
   smartsheet description 弃「未说明在线表格默认本技能」,两包正文路由表
   各补一条泛指消歧行。smartpage description 去「仅当」排他句式,保留其
   「泛指创建/写/整理文档默认承接」定位(与 doc 技能让位口径一致)。
2. **批量更新拆分口径**:smart-sheet-edit.md「更新记录必须一次完成、严禁
   拆分」改为单批完整提交、总量超 2000 条按 2000/批分段,消除与记录操作
   参数说明(单次 1~2000、超 2000 分批)的互斥。
3. **删除纳入 >100 条强制确认**:SKILL.md 确认机制由「新增或修改」扩为
   「新增、修改或删除」,smart-sheet-edit.md 数据破坏警示同步补充;仅对齐
   既有 >100 规则,不改其他确认策略。
4. **smartpage→smartsheet 委托契约**:smartsheet SKILL.md 的 docid 合法
   来源清单与前置阻断各补一条豁免——`smartpage databases get` 返回的
   `database_info.id` 可直接作 smartsheet 侧 `docid`/`file_id`;smartpage
   侧(SKILL.md 委托关系、smartpage-edit.md databases get)同步说明。
5. **sheet-rows-append.md 结构表补 `data_type` 行**:与
   sheet-contents-update.md 同语义结构对齐(SKILL.md 硬规则要求
   cell_value 必须同时设 data_type)。
6. **smartsheet 路由表修正**:文件级操作行不再把搜索/成员/加入规则归
   common.md(实际转交 doc-manage);跨技能依赖不再把「新建文档」归
   doc-manage(新建/导入智能表格在本包 common.md)。
7. **删除 smartpage 无参数表支撑的 open_vid/userid 等价句**:六接口无任何
   userid 消费点,整句删除。

### 各技能重放基线

14 个技能全部 = 上游 `cd0480e0`(v1.1.0 发布提交,npm 1.1.0 同源),技能目录与
上游同名同构;本地分叉为上文「本轮品悟适配清单」六类与「路由口径统一与
文档缺陷修复(2026-08-27)」(仅涉及 sheet/smartsheet/smartpage 三技能)。

### 文档缺陷修复(2026-08-27,calendar/meeting/email/message 四技能)

审计驱动的 14 条文档缺陷修复(sheet/smartsheet/smartpage 三技能由同日另一轮
登记覆盖)。修复原则:仅消除矛盾/虚构/漂移,不新增展示能力、不改任何发送类
确认策略(email「预览后直接发送」、message 无预览不对称等产品决策项一律不动)。

1. **calendar end_time 追问口径自相矛盾**:SKILL.md 规则 3 与 calendar-create.md
   注意事项把 `end_time` 列入"缺失必须询问",而同文件参数表/工作流步骤 1 又写
   "默认 1 小时不追问"。统一为 meeting 包口径:end_time(时长)缺失不追问、默认
   1 小时(`begin_time + 1h`),两处均加显式 carve-out。
2. **calendar「看会议链接」三方冲突**:SKILL.md 路由行承诺"看会议链接",与
   meeting 包"禁止展示 meeting_code/meeting_link"的全局禁令冲突,且 calendar
   各字段表无展示支撑。保守裁决:**删除承诺**(路由行、浏览 vs 搜索原则句、
   agenda 典型场景标题三处),不新增任何展示能力。
3. **meeting-search.md get 补充能力虚构**:"需入会链接等时用 get 补充"与
   meeting-list.md get 字段表不符(无 meeting_link/meeting_code,且 SKILL.md
   禁止展示)。改写为:get 补充状态/参会人等;入会链接仅 create 返回,任何场景
   不展示。
4. **合并展示判据字段漂移**:meeting SKILL.md 合并展示用 `meeting_link` 非空,
   calendar 包各处均用 `meeting_code`。统一为 `meeting_code`。
5. **"明确是在线会议"信号清单不对称**:meeting SKILL.md 易混淆场景路由句缺
   "腾讯会议"(calendar 同位句有)。补齐,两包对齐。
6. **meeting-create.md 示例时间格式违规**:示例输出 `<明天日期> 14:00:00 -
   15:00:00` 违反本包 REQUIRED 相对日期标签格式。改为 `明天 6月12日 14:00-15:00`。
7. **calendar-meeting-room.md 链接显示文字与目标不符**:参考区显示文字
   `wecomcli-calendar.md` 指向 `../SKILL.md`。改显示文字为 `wecomcli-calendar`。
8. **meeting-update.md 缺跨载体消歧**:meeting-cancel.md 有"[REQUIRED] 模糊无法
   判定→两边都查"而 update 缺。补同款「定位目标时的跨载体消歧」章节,与
   cancel 对称(更新同为写操作,先让用户选定唯一目标再执行)。
9. **meeting 包缺时区标注规则**:calendar SKILL.md 有完整时区标注
   [REQUIRED] 而 meeting 无(meeting 返回同构 timezone 字段)。在输出格式规范
   补镜像规则(非东八区带时区标注,东八区不标注)。
10. **email SKILL.md 虚构 30 天搜索窗口限制**:平台限制段称"mail search 带
    begin_time/end_time/only_unread/only_reminder 时搜索范围不能超过最近 30 天,
    详见 search-mail.md",而 search-mail.md 全文无此限制、且明确"用户明确指定
    的时间范围以用户为准"。删除该虚构条目。
11. **message SKILL.md 媒体上传 type 指导虚构**:技能依赖表称"上传时传入的
    type 应和发送时的 msg_type 对齐",而 wecomcli-media upload 入参仅 file_path
    (type 是上传后自动判定的返回字段)。改写为:上传无需传类型,按返回的 type
    对齐 msg_type;四个消息类型的"以 type=X 上传获得"同步改为"上传后返回
    type=X 的结果"。
12. **message SKILL.md 会话数自造硬数字**:"如实告知目标不在最近 10 个会话中"
    与返回表"数量以实际回包为准"矛盾。去掉具体数字,改为"不在本次返回的最近
    会话中"。
13. **email get-mail.md contact 字段名错误**:"含 `mail` 字段"应为
    wecomcli-contact 实际返回的 `users[].email`。修正字段名。
14. **email references 章节引用名漂移**:六个引用点(send-mail/reply-mail/
    forward-mail/get-mail/send-schedule×2)写「接口失败处理规范」,SKILL.md 实际
    标题为「接口失败处理」。统一引用文字。

> 对账命令(仓库根执行):
> ```
> git clone https://github.com/WecomTeam/wecom-cli.git /tmp/wecom-upstream
> git -C /tmp/wecom-upstream worktree add /tmp/wecom-verify cd0480e0
> diff -rq /tmp/wecom-verify/skills/<skill> \
>   pinvou3-app/src-tauri/resources/common/bundle/wecom-skills/<skill>
> ```
> diff 只应报 SKILL.md(及被改的 references/*.md)differ;「Only in」之外的
> 差异即为新的未登记分叉。
