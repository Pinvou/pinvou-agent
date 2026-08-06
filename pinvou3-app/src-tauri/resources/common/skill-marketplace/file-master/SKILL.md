---
name: file-master
description: 本机文件管理——找文件与磁盘清理。用户说"找文件/文件在哪/搜索文件/帮我找一下xxx/那个文件放哪了/xxx存在哪个目录"时走「文件查找」流程（mcp_file_master_file_find 按文件名/目录名搜索常用目录并兜底主目录，支持 extensions/modified_after/modified_before 过滤）；用户说"C盘满了/磁盘满了/空间不够/清理电脑/清理缓存/释放空间/什么东西占空间/磁盘占用太多"时走「磁盘清理」流程（mcp_file_master_disk_scan 只读扫描系统盘与非系统盘、三级风险呈现、path 下钻，mcp_file_master_file_trash 后台异步移入回收站 + mcp_file_master_file_trash_status 轮询进度，mcp_file_master_file_empty_recycle 清空回收站，mcp_file_master_file_restore 误删还原）。本技能随工具市场「文件管理大师」挂载生效。不负责：检索知识集内容（走 kb_search）、读已知路径的文件（走 read_file）、按文件内容搜索（走 grep_files）、运行内存/RAM 占用问题。
metadata:
  requires:
    mcp: ["file-master"]
  note: "本技能随工具市场「文件管理大师」挂载生效——它提供 mcp_file_master_disk_layout / mcp_file_master_file_find / mcp_file_master_disk_scan / mcp_file_master_file_trash / mcp_file_master_file_trash_status / mcp_file_master_file_empty_recycle / mcp_file_master_file_erase / mcp_file_master_file_restore 八个工具；未安装时无法执行本机文件查找、扫描、回收站删除与清空、_pinvou_filemaster_trash 物理清除。"
---

# 文件管理（file-master）

帮用户管理本机文件：**找到文件在哪**，并安全回答"**C 盘空间被什么吃了**"。核心工具全部由「文件管理大师」MCP 提供：`mcp_file_master_disk_layout`（毫秒级盘符组成，开工先确认）、`mcp_file_master_file_find`（搜索，可过滤）、`mcp_file_master_disk_scan`（只读扫描，含非系统盘）、`mcp_file_master_file_trash`（后台异步删除）、`mcp_file_master_file_trash_status`（删除/清除进度轮询）、`mcp_file_master_file_empty_recycle`（清空回收站）、`mcp_file_master_file_erase`（物理删除 _pinvou_filemaster_trash 兜底内容）、`mcp_file_master_file_restore`（误删还原）。

## 通用铁律

1. **开工前先确认盘符组成**：找文件/扫描开始前，先调 `mcp_file_master_disk_layout`（毫秒级）确认本机有哪些盘、系统盘是哪个（不假设是 C:）；用户没给位置时，若目标可能在资料盘/项目盘，直接对盘根 `dir` 定向搜。
2. **先 MCP 工具，再 shell。** 全盘查找一律先调 `mcp_file_master_file_find`，磁盘占用统计首选 `mcp_file_master_disk_scan`；不要一上来就 `exec_shell` 跑 `dir /s` / `find` / PowerShell 统计——慢且输出爆炸、易被压缩截断。`exec_shell` 仅作兜底（工具报错或覆盖不到的目标时用），且命令必须带过滤/限量（如 `| Select-Object -First 20`），不要全量输出。
3. **删除只能走 `mcp_file_master_file_trash`**（Windows 移入系统回收站；macOS 移入废纸篓 `~/.Trash`；Linux 移入 XDG Trash——均可恢复，via 在结果里注明）。绝不用 `exec_shell`、`apply_patch` 或任何方式直接物理删除用户文件。**Windows 上超过回收站配额的目标（默认约为盘容量 5%）工具会自动改用同级 `_pinvou_filemaster_trash` 目录兜底**（Shell 对超配额对象会静默物理删除且误报成功，不可恢复）；macOS/Linux 上跨卷或失败时同样落 `_pinvou_filemaster_trash` 兜底——此时 `detail` 会明确说明。**物理删除只能走 `mcp_file_master_file_erase`，且仅限 file_trash 移入的备份**（_pinvou_filemaster_trash 兜底 / 系统废纸篓 / XDG Trash 内容，白名单 + 日志准入 + confirm 两步，不可恢复）。
4. **动文件必须用户点名。** 找到文件后只做"告知位置 / 经同意后打开所在目录"；移动、改名、删除必须用户明确要求并确认后另行处理。
5. **系统区域是禁区。** `C:\Windows`、`Program Files`、`pagefile.sys`、`hiberfil.sys`、注册表——不碰，只向用户解释。`mcp_file_master_file_trash` 内置白名单也会硬拒绝这些区域。

## 危险操作警示与免责声明

- **不可恢复的操作**：`file_empty_recycle`（清空回收站/废纸篓）与 `file_erase`（物理删除 _pinvou_filemaster_trash 内容）为物理删除，**不可恢复**；执行前必须向用户明确说明并取得确认，绝不因用户催促而省略确认步骤。
- **磁盘清理的影响面**：即使是 🟢 缓存，清理后个别应用可能重新下载数据或重置设置；删除前向用户说明影响面。
- **尽力保障但非保证**：本工具通过白名单、删除日志、双重确认与配额兜底降低误删风险，但**不保证覆盖所有异常场景**（系统/第三方工具并发修改文件、文件被占用、回收站被手动清空等）；因使用本工具造成的误删或数据损失，工具与项目方不承担责任，重要数据请用户先备份。

---

## 一、文件查找

帮用户在**整台电脑**上找到文件，告诉他/她文件在哪个目录。按概率序搜 Desktop/Documents/Downloads/Pictures/Videos/Music，再对主目录、根目录以及其他盘符下的目录进行搜索。

### 何时用 / 何时不用

- ✅ 用：找文件、定位所在目录、"xxx 文件放哪了"、确认某文件是否存在、找同名文件的所有副本。
- ❌ 不用：检索知识集内容 → `kb_search`；读已知路径的文件 → `read_file`；按文件**内容**搜索 → `grep_files`；清理磁盘 → 走下面「磁盘清理」流程。

### 搜索阶梯（按顺序走，找不到要迭代，不要放弃——至少走完 2 轮关键词放宽再如实告知）

1. **提取关键词**：从用户原话提取核心名词/文件名片段，去掉"帮我找一下""那个"等废话。例："上周那个关于散热的报表" → 先试 `散热`。**多词用空格分隔（AND 语义）**：用户记得多个片段（"report final"/"周报 散热"）→ 直接 `query="report final"`，只有名字同时含所有词才命中，比单子串更精准。**注意：多词是 AND——不要把候选词堆一起**（"install setup 安装" 会因全命中要求而必 miss）；**找某一类文件（"所有安装包/所有图片"）→ query 留空 + `extensions=["exe","msi"]`** 做纯类型搜索（query 留空时必须有 extensions/大小/时间过滤条件，否则工具拒绝）。**用户提到时间（上周/最近几天/昨天/某月）→ 直接用 `modified_after`/`modified_before` 过滤**——支持相对天数 `Nd`（`7d`=最近 7 天、`3d`=最近 3 天、`1d`=今天起），**用户说"上周/最近"直接转述为 Nd，不要自己推算具体日期**（模型推算日期会把年份算错，本机按今天计算才可靠）。换词搜不到时间线索，过滤才是正解。
2. **用户给了大致位置 → 直接定向搜**：用户说"在 D 盘某个项目里/在桌面上" → `mcp_file_master_file_find(query="<关键词>", dir="D:\\myWork")`（绝对路径，参考「路径先验」），一步命中，不用全盘扫。
3. **`mcp_file_master_file_find` 搜索**：`mcp_file_master_file_find(query="<关键词>")`。命中 → 去「结果呈现」。
4. **按类型/时间/大小收窄**：用户说"上周的 xlsx 报表/那个 pdf 文件/超过 1GB 的文件" → 加过滤参数：`extensions=["xlsx"]`（扩展名过滤，目录不参与）、**`modified_after="7d"`（相对天数：最近 7 天；"上周"→7d、"最近几天"→3d、"昨天"→1d，不要自己算日期）**、`min_size_mb=1000` / `max_size_mb=10`（大小过滤，只作用于文件）。参数非法（如 `2024/1/1`、负数）会返回 error 说明格式。
5. **看覆盖范围再决定下一步**：结果里的 `searched_dirs` 告诉你实际搜了哪些目录（默认 = 常用目录 + 主目录整体兜底，AppData 下应用目录如 `C:\Users\xxx\AppData\Local\QianwenUpdater` 也能命中）；`truncated: true` 表示超时截断、结果可能不全——值得换更精确关键词重试，或拿到位置线索后用 `dir` 定向补搜。
6. **关键词变体重试**（每次未命中换一招，不重复同一 query）：
   - 缩短/拆词：`散热周报表` → `散热`；去掉版本号、日期、括号等后缀
   - 中英文互换：报表 → report、汇总 → summary、会议 → meeting、方案 → plan
   - 换同义说法：简历 → CV、照片 → img/photo
7. 仍无结果 → **按盘符组成逐盘定向补搜**：结合 `disk_layout` 的盘符列表，按用户线索优先级对每个盘根 `dir` 定向搜（如 `dir="D:\\"` 搜 D 盘根、`dir="D:\\myWork"` 定向项目目录），每次定向都有完整预算（12 秒），比默认主目录搜索更集中；目录名像项目/工作目录时优先试常见盘符根。所有盘都搜过仍无 → 如实说明"实时搜索未找到"，列出已尝试的关键词与已覆盖目录（`searched_dirs`）。向用户追问大致位置（拿到位置 → 回第 2 步定向搜）或更多线索（大致日期、文件格式）。**要按内容找 → `grep_files`**。

### 路径先验（分平台）

- **Windows**：显示名 ≠ 真实文件夹名——桌面 → `Desktop`、文档 → `Documents`、下载 → `Downloads`、图片 → `Pictures`、视频 → `Videos`、音乐 → `Music`，形如 `C:\Users\<用户名>\Desktop`；微信文件在 `Documents\WeChat Files`（新版 `xwechat_files`），钉钉/飞书接收的文件多在 `Downloads` 或各自 `AppData` 目录。
- **macOS**：用户目录 `/Users/<用户名>`；应用缓存与数据在 `~/Library/Caches`、`~/Library/Application Support`；Xcode 构建产物在 `~/Library/Developer/Xcode/DerivedData`；废纸篓在 `~/.Trash`；微信/QQ 数据在 `~/Library/Containers/` 下。
- **Linux**：缓存与开发缓存在 `~/.cache`、`~/.npm`、`~/.gradle` 等；回收站（XDG Trash）在 `~/.local/share/Trash`。
- 用户说"桌面上/下载里/文档里"时，若结果太多，用上述真实路径前缀做人工筛选，或直接 `dir` 定向搜。

### 结果呈现

- **多结果不擅挑。** 匹配到多个文件时按相关度列出（全词 > 前缀 > 子串，同分按修改时间从新到旧；路径 + 大小 + 修改时间），编号列表最多 10 条，超过 10 条注明总数并建议补充线索，让用户点名；不得自行选定一个继续操作。结果顺序是"已收集命中里的最优"；**`total_hits` 大于 `count` 说明被 limit 截断**——用户要求"找全"时用 limit=50 或 dir 定向重搜；**"找全某类文件"本质是近似搜索**（时间预算 + limit 限制），如实告知找到的数量与可能不全即可，不要用 min/max_size 分段穷举验证（浪费轮次且仍不保证全）。可用 sort_by/order（mtime/size/name）改排序。
- **单个结果**：直接告知位置，并询问是否需要打开所在文件夹（Windows：`explorer /select,"<完整路径>"`；macOS：`open -R "<路径>"`；执行前先征得用户同意）。

---

## 二、磁盘清理

帮用户回答"C 盘空间被什么吃了"，并安全释放空间。`mcp_file_master_disk_scan` 只读扫描常见文件积聚地（系统盘分组 + `drives` 段附非系统盘容量与根目录大子项），按🟢🟡🔴三级风险列出大文件夹及其数据类型；用户点名要删的项经 `mcp_file_master_file_trash` 后台移入回收站，`mcp_file_master_file_trash_status` 轮询进度。

### 流程（下钻式）

1. **概览**：调 `mcp_file_master_disk_scan()`（无参数，只读）——返回磁盘总览 + 系统盘各分组总量/风险/说明 + **`drives`（非系统盘容量/剩余 + 根目录大子项，一律 🟡 让用户判断）** + >500MB 大文件清单。各组 `status` 为 `estimated` 表示限深/限时估算值、`denied` 表示无权限读取，均属正常，如实转述即可。**展示策略**：每组 `top_children` 展示 ≥组总量 5%（下限 50MB）的子项（最多 30 条），其余归入 `others_count`/`others_size_human`；**`others_count` 非零说明组内还有未展示子项（可能也有大项），应对该组根目录用 path 下钻补看，不要只按已展示的前几个下结论**。
2. **下钻**：对占用最大的 2-3 组（或 `drives` 里的大目录），用 `mcp_file_master_disk_scan(path="<该组路径>")` 列出直接子项（按大小降序 Top 20），像资源管理器一样**一层层进入**最大的子目录，直到定位具体的大文件/大文件夹。每层调用输出都很小，不要试图一次拿全。**注意 `estimated`/`size_estimated=true` 的语义：直接子项清单本身完整无遗漏，只是该项的大小为估算值**（递归求和超时）——不需要用 dir/shell 验证清单是否漏项；想拿准确大小可对其单独下钻。
3. 按 🟢🟡🔴 三级向用户呈现，每项说清"**是什么数据、删掉有什么影响**"：
   - 🟢 **可放心清**：纯缓存/临时文件（Temp、浏览器 Cache、微信 Cache、npm/pip 等开发缓存、缩略图、崩溃转储）——应用会自动重建，给出预计可释放空间
   - 🟡 **需人工判断**：含用户数据（Downloads、微信接收的文件、Documents、其他盘大文件）——给 Top 子项画像，让用户自己挑
   - 🔴 **谨慎/不动**：程序本体、系统区域——只建议"去系统设置里卸载"或使用 Windows 自带磁盘清理（cleanmgr）、存储感知
4. 对话里**结论先行**：总可释放估算 + 最值得先清的 2-3 项 + 占用最大的一项，细节用列表展开。
5. 用户点名 → `mcp_file_master_file_trash(paths=[...])`（默认 `confirm=false`，只回预览清单不执行）→ 把预览给用户过目 → 用户确认 → 用相同 paths 加 `confirm=true` 重调。**`confirm=true` 后删除在后台执行**：立即返回 `task_id`（`type=file_trash_submitted`），用 `mcp_file_master_file_trash_status(task_id="<task_id>")` 轮询直到 `status=done`，再按 `results` 逐项汇报（moved/error/rejected）。大目录删除不会阻塞对话。"帮我清一清"这类模糊指令必须先给清单确认范围。
6. **预览里 `warning` 提示"超过回收站配额"的项**（该目标超过回收站配额约 X，移入回收站会被 Shell 静默物理删除）→ **如实告知用户"该项将移入同级 _pinvou_filemaster_trash 兜底（可恢复，但不释放磁盘空间）"**，确认后按正常流程提交。**移入后（`detail` 注明"未释放磁盘空间"的项）**：告知 `_pinvou_filemaster_trash` 目录的实际路径 + "要真正删除可用 `mcp_file_master_file_erase`（仅限 _pinvou_filemaster_trash 内容，confirm 两步，不可恢复）或手动删除该目录"，并**询问是否需要帮助释放空间**——用户需要时按第 7 步走 file_erase。不要报"已释放空间"。注：file_trash 不支持物理删除（安全边界），超配额恒走 _pinvou_filemaster_trash，不存在"直接删除"选项。
7. **释放 _pinvou_filemaster_trash 空间（用户明确要求后）** → `mcp_file_master_file_erase(paths=[<_pinvou_filemaster_trash 内路径>])`：默认 `confirm=false` 只回预览清单（含大小）→ 展示给用户 → 用户确认 → `confirm=true` **后台异步执行**，返回 `task_id`，用 `mcp_file_master_file_trash_status(task_id="<task_id>")` 轮询到 `status=done` 再汇报逐项结果（erased/error/rejected）。**物理删除不可恢复**，对应删除日志记录标记 erased，`file_restore` 不再列出且 restore 会明确报"已被物理删除"；务必让用户知情后再执行。
8. 删完汇报实际释放的空间，并提醒：文件在回收站，误删可恢复（`mcp_file_master_file_restore`）。
9. **清空回收站/废纸篓（可选收尾）**：用户明确要求"把回收站也清空/彻底释放空间" → `mcp_file_master_file_empty_recycle(confirm=false)` 查占用（Windows 回收站 / macOS 废纸篓 ~/.Trash / Linux XDG Trash，三端均支持）→ 展示给用户 → 用户确认 → `confirm=true` 执行。**注意**：清空是物理删除不可恢复，且经 file_trash 删除的记录清空后无法再还原（_pinvou_filemaster_trash 兜底方式不受影响）——务必让用户知情后再确认。
10. **误删补救**：用户说"删错了/想找回"→ `mcp_file_master_file_restore(action="list")` 从本机删除日志列出待恢复项（删除时自动记录，不靠对话记忆）→ 用户指定后 `mcp_file_master_file_restore(action="restore", path="<original_path>")` 精确还原到原位置。

- **清理选项必须带退出/自定义出口**：凡向用户提供清理建议（无论 🟢🟡 哪一级），都必须同时给出「这次先不清理 / 都留着」选项，或让用户自定义清理范围、逐项点名；严禁只给"清 A/B/C"的封闭选项、替用户默认勾选或默认"全清"。用户明确表示先不清理后，不得反复推销清理。

### 呈现要点

- 🟡/🔴 不主动删：🟡 含用户数据，只给内容画像和处置建议；🔴 不提供删除选项，只给卸载/系统工具指引。
- 大小说"约 14 GB"，不堆精确字节。
- 结果很多时优先展示占用 Top 5，其余折叠为"还有 N 组共约 X GB"。
- 多盘符机器聚焦系统盘；`drives` 里的其他盘大文件一律归 🟡 让用户判断，大目录可对其 path 下钻。
- **清理选项必须带退出/自定义出口**：凡向用户提供清理建议（无论 🟢🟡 哪一级），都必须同时给出「这次先不清理 / 都留着」选项，或让用户自定义清理范围、逐项点名；严禁只给"清 A/B/C"的封闭选项、替用户默认勾选或默认"全清"。用户明确表示先不清理后，不得反复推销清理。
