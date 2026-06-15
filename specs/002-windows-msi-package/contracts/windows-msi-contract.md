# 契约：Windows MSI 构建与验收

## 目标

本契约定义实现阶段必须满足的 Windows MSI 构建、产物、安装验证和最小变更审查要求。它不是对外 API，而是本 feature 的交付契约。

## 构建前置契约

实现阶段 MUST 在正式构建前确认：

- 当前分支和 feature 目录为 `002-windows-msi-package` / `specs/002-windows-msi-package`。
- `DeepSeek-TUI/` submodule 已初始化，Cargo path dependency 可解析。
- Windows 构建机具备 Rust、Node/npm、Tauri CLI 或可通过项目依赖调用 Tauri CLI。
- Windows WebView2 可用。
- MSI 打包所需链路可用；若 VBSCRIPT 可选功能缺失，必须记录并给出启用建议。
- 当前工作树中与本 feature 无关的用户改动不得被回滚或格式化。

## 构建命令契约

实现阶段 MUST 提供一个可复现的 Windows 构建入口，满足以下任一形式：

- 在 `pinvou3-app/` 下执行项目脚本生成 MSI。
- 在 `pinvou3-app/` 下执行 Tauri CLI 并显式指定 MSI 目标。
- 使用 Windows 专用 Tauri 配置覆盖文件生成 MSI，同时保持 Linux `.deb` 配置不被破坏。

构建入口 MUST 记录：

- 执行目录。
- 执行命令。
- 关键环境变量。
- 产物输出目录。
- 成功或失败结果。

## MSI 产物契约

成功构建时 MUST 满足：

- 输出至少一个 `.msi` 文件。
- MSI 文件存在且大小大于 0。
- 产物版本可追溯到 `pinvou3-app/package.json` 和 `pinvou3-app/src-tauri/Cargo.toml` 的当前版本。
- 产物标识可追溯到 `tauri.conf.json` 的 `productName` 和 `identifier`。

若未能生成 MSI，MUST 记录：

- 失败命令。
- 失败日志摘要。
- 缺失前置条件或失败模块。
- 下一步补齐路径。

## 安装验收契约

MSI 生成后 MUST 至少执行或记录以下人工 smoke：

| 检查项 | 必填结果 | 通过标准 |
|---|---|---|
| 安装 | pass/fail | MSI 在 Windows 上完成安装 |
| 启动 | pass/fail | 用户能启动 pinvou3 主窗口 |
| 配置入口 | pass/fail | 用户能进入模型服务配置或相关设置入口 |
| 卸载 | pass/fail | Windows 常规卸载入口可用 |
| 用户数据保留 | pass/fail/未执行原因 | 卸载或重复安装不会默认删除用户价值数据，或明确记录尚未验证 |

模型服务不可用不应自动判定 MSI 失败；验收应区分“应用安装/启动失败”和“模型 endpoint 未配置或不可达”。

## 最小变更契约

实现阶段 MUST 维护最小变更说明，至少包含：

- 修改文件列表。
- 每个修改的必要性。
- 是否触碰业务/agent 行为。
- 未纳入本 feature 的 Windows 迁移项。
- 验证命令或人工验收项。

以下行为 MUST NOT 发生：

- 为 MSI 打包重写 DeepSeek-TUI 底座能力。
- 将 Linux `.deb` updater、`apt` 或 `pkexec` 描述为 Windows 可用能力。
- 默认删除用户 session、settings、artifact、workflow run 或自定义 skill/persona。
- 把代码签名、企业分发、Windows 原生自动更新伪装成本 feature 已完成内容。
