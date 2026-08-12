# CodeWhale Fork 修改清单

> 本文是 Pinvou 对 CodeWhale fork 的单一现状清单。
> 基线、主题边界、守护指纹和同步结论以本文与 `docs/fork-policy.md` 为准。
> English: [`docs/fork-modifications.en.md`](fork-modifications.en.md)

## 0. 当前状态（2026-08-11 · v0.9.5 r5 公开基线）

| 项 | 当前值 |
|---|---|
| 上游基线 | tag `v0.9.5`，commit `853cb707bbcf4f7dc4268fba6d811e0d04083f9c` |
| 公开维护分支 | `Pinvou/CodeWhale:pinvou3-clean`，head `2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd` |
| 已合并修复 | `Pinvou/CodeWhale#9`、`Pinvou/CodeWhale#11` 已合并；当前维护分支 head 为 `2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd` |
| 公开状态 | `pinvou3-clean` 与固定标签 `pinvou-v0.9.5-r5` 均指向公开维护分支 head；`r1`/`r2`/`r3`/`r4` 保留为不可变历史标签 |
| 旧基线备份 | tag `pinvou-v0.9.0-r4` + branch `backup/pinvou3-clean-v0.9.0-r4`，均指向 `03e9e1027c03ce1e4b35ab9e3ccce751b65b9624` |
| 组织方式 | 从 `v0.9.5` clean re-fork 的 5 个长期主题、7 个线性提交 |
| drift | `48 files changed, +2177/-269`；净增约 1908 行 |
| 守护 | 23 条 CodeWhale `forkguard_*` 行为测试 + 父仓指纹/行为测试 |
| 父仓适配 | gitlink、`Cargo.lock`、`EngineConfig` v0.9.5 字段适配 |

### 本次会话修复（已验证并发布）

- v0.9.5 的 `load_session` 会把无配对 `tool_use` 视为进程崩溃并立即补写失败结果；Pinvou 运行中持久化工具调用后再次读取同一会话时，这一假设并不成立。
- 底座修复已通过 `Pinvou/CodeWhale#11` 合入，公开 commit 为 `2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd`。
- T1 新增无修复副作用的 `load_session_snapshot` 与显式 `recover_session_for_resume`。Pinvou 的运行时读改写统一使用前者，仅在应用进程启动、任何 Engine 接管会话前执行后者，并把恢复结果原子落盘。
- 前端仅对真正的跨端回合保留 revision 对账门禁；本地 `chat:done` 直接释放下一轮发送，落盘读回异常不得阻塞普通本地对话，跨端未收敛提示按会话去重。
- 本次新增 2 条 CodeWhale `forkguard_*`、2 条父仓 `forkguard_*` 和 Tauri/Web 前端行为回归，分别锁定运行时无副作用读取、显式恢复可观测与幂等、二次 Store 打开安全、启动恢复落盘以及本地完成后连续发送。
- 本节改动已计入上方公开维护分支 head、drift 和固定标签 `pinvou-v0.9.5-r5`；CodeWhale required checks 与父仓自动测试均已通过。

### 待验证改动（2026-08-12 · 慢设备 vision 超时适配，未发布）

- 现象：Intel 核显（UHD 750，Vulkan 驱动 30.0.100.9805）上 `image_analyze` 冷启动请求
  超过 `120s` 客户端超时，4 次重试全部排队失败 → `Retry exhausted` → 主模型自动重试
  → 死循环（实测单请求冷启动 18-50s，首次含 Vulkan 管线编译可超 120s）。
- 改动（`CodeWhale/crates/tui/src/vision/tools.rs`，仅 2 处）：
  - `image_analyze` 改**流式接收**（`stream: true`）：总时长 300s 由流式循环控制
    （reqwest 整体 timeout 移除，连接阶段 30s）；超时返回已累积的部分内容 +
    `truncated` 标记，而不是整请求失败触发重试死循环。`DEFAULT_VISION_MAX_OUTPUT_TOKENS`
    保持上游 4096（本地/云端一致，长文档转写不阉割；慢设备生成不满 4096 时由
    流式超时兜底）。图片描述实测 ~150 tokens，正常场景上限不影响速度。
  - `image_analyze` 客户端超时 120s → 300s：只放宽上限，云端视觉模型（通常 10-30s）无感。
- 验证状态：基准确认单请求 8-50s（CPU/Vulkan 各 3 次），修复后超时窗口充足；
  完整 forkguard/契约回归待发布流程执行。
- 追加（同文件，同一节）：`image_analyze` 请求构造增强——加 system prompt
  （图片类型 / 文字逐字转写 / 元素布局三要素，禁止臆测）、temperature 0.7 → 0.2、
  默认 prompt 从 "Describe this image in detail." 增强为三要素模板。
  实测（同一张含中文截图的对比）：旧构造输出幻觉重复循环，新构造逐字转写全部中文正文。
  对云端视觉模型（gpt-4o 等）同样适用，低温度对转写类任务无副作用。
- 追加（同文件，同一节）：`image_analyze` 结果显式标记截断——`finish_reason == "length"`
  时结果 JSON 附加 `"truncated": true`，主模型据此知道转写不完整（密集文字图场景），
  避免基于残缺内容无感知总结。

### 软上限评估

总变更行和净增量均超过 1500 行软线。新增超量主要来自 T5 对结构化产出根、精确写入声明和符号链接逃逸的 fail-closed 加固；这些检查必须位于实际落盘和 SubAgent 生命周期内，不能安全下沉到 app。主要保留量：

- T4 `+373/-24`：稳定 conversation/thread 关联、Pinvou 历史 schema 兼容、misfire/no-overlap 和终态级联清理必须与 Task/Automation 持久化原子完成。
- T5 `+912/-45`：三省六部角色派发、结构化/文件产出验证、显式项目根、路径逃逸阻断、取消和真实终态必须位于 SubAgent 生命周期内。
- T3 `+253/-71`：嵌入宿主的静态指令、ambient context 和 Skill 单根来源必须在模型上下文生成前密封。

本轮不为压数字复制底座状态机到 app。后续减量顺序：T1 通用 embedding route API、T2 通用命令安全、T4 通用 Automation 生命周期；T3/T5 的 Pinvou 产品语义继续留 fork。

## 1. 五个长期 fork 主题

### T1：宿主嵌入与路由边界

- **公开 commits**：`331cb1594688c723d98499d9ca11f05af291b599`、`2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd`（`Pinvou/CodeWhale#11`）。
- **公开规模**：10 文件，`+394/-31`；仓库级 CI 恢复不计入 T1 主题规模。
- **核心文件**：`crates/tui/src/lib.rs`、`core/engine.rs`、`route_runtime.rs`、`runtime_threads.rs`、`automation_manager.rs`、`session_manager.rs`。
- **内容**：
  - 在 v0.9.5 原生 library target 上只公开 Pinvou 实际使用的模块和宿主类型，不恢复旧的全量 bin facade。
  - 以根级窄重导出公开 `FleetRoster` 与工作区角色目录常量，供嵌入宿主在写入角色文件后装配和热刷新名册；不公开整个 `fleet` 模块。
  - 提供只读持久化 worker 投影，供 live 宿主结合自身进程纪元判断状态；恢复入口仍按 v0.9.5 原语把孤儿 worker 收敛为 interrupted。
  - 提供 opaque resolved route、显式 route limits 和 embedding host route override。
  - 保留宿主需要的 runtime thread / Automation 接口和 `EngineConfig` 注入边界。
  - 将无副作用的运行时 session snapshot 与已知进程重启后的显式 tool history recovery 分开，避免嵌入宿主把仍在执行的工具调用误判为崩溃。
- **边界**：不实现 Pinvou 产品工具策略，不包含三省六部完成语义。
- **守护**：`forkguard_embedding_route_limits_preserve_wire_alias`、`forkguard_runtime_session_snapshot_preserves_in_flight_tool_call`、`forkguard_explicit_session_recovery_is_reported_and_idempotent_after_save`，以及父仓启动恢复、resolved-route 和 compaction 合约测试。

### T2：工具兼容与命令执行安全

- **commit**：`595adce47e2d1bcf895d7bfd6426c074eb969324`
- **规模**：15 文件，`+181/-98`
- **核心文件**：`core/engine.rs`、`core/engine/tool_setup.rs`、`core/ops.rs`、`tools/file.rs`、`command_safety.rs`、`tools/shell.rs`、`docs/TOOL_SURFACE.md`。
- **内容**：
  - `EngineConfig.extra_tools` 让宿主工具在 Plan、Agent、Yolo 等 turn registry 中一致注册。
  - `SetDisallowedTools` 支持工具商店、知识库和会话策略在不重建 Engine 的情况下动态收窄工具面。
  - 复用 v0.9.5 原生 `allowed_tools` 作为硬白名单入口；Pinvou 名单由 app 构造，底座不维护产品 blocklist。
  - `File` 写入保持 64 KiB 单次内容上限，并在落盘前拒绝超限输入。
  - 多行 Shell 按 segment 检查；破坏性命令在自动批准模式下仍被阻断。
  - 当前工具面不恢复已退役的独立追加文件工具，也没有改动 `request_user_input`。
- **边界**：不包含 Skill 来源、Automation 或三省六部角色协议。
- **守护**：`forkguard_host_extra_tools_register_in_all_modes`、`forkguard_file_content_caps_reject_before_writing`、`forkguard_multiline_still_blocks_destructive_segments`。

### T3：嵌入上下文与技能来源

- **commit**：`5a9f52941b83452c1e8b76c2d679bac315edcf70`
- **规模**：13 文件，`+253/-71`
- **核心文件**：`prompts.rs`、`project_context.rs`、`repo_law.rs`、`model_context/{fragment,world_state}.rs`、`skills/`、`tools/skill.rs`、`working_set.rs`。
- **内容**：
  - static prompt composer 由 app 接管时，停用 ambient project context 和 repo law，避免用户目录文件隐式进入系统上下文。
  - Skill 只从宿主显式 `skills_dir` 扫描；disabled Skill 同时从目录和 `load_skill` 消失。
  - `FragmentId::Permissions` 单独沿用 100 KiB instruction 上限，其他 WorldState fragment 保持 v0.9.5 的 40 KiB 上限，避免全局放宽。
  - 用户消息前置内部 `<system-reminder>` 不参与 Working Set 路径提取，历史原文保持不变。
- **边界**：app 负责生成和选择 bundle/会话 Skill 根；底座只保证显式来源与上下文不变量。
- **守护**：`forkguard_runtime_loader_ignores_ambient_project_authority`、`forkguard_instruction_fragment_preserves_content_beyond_default_cap`、`forkguard_disabled_skill_is_neither_rendered_nor_loadable`、`forkguard_working_set_ignores_leading_system_reminder_paths`。

### T4：定时任务与运行生命周期

- **commit**：`fc84f7d3e5dca0e3db404d43e218597764129f9b`
- **规模**：4 文件，`+373/-24`
- **核心文件**：`automation_manager.rs`、`task_manager.rs`、`tools/automation.rs`、`tui/automation_routing.rs`。
- **内容**：
  - Automation 透传选定 model，并以 automation id 建立稳定 conversation key。
  - 保持 v0.9.5 当前 task schema v2，同时兼容读取 Pinvou 历史 v3/v4，拒绝未知更新 schema；thread/turn 链接跨 worker 边界及时持久化。
  - HOURLY 调度保持创建时刻锚点；休眠/关机错过时段不补跑，存在 queued/running run 时不重叠执行。
  - 只清理终态 run/task，并级联删除相应 artifact；活动运行保持可恢复。
  - 强制审批不能被通用 auto-approve 绕过。
- **边界**：app 负责展示、通知和业务工作区；底座负责调度与耐久运行事实。
- **守护**：`forkguard_scheduler_skips_offline_misfires_without_backfill`、`forkguard_scheduler_does_not_overlap_active_automation_run`、`forkguard_conversation_key_and_created_thread_survive_worker_boundary`、`forkguard_accepts_pinvou_v4_tasks_but_rejects_unknown_newer_schema`。

### T5：三省六部编排、完成闸与结构化产出安全

- **commits**：`3782a78d4e11d1fb65042cf9c82231b9d644c20a`、`d1010aa3bbaf76780e29df4434fd1e03a95b2ca6`
- **规模**：16 文件，`+912/-45`
- **核心文件**：`core/{engine,events,ops}.rs`、`tools/subagent/{mod,tests}.rs`、`runtime_threads.rs` 及 SubAgent TUI 事件适配。
- **内容**：
  - 只为三省六部角色派发补充 role、显式工具范围、最大步数、output schema 和文件产出要求。
  - 将角色登记的具体产物文件在启动时注册为 v0.9.5 有界 write claim；拒绝工作区外路径，不放宽为整个工作区写权限。
  - `Custom` 角色的旧 action allowlist 映射到 v0.9.5 canonical action 工具面，不依赖 `load_skill` 才能运行。
  - 合成结构化提交入口，递归校验有限 JSON schema，只允许声明的安全相对路径落盘。
  - 结构化产出必须使用宿主显式选择的工作区内项目根，并与精确写入声明逐项匹配；拒绝路径穿越、符号链接组件和信任模式下的链接逃逸。
  - 文件产出型角色在真实文件未落盘时不能完成；失败信封保留最后工具错误供宿主展示。
  - 批量取消 live SubAgent，并通过携带 role/failed/result 的 `AgentComplete` 收敛真实终态。
- **边界**：不包含通用宿主配置、工具注入、路由、Automation、OAuth 或普通 Fleet 产品功能。
- **守护**：`forkguard_custom_workflow_legacy_action_allowlist_is_available_without_load_skill`、`forkguard_structured_output_validates_nested_required_fields`、`forkguard_structured_output_persists_only_declared_safe_paths`、`forkguard_structured_output_rejects_symlink_components`、`forkguard_structured_output_root_is_explicit_and_claim_bounded`、`forkguard_host_write_claim_rejects_symlink_even_in_trust_mode`、`forkguard_host_write_files_become_bounded_claims`，以及父仓三省六部协议测试。

## 2. 父仓能力与 fork 的分界

以下能力保留在 `pinvou3-app`，不进入 CodeWhale fork：

- `features/assistant/tool_policy.rs`：Pinvou canonical tools 白名单和 MCP namespace 策略。
- `disallowed_tools` 的会话/连接器动态取值与工具商店开关。
- bundle instructions、按会话 Skill 组合目录、用户 AGENTS 注入。
- UI、Tauri IPC、工作区与产物卡、Shell 输出观察和前端终态对账。
- 定时任务页面、通知、三省六部页面和业务日志展示。

CodeWhale fork 只提供这些产品能力不可缺少的底座生命周期入口和原子不变量。

## 3. v0.9.5 同步结论

### 上游已有，不再维护

- v0.9.5 原生 library/runtime crate 边界：T1 只保留必要公开面。
- 原生 `allowed_tools`：Pinvou 白名单直接复用，不恢复 fork-only 第二套白名单字段。
- 通用 OAuth 取消、Fleet roster、Runtime API、MCP registry 和 session-tree：直接使用上游。
- canonical action 工具面：不恢复旧独立工具名。

### v0.9.5 新增适配

- `EngineConfig` 新增 `subagent_state_root`，父仓显式透传默认值。
- 已删除的旧 `hidden_tools` 字段不再恢复；Pinvou 原有动态隐藏行为本就通过 `disallowed_tools` 完成。
- v0.9.5 WorldState 40 KiB fragment cap 只对 Permissions 做 100 KiB 窄例外，其他 fragment 不变。
- v0.9.5 workspace crate 拆分引起父仓 `Cargo.lock` 重算，未增加 Pinvou 直接依赖。

## 4. 验证

CodeWhale 当前已通过：

```text
cargo fmt --all -- --check
cargo check -p codewhale-tui --lib --locked
cargo test -p codewhale-tui --lib --locked forkguard_ -- --test-threads=1
23 passed / 0 failed
```

父仓当前已通过：

```text
cargo fmt --all -- --check
cargo check --locked
cargo test --locked --lib -- --test-threads=1
1077 passed / 0 failed / 12 ignored
./scripts/fork-guard.sh
CodeWhale 23 passed；pinvou3-app 18 passed
python3 scripts/architecture-guard.py
npm test
npm run lint:ui
npm run build:ui
npm run build:web
cargo build --locked --no-default-features --features local-embed --bin pinvou3-tauri
```

完整结果见 `docs/codewhale-upgrade-0.9.0-to-0.9.5.md`。12 个 ignored 测试依赖真实模型、外部工具或专用 fixture；公开标签一致性由 `scripts/verify-public-submodule.sh` 校验，不得以修改脚本方式绕过。

## 5. 后续修改规则

- 修改任一主题时，同步更新本文、`scripts/fork-guard.sh` 和对应 `forkguard_*` 行为测试。
- 通用修复从 upstream main 建净分支贡献；不得把整个 Pinvou 主题直接提交上游。
- 发布后把本节状态更新为远端维护分支、不可变标签和实际 commit，并验证父仓 gitlink 一致。
