# 工具市场统一改造 — 待完成清单（AI 交接文档）

> **本文档已过期，仅作历史快照保留。** 工具市场统一改造的后续工作（能力包模型 +
> 插件导入管线 + 插件中心前端）已在分支 `feat/plugin-protocol` 上以 **PR #302**
> 推进，本文不再是交接真相源；现行状态以 PR #302、`docs/plugin-package-spec.md`
> 与 `docs/plugin-protocol.md` 为准。以下为改造期间（分支 `feat/tool-market-integrated`）
> 的交接快照，其中「未推送、未建 PR」「下一步行动队列」等状态描述均已失效。

## 0. 交接必读（历史快照）

### 0.1 项目状态一句话

工具市场统一改造（设计基线 `docs/marketplace-unification.md`，激进版：BundleStore
单一真相源 + 底座注册缝直供）**Phase 2（app 侧）已完成 12 刀并全部提交**，
其中 A 节 scope 收敛（#287 已同步合入）已在本轮完成；
剩 Phase 3 底座缝（探查结论在 B 节）、回退分支清理（C 节）、Phase 4 收尾（D 节）。

### 0.2 分支与提交状态

- 分支：`feat/tool-market-integrated`，**本地领先 origin 16 个提交，未推送、未建 PR
  （用户明确：不推 PR，只按节点 commit）**。
- 本轮已完成 origin/main 同步（`git fetch origin` 成功，origin/main 由 710afdea 前移到
  1761b70b，取得 #287/#263/#289/#290），CodeWhale gitlink 对齐到 3bbf8421e。
- 提交规范：`<type>(marketplace): <中文描述>` + trailer
  `Signed-off-by: luzeyang (INT) <lu.zeyang@h3c.com>`；作者信息用
  `git -c user.name="luzeyang (INT)" -c user.email="lu.zeyang@h3c.com" commit`
  （仓库 git config 的 user 是另一个人，不要用默认值）。
- **不要**执行 push / PR / rebase 等操作，除非用户明确要求；commit 是允许的节点动作。

### 0.3 用户协作约定（本会话确立，继续遵守）

- **自主连续推进**：用户说过"继续继续，不要等我"——按本文档队列依次开工，
  每刀完成即提交并简短汇报，不等确认。
- **验证一轮制**：用户明确"不要完成一步就测一次，太费 token 和时间"——
  每刀全部写完后只跑一轮验证（见 §继续工作的方式）。
- 汇报用中文，简洁、给结论与文件路径。

### 0.4 可 resume 的子代理（上下文延续，优先复用）

- **coder `agent-2`**：刀 1-11 的全部实现者，持有全部实现细节与教训——续作实现
  一律 `Agent(resume="agent-2")` 而不是新起 coder。
- **explore `agent-3`**：底座消费点探查者（结论已入库 B 节）——Phase 3 需要追问
  底座细节时 `Agent(resume="agent-3")`。
- 若 resume 失败（会话重启后丢失），按本文档 + 设计基线重新派活即可，信息无缺失。

### 0.5 本机环境坑（Windows，必读，省大量排查时间）

- **rustc 随机崩溃**（堆损坏/rmeta panic/栈溢出/symbol panic，签名每次不同）：
  对策 = `RUST_MIN_STACK=33554432` + 崩溃后重试；根治建议 = 把
  `pinvou3-app/src-tauri/target/` 与 `%USERPROFILE%\.cargo` 加入 HnTrustCircle
  （企业安全软件）排除目录。**不要并发跑两个 cargo**（本会话因此损坏过一次 target，
  cargo clean 全量重建才恢复）。
- **测试 exe 运行期 0xc0000139**（STATUS_ENTRYPOINT_NOT_FOUND，本机既有问题，
  #287 也记录过）：编译没问题，是 DLL 入口点冲突。绕过 = 给测试 exe 重嵌
  comctl32 v6 manifest（asInvoker + dependency）；根治 = build.rs 嵌 manifest（未做）。
  agent-2 已掌握具体操作，resume 它即可。
- `python3` 是 Store 别名间歇性 exit 49，用 `python scripts/architecture-guard.py`。
- git add 的 LF→CRLF warning 是本仓库常态，忽略。

### 0.6 验证基线（刀 12 后）

Rust 全量串行 **1245 passed / 3 failed**（3 个全是 Windows-only 既有问题：
codex_acp 的 `remove_clears_current_candidate` / `store_roundtrip_and_atomic`
（InvalidFilename）与 `path_install_source_recognizes_official_script_dirs`
（路径断言））；前端 `tests/tool_store_smoke.js` **40/40**；`npm run lint:ui`、
fmt、architecture-guard 全绿。**任何新刀不得新增失败。**

## 1. 下一步行动队列（按序执行）

1. **Phase 3 底座缝第一刀：`Op::ToolingChanged`**（B 节，探查显示半径最小、
   MCP 侧内存 API 现成）。注意这是 CodeWhale 子模块改动，fork 纪律见 B 节。
2. C 节回退分支删除（下一个版本周期后）。
3. D 节 Phase 4 收尾。

## 2. 已完成（11 刀 + 2 个文档提交，全部已 commit）

| 提交 | 内容 |
|---|---|
| 41a0ebd3 | BundleStore 可写真相源（store.rs）+ 旧布局幂等导入 + connector_lock 原语下沉 platform/ |
| bae4dabb | 全部写路径（MCP/技能/CLI connect/disconnect）镜像接入 bundles.json |
| 022e2e10 | 首启导入接线（ensure_extracted 内）+ installed 真相源反转 + degraded 透传 |
| 64cd1f64 | CLI 资产版本化 `assets/cli/<name>/<version>/` + locked_cli_path 单点 + 存量迁移 |
| 98618525 | bundle_readiness 动作下发（actions.rs：七动作词汇表 + flow payload） |
| 716d519f | 禁用技能脚本 execpolicy 硬拦截（按脚本枚举 typed Deny，YOLO 不放水） |
| bb42d24d | CLI/ima 功能事实下沉（version 以 lock 表为准，修正前端腐化数据） |
| a8e81bfc | 前端 ToolStoreView 切 bundle_readiness + actions（逐连接器 status 调用全移除） |
| 4efa7c06 | bundle_readiness 嵌套 BundleInfo；version/configFields 切后端源 |
| a097cb8d | 技能按包聚合 `bundles/<id>/skills/` + 指纹化 update_available + 存量迁移 |
| 332579c9 | MCP 脚本按需释放 `bundles/<id>/mcp/` + mcp_catalog.rs + 全量释放退役 |
| 8a48e75b | 本文档初版 |
| 473badec | #287 合入后文档对齐 + B 节探查结论入库 |
| 645ccf28 | Merge origin/main（取得 #287/#263/#289/#290），CodeWhale gitlink 对齐 3bbf8421e |
| 1cf0b78f | 分支同步后对齐 #287 重命名（sync_deny_all_scopes / SessionPolicy::mode） |
| a02d58b7 | A 节 scope 收敛：包 id 单一禁用集 + enable_in(scope) 动作 + 前端切单一开关 |

**当前架构状态**：BundleStore 是安装态唯一真相源；存储按包聚合；CLI 无特殊地位；
前端动作驱动；治理三通道齐备（物化排除/disallowed_tools/execpolicy）；
开关已收敛为**包 id × SessionMode 单一禁用集**（`disabled_bundles.json`）。
**.installed-from 标记、cache/connectors 暂存、逐连接器 status 前端调用、
`skill:` 前缀跨文件借道已退役。**

## 3. 待完成明细

### A. scope 收敛到包 id × SessionMode（✅ 已完成，刀 12 / a02d58b7）

- **已完成**：#287 已同步合入；`disabled_connectors.json` + `disabled_skills.json`
  收敛为单一 `disabled_bundles.json`（`{scopes, initialized, project_skills_enabled}`，
  键 = 包 id），首读迁移旧双文件并清 `skill:` 前缀跨文件借道；companion 联动排除
  改由包模型现算（`bundle::skill_owner_package`）；actions 增加 `enable_in(scope)`
  （已装包每模式下发）；前端 SettingsView/composer-tool-menu-logic 切单一 bundle 开关。
- 实现落点：`marketplace/scope.rs`（单一持久化 + 迁移）、`skill_scope.rs`（别名层）、
  `skill_materialization::disabled_skill_names_for`（包模型现算）、`actions.rs`。
- 设计依据：unification 文档 §3.2 三态正交、§5 治理模型。

### B. Phase 3 底座缝（CodeWhale 子模块，upstream-first）

- **探查已完成**（结论摘要，行号基于本分支 CodeWhale 子模块；追问细节 resume agent-3）：
  - 宿主-底座为**同进程库链接**（`deepseek-tui` path 依赖）；引擎创建入口
    `features/assistant/engine.rs::spawn_engine`，配置翻译集中在 `bridge.rs`
    （mcp_config_path 在 bridge.rs:1135）。
  - mcp.json 消费：`McpPool`（`crates/tui/src/mcp.rs:2632`）lazy 建池
    （`engine.rs:5230 ensure_mcp_pool`），每次访问经 `reload_if_config_changed`
    （mtime+hash）惰性重查；**运行期增删已有内存 API**
    （`add_runtime_server_config`/`remove_runtime_server_config`，mcp.rs:3661/3689）
    与 `Op::ReloadMcp`（ops.rs:303）——MCP 半边缝很小，`McpConfig` 内存供给 API 已存在。
  - 技能消费：prompt 组装 `prompts.rs:1064` → skills 段渲染 `skills/mod.rs:1271`
    （## Skills 只是路由索引，正文由 load_skill 工具按需读）；宿主注入 `skills_dir`
    即权限边界（skills/mod.rs:987）；**每轮重建**（mtime 缓存）。技能半边要处理
    discovery 缓存与 load_skill 落盘假设，是主要工作量。
  - tooling_changed 落点：复用 `Op` 总线新增 `Op::ToolingChanged`（ops.rs，紧邻
    ReloadMcp/SetDisallowedTools），分发处做 McpPool 换配置 + 失效技能发现缓存
    （`clear_skill_discovery_cache`）+ refresh_system_prompt；宿主发送路径照搬
    `engine_pool.rs:1166` 的 SetDisallowedTools 先例。
  - **前缀缓存张力（2026-08 复核确认）**：`## Skills` 段在系统提示 `full_prompt`
    （constitution）里，属**缓存稳定前缀 Blocks[0]**（`prompts.rs:1200`
    「volatile-content boundary」之上）。**会话中开关技能 = 系统提示字节变 → 整段
    前缀缓存 miss**（`prefix_cache.rs` 对系统提示全文 SHA-256 指纹，
    `turn_loop.rs:782` verify 报 drift 后重新 freeze）。关键结论：前缀缓存只复用
    「token 0 起的最长公共前缀」，系统提示在最前、对话记录在其后，因此**任一系统提示
    字节变化都会连带失效整段对话记录**（上下文大头），与 skills 段在系统提示内前/后
    位置无关——把 skills 挪进 volatile 的 WorldState 区（`world_state.with_skills_tools`
    现成槽位，当前传 `None`）**并不能减小对话记录 miss**，只省系统提示尾巴。要「无 miss
    地真关掉」唯一出路是块级缓存控制（`SystemBlock.cache_control`，现成字段、当前全传
    `None`），属底座缝范畴。综上：开关成本 = 一次性整上下文重算（低频手动可接受），
    禁用保证仍靠执行层 execpolicy，与缓存 miss 无关。
  - 规模评估**中**；上游通用性**高**（EngineConfig 已服务 embedder，默认实现保持
    现状行为不变）。
- 实施顺序（设计基线 §2）：
  1. `tooling_changed` 重配置 API（半径最小，先解"新会话生效"）；
  2. `ToolingSource` 抽象双轨（FileSource 默认 = 现状行为；RegistrySource 由 pinvou3 注册）；
  3. `CredentialResolver`（删除 `${PINVOU3_MCP_SECRET_*}` 占位符体系）。
- **fork 纪律**：改底座必须同 PR 更新 `docs/fork-modifications.md` + 指纹 + 行为测试，
  跑 `./scripts/fork-guard.sh --fast`；缝按可上游化标准设计，不掺 Pinvou 语义。
- 落地后删除：技能物化模块（skill_materialization.rs 大幅瘦身）、mcp.json 写入逻辑
  （FileSource 回退保留一个版本周期后删）、`dump_session_tooling` 可观测命令随
  RegistrySource 同时交付（验收硬要求）。

### C. 遗留回退分支删除（刀 10/11 登记在案，下一个版本周期后执行）

1. `skill_marketplace.rs`：find_skill_dir 旧布局回退 + warn；
2. `skill_materialization.rs`：skill_source_dirs 末尾的 bundle/skills/ legacy 来源；
3. `skill_marketplace.rs`：uninstall 的旧布局带标记删除分支；
4. `skill_marketplace.rs`：preset_update_available 的无指纹记录回退直比；
5. `store.rs`：legacy_skill_records / import_legacy（首启迁移使命完成后退役）；
6. `marketplace/mod.rs`：available_tools / load_manifest 的旧布局回退、install 非内嵌包旧路径回退；
7. 前端 ToolStoreView/tool-common.jsx：无 actions 时的旧渲染分支（刀八注释注明 Phase 3 清理）。

### D. Phase 4 收尾

- V5 条件认领退出条件执行（gongwen 双形态归一，capability-governance.md 登记）；
- 占位卡（backendId:null 即将上线）改注册表 `upcoming` 条目，前端硬编码删除；
- tsSkillsData 清空（visual-design 包归属决策）；
- ima 卡的 update 本地补发路径清理（刀八注释注明 V5 认领退出后删）；
- category 词汇体系统一（manifest 中文自由文本 vs 前端 collab/docs/... 分组词汇）；
- 前端 overlay 的 tmeet/ima 自报 version 清理（后端有值后即删）。

### E. 环境/工程债（本机，详见 §0.5）

- 资产 GC：`assets/cli/` 旧版本目录保守保留，引用计数/GC 未实现（刀四注释注明）。

## 4. 继续工作的方式

- 节奏：一刀一提交；每刀内"新路径+旧路径删除"原子完成；实现优先 resume coder
  子代理 agent-2。
- 验证一轮制：全部写完后 `cargo fmt --check` → `RUST_MIN_STACK=33554432 cargo test --lib -- --test-threads=1`
  （重嵌 manifest 实跑）→ 前端相关则 `npm run lint:ui` + tool-store 系列 →
  `python scripts/architecture-guard.py`。不得新增测试失败（基线见 §0.6）。
