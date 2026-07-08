# Quickstart：Windows 系统监控改为 CPU 卡片

## 前置条件

- 当前分支：`023-windows-cpu-monitor`
- Windows 开发环境可运行 Tauri app
- 已安装 Rust、Node 依赖和项目现有构建依赖

## 实施后验证

1. 运行后端测试：

```powershell
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib monitor
```

2. 如果新增 OS 层 CPU 单元测试，运行相关测试：

```powershell
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib cpu
```

3. 启动应用：

```powershell
cd pinvou3-app
npm run tauri dev
```

4. 在 Windows 上打开“系统监控”页，确认：

- 原“图形处理器（GPU）”卡片替换为“处理器（CPU）”卡片。
- 卡片显示 CPU 名称。
- 卡片显示总体 CPU 使用率。
- 卡片显示 pinvou3 应用进程 CPU 使用率。
- 卡片显示逻辑处理器数量。
- 页面不再显示 `nvidia-smi` 缺失提示。
- RAM、模型服务、应用指标仍正常刷新。

5. 在非 Windows 平台回归验证：

- 系统监控页仍显示原 GPU 卡片。
- 原有 GPU 可用/不可用降级逻辑保持不变。

## 失败处理

- 如果 CPU 使用率始终为空，先检查 Windows CPU 采样是否需要两次间隔采样。
- 如果页面仍显示 GPU 文案，检查前端格式化层是否正确识别 `cpu` 快照。
- 如果非 Windows 页面也变成 CPU 卡片，检查平台/快照分支是否过宽。
