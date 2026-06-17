# 任务：Windows 软件更新

**输入**：`specs/007-windows-software-update/` 下的设计文档

**前置条件**：`plan.md`、`spec.md`、`research.md`、`data-model.md`、`contracts/`、`quickstart.md`

**测试**：本 feature 涉及 Windows 更新、安装器启动、网络反馈和包解压安全，任务包含与风险匹配的单元测试、契约测试、前端 smoke 和手动验收。

**组织方式**：任务按用户故事分组。Phase 1/2 完成后，US1 是 MVP；US2、US3 可在 US1 的共享模型和命令契约稳定后推进。

## 格式：`[ID] [P?] [Story] 描述`

- **[P]**：可并行执行，前提是修改不同文件且没有依赖关系。
- **[Story]**：任务对应的用户故事，例如 US1、US2、US3。
- 描述中包含精确文件路径或验证命令。

## Phase 1: 准备（共享基础）

**目的**：确认上下文、现有更新链路、参考协议和验证环境，避免误改 Linux 更新或 DeepSeek-TUI 底座。

- [X] T001 阅读 `specs/007-windows-software-update/plan.md`、`specs/007-windows-software-update/research.md`、`specs/007-windows-software-update/contracts/ota-service-contract.md` 并确认 H3C OTA 接口、样例包结构和宪章边界
- [X] T002 检查 `git status --short` 并阅读 `pinvou3-app/src-tauri/src/updater.rs`、`pinvou3-app/src-tauri/src/os/windows/windows_update.rs`、`pinvou3-app/src/tauri-bridge.js` 的现有更新链路，确认不覆盖用户改动
- [X] T003 [P] 准备本地验证资料并记录样例包路径 `C:\Users\z27014\Downloads\Megabook2_BIOS_12.0.0.0\Pinvou3_0.4.4.0.zip` 在 `specs/007-windows-software-update/quickstart.md`
- [X] T004 [P] 确认 C# 参考文件 `D:\0_Projects\3_Components\H3C.Updater\H3C.Updater\Services\UpdateAPI\UpdateHttpService.cs`、`D:\0_Projects\3_Components\H3C.Updater\H3C.Updater\Services\UpdateAPI\Request\CheckUpdateVersionRequest.cs`、`D:\0_Projects\3_Components\H3C.Updater\H3C.Updater\Services\UpdateAPI\Request\UpdateOtaLogRequest.cs` 的字段语义

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：建立 Windows OTA 所需的共享依赖、数据结构、路径和跨平台边界。

**⚠️ CRITICAL**：本阶段完成前，不应开始任何用户故事实现。

- [X] T005 在 `pinvou3-app/src-tauri/Cargo.toml` 中新增 Windows 更新包解析所需 zip 依赖，并保持非 Windows 构建兼容
- [X] T006 在 `pinvou3-app/src-tauri/src/bridge/paths.rs` 中新增待反馈记录路径函数，例如 `update_feedback_record_path()`，并确保路径落在 `~/.pinvou3/updates/`
- [X] T007 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 中定义 Windows OTA 共享数据结构，包括更新查询响应、下载信息、包解析结果、安装器信息和反馈记录
- [X] T008 在 `pinvou3-app/src-tauri/src/updater.rs` 中保留跨平台 Tauri 命令 DTO 和编排逻辑，调用 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 中的 Windows OTA 数据结构与 helper
- [X] T009 验证 `pinvou3-app/src-tauri/src/os/interface/update.rs` 保持 Linux `.deb` 薄接口不变，Windows `MSI` 安装入口收敛在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs`
- [X] T010 验证 `pinvou3-app/src-tauri/src/os/linux/linux_update.rs` 无需适配，现有 `.deb` 路径校验和安装行为不变
- [X] T011 [P] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 中建立 Windows-only helper 骨架，包含路径校验、安装器启动、包解析入口和反馈状态入口的函数签名

**检查点**：共享结构、路径和平台接口稳定，可以开始按用户故事实施。

---

## Phase 3: 用户故事 1 - 查询并发现可用更新 (Priority: P1) MVP

**目标**：Windows 用户可以检查当前版本是否有可用更新，并获得目标版本、下载地址、包校验和更新说明。

**独立测试**：使用 mock H3C OTA 响应分别返回“有更新”和“无更新”，调用 `check_for_update` 验证结果，不触发下载或安装。

### 测试 / 验证

- [X] T012 [P] [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 添加 H3C `CheckUpdateVersionResponse` 解析测试，覆盖 `success=true && code=200`、无更新、失败响应和字段缺失
- [X] T013 [P] [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 添加四段版本号比较测试，覆盖 `0.4.4.0 > 0.4.3`、相等、不合法版本和降级保护

### 实现

- [X] T014 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 实现 Windows H3C OTA host 配置读取，支持必填 host 环境变量和 SN/softwareId 默认值
- [X] T015 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 实现 `POST /ota/pkg/package/upgrade/check` 请求体序列化，字段包含 `sn`、`softwareId`、`version`、`hardwareInfo`
- [X] T016 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 实现查询响应到 `UpdateInfo` 的转换，映射 `updateInfo`、`updateType`、`updateVersion`、`pkgMd5` 和下载信息字段
- [X] T017 [US1] 在 `pinvou3-app/src-tauri/src/updater.rs` 实现 Windows `check_for_update` 分支薄编排，调用 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 并在查询失败或无更新时返回可理解状态且不进入下载
- [X] T018 [US1] 在 `pinvou3-app/src/tauri-bridge.js` 中适配 Windows `UpdateInfo` 字段，确保 `updateInfo.available`、`latest_version`、`notes` 和错误文案在设置页可用
- [X] T019 [US1] 在 `pinvou3-app/src/index.html` 中检查“版本与更新”显示逻辑，确保 Windows 查询结果不会展示 Linux `.deb` 专属文案

**检查点**：US1 可独立演示和验证；用户能检查更新并看到有更新、无更新或查询失败。

---

## Phase 4: 用户故事 2 - 下载并解析更新包 (Priority: P2)

**目标**：Windows 用户可以下载完整 zip 更新包，系统能按清单解压并定位正确的 `MSI` 安装文件。

**独立测试**：使用样例 zip 或测试 fixture 调用下载后的解析入口，验证能读取 `OtaInfo.json`，并定位 `Files/Pinvou3/*.msi`；异常包必须拒绝安装。

### 测试 / 验证

- [X] T020 [P] [US2] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 添加下载 zip 解析测试，覆盖 `OtaInfo.json` 缺失场景
- [X] T021 [P] [US2] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 添加 `OtaInfo.json` 解析测试，覆盖 `OtaInfo.json` 与 `FullPack/OtaInfo.json` 两种路径
- [X] T022 [P] [US2] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 添加 `MSI` 定位测试，覆盖 `softwareId=Pinvou3_Win`、`softwareType=Pinvou3`、`fileMetaInfos.filePath` 和 `attachData.exeName` fallback
- [X] T023 [P] [US2] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 添加安全测试，覆盖 zip 路径穿越、绝对路径、非 `.msi` 文件、清单指向不存在文件和 hash 不匹配

### 实现

- [X] T024 [US2] 在 `pinvou3-app/src-tauri/src/updater.rs` 扩展 `download_update` 的 Windows 分支，下载 zip 到 `~/.pinvou3/updates/` 并按 MD5 校验下载包
- [X] T025 [US2] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 实现安全解压下载 zip 到版本隔离目录，拒绝路径穿越和非受控路径
- [X] T026 [US2] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 实现读取下载 zip 中的 `OtaInfo.json`
- [X] T027 [US2] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 兼容读取 `OtaInfo.json` 与 `FullPack/OtaInfo.json`
- [X] T028 [US2] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 实现根据 `OtaInfo.json` 定位 `MSI` 文件并校验扩展名、受控路径和 hash
- [X] T029 [US2] 在 `pinvou3-app/src-tauri/src/updater.rs` 返回 Windows 下载解析结果，包含 `package_path`、`installer_path` 和 `latest_version`
- [X] T030 [US2] 在 `pinvou3-app/src/tauri-bridge.js` 中适配 `download_update` 的 Windows 返回结构，将 `installer_path` 传入安装阶段

**检查点**：US2 可独立验证；给定有效更新包能定位 `MSI`，异常包不会进入安装。

---

## Phase 5: 用户故事 3 - 启动安装并反馈结果 (Priority: P3)

**目标**：Windows 安装器成功启动后当前 pinvou 进程退出；升级后再次运行时反馈升级结果，失败可重试。

**独立测试**：用有效 `MSI` 路径触发安装启动 mock 或手动验收，确认待反馈记录写入；重启后调用反馈命令，mock 服务收到 `/ota/pkg/package/updateLog`。

### 测试 / 验证

- [X] T031 [P] [US3] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 添加安装路径校验测试，覆盖更新目录内 `.msi`、目录外文件、非 `.msi` 和不存在文件
- [X] T032 [P] [US3] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 添加 `UpdateFeedbackRecord` 序列化和状态转换测试，覆盖 `START_INSTALL`、`UPGRADE_SUCCEED`、`UPGRADE_FAILED`、反馈失败重试
- [X] T033 [P] [US3] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 添加 H3C `/ota/pkg/package/updateLog` 请求序列化测试，覆盖 `softwareIdentification`、`sn`、`currentVersion`、`updateVersion`、`updateErrorInfo`、`updateResult`

### 实现

- [X] T034 [US3] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 实现 `MSI` 安装器启动，使用受控路径校验后调用 Windows 系统安装器
- [X] T035 [US3] 在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 实现安装器启动前后的待反馈记录写入，记录软件标识、SN、当前版本、目标版本和 `START_INSTALL`
- [X] T036 [US3] 在 `pinvou3-app/src-tauri/src/updater.rs` 实现 Windows 安装器成功启动后的当前 pinvou 进程退出流程
- [X] T037 [US3] 在 `pinvou3-app/src-tauri/src/updater.rs` 实现 `report_pending_update_result` Tauri 命令薄编排，调用 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs` 读取待反馈记录并上报 H3C `/ota/pkg/package/updateLog`
- [X] T038 [US3] 在 `pinvou3-app/src-tauri/src/lib.rs` 注册 `updater::report_pending_update_result` 命令
- [X] T039 [US3] 在 `pinvou3-app/src/tauri-bridge.js` 中应用启动或更新面板初始化时静默调用 `report_pending_update_result`，失败时不阻塞主界面
- [X] T040 [US3] 在 `pinvou3-app/src/index.html` 中调整 Windows 安装启动后的用户提示，避免继续显示 Linux “升级完成，重启后生效”语义

**检查点**：US3 可独立验证；安装器启动、进程退出和升级结果反馈形成闭环。

---

## Phase 6: 收尾与横切关注点

- [X] T041 [P] 更新 `specs/007-windows-software-update/quickstart.md`，补充实际实现后的环境变量、mock 服务启动方式和 Windows 安装器手动验收注意事项
- [X] T042 [P] 更新 `pinvou3-app/INSTALL.md`，补充 Windows 软件更新依赖、暂存目录和升级反馈行为说明
- [X] T043 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml updater --lib` 并记录结果到 `specs/007-windows-software-update/quickstart.md`
- [X] T044 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_update --lib` 并记录结果到 `specs/007-windows-software-update/quickstart.md`
- [X] T045 运行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml` 并确认 Linux `.deb` 更新链路没有被 Windows 改动破坏
- [ ] T046 在 Windows 上执行前端 smoke：`cd pinvou3-app && npm run dev`，验证检查更新、下载进度、包解析、安装器启动提示和反馈重试
- [X] T047 检查 `DeepSeek-TUI/` 没有本 feature 改动，并在 `git status --short` 中确认变更集中于 `pinvou3-app`、Spec Kit feature 文件和 feature 指针

## 依赖与执行顺序

- Phase 1 无依赖。
- Phase 2 阻塞所有用户故事。
- US1 依赖 Phase 2，作为 MVP 优先完成。
- US2 依赖 Phase 2 和 US1 的 `UpdateInfo`/下载字段契约稳定。
- US3 依赖 Phase 2 和 US2 的 `installer_path` 产物。
- Phase 6 在 US1-US3 完成后执行。

## 并行机会

- T003 与 T004 可并行准备验证资料和 C# 协议核对。
- T011 可在 T005-T010 明确接口后与部分文档/测试准备并行。
- US1 中 T012 与 T013 可并行编写，随后 T014-T019 顺序实现查询链路。
- US2 中 T020-T023 可并行编写不同解析/安全测试，T024-T030 按下载到解析到前端适配顺序执行。
- US3 中 T031-T033 可并行编写，T034-T040 按安装启动、持久化、反馈命令、前端调用顺序执行。
- T041 与 T042 可并行更新文档；T043-T046 建议按后端测试、静态检查、前端 smoke 顺序执行。

## 实施策略

1. 先完成 Phase 1/2，锁定共享模型、路径和平台接口。
2. 交付 US1 作为 MVP：Windows 能查询 H3C OTA 更新并正确显示有更新/无更新/失败。
3. 交付 US2：在不启动安装器的情况下完成下载、解压、清单解析和 `MSI` 定位。
4. 交付 US3：启动安装器、退出当前进程、升级后反馈结果。
5. 每个用户故事完成后立即运行对应测试，不把风险集中到最后。
