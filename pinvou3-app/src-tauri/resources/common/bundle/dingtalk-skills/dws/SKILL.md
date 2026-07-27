---
name: dws
description: 用 dws CLI 管理钉钉:AI表格/AI应用/AI搜问(找人首选)/目标管理(Agoal)/日历/通讯录/群聊与机器人消息/待办/审批/考勤/日志(日报周报)/DING消息/钉钉文档/云盘/AI听记/邮箱/在线电子表格(axls)/知识库/开放平台文档。用户要求操作上述钉钉产品时使用。
cli_version: ">=1.0.15"
---

# 钉钉全产品 Skill

通过 `dws` 命令管理钉钉产品能力。


> ⚠️ 命令与 flag 以当前 dws 二进制为准:`dws <cmd> --help` 是事实源,与本文档冲突时以 `--help` 为准。

## 严格禁止 (NEVER DO)
- 不要使用 dws 命令以外的方式操作（禁止 curl、HTTP API、浏览器）
- 不要编造 UUID、ID 等标识符，必须从命令返回中提取
- 不要猜测字段名/参数值，操作前必须先查询确认

## 严格要求 (MUST DO)
- 所有命令必须加 `--format json` 以获取可解析输出
- 危险操作必须先向用户确认，用户同意后才加 `--yes` 执行
- 单次批量操作不超过 30 条记录
- 所有命令必须**严格遵循**对应产品参考文档里面规定的参数格式（参数与参数值之间用空格隔开）
- **脚本优先**:[scripts/](./scripts/) 下的 `python scripts/<name>.py` 已封装翻页/批量逻辑(如 AI表格导入导出、文档创建并写入、钉盘目录树、机器人广播),对应场景优先调用脚本;各脚本参数不统一,先 `python scripts/<name>.py --help` 确认 flag


## 产品总览

| 产品                | 用途                                                   | 参考文件                                                           |
|-------------------|------------------------------------------------------|----------------------------------------------------------------|
| `aiapp`           | AI应用：创建/查询/修改AI应用                                       | [aiapp.md](./references/products/aiapp.md)                     |
| `agoal` | 目标管理:战略解码/经营合约/计分卡/用户目标/目标模板/周月报 | [agoal.md](./references/products/agoal.md) |
| `aisearch`        | AI搜问（搜人首选）：按姓名/部门/职位/职责/上级/下级/手机号/工号维度找人，"谁负责 XX/XX 的负责人/某事项/某项目的人"统一走本产品 | [aisearch.md](./references/products/aisearch.md)               |
| `aitable`         | AI表格：Base/数据表/字段/记录/视图/附件/图表/仪表盘/导入导出/模板搜索            | [aitable.md](./references/products/aitable.md)                 |
| `attendance`      | 考勤：打卡结果/打卡流水/考勤组查询/考勤规则/汇总统计/假期类型/假期余额（P0 已落地，部分管理类命令仍属 P1） | [attendance.md](./references/products/attendance.md)           |
| `calendar`        | 日历：日历列表/日程/参与者/附件/响应/会议室/闲忙查询/时间建议                  | [calendar.md](./references/products/calendar.md)               |
| `chat`            | 群聊与机器人：搜索群/建群/群成员管理/改群名/消息发送(文本/Markdown/图片/文件)/拉取消息/@我/特别关注/机器人群发/单聊/撤回/转发/引用回复/Webhook/机器人搜索     | [chat.md](./references/products/chat.md)                       |
| `contact`         | 通讯录：用户查询(当前用户/搜索/详情/手机号)/花名册档案(学历/家庭/银行卡/合同)/离职员工查询(姓名/时间范围/部门)/部门查询(搜索/详情/子部门/成员)/角色查询(主管/管理员/财务/HR 等 label)/特别关注列表              | [contact.md](./references/products/contact.md)                 |
| `devdoc`          | 开放平台文档：搜索开发文档                                        | [devdoc.md](./references/products/devdoc.md)                   |
| `ding`            | DING消息：发送/撤回（应用内/短信/电话）                              | [ding.md](./references/products/ding.md)                       |
| `doc`             | 钉钉文档：搜索/浏览/读写/块级编辑/评论/文件创建/复制/移动/重命名/**删除/导出 docx/权限管理/媒体上传下载**       | [doc.md](./references/products/doc.md)                         |
| `drive`           | 钉钉云盘：文件列表/元数据/文件夹/上传(两步)/下载                        | [drive.md](./references/products/drive.md)                     |
| `minutes`         | AI听记：听记列表/摘要/关键词/转写/待办/思维导图/发言人/发言人段落总结/热词/录音控制/成员权限/上传 | [minutes.md](./references/products/minutes.md)                 |
| `oa`              | OA审批：待处理/详情/同意/拒绝/撤销/记录/已发起/任务/转交/评论/抄送              | [oa.md](./references/products/oa.md)                           |
| `report`          | 日志：按模版创建/收件箱/已发送/模版查看/详情/已读统计                         | [report.md](./references/products/report.md)                   |
| `mail`            | 邮箱：邮箱地址查询/邮件搜索(KQL)/邮件详情/发送邮件                        | [mail.md](./references/products/mail.md)                       |
| `sheet`           | 在线电子表格(axls)：工作表 CRUD/区域读写/CSV 批量写入/行列增删/合并/查找替换/筛选视图/全局筛选/排序/下拉列表/条件格式/浮动图片/浮动图表/模板/导出 xlsx(单命令一站式) | [sheet.md](./references/products/sheet.md)                     |
| `todo`            | 待办：创建(含优先级/截止时间/循环)/查询/修改/标记完成/删除                   | [todo.md](./references/products/todo.md)                       |
| `wiki`            | 知识库：空间创建/详情/列表/搜索 + 成员管理                                | [wiki.md](./references/products/wiki.md)                       |

## 意图判断决策树

用户提到"AI应用/创建应用/生成系统/做工具/管理后台/低代码" → `aiapp`
用户提到"目标管理/Agoal/战略解码/经营合约/计分卡/目标模板/周月报提交统计" → `agoal`
用户提到"找人/搜人/谁负责 XX/某事项的负责人/某项目的人/团队成员/上级/下级/按工号找人/按手机号找人" → `aisearch`
用户提到"表格/多维表/AI表格/记录/数据/视图/图表/仪表盘" → `aitable`
用户提到"考勤/打卡/排班" → `attendance`
用户提到"日程/日历/会议室/约会/时间建议" → `calendar`
用户提到"群聊/建群/群成员/群管理/发消息/发图片消息/发文件消息/发 Markdown 消息/截图发钉钉/转发消息/引用回复/@我/特别关注消息/机器人发消息/Webhook/机器人群发/机器人单聊/通知" → `chat`
用户提到"通讯录/同事/部门/组织架构/子部门/部门多少人/离职员工/离职名单/离职花名册/花名册/员工档案/学历/家庭/银行卡/紧急联系人/合同/角色/主管角色/管理员角色/财务/HR/特别关注/星标联系人" → `contact`
用户提到"开发/API/调用错误 文档" → `devdoc`
用户提到"DING/紧急消息/电话提醒" → `ding`
用户提到"钉钉文档/云文档/知识库/读写文档/块级编辑/文档评论/文档复制移动" → `doc`
用户提到"云盘/文件存储/文件上传下载/文件夹" → `drive`
用户提到"听记/AI听记/会议纪要/转写/摘要/思维导图/发言人/热词" → `minutes`
用户提到"邮箱/邮件/发邮件/收邮件/搜邮件/查邮件/邮件草稿/转发邮件/回复邮件/邮件附件/抄送" → `mail`
用户提到"审批/请假/报销/出差/加班/同意/拒绝/撤销审批" → `oa`
用户提到"日志/日报/周报/日志统计/写日报/提交周报/发日志/填日志" → `report`
用户提到"在线电子表格/钉钉表格/axls/工作表/单元格读写/合并单元格/筛选视图/导出 xlsx" → `sheet`
用户提到"待办/TODO/任务提醒/循环待办" → `todo`
用户提到"知识库/wiki/团队空间/知识库成员管理" → `wiki`

关键区分: aitable(数据表格) vs todo(待办任务)
关键区分: report(钉钉日志/日报周报) vs todo(待办任务)
关键区分: chat send-by-bot(机器人身份发消息) vs send-by-webhook(自定义机器人Webhook告警)
关键区分: doc(钉钉文档/富文本协同) vs drive(钉钉云盘/二进制文件)
关键区分: oa tasks(审批 taskId，审批/拒绝用) vs oa list-pending(收件箱 processInstanceId，查看用)


> 更多易混淆场景见 [intent-guide.md](./references/intent-guide.md)

## 危险操作确认

以下操作为不可逆或高影响操作，执行前**必须先向用户展示操作摘要并获得明确同意**，同意后才加 `--yes` 执行。确认方式：展示操作摘要（操作类型 + 目标对象 + 影响范围），用户明确同意后才加 `--yes` 执行。

| 产品 | 命令 | 说明 |
|------|------|------|
| `aitable` | `base delete` | 删除整个 AI 表格，含全部数据表和记录 |
| `aitable` | `table delete` | 删除数据表（含全部字段/视图/记录） |
| `aitable` | `field delete` | 删除字段（该列所有值同步清空） |
| `aitable` | `view delete` | 删除视图 |
| `aitable` | `record delete` | 删除记录（支持批量） |
| `aitable` | `chart delete` / `dashboard delete` | 删除图表/仪表盘 |
| `calendar` | `event delete` | 删除日程，所有参与者同步取消 |
| `calendar` | `participant delete` | 移除日程参与者 |
| `calendar` | `room delete` | 取消会议室预定 |
| `chat` | `group members remove` | 移除群成员 |
| `chat` | `message recall-by-bot` | 撤回机器人已发消息 |
| `doc` | `delete` | **删除整篇文档/文件**到回收站（与 `block delete` 不同，本命令删除整个 node） |
| `doc` | `block delete` | 删除文档单个块（不可恢复） |
| `doc` | `permission update` | 修改协作者权限（降权可能影响他人访问） |
| `ding` | `message recall` | 撤回已发 DING 消息 |
| `oa` | `approval revoke` | 撤销自己发起的审批实例 |
| `oa` | `approval reject` | 拒绝待审批（需加明确理由） |
| `todo` | `task delete` | 删除待办 |
| `minutes` | `replace-text` | 全文批量替换转写与摘要 |

## 核心流程
在选择 `dws` 的产品命令前，遵循以下流程：

0. **URL 预检**：输入含 `alidocs.dingtalk.com` URL 时，该域名下存在多种路径格式（`/i/nodes/...`、`/i/p/...`、`/spreadsheetv2/...`、`/document/edit|preview?dentryKey=...` 等），每种的处理流程不同。**必须先读取 [url-patterns.md](./references/url-patterns.md) 中的「alidocs URL 分流决策」**，按其中规则识别 URL 类型后再选择对应产品。含 `shanji.dingtalk.com` URL 时直接路由到 `minutes`。URL 已识别后直接进入对应产品流程，无需后续步骤。
1. 意图分类：判断用户指令的核心动词/动作属于哪一类。
2. 歧义追问：指令模糊或包含多个产品的关键字时，严禁猜测，主动向用户追问澄清意图。
3. 选定产品：参考产品总览和意图判断决策树选定产品，充分阅读对应产品参考文件后执行。

## 命令发现（flag / 参数以 binary 为准）

产品参考文档（`references/products/*.md`）里的 flag 列表是**便于理解用途的参考**，不是权威契约。参数名称、默认值、必填约束以当前二进制编译出的 Cobra 命令为准，**`--help` 是产品命令调用的事实源**：

```bash
# 人读视图：看 Usage / Example / Flags
dws <command-path> --help
# 例：dws calendar event list --help

# helper-only schema 查询（如 dev.*），普通产品命令不要依赖 schema 推断参数
dws schema "dev app create"
# 注：--jq 对 schema 输出无效（不过滤，仍返回完整对象）；schema 结构里必填标在
# .parameters.<字段>.required，没有 .tool 键。要看必填字段自行读 .parameters 即可。
```

**何时用哪条路径：**
- 只需看某个命令怎么调用 → `dws <cmd> --help`
- 构造 `--params` / `--json` 时不确定字段类型、必填、别名 → 先看 `dws <cmd> --help`，helper-only 命令再看 `dws schema`
- 参考文档和 `--help` 冲突时 → **以 `--help` 为准**，文档视为过期

`dws schema` 在静态端点模式下只保留 helper-only 子树；普通产品命令和 flag 不再通过远程 schema 动态发现。

## 错误处理
1. 遇到错误，加 `--verbose` 重试一次
2. 若 stderr 出现 `RECOVERY_EVENT_ID=<event_id>`，优先按 [recovery-guide.md](./references/recovery-guide.md) 执行 recovery 闭环
3. 仍然失败，报告完整错误信息给用户，禁止自行尝试替代方案
4. 认证失败时，参考 [global-reference.md](./references/global-reference.md) 中的认证章节处理
5. 各产品高频错误及排查流程见 [error-codes.md](./references/error-codes.md)
6. 遇到 [capability-limits.md](./references/capability-limits.md) 中列出的「已知不支持操作」时，**直接告知用户不支持并建议在钉钉客户端操作**，不要重试或变通


## 详细参考 (按需读取)

- [references/products/](./references/products/) — 各产品命令详细参考（flag 细节以 `--help` / `dws schema` 为准）
- [references/intent-guide.md](./references/intent-guide.md) — 意图路由指南（易混淆场景对照）
- [references/url-patterns.md](./references/url-patterns.md) — URL 格式规范 + alidocs URL 分流决策与类型探测流程（含钉盘 `document/edit|preview?dentryKey=` 链接）
- [references/global-reference.md](./references/global-reference.md) — 全局标志、认证、输出格式
- [references/field-rules.md](./references/field-rules.md) — AI表格字段类型规则
- [references/error-codes.md](./references/error-codes.md) — 错误码 + 调试流程
- [references/recovery-guide.md](./references/recovery-guide.md) — recovery 闭环、`RECOVERY_EVENT_ID`、`execute/finalize` 规范
- [scripts/](./scripts/) — 各产品批量/复合操作脚本（AI表格批量导入导出、日历、机器人消息、通讯录、考勤、日志、待办、文档创建并写入、钉盘目录树等）
- [references/best_practices/](./references/best_practices/) — 11 个编号场景 recipe(01 消息/02 任务/03 会议/04 文档/05 汇报/06 数据分析/07 听记/08 通讯录/09 邮件/10-11 听记发言人)+ [lite-recipes.md](./references/best_practices/lite-recipes.md) 速查;通用规范在 `_common/`
- [references/products/aitable/](./references/products/aitable/) — AI表格细分章节(单元格值/字段/公式/筛选/导入导出/记录增删改查/错误恢复/最佳实践);记录操作另见 [aitable-record-ops.md](./references/products/aitable-record-ops.md)
- [references/capability-limits.md](./references/capability-limits.md) — 已知能力限制（doc/aitable/chat/minutes，遇到时直接告知用户不支持）
