# 原生代码会话安全审查修复 — 改动说明与验收矩阵

> 关联：`docs/code-native-agent-安全审查问题清单.md`（问题来源，下称「清单」）。
> 分支：`fix/code-native-agent-review-issues`，基线已同步 origin/main（0.7.5）。
> 本文件登记清单 28 项问题 + 1 项验证缺口的处置结果，作为 PR 的验收依据。

## 一、修复总览

- **6 项 Major 全部修复**：#1 symlink 拒读、#2 家目录边界、#3 sidecar 回填、#4/#5 方案 D（代码会话禁用 `load_skill` + UI 诚实化）、#6 SessionRoots 完整收敛。
- **14 项 Minor 全部修复**（#7-#20）。
- **Nit 修复 7/8**：#21-#27 已修；**#28 不修**（底座既有行为、纯外观，按 fork 边界留上游处理）。
- **验证缺口 #29**：本机复现 `0xc0000139`（DLL 加载期失败，与代码无关）；全部改动**编译零新增警告**，测试实际运行以 CI `rust-test` 为准。

## 二、逐项改动说明

### Major

**#1 AGENTS.md symlink 注入** — `bridge.rs` 新增 `is_plain_file()`（`symlink_metadata` 拒读 symlink，对齐上游 `load_context_file` 防御范式）；`code_session_project_rules` 改用它。测试 `code_session_project_rules_rejects_symlinked_agents_md`（Windows 无 symlink 权限时优雅跳过）。

**#2 家目录边界 Windows 死代码 + #12 + #22** — 新增 `normalize_rule_boundary_path`（canonicalize + `platform_compat_path` 去 `\\?\` 前缀，与绑定入口同源）；边界判断抽为纯函数 `collect_project_rule_chain(project_root, home)`（home 可注入，解决 #22 不可测）；补 `dir == home` 停止条件（#12）；归一化失败 fail-closed（旧实现 fail-open 直穿盘符根）。测试 `project_rule_chain_stops_at_home_boundary` 覆盖四种情形。

**#3 sidecar 单向修复** — `SessionStore::backfill_missing_code_session_sidecars()`：启动恢复时对「索引在而 sidecar 缺失」记录补写；写失败逐条日志、不再静默。测试 `missing_code_native_sidecar_is_backfilled_from_index`（含幂等、ACP 不参与）。

**#4/#5 skill 假开关 / companion skills 绕过（过渡方案 D）** —
- Rust：`shape_disallowed_tools` 对代码会话追加禁用 `load_skill`（spawn 与热刷两路生效），doc 注释说明理由并指向能力档案设计。断言挂既有整形测试。
- 前端：`buildComposerToolMenuState` 增加 scope 入参，code scope 下 skill 行只读灰显（`unavailableRow`）、不计入 enabledCount，并显示说明文案 `composerSkillCodeDisabled`（中英日三语）。plain scope 逐字节不变。测试 `composer_tool_menu_logic.test.js` 新增 code scope 与 plain 回归用例。
- **如实登记的残留**：`## Skills` catalogue 仍印 skill 路径，`read_file` 侧路未封；根治见 `docs/code-native-agent-会话能力档案设计.md`（已更新为替换语义：「plain 关、code 开」成立）。已登记 `docs/code-native-agent.md` §9。

**#6 SessionRoots 零迁移（完整收敛，用户敲定）** —
- 入口下沉：`SessionRoots`/`ExecutionRootResolver`/纯函数 `session_roots_for` 移至 `features/sessions/mod.rs`；`SessionStore` 新增 `session_roots(id)`，lib.rs 把同一份 resolver 闭包同时注入 bridge 与 store（clone 共享 Arc）。
- 全量改名 `execution_workspace` → `ledger_root`（旧名值在所有会话类型下恰等于账本根，薄封装逐分支等价；选全量改名而非 deprecated 是避免误导命名残留与新增警告）。
- 迁移点：`chat.rs`（含 `reference_absolute` 改判 `roots.ledger != roots.execution`，消除对 AcpPool state 的依赖）、`engine.rs`、`engine_pool.rs`、`codex.rs`、`codex_acp/mod.rs`、`snapshot.rs` 及 20+ 账本消费点；清单外新发现并收敛 `codex.rs:588`、`codex_acp/mod.rs:248` 等 5 处。行为零变化，逐点等价性论证见提交 diff。新增 3 个 sessions 单测。
- 遗留风险（已注释）：`chat.rs` 等价性依赖「store 与 AcpPool 注入同一份 SessionAgentStore 闭包」（lib.rs 已写明）。

### Minor

- **#7** 恢复日志误报：`restore_missing_code_session_record` 返回 `Result<bool>`，仅真实恢复计数。
- **#8** `session_roots` 双次调 resolver 竞态：随 #6 收敛为 `SessionStore::session_roots` 单次解析（resolver 经 RwLock 读取一次），账本根不再短暂错指。
- **#9** 工具 profile 缺口记录恢复：`docs/code-native-agent.md` §10 新增待决策项 3，精确描述现状与能力档案承接关系（原删除提交 84e9e5e6 为 PR 合入前提交、不在本地历史，按清单描述恢复并精确化）。
- **#10** 远程事件过滤器 6 个单测（字段名变体、None predicate、误伤普通会话、无会话 id 事件不误杀）。
- **#11** RPC 残留登记：manager.rs `validate_web_rpc_scope` 上方注释 + `docs/code-native-agent.md` §9 正式登记，封堵策略待审阅者确认。
- **#13** 注入顺序改 root→cwd（用户敲定，行为变更）：`chain.reverse()`，祖先在前、项目根最后，与 codex/claude 惯例对齐；顺序断言入测试。
- **#14** 注释矛盾：§8.5 与 bridge.rs doc 按最终行为重写。
- **#15** fork 登记精确化：`docs/fork-modifications.md` T3 条目补「归一化比较 Windows 生效 / symlink 拒读 / fail-closed」。
- **#16** sidecar `version` 读时校验：高于当前版本拒读按缺失处理，由回填按当前版本重写。测试 `code_native_sidecar_rejects_newer_schema_version`。
- **#17** ACP 残留 sidecar 自愈：启动扫描直接清理并如实记日志，不再每启动误报。
- **#18** 扫描根单一来源：`mod.rs` 改用 `store::code_session_sidecar_root`。
- **#19** `disabled_connectors.json` 原子写（底座 `write_atomic` 惯例）+ 进程内 Mutex 串行化三个并发写入口；跨进程并发不在范围。测试 `concurrent_scope_writes_do_not_lose_updates`。
- **#20** `parse_connector_scope` 返回 `Result`：非法非空 scope 报错回显，缺省保持 plain。3 个测试。

### Nit

- **#21** 注释修正「仅品悟原生，不含 ACP」。
- **#24** §8.5 登记「会话中途编辑 AGENTS.md 不生效（spawn 时定型）」。
- **#25** `entries.flatten()` 改显式 match，迭代错误记日志。
- **#26** `set_acp_workspace` 调整为先 persist 后删 sidecar，消除中间态。测试 `failed_index_persist_keeps_code_native_sidecar`。
- **#27** 死代码删除（`ConnectorScope::as_str`、`apply_disabled_connectors`），grep 确认零调用方。
- **#23** 补 100KB 截断端到端断言 + 双门控「resolver 命中但非 code_session」分支测试。
- **#28 不修**：prompts.rs `source` 属性未转义为底座既有行为（纯外观），按 fork 边界留上游，不在本 PR 制造 fork  diff。

## 三、验收矩阵

| # | 严重度 | 修复方案 | 主要改动位置 | 测试 | 验收状态 |
|---|---|---|---|---|---|
| 1 | Major | symlink_metadata 拒读 | bridge.rs `is_plain_file` | `..._rejects_symlinked_agents_md` | ✅ 编译过，CI 待跑 |
| 2/12/22 | Major | 同源归一化 + 纯函数 + fail-closed + dir==home | bridge.rs `normalize_rule_boundary_path`/`collect_project_rule_chain` | `project_rule_chain_stops_at_home_boundary` | ✅ 编译过，CI 待跑 |
| 3 | Major | 启动回填 sidecar | store.rs `backfill_missing_code_session_sidecars` | `..._is_backfilled_from_index` | ✅ 编译过，CI 待跑 |
| 4/5 | Major | 方案 D：禁 load_skill + UI 只读 | bridge.rs `shape_disallowed_tools`；SettingsView.jsx/composer-tool-menu-logic.js/i18n.js | 整形断言 + `composer_tool_menu_logic.test.js` | ✅ 前端测试已绿；Rust CI 待跑 |
| 6 | Major | 入口下沉 SessionStore + 全量改名 ledger_root + 调用点迁移 | sessions/mod.rs、bridge.rs、chat.rs、engine.rs、engine_pool.rs、codex.rs、codex_acp/mod.rs、snapshot.rs 等 | sessions 3 个新单测 | ✅ 编译零警告 + architecture-guard 过，CI 待跑 |
| 7 | Minor | 真实恢复才计数 | store.rs `restore_missing_code_session_record` | `..._reports_real_recovery_only` | ✅ 编译过，CI 待跑 |
| 8 | Minor | 随 #6 单次解析 | sessions/mod.rs `session_roots` | 同 #6 | ✅ |
| 9 | Minor | §10 恢复待决策项 | docs/code-native-agent.md | — | ✅ 文档 |
| 10 | Minor | 过滤器 6 单测 | remote_control/manager.rs tests | 6 个新测试 | ✅ 编译过，CI 待跑 |
| 11 | Minor | 注释 + §9 登记 | manager.rs、docs/code-native-agent.md | — | ✅ 登记，待审阅者确认 |
| 13 | Minor | 注入顺序 root→cwd | bridge.rs `chain.reverse()` | 顺序断言 | ✅ 编译过，CI 待跑 |
| 14/15 | Minor | 注释/登记按最终行为重写 | bridge.rs、docs/code-native-agent.md §8.5、fork-modifications.md | — | ✅ 文档 |
| 16 | Minor | version 校验拒读 | store.rs `read_code_session_sidecar` | `..._rejects_newer_schema_version` | ✅ 编译过，CI 待跑 |
| 17 | Minor | 残留 sidecar 启动清理 | codex_acp/mod.rs | `sidecar_startup_recovery_...` | ✅ 编译过，CI 待跑 |
| 18 | Minor | 扫描根单一来源 | store.rs/mod.rs | 同 #17 测试 | ✅ |
| 19 | Minor | 原子写 + 进程内锁 | marketplace/mod.rs | `concurrent_scope_writes_do_not_lose_updates` | ✅ 编译过，CI 待跑 |
| 20 | Minor | scope 严格解析 | app/commands/connectors.rs | 3 个新测试 | ✅ 编译过，CI 待跑 |
| 21/24/25/26/27/23 | Nit | 见逐项说明 | 各文件 | 对应新测试 | ✅ |
| 28 | Nit | **不修**（留上游） | — | — | ⚪ 说明见上 |
| 29 | 验证缺口 | 本机复现 0xc0000139，编译全绿 | — | — | ⚠️ 以 CI `rust-test` 为准 |

## 四、验证记录

- `cargo check` / `cargo test --no-run`：全部通过，pinvou3-tauri **零新增警告**（仅 CodeWhale 既有 dead_code 警告）。
- 前端：`npm run test:composer-tools`、`npm run test:ui-language` 通过（三语 key 覆盖校验含新增 `composerSkillCodeDisabled`）。
- `python3 scripts/architecture-guard.py`：通过。
- `./scripts/fork-guard.sh --fast`：指纹层全过（本 PR 未改 fork 代码；T3 登记文字精确化，锚点未动）。
- Rust 测试二进制本机无法启动（0xc0000139，清单 #29 已知环境问题）：**所有新增/修改测试的实际运行以 CI `rust-test` 为验收门禁**。

## 五、残留风险（如实声明）

1. **#4/#5 侧路**（方案 D 不封口）：catalogue 仍印 skill 路径，`read_file` 可读 SKILL.md；根治由会话能力档案 PR 承接（设计已定稿，替换语义）。
2. **#11 RPC 面未封**：远程端凭 session id 可读写代码会话消息；封堵策略需审阅者确认。
3. **#13 行为变更**：注入顺序 root→cwd 改变所有绑项目代码会话的提示词内容（prefix-cache 一次性 miss），已与用户确认。
4. **存量在跑代码会话**：`load_skill` 禁用经热刷广播生效；若升级后无任何 toggle，已存活会话需 respawn 或下次广播后继承（启动路径未补主动刷新，与 disallowed_tools 现有语义一致）。
5. **#6 遗留**：`chat.rs` 附件引用等价性依赖 lib.rs 双侧注入同一闭包（已注释）。
