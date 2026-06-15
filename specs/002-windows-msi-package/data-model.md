# 数据模型：Windows MSI 安装包构建

## 实体：WindowsMsiArtifact

**含义**：本 feature 生成的 Windows MSI 安装包。

**字段**：

- `artifact_path`：MSI 产物路径，预期位于 Tauri release bundle 输出目录。
- `product_name`：安装包展示的产品名，应与当前应用配置一致。
- `version`：安装包版本，应能追溯到项目当前版本。
- `identifier`：应用标识，应与当前桌面应用配置一致。
- `architecture`：目标架构，优先 x64。
- `generated_at`：产物生成时间。
- `build_status`：`not_started`、`success`、`failed`。
- `failure_reason`：构建失败时的明确原因。

**验证规则**：

- `artifact_path` 必须以 `.msi` 结尾。
- `build_status=success` 时，`artifact_path` 必须存在且文件大小大于 0。
- `version` 必须与当前应用版本一致或在变更记录中说明差异。

## 实体：WindowsBuildEnvironment

**含义**：用于生成 MSI 的 Windows 构建环境和前置条件集合。

**字段**：

- `os_name`：操作系统名称。
- `os_version`：Windows 版本。
- `rust_version`：Rust 工具链版本。
- `node_version`：Node.js 版本。
- `npm_version`：npm 版本。
- `tauri_cli_version`：Tauri CLI 版本。
- `webview2_status`：WebView2 可用性。
- `wix_status`：MSI/WiX 打包链路可用性。
- `vbscript_status`：Windows VBSCRIPT 可选功能状态。
- `submodule_status`：`DeepSeek-TUI/` 是否初始化并可被 Cargo path dependency 解析。

**验证规则**：

- `submodule_status` 必须为可用，否则不得进入正式构建。
- `vbscript_status` 缺失时必须记录阻塞或修复建议。
- 任何关键依赖缺失时，构建流程必须输出可定位的失败原因。

## 实体：InstallationValidationRecord

**含义**：安装包生成后在 Windows 机器上的验收记录。

**字段**：

- `msi_path`：被验证的 MSI 文件路径。
- `install_result`：安装是否成功。
- `launch_result`：主窗口是否可启动。
- `config_entry_result`：模型服务配置入口是否可达。
- `uninstall_result`：卸载是否成功。
- `user_data_retention_result`：用户数据保留预期是否被确认。
- `notes`：限制、异常和补充说明。

**验证规则**：

- `install_result`、`launch_result`、`uninstall_result` 必须有明确 pass/fail。
- `config_entry_result` 可以在模型服务不可用时仍为 pass，只要配置入口可达且失败提示可理解。
- `user_data_retention_result` 必须说明检查路径或人工判断依据。

## 实体：MinimalChangeRecord

**含义**：用于证明本 feature 遵守“尽量不改变当前项目代码”的变更记录。

**字段**：

- `changed_file`：被修改或新增的文件路径。
- `change_type`：`config`、`script`、`docs`、`validation`、`code`。
- `reason`：为什么需要该变更。
- `risk`：变更可能影响的行为。
- `verification`：对应验证方式。
- `out_of_scope_note`：如某能力刻意不改，说明原因。

**验证规则**：

- 任何 `change_type=code` 的记录都必须说明为什么配置或文档无法解决。
- 不允许出现 DeepSeek-TUI 底座重写类变更。
- 变更清单必须覆盖所有与 MSI 打包相关的实际改动。

## 状态流转

```text
WindowsBuildEnvironment
  ↓ 前置检查通过
WindowsMsiArtifact(not_started)
  ↓ 执行构建
WindowsMsiArtifact(success 或 failed)
  ↓ success 时执行安装 smoke
InstallationValidationRecord
  ↓ 汇总变更与限制
MinimalChangeRecord
```

失败流转：

```text
前置条件缺失
  ↓
WindowsMsiArtifact(failed)
  ↓
记录 failure_reason、缺失项和补齐路径
```
