钉钉内置技能来自钉钉官方 dingtalk-workspace-cli 的 dws-skills.zip mono 形态。

- npm package: dingtalk-workspace-cli
- skill/CLI version: 1.0.58
- 各平台 dws 二进制 SHA-256 见 `pinvou3-app/src-tauri/resources/platforms/<os>/<arch>/bundle/connectors/connectors.lock.json`（仓库相对路径；`<os>/<arch>` 取 macos/aarch64、macos/x86_64、linux/aarch64、linux/x86_64、windows/x86_64 五份，版本一致）
- Linux ARM64 dws SHA-256: de6f8a51de83a18cbd2691c1bc03ddc8809d4e33b51fab407c5313fa9d8140ea
- license: Apache-2.0

来源与更新方式（新手操作手册）：

1. 查当前版本：读任一 `connectors.lock.json` 中 `name: "dws"` 的 `version`（当前 1.0.58）。
2. 拉技能源：GitHub Releases 资产，URL 模式
   `https://github.com/DingTalk-Real-AI/dingtalk-workspace-cli/releases/download/v<version>/dws-skills.zip`（当前 v1.0.58；版本号带 `v` 前缀）。
3. 取 mono 形态：zip 解压后顶层为 `NOTICE`、`mono/`、`multi/` 三部分——品悟收录的是 **`mono/` 子目录**（单一 `dws/SKILL.md` 入口 + references/ + scripts/，LICENSE 与 NOTICE 在 mono/ 内也各有一份）；顶层与 `mono/` 内容经 diff 确认一致，`multi/`（dingtalk-chat/ 等 14 个子 skill 布局）不随包分发。核对 zip 真伪可对照同 Release 的 `checksums.txt`。
4. 以该 zip 的 `mono/` 为三方合并基线，按下文登记逐条重放本地修改后，保留本声明。

Pinvou3 随应用内置并按用户连接状态门控该 skill；dws CLI 在首次使用时按 lock 在线下载、校验并安装到用户目录。凭证由官方 CLI 管理。

## Pinvou3 本地修改登记

依据 Apache-2.0 §4(b) 登记对 `dws/SKILL.md` 的本地修改（2026-07-25）。下次升级 dws npm 版本时本节修改需重放。

1. frontmatter `description` 重写：修复「在线电子表格」重复出现，删除随包 CLI 不支持且引用缺失的 AI应用入口，补入目标管理(Agoal)，并压缩为一句话触发说明。
2. 修正脚本能力描述：`scripts/` 下无 AI 应用创建轮询脚本，删除该说法（MUST DO「脚本优先」条与「详细参考」scripts 行两处）。
3. 修正「脚本均支持 `--dry-run` 预览、`--format json` 输出」的不实表述，改为提示各脚本参数不统一、先用 `--help` 确认 flag。
4. 产品总览表补 `agoal`（目标管理）行，与意图决策树已有路由对齐。
5. 压缩顶部警告块为一行（与「命令发现」节内容重复）。
6. `--yes` 确认规则去重：删除「确认流程」三步代码块与「命令发现」节末尾重复句，确认方式合并为「危险操作确认」节开头一句。
7. 「核心流程」删除元话术，压缩为 0-3 步（URL 预检/意图分类/歧义追问/选定产品读参考后执行）。
8. MUST DO 参数格式括号注压缩。
9. 「详细参考」中 best_practices 逐文件枚举压缩为单行汇总，aitable 两行合并为一行。

除上述 9 条外，仓库对 `dws/` 另有两处已登记于 git 历史的本地修改，同步时同样重放：

- `references/products/attendance.md`、`references/products/minutes.md`：将宿主已退役的工具名 `read_file` 改为 `File(action="read")`（CodeWhale v0.9.5 canonical 工具族适配，PR #231）。
- `scripts/attendance_report_common.py`：图片缓存文件名的 URL 哈希由 MD5 改为 SHA-256（CodeQL py/weak-sensitive-data-hashing，PR #54）。

## 同步记录（2026-08-16 → 1.0.58）

本次同步自 v1.0.58 dws-skills.zip 的 mono 形态（zip 顶层与 mono/ 内容经 diff 确认一致）。`dws/LICENSE`、`dws/NOTICE` 与上游一致，未改动。

上游结构变化（1.0.51 → 1.0.58 mono）：

- references 新增：`products/event.md`、`products/hrbrain.md`、`products/markdown.md`、`products/pat.md`、`products/whiteboard.md`、`products/whiteboard/`（open-nodes-v1 全套 + recipes）、`products/oa/`（表单组件/流程节点）。上游另有 `channel-login.md`，品悟不随包分发（见下方补录第 9 条）。
- references 删除：`recovery-guide.md`（SKILL.md「错误处理」同步移除 RECOVERY_EVENT_ID 闭环说明）。（状态注：已删，无需重放。）
- scripts 删除：`bot_broadcast.py`、`chat_export_messages.py`、`chat_history_with_user.py`、`doc_create_and_write.py`、`extract_media_id.py`（Chat 历史导出与机器人广播下沉 Runtime）。（状态注：已删，无需重放。）
- SKILL.md 大改：新增 Shortcut 使用原则/总览、多组织多账号（profile）、确认门禁协议、Schema 渐进查询；产品域新增 hrbrain/markdown/pat/whiteboard/event，`aiapp` 移除（标注无稳定产品参考），`agoal` 保留（mono 意图树仍路由，CLI 1.0.58 `--help` 服务列表仍含 agoal）。

9 条登记重放结果（逐条）：

1. description 重写 — 已重放（保持一句话触发式，压缩至 280 字符截断上限内，并纳入 1.0.58 新产品域组织大脑/Markdown/白板/事件订阅；上游新 description 未含 Agoal，本地继续补入）。
2. 脚本能力描述修正 — 已重放（1.0.58 删除了 5 个脚本但 MUST DO 新文本仍提及「AI 应用创建轮询、文档创建后写内容」两个已不存在脚本的场景，且「详细参考」scripts 行仍列「文档创建并写入」，两处均删除该说法）。
3. `--dry-run`/`--format json` 不实表述 — 上游已等价解决（上游删除「脚本均支持」句式，改为按脚本说明参数，无需重放）。
4. 产品总览表补 `agoal` 行 — 已重放（1.0.58 mono 产品表仍缺 agoal 行而意图决策树仍路由 `agoal`，实测 CLI 1.0.58 仍提供 `agoal` 服务，继续对齐补入）。
5. 压缩顶部警告块为一行 — 已重放（1.0.58 警告块引入 Schema 事实源语义，压缩为一行时保留该要点）。
6. `--yes` 确认规则去重 — 已重放（删除「确认流程」三步代码块，确认方式并入「危险操作确认」开头一句；上游新增的「确认门禁的识别与重试协议」小节为新增语义，保留）。
7. 「核心流程」压缩为 0-3 步 — 已重放（删除元话术；上游新增第 4 步「按任务最小化读取」的语义并入第 3 步）。
8. MUST DO 参数格式括号注压缩 — 已重放（沿用「参数与参数值之间用空格隔开」）。
9. best_practices 逐文件枚举压缩为单行汇总、aitable 两行合并 — 已重放（1.0.58 上游改为 14 行逐文件枚举，压缩回单行汇总；新增的 pat.md 参考行保留独立行）。

另重放 git 历史登记的两处修改：`read_file` → `File(action="read")`（attendance.md 6 处、minutes.md 6 处，与 PR #231 一致）；`attendance_report_common.py` 缓存哈希 md5 → sha256（与 PR #54 一致）。1.0.51 导入时的 4 个文件尾随空白差异（07-minutes.md/08-directory.md/calendar.md/oa.md）不再重放，跟随上游原文；对账注（2026-08-16，对账基线为 HEAD `77c19912`）：calendar.md 与 oa.md 在该基线上与上游逐字节一致；07-minutes.md 与 08-directory.md 因后续真实性审查补录的正文修改重新分叉（diff 实测差异均为内容行，无空白-only 行）。对账期间（2026-08-16 15:2x）工作区另出现一批未提交的尾随空白清理（07-minutes.md/08-directory.md/minutes.md/oa.md），不属于四轮登记范围——若保留须随提交另行登记，否则下次 sync 按本登记跟随上游原文。
  （第六轮核验注 2026-08-16，基线 HEAD `4a42fdb9`、工作区 clean：该批尾随空白清理已不在工作区——上述 4 文件当前与上游的尾随空白行数一致（07/08/oa/minutes 分别 1/1/3/5 行，与上游同），oa.md 与上游逐字节一致，07/08/minutes 的差异均为正文行（08-directory 为 File(action="read") + aisearch 链接化；07-minutes 为「开源版未引入」标注删除；minutes.md 为 read_file → File(action="read")）。本对账注的「未提交清理」悬念已消除，下次 sync 直接跟随上游原文重放各登记条目即可。）

## 真实性审查补录（2026-08-16，同轮次复审）

同步后复审发现并修复的机械迁移问题（均在 `dws/` 内，CLI 1.0.58 实测核对）：

1. `File(action="read")` 工具名漏网修复 12 处：`doc/` 下 10 个子文档与 `doc/style/doc-create-workflow.md`、`sheet/sheet-comment.md` 的「必须先用 Read 工具读取」前置块（上轮仅重放了 attendance.md/minutes.md，doc/sheet 子文档漏改）。
2. `SKILL.md` Shortcut 总览表删除「multi skill」列（`dingtalk-aitable`/`dingtalk-misc` 等 16 处 multi 形态子 skill 名，mono 收录形态不存在这些入口）；shortcut 计数经 `dws shortcut list --service <svc>` 逐一实测与 1.0.58 一致，保留。
3. `SKILL.md` 意图决策树 aiapp 行删除「multi 布局见 `dingtalk-misc` 的 `unsupported-scripts.md`」尾注（mono 包内无该文件）。
4. `references/products/calendar.md`：3 个脚本链接 `../scripts/` → `../../scripts/`（路径错误导致悬空）；「相关产品」中 `../../dingtalk-contact/references/contact.md` 悬空链接改为 `./contact.md`（mono 包内实际路径 `references/products/contact.md`）。
5. `references/products/sheet.md`：删除标题「原 dingtalk-sheet/SKILL.md 正文」multi 话术；「跨产品协作」两处 `dingtalk-aitable`/`dingtalk-doc` 子 skill 引用改为指向包内 `./aitable.md`/`./doc.md`。
6. `references/best_practices/07-minutes.md`：删除 2 处「（开源版未引入）」不实标注（`minutes_extract_todos.py`/`minutes_recent_summary.py` 均随包存在）；browse-minutes 中脚本参数 `--limit` 修正为脚本实际定义的 `--max`。
7. `references/best_practices/`（08-directory.md、10-minutes-speaker-match.md、lite-recipes.md）：3 处「`aisearch`（开源版未引入，悟空内部产品）」改为链接 `../products/aisearch.md`（该产品参考随包存在，服务在 1.0.58 服务列表内）。
8. `references/products/report.md` 示例表格中占位链接 `[在钉钉中查看日志](...)` 目标改为 `(<dingtalkOpenUrl>)`（与同文档操作列规则用语一致，避免悬空 `...` 目标）。
9. 删除 `references/channel-login.md`：该文件是上游面向阿里内部受控渠道场景的配置参考，含内部评测渠道的具体 `DWS_CHANNEL` 哈希、内部 profile 名与「EI智能体评测」渠道归因，对品悟社区版用户无意义且违反「不依赖企业专属数据」的社区版公约；文件未被包内任何其他文档引用。后续 sync 若上游仍带此文件，继续不随包分发。

## 第四轮结构终检补录（2026-08-16）

1. `dws/SKILL.md` frontmatter 补 `metadata.requires.bins: ["dws"]` 与 `metadata.cliHelp: "dws --help"`，与 lark/wecom/tmeet 各域对齐（`dws --help` 实测存在）。下次 sync 需重放。
2. description 加【何时用:…】防误用前缀（240 字符，≤280 上限）。下次 sync 需重放。

## 第四轮本地工具依赖审查补录（2026-08-16，跨平台与 Agent 会话口径）

品悟为三端应用（macOS/Linux/Windows），Windows 不保证本地 `jq`/`grep` 可用；dws 全局 `--jq` 对产品命令已生效（global-reference.md 明示）。以下修改均改为 CLI 内置能力或模型直接读取，下次 sync 需重放：

1. `references/products/aitable/aitable-workflow.md`：创建确认流删除 `| tee` + 本地 `jq` 链（3 命令）改为 `--jq` 直出；list 汇总/状态过滤 3 处 `| jq` 改 `--jq`；`for ... | jq -r` 批量禁用循环改为「list --jq 列 flowId + 逐条 disable」两步，消除 shell 循环依赖。
2. `references/products/aitable/aitable-record-history.md`：3 处 `| jq` 改 `--jq`。
3. `references/products/aitable/aitable-record-share.md`：批量示例 `| jq` 改 `--jq`。
4. `references/products/aitable/aitable-view-config.md`：Kanban 封面 `for` 循环 `| jq .status` 改逐视图命令 + `--jq .status`。
5. `references/products/doc/style/doc-update-workflow.md`：读取策略表 `grep -B2 -A2`/`grep -n` 定位改为模型直接读 JSON/markdown（并给 `--jq '.. | .text? // empty'` 备选）。
6. `references/products/event.md`：Subprocess contract 的 stdin 常驻技巧 `< <(tail -f /dev/null)`（POSIX-only）改为「Agent 会话跑批用 `--max-events`/`--duration`，或 `< /dev/null`，进程替换仅 POSIX 可用」；订阅清理条补「Agent 会话优先有界退出而非外部 kill」。`--max-events`/`--duration` 经 `dws event consume --help` 实测存在。

## 第三轮盲区审查补录（2026-08-16，此前漏登记，NOTICE 对账补录）

第三轮（c9c6afb3）对 `dws/` 深层 references 的以下修改此前未登记，依据 v1.0.58 mono 上游 diff 实测补录，下次 sync 需重放（均经 dws 1.0.58 `--help` 实测核对；下列裸写文件均位于 `dws/references/` 下，全称即 `references/<文件>`）：

1. `capability-limits.md`：「文档权限管理 ⚠️ transitional helper 部分支持」改写为「✅ 已支持：节点级协作者管理走 `dws drive permission add/update/list/remove`（仅文档空间节点，不适用于钉盘文件）；按姓名批量授权走 `dws doc +access-grant` / `+access-change` / `+access-revoke`」（实测口径）。
2. `field-rules.md`：field 子命令计数「4 个」改「5 个」——补 `field search-options`（搜索单选/多选字段选项）行，并注明 `list` 仅是 `field get` 的别名。
3. `url-patterns.md` 三处：axls 条补「导出用 `dws sheet export`（单命令一站式提交→轮询→可选下载）」；「在线表格导出上游未暴露」改为「用 `dws sheet export`（axls→xlsx），单工作表纯 CSV 用 `dws sheet export-csv`」；「分享」路由表两条的 `doc permission add --user` 改为 `dws drive permission add --users`（按姓名授权用 `doc +access-grant`），区分依据同步改为 drive permission。
4. `references/products/markdown.md`：命令表补 `markdown diff` 行，并新增「比较差异」节（`--node`/`--version`/`--version2`/`--file`/`--context` 用法与 4 种组合示例，经 `dws markdown diff --help` 实测）。
5. `references/products/doc/doc-permission.md`（2026-08-17 计数勘误：实测为 14 行，上游原文含 14 处 `--user`、本地 0 处，原记「9 处」不准）：`--user` → `--users`（flag 实名，`dws doc permission add --help` 实测）；`--max-results` → `--limit`；角色「大小写敏感，必须全大写」改「大小写不敏感」。
6. `references/products/doc/doc-export.md`：标题与 `--export-format` 说明由「仅 docx」改「docx（默认）/ markdown（或 md）/ pdf」，补 markdown 导出示例与「`--output` 传目录时按格式自动追加扩展名」。
7. `references/products/event.md`：删除末尾 Full reference 节的 3 条 multi 形态断链（`skills/multi/dingtalk-event/…`），改为「命令级入参用 `dws event consume --help`、`dws event +listen-im --help` 查看；事件目录 `dws event list`（可加 `--category oa`）；单事件字段 `dws event schema <event_key> --flatten`」（mono 收录形态无 multi 目录）。
8. `references/products/doc.md` 导出格式口径 3 处（2026-08-17 评审补充，第 6 条同源修正的传播；`dws doc export --help` 实测为 docx（默认）/markdown (.md)/pdf）：`--export-format` flag 说明行「当前仅支持 docx (默认)」改「导出格式: docx (默认) / markdown (或 md) / pdf」；「用户说下载/导出」路由条「格式转换后导出为 docx，未迁移」与末尾「严禁路由到 drive download」提示条「导出为 docx」均改为「按 `--export-format` 导出为 docx（默认）/ markdown / pdf」并指向 `./doc/doc-export.md`。
9. （2026-08-17 第十轮独立复审补充，第 5 条 `--user` 修正的两处传播漏网）`references/products/doc.md` 权限路由摘要行「`permission add`（需 `--node` + `--user` + `--role`）」改 `--users`；`references/best_practices/04-document.md` grant-doc-access 行「`doc permission add --node <nodeId> --user <UID1,UID2> --role EDITOR`」改 `--users`。另注：`chat message list --user`、`calendar acl add --user` 等处的 `--user` 是各自命令的实名 flag，**不要**随本条误改。
10. （2026-08-17 第十轮复审勘误）第 1 条「minutes.md 6 处」实为 7 行：`references/products/minutes.md` 的 `Read 工具`/`read_file` → `File(action="read")` 共 5 处标准形态 + 2 处 `File.read` 简写（R4 行与速查表行，简写一并改写为完整形态）。
11. （2026-08-17 第十一轮复审补充，第 6/8 条导出口径的同族传播漏网 + 二进制实测修正）`doc-export.md`「关键说明」区「当前仅支持…导出为 `docx`，在线表格导出请使用其他命令」改为「仅作用于 alidocs 文档、`--export-format` 决定 docx/markdown/pdf，axls 走 `dws sheet export`」，`--output` 目录扩展名同步改三格式口径；`doc.md` 注意事项区同款「仅支持 docx」+「当前开源 CLI 未暴露在线表格导出命令」（失实：`dws sheet export` 存在且 sheet.md/url-patterns.md 已文档化）一并改写。
12. （2026-08-17 第十一轮复审，失效 flag 修正）`best_practices/04-document.md` export-doc-as-docx 行的 `--timeout-sec 600` 删除（实测 `unknown flag`；`doc export` 无 timeout flag，轮询窗口内置约 5 分钟）；`best_practices/06-data-analytics.md` export-aitable-to-xlsx 行「加 `--timeout-sec 900`」改写为「`--timeout-ms` 默认且上限 30000、超时返回 taskId 续等」（与 aitable-export-import.md「大表/长任务超时续等」节口径对齐，实测一致）。脚本自身的 `--timeout-sec`（aitable_export/import_via_task.py argparse 参数）不受影响，勿随本条误删。

## 第六轮收官语义扫描补录（2026-08-16）

按统一模式清单（上游宿主名/不存在形态措辞/npm 残留/skills 管理命令/裸 auth login/Read 工具名/api-version/假命令/multi 形态/已删文件引用/自更新指令/内部 URL 密钥）对四个技能包全量复扫，`dws/` 内修复以下残留，下次 sync 需重放：

1. `scripts/` 8 个考勤脚本 docstring 的强制门禁阅读路径 `dingtalk-workspace/references/products/attendance-*.md` 改为包内相对路径 `../references/products/attendance-*.md`（上游仓库名形态在品悟包内不存在，实际文件位于 `dws/references/products/`）：attendance_report_checkin.py、attendance_report_common.py、attendance_report_daily.py、attendance_report_detail.py、attendance_report_monthly.py、attendance_schedule_export.py、attendance_schedule_import.py、attendance_vacation_balance.py。
2. `references/products/minutes.md` 跨平台兼容性条：「悟空运行环境可能是 Windows cmd / PowerShell / macOS bash」宿主断言改为「品悟应用运行在 Windows cmd / PowerShell / macOS bash 等不同 shell 环境」（同文档其余「悟空」均为听记热词/替换示例词，非宿主口径，保留）。
3. `references/best_practices/06-data-analytics.md` export-aitable-to-xlsx 条：「与悟空脚本路径并存」改为「与脚本路径并存」（指包内 `scripts/aitable_export_via_task.py`，非悟空平台）。
4. `references/products/sheet/sheet-filter.md`：删除「（参照飞书 core-operations）」括注（lark 域 `lark-sheets-core-operations.md` 未随包收录，为悬空跨域参照；规范正文本身完整，删除后语义不变）。
5. `references/products/attendance.md` 命令可用性提示：禁止性理由「不要以"开源版不支持"为由拒答」改为「不要以"命令不存在/不支持"为由拒答」（保留禁拒答语义，去除上游"开源版"形态措辞，为真实性审查补录第 6/7 条「开源版未引入」清理的漏网变体）。

其余命中均为已登记豁免或合理保留：`OPENCLAW_WORKSPACE` 环境变量（scripts/import_records.py、bulk_add_fields.py 路径安全护栏，未设时回退 `os.getcwd()`，品悟内不依赖该变量亦可用，上游原样保留）；dws/tmeet 域裸 `auth login`（各自 CLI 真实教学）；`alidocs.dingtalk.com`（钉钉文档公网 URL 域名，非内部地址）。

## 第七轮脚本代码安全审计（2026-08-16）

对 `dws/scripts/` 全部 33 个 Python 脚本做逐脚本安全审计（路径安全 / 注入面 / 危险操作 / 数据外泄面 / 异常与退出码 / 资源占用 / 跨平台编码）。审计结论：无 shell=True、无 os.system/eval/exec/pickle、无递归删除；subprocess 全部列表形式；文件读入均有大小上限；`OPENCLAW_WORKSPACE` 白名单无 cwd 覆盖绕过。修复以下 5 类，下次 sync 需重放：

1. **subprocess 解码容错（25 处，全部含 `text=True` 的调用；2026-08-17 计数勘误：原记 26，另 3 个 `text=True` 命中是 openpyxl 样式参数、非 subprocess）**：`subprocess.run(..., text=True)` 均补 `errors='replace'`。此前 Windows GBK 控制台下 dws 输出含 GBK 外字符（emoji、✓✗ 等 BMP 符号）会抛未捕获 `UnicodeDecodeError`，脚本以 traceback 崩溃且退出码不可控；`errors='replace'` 保证 stderr 信息完整、失败路径仍走受控 return-None/sys.exit。涉及 scripts/ 下 25 个脚本（全覆盖，逐调用核对无一遗漏）。
2. **白名单符号链接绕过防护（2 处，复核勘误）**：`import_records.py` 与 `bulk_add_fields.py` 的 `resolve_safe_path` 补「relative_to 失败且目标本身是符号链接时按『路径解析到白名单外』拒绝」分支（保留原错误信息格式）。（2026-08-16 独立复审勘误：实测拦截实际由原有 `(Path.cwd()/path).resolve()` + `relative_to(allowed_root)` 逻辑提供——`resolve()` 跟随符号链接、越界即抛 ValueError，根内链接指向根外/绝对路径/`../` 在补丁前后均被拒绝，本条早先「此前放行」的表述不准；新增分支属纵深防御冗余，保留无害，sync 重放可选。）
3. **导出文件名净化（aitable_export_via_task.py）**：服务端返回的 `fileName` 未校验直接 `Path.cwd() / file_name` 落盘，含 `../` 可越目录写、含 Windows 保留字符（`:*?"<>|`）创建失败、设备名（CON/NUL）被劫持。新增 `safe_file_name()`：截 basename、非法字符替换 `_`、设备名加 `_` 前缀；默认下载路径与轮询更新 fileName 两处接入（`--output` 显式指定时尊重用户路径）。
4. **翻页死循环防护（report_inbox_today.py、report_received_today.py）**：`fetch_inbox` 的 cursor 翻页循环无重复游标检测，服务端异常恒定返回同一 `nextCursor` 时无限重拉 dws。补 `seen_cursors` 集合，重复游标 stderr 报错后 break。已 monkeypatch 实测恒定游标在第 2 次调用即终止。
5. **维持原样的安全判定（登记豁免，勿在上游 sync 时"修复"回去）**：`aitable_import_via_task.py`/`upload_attachment.py`/`aitable_export_via_task.py` 的 urlopen 仅用于 CLI 返回的 OSS uploadUrl/downloadUrl 直传/直下（CLI 未封装该原语，属合理例外）；`attendance_report_common.py` 图片下载为 requests.get + sha256 缓存键、`tempfile.gettempdir()` 固定子目录但文件名为 URL sha256（不可抢注）；`OPENCLAW_WORKSPACE` 回退 `os.getcwd()` 属白名单根退化、非绕过（绝对路径语义按设计放行）。

验证：全量 `python3 -m py_compile` 通过；usage-error 路径 exit code 非 0；`--help` 冒烟通过。注：第 2-4 类防护的验证为开发期一次性实测/monkeypatch（未提交可重放单测），本仓库防回归门禁仅覆盖文档口径（`connector_skills_contract.test.js` 的 python3 断言）；如需长期回归防护，后续可在 `pinvou3-app/tests/` 补脚本级单测。

## PR #299 审阅代修（2026-08-17）

对第七轮登记的第 2、3 条做收口修正，下次 sync 需重放：

1. **`resolve_safe_path` 符号链接冗余分支删除（import_records.py、bulk_add_fields.py）**：第七轮第 2 条新增的 `target_path.is_symlink()` 分支检查的是 `resolve()` 之后的路径（恒为 False，死代码），且 NOTICE 已勘误拦截实际由 `resolve()` + `relative_to()` 提供。删除该死分支，统一为「路径超出允许范围」错误信息，注释改为说明 resolve 语义；安全属性不变（实测 `../`、绝对路径、根内符号链接指向根外仍全部拒绝）。
2. **`safe_file_name` 截断改为按 UTF-8 字节（aitable_export_via_task.py）**：原实现两处缺陷——扩展名本身超 200 字符时 `200 - ext_len` 为负索引、截断后反而更长；按字符数截断 200 对中文名无效（200 中文字符约 600 字节，仍超 macOS 255 字节限制）。改为按 UTF-8 字节截断到 200 字节，扩展名 ≤20 字节时保留、超长扩展名按无主名扩展整体截断；docstring 同步更正。

### 第七轮文档层走查类补登记（2026-08-16，独立复审补录）

第七轮同批 commit 还包含以下文档层改动（此前未登记，依据上游 v1.0.58 zip diff 实测，下次 sync 需逐条重放）：

- **裸 `python` → `python3` 改写（55 处/19 个 references 文件，上游 v1.0.58 原文均为裸 `python`；2026-08-17 计数勘误：原记「约 50 处/17 个」为低估）**：`references/` 下全部命令示例统一为 `python3 scripts/...`（品悟宿主无裸 `python`，实测 command not found）。脚本自身 docstring 中的 usage 示例仍为上游原文裸 `python`，属已知豁免（模型按 SKILL.md 调用约定执行，不按 docstring）。
- **SKILL.md 新增「脚本调用约定」节**：统一 `python3` 调用、`scripts/...` 相对路径须拼完整路径、不假设 CWD（上游 SKILL.md 无此节）。
- **attendance-report.md**：「报表类型（四选一）」改「五选一」（补签到报表项，`attendance_report_record.py` 覆盖考勤记录/签到报表两型）；「不适用于」指引的三处命令指针修正（`attendance check record`→`attendance record get`、班次查询补 `attendance class search` 并将排班导出导向 attendance-schedule.md 工作流）。

## PR #299 审阅代修（2026-08-18）

协作者复审阻塞项收口，下次 sync 需重放：

1. **`safe_file_name` 补 Windows 尾随点/全点名规整（aitable_export_via_task.py）**：Win32 剥离末组件尾随点/空格——`"report.xlsx."` 落盘被静默改名为 `report.xlsx`（与回显 savedPath 不一致）；`"..."` 等全点名是目录别名、`write_bytes` 抛 `PermissionError`；超长字节截断也可能切在点上留下新尾点。修复：字符替换后 `rstrip(". ")`、剥空后回退 `export_result.bin`（取代原先仅判 `"."`/`".."` 的分支）、设备名判断与字节截断之后各再规整一次。补可重放纯函数单测 `scripts/tests/test_dws_safe_file_name.py`（fast-gate `unittest discover` 自动执行）——第七轮登记注「第 3 条仅一次性实测」自此有持久化回归防护。
