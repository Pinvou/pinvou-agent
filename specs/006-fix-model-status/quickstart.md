# Quickstart：大模型状态监控验证

## 前置条件

- 当前分支：`008-fix-model-status`
- Rust 工具链可用。
- Windows 环境可运行 Tauri 应用。
- 如需完整 smoke 验证，需要准备至少一个远端模型配置和一个本地模型配置。

## 自动检查

在仓库根目录执行：

```powershell
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml bridge --lib
```

再执行：

```powershell
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml monitor --lib
```

最后执行：

```powershell
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
```

预期结果：

- 模型目标推导测试通过。
- 远端、本地、鉴权失败、非模型服务、模型不匹配和指标适用性测试通过。
- 编译检查通过。

## 远端模型 smoke

在设置中选择远端模型配置并保存重启，例如远端官方模型或远端 OpenAI-compatible 模型。

打开“系统监控”页。

预期结果：

- 大模型状态卡显示当前远端模型目标地址。
- 状态不再由 `127.0.0.1:8000` 等本机默认地址决定。
- 远端鉴权失败时显示鉴权相关原因。
- 队列、KV 命中率、TTFT、吞吐和 token 统计不被显示为本地模型异常，而是说明远端模型不适用本地运行指标。

## 本地模型 smoke

确保本地模型服务可访问，例如：

```powershell
curl http://127.0.0.1:8001/v1/models
curl http://127.0.0.1:8001/metrics
```

在设置中把模型地址配置为对应本地地址并保存重启。

预期结果：

- 大模型状态卡显示当前本地模型目标地址。
- `/v1/models` 可用时状态不是离线。
- metrics 可用时展示上下文长度、队列、KV 命中率、首字延迟、吞吐和 token 统计。
- metrics 不可用时保留基础模型状态，并提示指标缺失。

## 非模型服务占用端口 smoke

将本地模型地址配置为一个被非模型服务占用的本机地址。

预期结果：

- 大模型状态显示不可用或响应异常。
- 不误报模型在线。
- 页面能显示当前检测地址，方便用户定位端口冲突。

## 回归检查

- 切换远端模型后，系统监控页不检测本机默认模型地址。
- 切换本地模型后，系统监控页检测当前配置的本地地址，而不是固定端口。
- ChatRoom 顶部 live dot 与系统监控页的大模型在线语义一致。
- GPU、系统内存和应用信息不受大模型状态检测失败影响。

## 2026-06-16 实施验证记录

- `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`：通过。仍有 DeepSeek-TUI 上游与既有 app 警告，未新增编译错误。
- `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml monitor --lib`：通过，13 passed。
- `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml bridge --lib -- --test-threads=1`：通过，65 passed。
- `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml bridge --lib`：并行执行时失败于既有 `PINVOU3_HOME`/路径环境变量竞态，单线程同范围通过；本 feature 未修改失败用例所在的 `bundle.rs` 和 `paths.rs`。
- DeepSeek-TUI 边界检查：`git status --short DeepSeek-TUI` 无输出，本 feature 未修改底座 fork。
- Windows 手动 smoke：通过。用户已手动完成远端官方模型、远端兼容模型和本地模型三类配置验证，T041 已完成。
