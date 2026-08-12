# skill 按 scope 治理 — 改动说明

> 关联：`.luzeyang/code-plain-decoupling/skill-scope-governance-实施方案.md`（设计方案与验收矩阵，下称「方案」，已归档）、`docs/code-native-agent.md` §8.3（连接器双 scope 范本）与 §8.6（本改动落地小节）。
> 分支：`feat/skill-scope-governance`（自 `origin/main` HEAD 拉出）。
> 本文件登记方案 §3 七个实施步骤的结果，作为 PR 的验收依据。

## 一、为什么需要这些改动

skill 能力的按会话隔离此前有三条结构性缺陷（方案 §0 的 P1-P4）：

1. **catalogue 泄露面（P1）**：code 会话被禁用 `load_skill`，但 prompt 的 `## Skills` 块仍印全部技能名与磁盘路径（`~/.pinvou3/bundle/skills/...`），被注入引导的模型可 `read_file` 侧读 SKILL.md。
2. **code 会话 skill 能力残废（P2）**：`load_skill` 对 code 会话整体隐藏（过渡方案 D），代码页 skill 开关只读灰显（"假开关"诚实化）——用户想用技能用不了。
3. **项目级 skills 不可见（P3）**：fork #41 后扫描收窄到 bundle 目录，绑项目的 code 会话看不到 `<项目>/.claude/skills` 等。
4. **同一技能"plain 关、code 开"不可表达（P4）**：底座 `DISABLED_SKILLS` 是进程级全局集合，无会话/scope 维度。

统一解法（照方案 §0）：**开关双 scope 持久化（照抄连接器 §8.3）+ 按会话拼组合 skills_dir**——把"按会话的技能集"翻译成引擎现成的发现协议（`EngineConfig.skills_dir` 单根注入，底座每轮重扫渲染 `## Skills` 块；空目录 → 整个块不渲染，泄露面随之封闭）。

**实施中发现并上报的关键事实**：方案 §1/§5 假设"`EngineConfig.skills_dir` 是发现集的唯一配置根"，但 fork #41 后 `skills_directories_with_home_and_mode` 硬编码返回 `~/.pinvou3/bundle/skills`，`insert_configured_skills_dir` 只是 union 追加——组合目录永远无法从发现集排除 bundle/skills 中的禁用技能，P1/P4 无法达成。经用户批准后做了一处**最小 fork 改动**（bundle/skills 硬编码移除，发现完全由 `EngineConfig.skills_dir` 注入），并同步更新 fork-guard 指纹、行为测试与 `docs/fork-modifications.md` 登记（详见 §五）。

## 二、改动总览

| 项 | 内容 | 性质 |
|---|---|---|
| S-1 | `disabled_skills.json` 双 scope 持久化（`{plain, code, code_initialized, project_skills_enabled}`）+ 旧数据迁移 + 全局 `DISABLED_SKILLS` 退役 | 重构 + 迁移 |
| S-2 | 组合目录物化模块 `features/assistant/skill_materialization.rs`（来源枚举、first-wins、拷贝、diff 增量重写、`exists()` 自愈） | 新模块 |
| S-3 | 三时机接线（spawn 全量拼、toggle/安装/卸载事件驱动增量重写、发送路径自愈） | 接线 |
| S-4 | 策略接线（`EngineConfig.skills_dir` 按会话注入、`shape_disallowed_tools` 按组合目录空否决定 `load_skill`） | 接线 |
| S-5 | fork 最小改动（bundle/skills 硬编码移除）+ fork-guard 登记/指纹/行为测试 | fork 改动 |
| S-6 | 前端（代码页 skill 开关恢复可写、全禁空态提示、项目级 skills 开关 + 注入警告，i18n 三语） | 前端 |
| S-7 | 项目级 skills 兜底（fork #41 确认砍断 workspace 并集 → 同一物化通道拷贝，默认关） | 功能 |
| S-8 | 文档（`code-native-agent.md` §3.4/§3.6/§8.6/§9/§10、`fork-modifications.md`、本文件） | 文档 |

## 三、逐项改动说明

### S-1：开关双 scope 持久化

- **什么**：`~/.pinvou3/disabled_skills.json` 存 `DisabledSkillsFile { plain, code, code_initialized, project_skills_enabled }`（`features/assistant/skill_materialization.rs`），与连接器 `disabled_connectors.json` 同构；旧数据自动迁移——
  - 裸数组 `["a","b"]`（旧 disabled_skills.json 形态）→ plain scope；
  - 旧版借道 `disabled_connectors.json` 的 `skill:<id>` 条目（本分支历史实现 `refresh_disabled_skills`）→ strip 前缀提取进 plain scope，并清除连接器文件残留（避免两处真相）；迁移失败按「全部启用（plain）+ code 默认全禁」安全兜底。
- **code scope 未初始化时默认全禁已装技能**（外部能力显式开启，与连接器 §8.3 同语义）——这一步同时天然封掉 P1：code 默认组合目录为空 → `## Skills` 块不渲染。用户首次在代码页改开关后 `code_initialized=true`，以落盘为准。
- 新装技能：code 已初始化时默认加入 code 禁用集（`sync_code_scope_after_skill_install`，与连接器同语义）；卸载时从两个集合清除残留（`remove_skill_from_disabled_scopes`）。**语义变化**：旧实现卸载后保留用户关闭选择、重装后仍禁用；新语义卸载清除残留、重装默认启用——与连接器卸载语义统一（方案 §2.1 明确要求）。
- **全局 `DISABLED_SKILLS` 退役**：启动段 `set_disabled_skills(vec![])`，过滤职责移交组合目录。`refresh_disabled_skills` 函数删除，11 处调用点全部迁移（启动、连接器开关、技能安装/卸载/导入、ima 连接/退出）。

### S-2：组合目录物化模块

- 位置：`~/.pinvou3/sessions/<sid>/skills/`（会话私有目录，删会话自动清理，V-9）。
- 内容：该会话 scope 中**启用**的技能目录。来源 first-wins 顺序：用户 skills 目录（`~/.pinvou3/user/skills/`）> bundle/skills（市场安装 + 内置）；同名技能按高优先级来源入目录、低优先级跳过。另排除该 scope 被禁用连接器的 companion skills（保持「关 MCP → 关联技能一并隐藏」的既有联动，且从旧的"只认 plain scope"修正为按 scope 各自联动）。
- 物化方式：整目录拷贝（SKILL.md + 伴随文件，跳过市场标记 `.installed-from`），staged + rename 原子替换；留 `copy_skill_dir` 单点，后续可换 junction 优化（首版零新依赖）。

### S-3：三时机接线（不做每轮 diff）

1. **spawn 全量拼**：`EnginePool::get_or_spawn_with_policy` 在 spawn_for_session 前按该会话 scope（含项目级开关）全量物化（V-7）。
2. **事件驱动增量重写**：`EnginePool::refresh_live_sessions_skills[_blocking]` 对**所有在线会话**做 diff 增删（幂等；toggle/安装/卸载命令落盘后调用）；不在线的会话下次 spawn 全量拼。底座每轮重扫 → **下一轮 prompt 即生效**（V-3）。
3. **发送路径自愈**：`build_send_message_op` 对组合目录做一次 `exists()` 检查，缺失则重建（微秒级 stat，防手动删除后静默丢失；V-7 自愈项）。

### S-4：策略接线

- `EngineConfig.skills_dir` 在 `build_engine_config_for_session_roots` 按会话指向组合目录（headless 单引擎路径保持 bundle/skills）。
- `SessionPolicy::extra_hidden_tools()` 不再恒含 `load_skill`（只保留 `present_artifact`）；`bridge.shape_disallowed_tools` 对 code 会话按 `session_skills_is_empty` 动态补 hide `load_skill`——空 → 隐藏（避免"开关开着但没技能"的假状态），非空 → 放行（V-5）。spawn 初值与 `set_disallowed_all` 热刷两路都经此整形。

### S-5：fork 最小改动（经用户批准）

- `CodeWhale/crates/tui/src/skills/mod.rs`：`skills_directories_with_home_and_mode` 移除 `~/.pinvou3/bundle/skills` 硬编码，返回空集——技能发现完全由 `EngineConfig.skills_dir` 单一配置根注入。
- 影响：app plain 会话（skills_dir=bundle/skills）单根 [bundle/skills]，与现状**逐字节等价**；app code 会话（skills_dir=组合目录）单根 [组合目录]，过滤生效；`discover_in_workspace`（无 skills_dir 分支）返回空集（app 全链路恒设 skills_dir，不受影响；裸 CLI 不再扫 pinvou3 私有路径）。
- fork-guard：T3 指纹 `skill 来源收敛到 bundle` 更新为 `skill 发现单一配置根注入`；新增行为测试 `forkguard_skill_discovery_is_single_root_engine_config_skills_dir`（无 skills_dir 发现集为空、`_and_dir` 只扫注入目录）；`docs/fork-modifications.md` T3 登记。

### S-6：前端

- `composer-tool-menu-logic.js`：移除 `skillsUnavailable`（code scope 技能行只读灰显），技能行两个 scope 都可写；技能行启用态改由独立 `disabledSkillIds` 判定。
- `SettingsView.jsx` ComposerToolMenu：`toggleTool` 按行 kind 分流——技能行走新命令 `set_disabled_skills(skillIds, scope)`（写 `disabled_skills.json` 双 scope），工具/服务行走原 `set_disabled_connectors`；"该 scope 全部技能已关闭"空态提示（组合目录为空 → 模型看不到任何技能）；code scope 下新增**项目级 skills 开关** + 开启后注入风险警告文案；代码页（`CodexAcpView` scope="code"）自动继承可写行为。
- 新命令：`set_disabled_skills` / `get_disabled_skills` / `set_project_skills_enabled` / `get_project_skills_enabled`（`app/commands/connectors.rs`，注册于 lib.rs；`set_*` 落盘后重写在线会话组合目录 + 热刷 disallowed_tools + 广播 `remote_control:tools_changed`）。
- i18n：`composerSkillAllDisabled` / `composerProjectSkills` / `composerProjectSkillsDesc` / `composerProjectSkillsWarning` 三语齐全；废弃 `composerSkillCodeDisabled`。

### S-7：项目级 skills（P3 兜底）

- 验证结论：fork #41 确认砍断 workspace 并集扫描（`skills_directories_with_home_and_mode` 完全忽略 workspace），且 `skills_scan_codewhale_only` 旗标对收窄后两 mode 行为一致——无 config 可恢复 → 走方案 §2.4 兜底：**项目技能经同一物化通道拷入组合目录**。
- 来源与优先级：`.agents/skills` > `.pinvou/skills` > `skills` > `.opencode/skills` > `.claude/skills` > `.cursor/skills` > `.codewhale/skills`（底座上游 #432 顺序，`.pinvou/skills` 为 pinvou3 自有约定插在 `.agents/skills` 之后），排在用户/市场来源之前（workspace 目录优先语义）。
- **策略开关默认关**（`project_skills_enabled`，持久于 disabled_skills.json）：项目内文本是 prompt-injection 面，开启路径有警告文案。仅 code scope 生效；plain 不受影响。
- 物化路径：`enabled_skills_for(scope, project_workspace)`——EnginePool spawn/事件重写/发送自愈三时机都从 `session_roots(session_id).execution` 取项目根传入（绑项目 code 会话 = 项目目录）。

### S-8：文档

- `docs/code-native-agent.md`：§3.4 工具整形描述、§3.6 策略取值描述更新；新增 §8.6 skill 按 scope 治理小节；§9 移除 skill 侧路残留条目（改为已根治登记）；§10 #3 更新（X-1 标记项关闭）。
- `docs/fork-modifications.md`：T3 登记 fork 改动。
- 本文件。

## 四、验证结果

- 前端：`node tests/composer_tool_menu_logic.test.js` ✅（code scope 可写、全禁状态、enabledCount 断言更新）。
- Rust：`cargo check --tests` 通过（见 §五）；新增单测覆盖——
  - 持久化：双 scope roundtrip、裸数组迁移、`skill:` 借道迁移（含连接器文件残留清除）、code 未初始化默认全禁、安装/卸载 scope 同步、项目级开关 roundtrip；
  - 组合目录：first-wins（user 覆盖 bundle）、scope 禁用过滤、materialize→rewrite 幂等、增量增删、空集空目录 + 自愈、companion 排除、项目技能开关/优先级、会话私有根。
- fork 侧：`forkguard_skill_discovery_is_single_root_engine_config_skills_dir`（编译通过；本机测试 exe 启动 `0xc0000139` 为既有环境问题，实际运行以 CI 为准）。
- 待做（需启动应用，人工走查）：V-1 dump_system_prompt 无 `## Skills` 块、V-2 两会话对照、V-3 toggle 时效、V-4 代码页开关真实、V-9 删会话清理组合目录。

## 五、验收矩阵对照（方案 §4）

| # | 验收项 | 状态 |
|---|---|---|
| V-1 | 泄露面封闭（code 全禁默认 → 无 `## Skills` 块） | 机制成立（空组合目录 → 底座空 registry 不渲染块）；手动 dump 待做 |
| V-2 | 双 scope 独立（plain 关 X / code 开 X） | 单测覆盖（enabled_skills_for 按 scope 过滤 + code 未初始化全禁）；手动两会话待做 |
| V-3 | toggle 时效（下一轮 prompt 生效，仅该 scope 会话字节变化） | 事件驱动增量重写 + 每轮重扫机制成立；手动待做 |
| V-4 | 代码页开关真实（可写、写 code scope 不影响 plain） | 前端恢复可写 + `set_disabled_skills` 双 scope 落盘；手动待做 |
| V-5 | load_skill 放行联动（非空可用/空隐藏） | `shape_disallowed_tools` 按目录空否 + 单测编译通过；手动待做 |
| V-6 | 项目级 skills（默认关、开启可见可 load、警告） | 默认关 + 拷贝通道 + 前端警告落地；手动待做 |
| V-7 | 时机正确性（spawn/事件/自愈一致） | 单测覆盖（materialize/rewrite 幂等、自愈重建）；手动待做 |
| V-8 | plain 零影响（全局开关 → plain scope 透明迁移） | 迁移 + 等价机制（plain 组合目录 = 原 bundle/skills 全量 − plain 禁用集）；前端测试全绿 |
| V-9 | 清理（删会话删组合目录） | 组合目录在 sessions/<sid>/ 下随会话删除自动清理；手动待做 |
| V-10 | 缓存行为（无 toggle 时逐轮字节稳定） | 物化只在三时机发生、无每轮 diff（代码走查）；日志审查待做 |
| V-11 | 自动检查 | 见 §四/§五 |

## 六、行为兼容与遗留

- **行为不变面**：plain 会话 catalogue/load_skill/开关语义与改动前一致（全局禁用集 → plain scope 透明迁移，companion 联动保留）；连接器开关链路零变化。
- **行为变化面（刻意）**：
  - code 会话 skill 从"整体禁用 + 只读灰显"变为"按 code scope 开关真实生效 + 可写"（P2 修复）；
  - code 默认全禁 → 用户首次在代码页打开技能前，code catalogue 无技能块（P1 修复，安全默认）；
  - 卸载技能清除两个 scope 禁用集残留 → 重装后默认启用（与连接器卸载语义统一）；
  - 用户手放技能（`~/.pinvou3/user/skills/`）进入组合目录（新能力，fork #41 砍掉的手放路径以受控方式回归）；
  - companion 联动从"只认 plain"修正为按 scope 各自联动（code 禁用连接器 → 其 companion 技能在 code 也隐藏）。
- **遗留**：组合目录路径（`~/.pinvou3/sessions/<sid>/skills/...`）出现在 catalogue（方案 §2.2 明确可接受，不再泄露 bundle 内部结构）；bundle 解包更新技能内容后运行中会话拷贝滞后到下次 spawn（技能低频更新，可接受）；`cargo test` 本机 `0xc0000139` 既有环境问题照旧，Rust 单测实际运行以 CI 为准。
