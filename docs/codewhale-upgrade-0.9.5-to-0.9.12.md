# CodeWhale v0.9.5 r13 → v0.9.12 r1 升级报告

> 状态：CodeWhale 受保护维护分支与不可变 r1 tag 已发布；父仓 gitlink 已对齐并进入最终评审（2026-09-09）。真实 vLLM 因本机无可用 endpoint/credential 仍未执行。
> 目标：把 Pinvou Agent 底座完整迁到官方 CodeWhale v0.9.12，同时把 fork 收敛为可解释、可测试、可回退的 4 个长期主题。

## 1. 基线与隔离

| 对象 | 值 |
|---|---|
| 父仓基线 | `origin/main` `f6c38879`（为解决 PR #453 的真实合并冲突而重放） |
| 父仓工作分支 | `codex/codewhale-v0.9.12-sync` |
| 上游 release | `v0.9.12` `dcd4c200f72f0c1ffd60d8e7f6850313db879fc5` |
| 新 fork 分支 | `pinvou3-clean`，由 CodeWhale PR #44 与 fast-follow PR #46 发布 |
| 新 fork head | `1fafee7e26b60a59457a43bce50c63aa2ad9dbaf`；不可变 tag `pinvou-v0.9.12-r1` |
| 旧 fork 回退点 | 公开不可变 tag `pinvou-v0.9.5-r13`，commit `f853f8f1566c57e6be40d5439a222a932aa79ef5` |

所有开发在隔离 worktree 完成；原始工作区的已有改动和未跟踪文件未被修改、清理或提交。

## 2. 评估结论

升级前同时做了三个独立视角的审视：

1. **fork 差异审计**：逐文件判断旧 r13 patch 在 v0.9.12 中是已吸收、需语义迁移，还是仍需底座实现。
2. **主应用兼容审计**：追踪 `pinvou3-app` 实际调用的 EngineConfig、事件、工具、会话、Automation 和持久化路径。
3. **验证/发布审计**：核对 CI、跨平台、包发布、真实模型、公开 submodule 与回退门禁。

旧 r13 修改 110 个文件，其中 104 个同时被 v0.9.12 上游改动，直接移植预计冲突 57 个文件。旧 fork drift 为 `+10895/-1195`；新基线从官方 tag clean re-fork 后为 `+5022/-944`（94 文件，净增 4078 行），因此没有采用 merge/cherry-pick 冲突树。新增触达文件包含当前 Rust/Clippy/rustdoc 发布门禁所需的等价条件折叠、`Default` 补全、窄 lint 说明和文档链接修复，以及评审要求的生命周期、评测结果式回归、API 搜索后备链可达性修复与错误提示、无调用比较 helper 清理、运行时契约精确预算、macOS 冷启动 wrapper 的有界超时和 overdue one-shot 调度修复，不引入新的 fork 行为主题。

## 3. 处置矩阵

### 3.1 采用上游，不再维护 fork

- Session snapshot/recovery 与 crash repair 生命周期
- edit-last-turn 目标分类和权威历史处理
- compaction 后输入 token 估算
- provider/model pin、严格直连模型匹配、route/output budget
- JSON schema 容器修复
- DeepSeek/Qwen/Kimi 等原生搜索与 provider 兼容
- Windows Shell UTF-8 增量解码
- 上游 Task/Fleet/Automation 基础设施

### 3.2 语义迁移到 v0.9.12 接口

| 旧语义 | v0.9.12 适配 |
|---|---|
| `Yolo` / `Auto` AppMode | `Agent` + `allow_shell` / `auto_approve` / trust policy |
| EngineConfig 全局 reasoning | 每轮 `Op::SendMessage` reasoning effort |
| 旧 custom-tools 入口 | `ExtraTools`，进入原生 Agent/Plan catalog |
| 全局 disabled-skills shim | 显式 `skills_dir`、bundle registry 与每会话 disallowed tools |
| 字符串 role | `models::Role::User` |
| 旧 CompactContext id | UUID |
| 无界 scheduled executor channel | 有界 `mpsc::Sender` + `send().await` |

### 3.3 仍需 fork 的四个主题

- **T1 宿主/路由**：host compatibility facade、route-with-limits、可靠 steer、session owner、批量 child cancel、raw worker ledger；当前 18 个 `pub mod` 的收窄工作已单独登记，父仓全部 Rust 源码中有 61 个文件、340 处直接引用，需先迁移这些调用点。
- **T2 工具/安全**：ExtraTools、MCP secret resolver、逐轮精确策略、最终分发复检、只读/审批 fail-closed、File 64 KiB、受限日志脱敏。
- **T3 Prompt/Skills**：static composer 所有权、ambient authority 隔离、单一显式 Skills 根、Permissions 100 KiB 窄预算、working-set reminder 隔离。
- **T4 Automation/Task**：稳定 conversation key、ThreadCreated、v3 writer/v4 窄兼容、离线不补跑、不重叠、终态清理。

## 4. 主应用改造

- `deepseek_tui::AppMode` 直接使用 v0.9.12 根 re-export；旧 `Yolo/Auto` 逻辑改成 Agent 与显式 approval/trust 组合。
- bridge 按 v0.9.12 `EngineConfig` 重建字段，保留有限 turn/tool budget、read denylist、bubblewrap、MCP OAuth、goal 与 telemetry 安全配置。
- `session_id` 在 `Engine::spawn` 前写入配置；不依赖事件到达后猜测归属。
- 精确工具/只读策略以 `TurnToolSecurityPolicy` 随 `SendMessage` 下发，restricted turn 不携带 dynamic tools。
- 产品工具面迁移到 v0.9.12 model-visible canonical 名称，并以真实 `ToolRegistry::to_api_tools()`/Engine `tool_catalog` 回归防止隐藏 replay 别名泄漏。
- canonical `bash`/`write`/`edit` 同步进入 Shell 任务归属、scheduled artifact 持久化及 Tauri/Web 渲染 matcher；保留旧名仅用于历史回放。Windows 没有 terminal session 时，本地预览使用底座保留的 `Bash` 后台控制面，普通执行仍使用 foreground-only 的 lowercase `bash` 小契约。
- forwarder 只接收 owner session 匹配的 Agent/Workflow/SubAgent 事件。
- `ToolGateDecision` 与 `CompactionCancelled` 进入桌面/远程用户可见通道，其余 v0.9.12 事件全部显式处置或记录，不再被 wildcard 静默吞掉。工具门禁落盘同时保留 `toolName`、`reason`、`risk`，历史回放可重建审计明细；升级前只存文本的旧记录继续按原文本显示。`GoalContinuationWaiting/WaitEnded` 当前不投影，因产品没有开放延迟配置且 Engine 默认为 0，回归测试锁定其不可达性。
- Scheduled executor 使用 v0.9.12 `TaskExecutionLimits`、`TaskTerminalReason` 和 `ThreadCreated`；产品将 idle 设为 31 分钟、wall 设为 30 分钟，避免未投影每个模型 delta 时健康本地推理被默认 2 分钟 idle 看门狗误杀；Pinvou 精确 `model_id` 继续由 automation id 对应的 companion binding store 持久化，新增的 CodeWhale provider registry 字段刻意留空，二者不混用。
- `rusqlite` 对齐 0.40.2，并同步重生成 app 与 `pinvou-cli` 两份 `Cargo.lock`；重型 CI 显式执行 `benchmark-hooks` 及 `pinvou-product-backend --locked` 编译契约。`benchmark-hooks` 只启用有运行语义的 `benchmark-eval-controls`；底座空的 `benchmark-observability` feature 已删除，观测继续由父仓 backend adapter 负责。

## 5. 安全与数据不变量

- restricted turn 的 tool args/result/error 不进入普通 audit/tracing。
- tool name 和 read-only action 在真正 backend dispatch 前再次校验，不能靠伪造 catalog 绕过。
- Full Access 不能绕过 non-bypassable approval。
- restricted turn 结束后的 goal、MCP reload、edit、child/shell follow-up 在新显式消息前不获得更高权限。
- MCP secret resolver 不写进程环境或普通配置文件。
- Task 只额外接受历史 Pinvou v4；未知 v5+ fail closed。
- Automation 离线 slot 不 backfill，同一 automation 不并发重叠。

## 6. 验证记录

| 层级 | 命令/场景 | 当前结果 |
|---|---|---|
| CodeWhale compile | `cargo check -p codewhale-tui --lib --bins --locked`；`--lib --tests`；`cargo check --workspace --all-targets --locked` | 均通过 |
| CodeWhale full lib | `RUST_MIN_STACK=16777216 cargo test -p codewhale-tui --lib --locked -- --test-threads=1` | 复审修复后 11,703 通过，0 失败，12 ignored |
| CodeWhale release lane | 精确 SHA `workflow_dispatch` 全量 CI：workspace/all-features tests、严格 Clippy、rustdoc、protocol/state parity、OHOS/mobile 与三平台 wrapper | 最终 `1fafee7e2` 全量 run 通过；其中 rustdoc 私有链接、dispatch manifest baseline、dead-code/runtime-contract budget、macOS wrapper 冷启动超时、overdue one-shot 与搜索错误提示均由真实失败或复审驱动修复并在新 head 复验 |
| CodeWhale fork behavior | 默认 `forkguard_`；另以 `--features benchmark-eval-controls` 跑 `forkguard_benchmark_` | 31/31 默认行为 + 6/6 feature-gated eval controls 通过 |
| Parent compile | app `cargo check --lib --locked`、`--all-targets --features benchmark-hooks --locked`；CLI `pinvou-product-backend --locked` | 全部通过；修复前 CLI lock 仍锁在 CodeWhale 0.9.5/rusqlite 0.39.0 并会 fail closed，重生成后解析为 0.9.12/0.40.2 |
| Parent Rust tests | 普通全量 + CI 同构 `PINVOU_REQUIRE_REMOTE_E2E=1`/ARM64 Chromium/Relay 全量 | 重放最新 main 并完成定时 memory-organize 适配后，普通全量为 2,079 通过，0 失败，12 个明确外部场景 ignored；CI 同构全量由父仓 PR 最终 head 复验 |
| Fork topology/fingerprints | `./scripts/fork-guard.sh` | 全过；37 个底座行为回归（含 6 个 feature-gated eval controls）+ 23 个 app 行为回归，并编译/定向执行父仓 `benchmark-hooks` |
| Rust quality | app `fmt`、两级 `clippy`、`RUSTDOCFLAGS=-D warnings cargo doc --no-deps` | 均通过 |
| Rust dependency policy | `cargo shear --deny-warnings`（app/knowledge）；`cargo deny check advisories licenses bans sources`（app/knowledge） | 两个工作区 shear 均为 0 问题；deny 四项策略均通过，仅有策略允许的 path dependency/未命中历史 ignore 提示 |
| Architecture guard | `python3 scripts/architecture-guard.py` | 通过，无新增 architecture debt |
| Prompt ownership | 最终父仓 `dump_system_prompt` + memory enabled/disabled 组装测试 | memory 关闭的真实 dump 为 10,950 bytes、88 行、含输出换行的 SHA-256 `5b019a2871c7bdb93d989f37402fa9d8e4049da83e7b9354df99a969c28b9148`；它不再与 r13 逐字节相同，因为当前 prompt 使用 v0.9.12 canonical `read`/`bash`，保留仅用于 Windows preview 后台会话控制的 legacy `Bash` fallback，并新增按开关填充的 memory section。静态 composer 仍由父仓持有，placeholder 两态和 ambient 隔离均有结果式回归 |
| Frontend | Node 逻辑/静态、build/audit + diff selector 的 19 组 Chromium UI smoke | 重放最新 main 后刷新 UI build，Node 为 544 通过、0 失败、15 skipped，pet 资源验证通过；既有 19 组 UI smoke 全过，其中真实 Relay WebUI 32/32 桌面/移动旅程通过 |
| Relay | `npm --prefix remote-control-relay test` | 23 通过，0 失败 |
| Script/MCP contracts | Python unittest + `mcp-server-contract-smoke.py` + 各内置 MCP `test_*.py` | 95 通过；全部 MCP server contract 及 3 个服务级测试文件通过 |
| Repository/CI contracts | commit message、版本同步、PR routing、fork-link change contract | 均通过；版本文件一致为 `0.9.2`，gitlink 与 fork 文档同步变更 |
| Secret scan | Gitleaks v8.30.1，父仓 `origin/main..HEAD` 与 CodeWhale `v0.9.12..HEAD` | 最终发布提交数为父仓 6 + CodeWhale 15；两个增量范围均复跑通过。裸全历史模式的 105 条升级前历史命中不归因于本次升级 |
| Knowledge crate | fmt/clippy/all-features test/install shell syntax | 92 通过，0 失败；其余均通过 |
| Real model | strict L1 vLLM harness | 27 个场景可枚举；本机无 8000/11434/8080/3000 listener，且 L1/OpenAI/DeepSeek endpoint/credential 环境均未配置，故未执行、不得计为通过 |
| Platform/package | `npm run build`；Linux/Windows/macOS 配置与打包契约 | Linux arm64 release 与 `.deb` 构建通过；4 组契约均通过（knowledge host 17/17）；本机仅安装 `aarch64-unknown-linux-gnu`，Windows/macOS 原生编译留给公开分支 CI |
| Public submodule | branch/tag/gitlink 三者一致 | 通过：公开 `pinvou3-clean`、annotated tag `pinvou-v0.9.12-r1` 的 peeled commit 与父仓 gitlink 均为 `1fafee7e26b60a59457a43bce50c63aa2ad9dbaf` |

`hashlink` 未强制统一：app 的独立锁图把 `rusqlite 0.40.2` 解析到 `hashlink 0.12.1`，`pinvou-cli` 的独立锁图则把同一个 `rusqlite 0.40.2` 解析到兼容的 `hashlink 0.12.2`；各自产物的依赖树内都只有一个 `hashlink` 版本，不存在运行时双版本冲突。没有安全公告或功能修复要求升级，因此不为表面一致性制造无关 lockfile 漂移。

GAIA 观测兼容旧后端缺少 TTFT 的记录：`ttft_ms` 从协议到聚合均保持可选，缺失值计入请求总数但不计入 TTFT/effective-decode 样本；新增 `gaia_diagnostics_accept_model_requests_without_ttft` 回归锁定该行为，避免旧事件因字段缺失导致诊断失败。

补充执行了非 required gate 的 `npm run check:types`。它仍会命中仓库既有的全量 JavaScript 类型债务；本次涉及 JavaScript 事件投影与 canonical 工具展示适配，但 `check:types` 的既有跨仓类型债务不能据此伪装为通过。受影响 JavaScript 路径由对应契约测试和应用构建覆盖。

本机产物为 `pinvou3_0.9.2_arm64.deb`（124,566,246 bytes，SHA-256 `3dab6ae747ed4f6a9806d932fd25d9ed37dfcd48755fd63b83579bf735f647f8`）；只做了只读包清单检查，没有在开发机安装。包内包含主程序、`pinvou-knowledge-server` 与 host helper。

“编译通过”不等于升级完成：只有上述适用门禁完成、自审无未处置 P0/P1，并明确区分本地候选与公开发布状态后，分支才可交付合并。

## 7. 发布与回退

发布链按授权完成：CodeWhale PR #44 先建立 v0.9.12 clean re-fork 基线，PR #46 再收口精确 SHA 发布门禁；最终 head 通过全量 CI 后快进 `pinvou3-clean`，创建不可变 annotated tag `pinvou-v0.9.12-r1`，再将父仓 gitlink 对齐同一 SHA 并执行 `scripts/verify-public-submodule.sh`。受保护分支更新期间只临时移除了无法在 `pinvou3-clean` 上触发的 required status contexts，快进完成后立即恢复原保护配置；force-push 始终关闭，既有 tag 未重写。

父仓 PR #453 在公开一致性门禁通过后进入最终复审；正式合并仍由仓库评审和 Merge Queue 决定，不绕过未解除的 review decision。

出现回归时，父仓通过新提交把 gitlink 回到公开不可变 tag `pinvou-v0.9.5-r13`，并恢复配套应用代码和 lockfile；本地 `backup/pre-v0.9.12-sync` 只作便利引用，不作为公开回退前提。若新版本已经迁移用户数据，还必须一并恢复升级前的数据备份，不能只降二进制。回退不会删除原工作区或历史 tag。
