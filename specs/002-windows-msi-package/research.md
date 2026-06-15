# 研究记录：Windows MSI 安装包构建

## 决策 1：MSI 产物必须在 Windows 构建环境生成

**决策**：本 feature 以 Windows 构建机作为 MSI 产物生成环境，不规划从当前非 Windows 环境直接交叉生成 MSI。

**理由**：Tauri 官方 Windows installer 文档说明，Tauri Windows 应用可以分发为 MSI 或 NSIS；其中 MSI 使用 WiX Toolset v3，MSI 只能在 Windows 上创建，因为 WiX 只能在 Windows 上运行。官方构建入口是在 Windows 机器上运行 Tauri CLI 的 build 命令。

**备选方案**：

- 在 Linux/macOS 上交叉生成 MSI：不采用。与 Tauri 官方限制冲突，容易把实现阶段拖入不可控的交叉编译和安装器链路。
- 改用 NSIS：暂不采用。用户明确要求 MSI；NSIS 可作为后续 Windows installer 备选方案。

**来源**：

- Tauri 官方文档：<https://v2.tauri.app/distribute/windows-installer/>

## 决策 2：优先使用 Tauri 2 现有打包机制

**决策**：实现阶段优先通过 Tauri 2 现有 bundle 配置或构建命令生成 MSI，不引入额外安装器框架，不手写 WiX 模板，除非默认模板无法满足最小安装需求。

**理由**：当前项目已经是 Tauri 2 桌面应用，`tauri.conf.json` 中已有 bundle 配置、产品名、版本、identifier 和 Windows 图标 `icons/icon.ico`。最小变更路径是复用现有 Tauri CLI 和官方 Windows installer 能力。

**备选方案**：

- 手写 WiX `.wxs` 模板：暂不采用。会增加维护面，不符合“尽量不改变当前项目代码”。
- 引入第三方安装器生成工具：暂不采用。项目已有 Tauri bundler，新增工具链会扩大风险。

## 决策 3：Linux `.deb` updater 和 `apt/pkexec` 不纳入本 feature

**决策**：Windows MSI 打包阶段只保证安装包生成、安装、启动和卸载基础验收；Linux `.deb` 应用内升级、`apt` 依赖安装、`pkexec` 提权安装不在本 feature 范围内。

**理由**：现有 `updater.rs`、`file_ingest.rs` 的依赖安装和升级链路明显偏 Linux。强行迁移到 Windows 会引入 Windows updater、UAC、代码签名、installer 权限和依赖安装策略设计，超出“先生成 MSI 且尽量少改代码”的目标。

**备选方案**：

- 同时实现 Windows 原生 updater：不采用。需要独立规格和安全设计。
- 在 MSI 中自动安装所有附件解析依赖：不采用。Poppler、Tesseract、LibreOffice、7z 等依赖体积和授权/路径问题复杂，应后续单独规划。

## 决策 4：以安装 smoke 和数据保留作为验收核心

**决策**：计划要求实现阶段记录 MSI 产物路径、安装结果、启动结果、配置入口可达性、卸载行为和用户数据保留预期。

**理由**：Windows 打包不是单纯编译成功。对桌面应用而言，安装入口、启动主窗口、用户数据目录和卸载行为更接近用户真实风险。项目宪章也要求高风险打包迁移必须有可验证交付。

**备选方案**：

- 只检查 `.msi` 文件存在：不采用。无法证明安装包可用。
- 立即覆盖所有附件、workflow、vLLM 聊天端到端场景：暂不采用。应在 MSI 基础可用后逐步扩展 Windows 原生 smoke。

## 决策 5：最小变更清单是交付物之一

**决策**：后续任务必须生成或维护最小变更清单，说明实际修改文件、修改原因、未触碰区域和遗留风险。

**理由**：用户明确要求尽量不改变当前项目代码。通过最小变更清单可以把“少改”从口头偏好变成可审查契约，防止实现阶段顺手迁移过多无关能力。

**备选方案**：

- 仅依赖 git diff：不充分。diff 能展示改动，但不能说明为什么改、为什么没改，以及哪些能力被刻意排除。
