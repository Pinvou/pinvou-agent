# CodeWhale v0.9.5 r13 → v0.9.12 r1 升级报告

> 状态：本地候选；自动化与本机可执行门禁已通过，真实 vLLM 和公开发布待外部条件（2026-09-07）
> 目标：把 Pinvou Agent 底座完整迁到官方 CodeWhale v0.9.12，同时把 fork 收敛为可解释、可测试、可回退的 4 个长期主题。

## 1. 基线与隔离

| 对象 | 值 |
|---|---|
| 父仓基线 | `origin/main` `0147e2ef` |
| 父仓工作分支 | `codex/codewhale-v0.9.12-sync` |
| 上游 release | `v0.9.12` `dcd4c200f72f0c1ffd60d8e7f6850313db879fc5` |
| 新 fork 分支 | `codex/pinvou-v0.9.12-r1` |
| 新 fork head | `b4c02616b8561dfca43d540fe778bb15287fa719` |
| 旧 fork 回退点 | `backup/pre-v0.9.12-sync` `f853f8f1566c57e6be40d5439a222a932aa79ef5` |

所有开发在隔离 worktree 完成；原始 `/home/whc0005/workspace/pinvou-agent` 的已有改动和未跟踪文件未被修改、清理或提交。

## 2. 评估结论

升级前同时做了三个独立视角的审视：

1. **fork 差异审计**：逐文件判断旧 r13 patch 在 v0.9.12 中是已吸收、需语义迁移，还是仍需底座实现。
2. **主应用兼容审计**：追踪 `pinvou3-app` 实际调用的 EngineConfig、事件、工具、会话、Automation 和持久化路径。
3. **验证/发布审计**：核对 CI、跨平台、包发布、真实模型、公开 submodule 与回退门禁。

旧 r13 修改 110 个文件，其中 104 个同时被 v0.9.12 上游改动，直接移植预计冲突 57 个文件。旧 fork drift 为 `+10895/-1195`；新候选从官方 tag clean re-fork 后为 `+3127/-614`（62 文件，净增 2513 行），因此没有采用 merge/cherry-pick 冲突树。新增触达文件包含当前 Rust/Clippy 发布门禁所需的等价条件折叠、`Default` 补全和窄 lint 说明，不引入新的 fork 行为主题。

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

- **T1 宿主/路由**：窄 facade、route-with-limits、可靠 steer、session owner、批量 child cancel、raw worker ledger。
- **T2 工具/安全**：ExtraTools、MCP secret resolver、逐轮精确策略、最终分发复检、只读/审批 fail-closed、File 64 KiB、受限日志脱敏。
- **T3 Prompt/Skills**：static composer 所有权、ambient authority 隔离、单一显式 Skills 根、Permissions 100 KiB 窄预算、working-set reminder 隔离。
- **T4 Automation/Task**：稳定 conversation key、ThreadCreated、v3 writer/v4 窄兼容、离线不补跑、不重叠、终态清理。

## 4. 主应用改造

- `deepseek_tui::AppMode` 直接使用 v0.9.12 根 re-export；旧 `Yolo/Auto` 逻辑改成 Agent 与显式 approval/trust 组合。
- bridge 按 v0.9.12 `EngineConfig` 重建字段，保留有限 turn/tool budget、read denylist、bubblewrap、MCP OAuth、goal 与 telemetry 安全配置。
- `session_id` 在 `Engine::spawn` 前写入配置；不依赖事件到达后猜测归属。
- 精确工具/只读策略以 `TurnToolSecurityPolicy` 随 `SendMessage` 下发，restricted turn 不携带 dynamic tools。
- forwarder 只接收 owner session 匹配的 Agent/Workflow/SubAgent 事件。
- Scheduled executor 使用 v0.9.12 `TaskExecutionLimits`、`TaskTerminalReason` 和 `ThreadCreated`；Pinvou 精确 `model_id` 继续由 automation id 对应的 companion binding store 持久化，新增的 CodeWhale provider registry 字段刻意留空，二者不混用。
- `rusqlite` 对齐 0.40.2，并重生成父仓 `Cargo.lock`。

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
| CodeWhale full lib | `RUST_MIN_STACK=16777216 cargo test -p codewhale-tui --lib --locked -- --test-threads=1` | 11,690 通过，0 失败，12 ignored |
| CodeWhale release lane | `cargo test --workspace --all-features --locked -- --test-threads=1`；严格 workspace/all-targets/all-features Clippy；protocol/state parity；OHOS dependency contract | 全部通过；关键集合为 TUI 11,705、PTY/Cucumber 33、integration 283，均 0 失败，另含其余 crate 与 doc tests |
| CodeWhale fork behavior | `cargo test -p codewhale-tui --lib --locked forkguard_ -- --test-threads=1` | 18/18 通过 |
| Parent compile | `cargo check --lib --locked` | 通过 |
| Parent Rust tests | 普通全量 + CI 同构 `PINVOU_REQUIRE_REMOTE_E2E=1`/ARM64 Chromium/Relay 全量 | 两轮均为 2,006 通过，0 失败，12 个明确外部场景 ignored |
| Fork topology/fingerprints | `./scripts/fork-guard.sh` | 全过；18 个底座行为回归 + 23 个 app 行为回归 |
| Rust quality | app `fmt`、两级 `clippy`、`RUSTDOCFLAGS=-D warnings cargo doc --no-deps` | 均通过 |
| Rust dependency policy | `cargo shear --deny-warnings`（app/knowledge）；`cargo deny check advisories licenses bans sources`（app/knowledge） | 两个工作区 shear 均为 0 问题；deny 四项策略均通过，仅有策略允许的 path dependency/未命中历史 ignore 提示 |
| Architecture guard | `python3 scripts/architecture-guard.py` | 通过，无新增 architecture debt |
| Prompt ownership | v0.9.5 r13 与 v0.9.12 r1 dump/字节对照 | 均为 89 行、10,419 bytes、SHA-256 `02c4d4b6b1b4b1f33ffaacd3157508f239400547d5550dcea06e6ad44cfedbe5`，`cmp=0` |
| Frontend | 逻辑/静态/build/audit + diff selector 的 19 组 Chromium UI smoke | 537 通过、0 失败、16 skipped；19 组 UI smoke 全过，其中真实 Relay WebUI 32/32 桌面/移动旅程通过；无未引用项或循环依赖（静态检查仅有既有 warn 级提示） |
| Relay | `npm --prefix remote-control-relay test` | 23 通过，0 失败 |
| Script/MCP contracts | Python unittest + `mcp-server-contract-smoke.py` + 各内置 MCP `test_*.py` | 89 通过；全部 MCP server contract 及 3 个服务级测试文件通过 |
| Repository/CI contracts | commit message、版本同步、PR routing、fork-link change contract | 均通过；版本文件一致为 `0.9.2`，gitlink 与 fork 文档同步变更 |
| Secret scan | Gitleaks v8.30.1，父仓 `origin/main..HEAD` 与 CodeWhale `v0.9.12..HEAD` | 本次 3 + 5 个提交均为 0 新命中；裸全历史模式仍报告 105 条升级前历史命中，最近 10 次 GitHub PR secret-scan 工作流均成功 |
| Knowledge crate | fmt/clippy/all-features test/install shell syntax | 92 通过，0 失败；其余均通过 |
| Real model | strict L1 vLLM harness | 27 个场景可枚举；本机无 8000/11434/8080/3000 listener，且 L1/OpenAI/DeepSeek endpoint/credential 环境均未配置，故未执行、不得计为通过 |
| Platform/package | `npm run build`；Linux/Windows/macOS 配置与打包契约 | Linux arm64 release 与 `.deb` 构建通过；4 组契约均通过（knowledge host 17/17）；本机仅安装 `aarch64-unknown-linux-gnu`，Windows/macOS 原生编译留给公开分支 CI |
| Public submodule | branch/tag/gitlink 三者一致 | 未通过（预期）：公开 `pinvou3-clean` 仍为旧 r13 `f853f8f1...`，`pinvou-v0.9.12-r1` 尚不存在；必须等明确授权发布 |

补充执行了非 required gate 的 `npm run check:types`。它仍会命中仓库既有的全量 JavaScript 类型债务；本次没有修改 JavaScript 源码，因此不把该结果归因于底座升级，也不把它伪装为通过。

本机产物为 `pinvou3_0.9.2_arm64.deb`（124,566,246 bytes，SHA-256 `3dab6ae747ed4f6a9806d932fd25d9ed37dfcd48755fd63b83579bf735f647f8`）；只做了只读包清单检查，没有在开发机安装。包内包含主程序、`pinvou-knowledge-server` 与 host helper。

“编译通过”不等于升级完成：只有上述适用门禁完成、自审无未处置 P0/P1，并明确区分本地候选与公开发布状态后，分支才可交付合并。

## 7. 发布与回退

本轮不会自动 push、创建 tag 或发布 release。获得明确授权后按以下顺序执行：

1. 推送 CodeWhale `pinvou3-clean` 到候选 head。
2. 创建不可变 tag `pinvou-v0.9.12-r1` 指向同一 head。
3. 确认父仓 gitlink 与 branch/tag 三者一致。
4. 执行 `scripts/verify-public-submodule.sh` 的真实远端校验。
5. 再推送父仓分支并进入 PR/CI。

出现回归时，底座可回到 `backup/pre-v0.9.12-sync`；回退不会删除原工作区或历史 tag。
