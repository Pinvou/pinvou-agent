钉钉内置技能来自钉钉官方 dingtalk-workspace-cli 的 dws-skills.zip mono 形态。

- npm package: dingtalk-workspace-cli
- skill/CLI version: 1.0.58
- 各平台 dws 二进制 SHA-256 见 `resources/platforms/<os>/<arch>/bundle/connectors/connectors.lock.json`
- Linux ARM64 dws SHA-256: de6f8a51de83a18cbd2691c1bc03ddc8809d4e33b51fab407c5313fa9d8140ea
- license: Apache-2.0

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
- references 删除：`recovery-guide.md`（SKILL.md「错误处理」同步移除 RECOVERY_EVENT_ID 闭环说明）。
- scripts 删除：`bot_broadcast.py`、`chat_export_messages.py`、`chat_history_with_user.py`、`doc_create_and_write.py`、`extract_media_id.py`（Chat 历史导出与机器人广播下沉 Runtime）。
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

另重放 git 历史登记的两处修改：`read_file` → `File(action="read")`（attendance.md 6 处、minutes.md 6 处，与 PR #231 一致）；`attendance_report_common.py` 缓存哈希 md5 → sha256（与 PR #54 一致）。1.0.51 导入时的 4 个文件尾随空白差异（07-minutes.md/08-directory.md/calendar.md/oa.md）不再重放，跟随上游原文。

## 真实性审查补录（2026-08-16，同轮次复审）

同步后复审发现并修复的机械迁移问题（均在 `dws/` 内，CLI 1.0.58 实测核对）：

1. `File(action="read")` 工具名漏网修复 12 处：`doc/` 下 10 个子文档与 `doc/style/doc-create-workflow.md`、`sheet/sheet-comment.md` 的「必须先用 Read 工具读取」前置块（上轮仅重放了 attendance.md/minutes.md，doc/sheet 子文档漏改）。
2. `SKILL.md` Shortcut 总览表删除「multi skill」列（`dingtalk-aitable`/`dingtalk-misc` 等 16 处 multi 形态子 skill 名，mono 收录形态不存在这些入口）；shortcut 计数经 `dws shortcut list --service <svc>` 逐一实测与 1.0.58 一致，保留。
3. `SKILL.md` 意图决策树 aiapp 行删除「multi 布局见 `dingtalk-misc` 的 `unsupported-scripts.md`」尾注（mono 包内无该文件）。
4. `references/products/calendar.md`：3 个脚本链接 `../scripts/` → `../../scripts/`（路径错误导致悬空）；「相关产品」中 `../../dingtalk-contact/references/contact.md` 悬空链接改为 `./contact.md`（mono 包内实际路径）。
5. `references/products/sheet.md`：删除标题「原 dingtalk-sheet/SKILL.md 正文」multi 话术；「跨产品协作」两处 `dingtalk-aitable`/`dingtalk-doc` 子 skill 引用改为指向包内 `./aitable.md`/`./doc.md`。
6. `references/best_practices/07-minutes.md`：删除 2 处「（开源版未引入）」不实标注（`minutes_extract_todos.py`/`minutes_recent_summary.py` 均随包存在）；browse-minutes 中脚本参数 `--limit` 修正为脚本实际定义的 `--max`。
7. `references/best_practices/`（08-directory.md、10-minutes-speaker-match.md、lite-recipes.md）：3 处「`aisearch`（开源版未引入，悟空内部产品）」改为链接 `../products/aisearch.md`（该产品参考随包存在，服务在 1.0.58 服务列表内）。
8. `references/products/report.md` 示例表格中占位链接 `[在钉钉中查看日志](...)` 目标改为 `(<dingtalkOpenUrl>)`（与同文档操作列规则用语一致，避免悬空 `...` 目标）。
9. 删除 `references/channel-login.md`：该文件是上游面向阿里内部受控渠道场景的配置参考，含内部评测渠道的具体 `DWS_CHANNEL` 哈希、内部 profile 名与「EI智能体评测」渠道归因，对品悟社区版用户无意义且违反「不依赖企业专属数据」的社区版公约；文件未被包内任何其他文档引用。后续 sync 若上游仍带此文件，继续不随包分发。
