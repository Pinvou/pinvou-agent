# Quickstart：Windows 系统内存监控验证

## 前置条件

- 当前分支：`006-windows-memory-monitor`
- Rust 工具链可用。
- Windows 环境可运行 Tauri 应用。

## 自动检查

在仓库根目录执行：

```powershell
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml
```

再执行：

```powershell
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
```

预期结果：

- 命令成功完成。
- Linux 内存解析相关测试继续通过。
- Windows 目标下无编译错误。

## 手动 smoke 验证

启动应用：

```powershell
cd pinvou3-app
npm run tauri dev
```

打开“系统监控”页，观察“系统内存”区域。

预期结果：

- 已用内存显示为有效数值，不是 `—`。
- 总内存显示为有效数值，不是 `—`。
- 物理内存进度条显示合理百分比。
- GPU 和 vLLM 即使仍不可用，也不影响系统内存展示。

## 回归检查

如具备 Linux 环境，执行同样的 Rust 测试，并手动确认 Linux 下系统内存展示仍可用。

## 故障判断

- 若系统内存仍显示 `—`，优先检查 `get_monitor_snapshot` 返回的 `ram` 是否为 `null`。
- 若 `ram` 为 `null`，检查当前平台内存采样实现是否返回空。
- 若 `ram` 有值但页面仍显示不可用，检查 `tauri-bridge.js` 对 `ram.used_kib` 与 `ram.total_kib` 的格式化逻辑。

## 本次验证记录

- `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_memory --lib`：通过，4 个 Windows 内存采样相关测试全部通过。
- `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml sample_all_includes_os_ram_and_app_snapshot --lib`：通过，确认监控聚合快照包含非空 `ram` 且应用信息仍返回。
- `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`：通过，仅保留既有 warning。
- `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml`：已执行，但存在 3 个既有非本功能失败：`bridge::bundle::tests::ensure_extracted_behavior`、`commands::tests::validate_user_path_blocks_etc_shadow`、`workflow_migrate::tests::host_archive_failure_returns_err_and_resumes_from_pending_file`。
- Windows GUI 手动 smoke：未在本次自动流程中完成；后端采样和监控快照已通过测试验证。
