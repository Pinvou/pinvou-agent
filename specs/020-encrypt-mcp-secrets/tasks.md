# 任务：加密存储 MCP API 密钥

**输入**：`specs/020-encrypt-mcp-secrets/` 下的设计文档。

**前置条件**：`plan.md`、`spec.md`、`research.md`、`data-model.md`、`contracts/`、`quickstart.md`

**测试**：本 feature 涉及密钥迁移与安全边界，任务包含先行测试、静态扫描和 Windows smoke 验证。

**组织方式**：任务按用户故事分组，保证每个故事可以独立实现和验证。

## 格式：`[ID] [P?] [Story] 描述`

- **[P]**：可并行执行，前提是修改不同文件且没有依赖关系。
- **[Story]**：任务对应的用户故事，例如 `US1`、`US2`、`US3`。
- 描述中包含精确文件路径。

## Phase 1: 准备（共享基础）

**目的**：确认当前实现、敏感落点、测试入口和底座边界。

- [X] T001 阅读并核对 `specs/020-encrypt-mcp-secrets/plan.md`、`specs/020-encrypt-mcp-secrets/research.md`、`specs/020-encrypt-mcp-secrets/contracts/mcp-runtime-config.md` 中的安全取舍和底座边界
- [X] T002 检查当前 worktree 状态并记录本 feature 会触及的文件，避免覆盖用户修改：`pinvou3-app/resources/mcp-servers/`、`pinvou3-app/src-tauri/src/bridge/marketplace.rs`、`pinvou3-app/src-tauri/src/bridge/bundle.rs`、`pinvou3-app/src-tauri/src/credential_store.rs`
- [X] T003 [P] 用 `rg -n "AMAP_KEY|IWENCAI_API_KEY|QCC_API_KEY|Authorization|Bearer|sk-" pinvou3-app/resources/mcp-servers pinvou3-app/src-tauri/src/bridge` 建立当前明文落点基线，并把验证命令同步到 `specs/020-encrypt-mcp-secrets/quickstart.md`
- [X] T004 [P] 阅读 `DeepSeek-TUI/crates/tui/src/mcp.rs` 与 `DeepSeek-TUI/crates/tui/src/child_env.rs`，确认当前 `headers` 与 stdio `env` 不做 `${ENV}` 替换的限制并记录在 `docs/fork-modifications.md`

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：建立 MCP 密钥声明、引用、脱敏和测试辅助能力，后续故事均依赖本阶段。

**⚠️ CRITICAL**：本阶段完成前，不应开始任何用户故事实现。

- [X] T005 在 `pinvou3-app/src-tauri/src/credential_store.rs` 中新增 MCP 凭据引用构造方法，支持 `mcp:<tool_id>:<target>:<secret_name>` 账号命名并复用现有 `CredentialStore`
- [X] T006 在 `pinvou3-app/src-tauri/src/credential_store.rs` 中扩展脱敏规则，确保 `AMAP_KEY`、`IWENCAI_API_KEY`、`QCC_API_KEY`、`Authorization: Bearer ...` 的错误内容不会原样输出
- [X] T007 [P] 在 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 中为 `ToolManifest` 增加 `secret_env`、`secret_headers`、敏感 `ConfigField` 反序列化结构，保持旧 manifest 的 `env` 兼容读取
- [X] T008 [P] 在 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 中新增测试辅助函数，构造临时 `PINVOU3_HOME`、目标 manifest、`mcp.json` 和内存凭据存储
- [X] T009 在 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 中新增统一的 MCP 密钥解析服务，负责从 manifest 声明、用户配置、旧版明文和系统凭据生成非敏感运行配置
- [X] T010 在 `DeepSeek-TUI/crates/tui/src/mcp.rs` 中增加 MCP `headers` 与 stdio `env` 值的 `${ENV_NAME}` 展开支持，并保证未设置变量时返回可诊断错误而不是写入明文
- [X] T011 [P] 在 `DeepSeek-TUI/crates/tui/src/mcp.rs` 中新增单测覆盖 `headers.Authorization = "Bearer ${PINVOU3_MCP_SECRET_QCC_API_KEY}"` 和 `env.AMAP_KEY = "${PINVOU3_MCP_SECRET_AMAP_KEY}"` 的展开行为
- [X] T012 在 `docs/fork-modifications.md` 中记录 T010 的 DeepSeek-TUI fork 改动、通用性、风险和后续上游 PR 判断

**检查点**：MCP 凭据结构、脱敏、manifest 解析和底座安全展开能力可用，后续故事可以开始。

---

## Phase 3: 用户故事 1 - 内置 MCP 密钥不再明文落盘 (Priority: P1) 🎯 MVP

**目标**：新安装或新启动时，内置 manifest 和生成的 `mcp.json` 不再包含真实 MCP API 密钥。

**独立测试**：启动或安装 MCP 工具后，检查用户目录下 manifest 与 `mcp.json`，目标供应商真实密钥明文出现次数为 0。

### 测试 / 验证

- [X] T013 [P] [US1] 在 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 中新增单测：解析新格式 `weather`/`iwencai`/`qcc` manifest 时只得到敏感声明，不得到真实密钥值
- [X] T014 [P] [US1] 在 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 中新增单测：安装本地 stdio 工具时生成的 `mcp.json` 只包含 `${PINVOU3_MCP_SECRET_*}` 或非敏感引用，不包含真实 `AMAP_KEY`/`IWENCAI_API_KEY`
- [X] T015 [P] [US1] 在 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 中新增单测：安装企查查远程 server 时生成的 `headers.Authorization` 不包含真实 `QCC_API_KEY`

### 实现

- [X] T016 [US1] 更新 `pinvou3-app/resources/mcp-servers/weather/manifest.json`，删除 `env.AMAP_KEY` 明文并加入 `secret_env` 声明
- [X] T017 [US1] 更新 `pinvou3-app/resources/mcp-servers/iwencai/manifest.json`，删除 `env.IWENCAI_API_KEY` 明文并加入 `secret_env` 声明
- [X] T018 [US1] 更新 `pinvou3-app/resources/mcp-servers/qcc/manifest.json`，删除 `env.QCC_API_KEY` 明文并加入 `secret_headers` 声明
- [X] T019 [US1] 更新 `pinvou3-app/src-tauri/src/bridge/bundle.rs`，确保写入 `~/.pinvou3/bundle/mcp-servers/*/manifest.json` 的内容来自已脱敏 manifest
- [X] T020 [US1] 更新 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 的本地 stdio 工具安装逻辑，使 `weather` 和 `iwencai` 的 `mcp.json` 使用安全环境变量占位而非真实密钥
- [X] T021 [US1] 更新 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 的远程工具安装逻辑，使 `qcc-*` 的 `headers.Authorization` 使用安全环境变量占位而非真实 bearer
- [X] T022 [US1] 在 `pinvou3-app/src-tauri/src/bridge/mod.rs` 或 MCP pool 创建前的现有入口中，从系统凭据读取已配置 MCP 密钥并设置仅进程内使用的 `PINVOU3_MCP_SECRET_*` 环境变量

**检查点**：US1 可独立演示：新写入的 manifest 与 `mcp.json` 不再含真实密钥，已配置凭据仍可在运行期提供给 MCP。

---

## Phase 4: 用户故事 2 - 已有明文配置自动迁移 (Priority: P2)

**目标**：升级用户目录中已有明文 `manifest.json` 和 `mcp.json` 会迁移到系统凭据存储，并清理原文件中的明文。

**独立测试**：准备旧版明文用户目录，启动迁移后真实测试密钥不再出现在用户目录，对应凭据可读取，再次迁移幂等。

### 测试 / 验证

- [X] T023 [P] [US2] 在 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 中新增单测：旧版 `weather`/`iwencai` manifest 的 `env` 明文迁移到内存凭据存储并从文件 JSON 中清除
- [X] T024 [P] [US2] 在 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 中新增单测：旧版 `qcc` 远程 `mcp.json` 的 `Authorization: Bearer <secret>` 迁移为安全占位且只保存一个共享凭据
- [X] T025 [P] [US2] 在 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 中新增单测：已有有效凭据时迁移旧明文不会覆盖系统凭据，并会清理旧文件明文

### 实现

- [X] T026 [US2] 在 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 中实现 `migrate_mcp_plaintext_secrets`，扫描 `~/.pinvou3/bundle/mcp-servers/{weather,iwencai,qcc}/manifest.json`
- [X] T027 [US2] 在 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 中实现 `mcp.json` 迁移逻辑，识别 `AMAP_KEY`、`IWENCAI_API_KEY`、`QCC_API_KEY` 和目标 server 的 bearer header
- [X] T028 [US2] 在 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 中实现迁移结果结构，记录 migrated/skipped/failed 且所有消息经过脱敏
- [X] T029 [US2] 在 `pinvou3-app/src-tauri/src/bridge/bundle.rs` 的 `ensure_extracted` 流程中调用 MCP 明文迁移，确保应用启动时自动处理旧用户目录
- [X] T030 [US2] 在 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 的 `install` 流程前调用迁移，确保重新安装工具时也能清理旧 `mcp.json`
- [X] T031 [US2] 在 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 中确保迁移写回 JSON 使用 UTF-8 且不带 BOM，并在解析失败时不覆盖原文件

**检查点**：US1 与 US2 均可独立验证：旧配置迁移完成后用户目录不残留明文，已安装工具配置仍可继续使用。

---

## Phase 5: 用户故事 3 - 密钥缺失时给出可恢复反馈 (Priority: P3)

**目标**：密钥缺失、凭据不可用或迁移失败时，用户能知道哪个 MCP 工具需要重新配置，且错误不泄露密钥。

**独立测试**：删除某个 MCP 工具凭据后安装或启用该工具，应用返回包含工具名和字段名的可恢复错误，不包含密钥内容。

### 测试 / 验证

- [X] T032 [P] [US3] 在 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 中新增单测：缺失 `IWENCAI_API_KEY` 时安装返回包含 `iwencai` 和字段名的错误，且不包含任何测试密钥
- [X] T033 [P] [US3] 在 `pinvou3-app/src-tauri/src/credential_store.rs` 中新增单测：凭据存储失败消息会脱敏 `Bearer` 和目标供应商 key 形态
- [X] T034 [P] [US3] 在 `pinvou3-app/src/index.html` 或相关工具市场 UI 区域中验证安装失败提示使用明确文本而非只依赖颜色

### 实现

- [X] T035 [US3] 在 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 中实现 `McpSecretError` 到用户提示文案的转换，包含工具名、供应商和缺失字段名
- [X] T036 [US3] 在 `pinvou3-app/src-tauri/src/commands.rs` 的 `install_marketplace_tool` 错误返回路径中接入脱敏后的 MCP 密钥错误
- [X] T037 [US3] 在 `pinvou3-app/src/index.html` 或工具市场安装反馈逻辑中显示 MCP 密钥缺失/迁移失败的清晰文本提示
- [X] T038 [US3] 在 `pinvou3-app/src-tauri/src/bridge/marketplace.rs` 中确保凭据不可用时不回退写入 manifest 或 `mcp.json` 明文

**检查点**：所有计划内用户故事均可独立验证，失败路径可恢复且不泄密。

---

## Phase 6: 收尾与横切关注点

- [X] T039 [P] 更新 `specs/020-encrypt-mcp-secrets/quickstart.md`，补充最终采用的安全占位命名、运行命令和 Windows smoke 结果记录格式
- [X] T040 [P] 运行 `rg -n "真实测试密钥|sk-|AMAP_KEY\\\": \\\"|IWENCAI_API_KEY\\\": \\\"|QCC_API_KEY\\\": \\\"|Authorization\\\": \\\"Bearer " pinvou3-app/resources/mcp-servers pinvou3-app/src-tauri/src specs/020-encrypt-mcp-secrets` 并确认只剩非敏感示例或测试断言
- [X] T041 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml marketplace --lib` 并在 `specs/020-encrypt-mcp-secrets/quickstart.md` 记录结果
- [X] T042 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml credential_store --lib` 并在 `specs/020-encrypt-mcp-secrets/quickstart.md` 记录结果
- [X] T043 运行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml` 并在 `specs/020-encrypt-mcp-secrets/quickstart.md` 记录结果
- [X] T044 运行 DeepSeek-TUI 相关 MCP 单测或最小 `cargo test` 范围，并在 `docs/fork-modifications.md` 记录 fork 验证结果
- [ ] T045 在 Windows 上手动验证高德天气、同花顺问财、企查查的安装/启用/缺失凭据提示，并在 `specs/020-encrypt-mcp-secrets/quickstart.md` 记录结果

---

## 依赖与执行顺序

- Phase 1 无依赖。
- Phase 2 阻塞所有用户故事。
- US1 是 MVP，必须先完成新写入路径不落明文。
- US2 依赖 Phase 2，建议在 US1 后完成，确保旧配置迁移写回到新格式。
- US3 依赖 Phase 2，可在 US1/US2 后实现，也可与 US2 的错误处理并行推进。
- Phase 6 依赖所有用户故事完成。

## 并行机会

- T003 与 T004 可并行，因为分别是静态扫描和底座阅读。
- T007、T008 可与 T005、T006 分工并行，但 T009 依赖它们的结构。
- US1 中 T013、T014、T015 可并行写测试；T016、T017、T018 可并行改不同 manifest。
- US2 中 T023、T024、T025 可并行写迁移测试。
- US3 中 T032、T033、T034 可并行覆盖不同层面的错误反馈。
- Phase 6 中 T039、T040 可并行，T041-T045 建议按风险从单测到 smoke 顺序执行。

## 实施策略

1. 先完成 Phase 2 和 US1，交付“新安装/新启动不再写明文”的 MVP。
2. 再完成 US2，覆盖老用户目录和已安装工具的明文迁移。
3. 最后完成 US3，确保缺失凭据和迁移失败都可恢复、可理解、不泄密。
4. 所有用户故事完成后运行 quickstart 全量验证，尤其是静态扫描和 Windows smoke。
