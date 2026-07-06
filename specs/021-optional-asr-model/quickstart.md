# Quickstart：ASR 模型可选下载验证

## 1. 静态检查

```powershell
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
```

预期：编译通过，无新增 DeepSeek-TUI fork 改动要求。

## 2. 运行目标测试

```powershell
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml voice_asr --lib
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_path --lib
```

预期：
- ASR 状态能区分 runtime、模型和 ready。
- Windows 路径测试覆盖安装目录 runtime、用户目录模型和模型缺失场景。

## 3. 检查 Windows 主包资源

构建 Windows 安装包后检查产物或临时 bundle 目录：

```powershell
Get-ChildItem -Recurse pinvou3-app/src-tauri/target/release/bundle |
  Where-Object { $_.Name -like "*sensevoice-small-q8.gguf*" }
```

预期：主安装包不包含 `sensevoice-small-q8.gguf`。

## 4. 无模型环境 smoke

1. 确认 `~/.pinvou3/asr/sensevoice-small-q8.gguf` 不存在，或临时改名备份。
2. 启动应用。
3. 点击语音输入入口。
4. 确认前端显示 ASR 模型缺失和下载入口，而不是提示重装应用。

预期：非语音功能仍可用；语音入口弹出可理解的下载提示。

## 5. 下载模型 smoke

1. 在模型缺失状态点击下载。
2. 观察 `voice_asr:progress` 对应的进度展示。
3. 下载完成后再次点击语音输入并录制短音频。

预期：下载完成无需重启即可转写；重启应用后不重复提示下载。

## 6. 失败恢复 smoke

分别模拟以下场景：

- 下载 URL 不可达。
- 下载中断。
- `.part` 文件残留。
- 模型文件损坏或大小不匹配。

预期：应用展示失败原因和重试入口，不启用损坏模型，不影响其它功能。

## 7. 体积验收

对比当前含模型 Windows 安装包基线与改造后主包体积。

预期：Windows 主安装包减少至少 150 MB。

## 当前资源基线

记录时间：2026-07-06

Windows ASR 资源目录当前关键文件体积：
- `pinvou-asr.exe`：503,296 bytes
- `llama-funasr-sensevoice.exe`：1,594,368 bytes
- `models/fsmn-vad.gguf`：1,720,512 bytes
- `models/sensevoice-small-q8.gguf`：254,208,320 bytes

本 feature 后，`tauri.conf.json` 仅映射 README、`pinvou-asr.exe`、`llama-funasr-sensevoice.exe` 和 `fsmn-vad.gguf`；`sensevoice-small-q8.gguf` 保留在源码资源目录中用于开发/兼容，但不进入 Windows 主安装包资源映射。

## 验收记录模板

```text
日期：
执行人：

cargo fmt：
cargo test voice_asr --lib：
cargo test windows_path --lib：
cargo check：

Windows 主包资源检查：
- 是否包含 sensevoice-small-q8.gguf：
- runtime 文件是否存在：
- fsmn-vad.gguf 是否存在：

无模型 smoke：
- voice_asr_status：
- 前端提示：

下载 smoke：
- 下载源：
- sha256：
- 下载完成后 ready：
- 重启后状态：

失败恢复 smoke：
- URL 不可达：
- 取消下载：
- 损坏模型：
- .part 残留：

体积对比：
- 改造前：
- 改造后：
- 减少：

DeepSeek-TUI 改动检查：
```

## 本轮自动验证记录

日期：2026-07-06
执行人：Codex

```text
cargo fmt：
- 已运行 `cargo fmt --manifest-path pinvou3-app/src-tauri/Cargo.toml`
- 执行后已恢复非本 feature 相关文件的格式化噪声；本轮未重新生成完整安装包。

cargo test voice_asr --lib：
- PASS，4 passed; 0 failed
- 覆盖 sha256 helper、大小+sha256 校验、校验失败清理 .part、runtime ready 但 model missing 状态。

cargo test windows_path --lib：
- PASS，14 passed; 0 failed
- 覆盖 Windows ASR runtime 缺 q8 大模型、用户目录模型优先、旧内置模型 fallback。

cargo check：
- PASS，编译通过；仍有既有 warning，包括 DeepSeek-TUI private_interfaces 和若干 unused/dead_code。

Windows 主包资源检查：
- 静态检查 `tauri.conf.json`：未映射 sensevoice-small-q8.gguf。
- runtime 文件已映射：README.md、pinvou-asr.exe、llama-funasr-sensevoice.exe。
- fsmn-vad.gguf 已映射。

DeepSeek-TUI 改动检查：
- `git status --short DeepSeek-TUI` 无输出，本 feature 未改动 DeepSeek-TUI。

未执行：
- Windows 完整打包产物体积对比。
- Windows 无模型、下载、重启、失败恢复真实 smoke。
- URL 不可达和写入失败的专门故障注入单测。
```
