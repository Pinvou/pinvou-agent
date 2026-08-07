# code 会话权限默认值与 mode 持久化 — 改动说明

> 关联：`docs/code-native-agent.md` §8.6（行为真相源）、`docs/code-plain-decoupling-改动说明.md`（前序解耦）、`.luzeyang/code-plain-decoupling/code-plain-decoupling-剩余待改动项评估.md`（S-1 背景，已归档）。
> 分支：`feat/code-mode-permission`（基于最新 main，含 #182 解耦成果）。
> 本文件登记 S-1 权限决策的实施结果，作为 PR 的验收依据。

## 一、为什么需要这个改动

code 会话执行根是用户真实项目目录，但此前权限语义有两个商业级缺口：

1. **默认暴露**：新 code 会话默认 Yolo（全自动 + 全工具 + 直写真实仓库），且 mode 不持久化——谨慎的用户切过 Plan，重启后默默回弹 Yolo，以为保护还在。
2. **无放权确认**：从只读到全自动没有任何告知环节，用户对"模型此刻可以无审批改写我的仓库"无感知。

产品决策（用户拍板，VS Code `chat.permissions.default` + 首次选 Bypass 弹警告的同款形态）：

- code 模式**首次使用默认 Plan**（R-1 已让 Plan 有真实审批体验）；
- **切 yolo 时弹一次性警告**，全局记忆，以后不再弹；
- **两层持久化**：每会话恢复各自 mode；新 code 会话默认跟随上次使用的 mode。

该方案吸收原 S-1 档 1（首绑警告）与档 3（默认 Plan）：放权动作只在切 yolo 时发生，警告时机比绑定目录时更精准。

## 二、改动总览

| 层 | 内容 |
|---|---|
| 后端 | `mode_state` 默认值解析（code→全局 last_mode→Plan；plain→Yolo 不变）；per-session mode 持久化（仅 code）；全局 `code_permission` 域；两个新命令 |
| 前端 | code 页 mode 由后端驱动（去三处写死 `'yolo'`）；切 yolo 确认门 + `NativeYoloConfirmCard` |
| 边界 | 仅品悟原生 code 会话；ACP 与 plain/work 行为逐字节不变 |

7 个文件修改 + 2 个文件新增（约 +550/-33）。

## 三、逐项改动说明

### 后端

- **默认值解析**（`features/sessions/mod.rs`）：`mode_state(id)` 有内存条目原样返回；无记录走 `resolved_default_mode`——code 会话取全局 `last_mode`（None→Plan），plain→Yolo。仅几次 RwLock 读、不触盘、不物化条目（chat.rs 每轮发送路径调用，保持廉价）。code 判定经 lib.rs 注入的 `code_session_predicate`（与 bridge/remote_control 共用同一份 `SessionAgentStore` 闭包，ACP 会话恒判 plain，天然排除）。
- **关键正确性修复**：plan/技能/persona/知识库挂载等 **9 处** `or_default()` 物化站点统一改走 `mode_state_entry`——否则首次默认 Plan 的 code 会话出方案时会被物化成 Yolo，plan 卡静默丢失。单测 `fresh code 会话 register_pending_plan` 钉住。
- **两层持久化**：`set_mode` 仅对 code 会话写 `~/.pinvou3/sessions/_code_mode_states.json`（仿 `_skill_bindings.json` 模式；删除会话/`reset_mode_state` 同步清理）并更新全局 `last_mode`；plain 完全不落盘。启动时加载合并进 mode_states。
- **全局键**（`platform/prefs/mod.rs`）：`UserPrefs` 新增 `code_permission { last_mode, yolo_confirmed }` 域（serde default 兼容旧 settings.json），经 `update_transaction` 字段级事务写盘——已核实所有设置写路径为补丁式，不会被设置页回写冲掉。
- **新命令**（`app/commands/interaction.rs`）：`get_code_permission_prefs() -> { last_mode, yolo_confirmed }`、`confirm_code_yolo()`。`exit_plan_to_yolo` 不做后端门控（确认是 UI 层语义，与 VS Code 同款）。
- **任务级切换不记忆**：`accept_plan` 经 `claim_pending_plan` 切 yolo 属任务级动作，不写两层持久化——重启后会话恢复其持久化 mode（刻意语义，批准方案 ≠ 变更权限偏好）。

### 前端

- **mode 后端驱动**（`CodexAcpView.jsx`）：去掉 useState 初值与两处回落共三处写死 `'yolo'`；切换/新建会话时按 `get_mode_state` + 全局偏好渲染 chip。新增纯逻辑模块 `code-permission-state.js`（fallback 矩阵：无记录→Plan、读取失败按未确认的安全方向）。
- **确认门**：`switchNativeMode` 切 yolo 前查 `yolo_confirmed`——false 弹 `NativeYoloConfirmCard`（复用审批卡风格与既有 backdrop 弹层，三语文案含"以后不再提示"），【确认】调 `confirm_code_yolo` 后继续原切换路径（含 busy 先 cancel），【取消】留在 Plan；true 直接切。草稿态（未建会话）同门，并补齐 yolo 方向暂存应用的原缺口。
- i18n：`uiCodex` 新增 5 key × zh/en/ja。

### 测试

- Rust 7 个新单测：默认值矩阵、per-session 重启恢复与删除清理、plain 不持久化、确认标志跨重启、fresh code 会话 plan 登记、prefs 旧 JSON 兼容。
- 前端新套件 `code_permission_state.test.mjs`（纯逻辑矩阵 + 源码接线契约），并入 `test:codex-acp`。

## 四、验证结果

- `cargo fmt --check` / `cargo check --tests` / `cargo clippy --tests` ✅（零 error、零新增 warning）。
- 前端：`test:codex-acp` 全链 ✅、`lint:ui` ✅、`test:ui-language` ✅、`check:architecture` ✅。
- **Rust 单测未实际运行**：本机测试 exe 启动即 `0xc0000139`（既有机型 DLL 问题，与改动无关）；新增用例以 CI `cargo test --lib` 为准。
- 人工走查待做：首用 code 默认 Plan 出方案审批卡；首次切 yolo 确认后全局不再弹；重启各会话恢复各自 mode、新会话跟随上次；plain/ACP 回归。

## 五、行为兼容与遗留

- **行为变化面（刻意，仅原生 code 会话）**：新会话默认 Plan（原 Yolo）；mode 持久化（原重启回弹）；切 yolo 一次性确认卡。plain/work 与 ACP 零变化。
- **遗留**：S-1 档 2（危险命令护栏，防注入/防幻觉命令，与本方案互补）、S-3（危险路径拦截 + 非 git 警告）、写绑定目录外确认——见剩余项评估；`accept_plan` 后即时重启会恢复持久化 mode（任务级 yolo 不记忆，刻意）。
