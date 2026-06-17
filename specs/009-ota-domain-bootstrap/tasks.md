# 任务：Windows OTA 域名引导

**输入**：`specs/009-ota-domain-bootstrap/` 下的设计文档

**前置条件**：plan.md、spec.md、research.md、data-model.md、contracts/、quickstart.md

**测试**：本 feature 涉及 Windows OTA 后台地址、SN、签名、下载/反馈来源和用户可见检查结果，必须包含单元测试、静态检查和 Windows 手动 smoke。测试任务在对应实现任务前定义，便于 TDD 或先补回归用例。

**组织方式**：任务按用户故事分组，保证每个故事可以独立实现和验证。US1 是 MVP，交付域名引导后完成 OTA 查询/下载/反馈来源贯通；US2 交付配置文件规则；US3 交付 BIOS SN 身份规则。

## 格式：`[ID] [P?] [Story] 描述`

- **[P]**：可并行执行，前提是修改不同文件且没有依赖关系。
- **[Story]**：任务对应的用户故事，例如 US1、US2、US3。
- 描述中必须包含精确文件路径。
- 文档、任务描述和验收说明默认使用中文；英文仅保留必要命令、路径、API 字段或原文。

## 路径约定

- **Windows OTA 代码**：`pinvou3-app/src-tauri/src/os/windows/`
- **跨平台更新薄层**：`pinvou3-app/src-tauri/src/updater.rs`
- **共享路径根**：`pinvou3-app/src-tauri/src/bridge/paths.rs`（仅复用 `pinvou3_home()` 等通用根目录约定）
- **文档**：`specs/009-ota-domain-bootstrap/`
- **验证命令**：Rust 命令从仓库根目录执行，使用 `--manifest-path pinvou3-app/src-tauri/Cargo.toml`

## Phase 1: 准备（共享基础）

**目的**：确认上下文、目录、依赖和验证方式。

- [X] T001 阅读 `specs/009-ota-domain-bootstrap/plan.md` 并确认宪章检查结果仍为 PASS
- [X] T002 阅读 `specs/009-ota-domain-bootstrap/contracts/domain-bootstrap-contract.md` 和 `specs/009-ota-domain-bootstrap/contracts/windows-ota-flow-contract.md`，确认 C# 参考契约、`smarthubOta` key 和反馈同源要求
- [X] T003 在仓库根目录 `E:\Pinvou\pinvou3` 检查当前 worktree 状态并记录与本 feature 无关的改动，命令：`git status --short --branch`
- [X] T004 [P] 阅读 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 中 `OtaConfig`、`UpdateFeedbackRecord`、`check_for_update`、`report_pending_update_result` 的现有实现边界
- [X] T005 [P] 阅读 `pinvou3-app/src-tauri/src/bridge/paths.rs` 的 `PINVOU3_HOME`、`pinvou3_home()` 和 `updates_dir()` 路径约定

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：完成所有用户故事共同依赖的代码骨架、路径入口和编译接线。

**CRITICAL**：本阶段完成前，不应开始任何用户故事实现。

- [X] T006 在 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs` 中创建 Windows 域名引导模块骨架，包含默认地址、`/v2/bootstrap` 路径、`smarthubOta` key、product id、secret、sign type 和固定 SN 常量
- [X] T007 在 `pinvou3-app/src-tauri/src/os/windows/mod.rs` 中声明并接线 `windows_domain_bootstrap` 模块，保持导出范围只服务 Windows OTA 内部调用
- [X] T008 在 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs` 中新增 Windows 专属 bootstrap 配置路径函数，返回 `paths::pinvou3_home().join("windows-ota-bootstrap.json")`
- [X] T009 [P] 在 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs` 的测试模块中补充 bootstrap 配置路径尊重 `PINVOU3_HOME` 的路径测试
- [X] T010 针对 `pinvou3-app/src-tauri/Cargo.toml` 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_domain_bootstrap --lib`，确认新增路径测试通过

**检查点**：基础模块和配置路径存在，后续故事可以在 Windows 专用文件内实现。

---

## Phase 3: 用户故事 1 - OTA 检查使用域名引导地址 (Priority: P1) MVP

**目标**：Windows OTA 查询前先请求域名引导，使用返回的 `smarthubOta` 作为本次查询、下载信息和升级反馈的 OTA 来源。

**独立测试**：使用 mock 域名引导服务返回 `smarthubOta=http://127.0.0.1:8787`，触发 Windows `check_for_update` 后，验证 OTA 查询请求发往该地址；有更新时下载信息和反馈记录也保留同一 `ota_host`。

### 测试 / 验证（如适用）

- [X] T011 [P] [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs` 中添加域名引导响应解析测试，覆盖 `message/msg`、`data.smarthubOta`、大小写不敏感 key、缺少 `data`、缺少 `smarthubOta` 和非法 `smarthubOta` URL
- [X] T012 [P] [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs` 中添加签名测试，使用固定 SN 和固定 timestamp 验证 MD5 小写结果与 C# 拼接规则一致
- [X] T013 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 中添加 Windows OTA host 贯通测试，验证 `check`、`getDownloadInfo` 和 `updateLog` endpoint 均基于同一个 `ota_host`
- [X] T014 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 中添加 `UpdateFeedbackRecord` 序列化兼容测试，验证新增 `ota_host` 可写入，旧记录缺少 `ota_host` 仍可反序列化

### 实现

- [X] T015 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs` 中实现域名引导请求结构、签名生成、HTTP POST、响应解析和 `smarthubOta` URL 规范化
- [X] T016 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 中将 `OtaConfig::from_env` 改为异步解析域名引导结果，生产路径不再默认使用 `DEFAULT_OTA_HOST`
- [X] T017 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 中让 `check_for_update`、下载信息查询和 `report_pending_update_result` 使用解析出的 `ota_host` 拼接 endpoint
- [X] T018 [US1] 在 `pinvou3-app/src-tauri/src/updater.rs` 和 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 中为 `UpdateInfo`/`WindowsUpdateInfo` 增加 `ota_host` 默认字段，并保持非 Windows 序列化兼容
- [X] T019 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 中扩展 `UpdateFeedbackRecord` 和 `write_install_started_record`，启动 MSI 前写入本次 `ota_host`
- [X] T020 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 中实现旧反馈记录缺少 `ota_host` 时的兼容路径：重新执行域名引导或返回可重试友好错误
- [X] T021 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 中将域名引导失败、缺少 `smarthubOta` 或 `smarthubOta` 非法时映射为友好的检查失败文案，避免暴露完整 SN
- [X] T022 [US1] 针对 `pinvou3-app/src-tauri/Cargo.toml` 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_update --lib`，确认 Windows OTA 单测通过

**检查点**：US1 可独立演示和验证；Windows 检查更新已经使用 `smarthubOta` 返回地址完成 OTA 查询。

---

## Phase 4: 用户故事 2 - 用户可配置域名引导后台地址 (Priority: P2)

**目标**：部署或测试人员可以通过 `~/.pinvou3/windows-ota-bootstrap.json` 切换域名引导后台；配置缺失、为空、缺字段或格式非法时回退默认地址。

**独立测试**：使用 `PINVOU3_HOME` 指向临时目录，分别模拟配置文件不存在、空文件、缺少 `bootstrapHost`、非法 URL、合法自定义 URL，验证最终请求的 bootstrap host 符合规则且不覆盖用户文件。

### 测试 / 验证（如适用）

- [X] T023 [P] [US2] 在 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs` 中添加配置读取测试，覆盖缺失、空文件、JSON 非法、缺少 `bootstrapHost`、非法 URL 回退默认地址
- [X] T024 [P] [US2] 在 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs` 中添加合法自定义 `bootstrapHost` 测试，验证末尾 `/` 被移除且优先于默认地址
- [X] T025 [US2] 在 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs` 中添加“不主动创建或覆盖 `windows-ota-bootstrap.json`”的测试，路径使用 `PINVOU3_HOME` 临时目录

### 实现

- [X] T026 [US2] 在 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs` 中实现 `WindowsOtaBootstrapConfig` 读取、默认值回退、URL 校验和末尾 `/` 规范化
- [X] T027 [US2] 在 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs` 中确保读取配置时不自动写入、不覆盖 `~/.pinvou3/windows-ota-bootstrap.json`
- [X] T028 [US2] 在 `specs/009-ota-domain-bootstrap/quickstart.md` 中补充配置文件联调步骤的实际路径、非法配置回退默认地址和不覆盖用户文件验收点
- [X] T029 [US2] 针对 `pinvou3-app/src-tauri/Cargo.toml` 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_domain_bootstrap --lib`，确认配置读取相关单测通过

**检查点**：US2 可独立验证；无需重新打包即可通过配置文件切换域名引导后台。

---

## Phase 5: 用户故事 3 - 根据 BIOS SN 确定域名引导请求身份 (Priority: P3)

**目标**：域名引导请求使用 BIOS SN；当 BIOS SN 不以 `2198`/`2199` 开头或不可读取时，使用固定 SN `219904A17T4257W00018`。

**独立测试**：纯函数测试覆盖 `2198`、`2199`、其他前缀、空白、空值和读取失败；Windows 手动 smoke 验证真实设备请求体中的 `device_id` 符合规则。

### 测试 / 验证（如适用）

- [X] T030 [P] [US3] 在 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs` 中添加有效 SN 选择测试，覆盖 `2198`、`2199`、其他前缀、前后空白、空字符串和读取失败
- [X] T031 [P] [US3] 在 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs` 中添加请求体测试，验证 `device_id` 使用有效 SN 且普通错误信息不包含完整 SN
- [X] T032 [US3] 在 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs` 中实现 BIOS SN 读取接口的可替换测试入口，避免单测依赖真实硬件

### 实现

- [X] T033 [US3] 在 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs` 中实现 `WindowsBootstrapIdentity`、SN trim、前缀判断和固定 SN 兜底
- [X] T034 [US3] 在 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs` 或 `pinvou3-app/src-tauri/src/os/windows/windows_system.rs` 中实现 Windows BIOS SN 读取，并设置超时或轻量路径以避免更新面板卡顿
- [X] T035 [US3] 在 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs` 中将 BIOS SN 读取结果接入域名引导请求体生成
- [X] T036 [US3] 针对 `pinvou3-app/src-tauri/Cargo.toml` 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_domain_bootstrap --lib`，确认 SN 选择和请求体测试通过

**检查点**：所有计划内用户故事均可独立验证；域名引导请求身份符合产品规则。

---

## Phase 6: 收尾与横切关注点

- [X] T037 [P] 更新 `specs/009-ota-domain-bootstrap/quickstart.md` 的“本轮验证结果”，记录已执行的 cargo 测试、cargo check 和未执行的手动 smoke 原因
- [X] T038 [P] 检查 `pinvou3-app/src/tauri-bridge.js` 和 `pinvou3-app/src/index.html` 是否需要承接新增 `ota_host` 字段或错误文案；若无需修改，在 `specs/009-ota-domain-bootstrap/quickstart.md` 记录结论
- [X] T039 针对 `pinvou3-app/src-tauri/Cargo.toml` 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml updater --lib`，确认跨平台更新命令兼容
- [X] T040 针对 `pinvou3-app/src-tauri/Cargo.toml` 运行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`，确认 Windows 域名引导接线不破坏编译
- [ ] T041 在 Windows 环境按 `specs/009-ota-domain-bootstrap/quickstart.md` 执行手动 smoke，验证配置文件、mock 域名引导、OTA 查询、下载准备、MSI 启动前反馈记录和升级后反馈路径
- [X] T042 检查 `DeepSeek-TUI/` 未被修改，命令：`git status --short DeepSeek-TUI`
- [X] T043 在仓库根目录 `E:\Pinvou\pinvou3` 检查 `git diff --check`，确认没有尾随空白或格式错误

## 依赖与执行顺序

- Phase 1 无代码依赖，先完成上下文确认。
- Phase 2 阻塞所有用户故事，尤其是 `windows_domain_bootstrap.rs`、模块接线和配置路径。
- US1 是 MVP，依赖 Phase 2；完成后即可验证域名引导到 OTA 查询的核心价值。
- US2 依赖 Phase 2，可在 US1 的响应解析与 HTTP 请求实现稳定后并行推进配置读取细节。
- US3 依赖 Phase 2，可与 US2 并行，但接入真实域名引导请求体前需与 US1 的请求结构合并。
- Phase 6 依赖 US1、US2、US3 完成后执行。

## 并行机会

- T004 与 T005 可并行阅读不同文件。
- T009 可与 T006/T007 并行，但 T010 依赖 T008/T009。
- T011 与 T012 可并行，因为分别覆盖响应解析和签名。
- T023、T024 可并行补配置测试；T025 依赖配置路径确认但可与实现前置测试并行。
- T030 与 T031 可并行补 SN 选择和请求体测试。
- T037 与 T038 可并行更新/检查文档和前端承接结论。

## 实施策略

1. 先完成 Phase 1 和 Phase 2，保证 Windows 专用模块和路径入口稳定。
2. 先交付 US1 作为 MVP：mock 域名引导返回 `smarthubOta` 后，Windows OTA 查询、下载信息和反馈目标都使用该地址。
3. 再交付 US2：用配置文件控制 bootstrap host，并确保非法配置回退默认地址。
4. 最后交付 US3：接入真实 BIOS SN 读取和固定 SN 兜底，完成请求身份规则。
5. 每完成一个用户故事即运行对应单测，不把所有验证堆到最后。
6. 收尾阶段必须执行 cargo check、updater 回归、Windows smoke 或明确记录未执行原因。
