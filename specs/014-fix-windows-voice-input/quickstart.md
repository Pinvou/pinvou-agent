# Quickstart：修复 Windows 语音输入

## 前置条件

- Windows 桌面环境。
- 至少一个可用麦克风。
- pinvou3 开发版或安装版可启动。
- 如语音识别依赖现有模型或 provider 配置，使用当前项目已有配置，不新增默认外部服务。

## 代码排查入口

1. 搜索 pinvou 前端语音入口：

   ```powershell
   rg -n "Mic|voice|speech|microphone|getUserMedia|MediaRecorder" pinvou3-app/src
   ```

2. 搜索 Tauri 权限和命令注册：

   ```powershell
   rg -n "permission|capabilities|invoke_handler|voice|speech|microphone" pinvou3-app/src-tauri
   ```

3. 搜索 DeepSeek-TUI 已有语音命令：

   ```powershell
   rg -n "AppAction::VoiceCapture|capture_and_transcribe|/voice|voice.toggle" DeepSeek-TUI/crates/tui/src
   ```

## 自动化验证

按实际修改范围选择运行：

```powershell
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml voice --lib
cargo test --manifest-path DeepSeek-TUI/Cargo.toml -p codewhale-tui voice
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
```

如果修改前端逻辑，至少执行构建检查：

```powershell
cd pinvou3-app
npm run build -- --bundles msi
```

## Windows 手动 smoke

1. 启动 pinvou。
2. 进入一个会话，在输入框输入一小段草稿。
3. 点击语音输入入口。
4. 首次使用时允许麦克风权限。
5. 说一句短句，停止录音。
6. 验证识别文本追加到当前输入框，草稿未丢失。
7. 再次启动语音输入，录音期间切换会话。
8. 验证识别结果不会写入错误会话。
9. 拒绝或关闭麦克风权限后重试。
10. 验证 2 秒内出现清晰失败提示。

## 回归检查

- 普通文本发送可用。
- 附件上传可用。
- 会话切换可用。
- 语音输入取消后原有草稿保留。
- Windows 下不出现额外控制台弹窗。

## 打包验证

如本 feature 进入交付包：

```powershell
cd pinvou3-app
npm run build -- --bundles msi
```

安装 MSI 后重复 Windows 手动 smoke。

## 本轮实施验证记录（2026-06-24）

- `Get-Command sox`：当前 Windows 环境未找到 `sox`，确认 DeepSeek-TUI 原 `/voice` Windows 录音器依赖不可用。
- `node --check pinvou3-app/src/tauri-bridge.js`：通过。
- `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml voice --lib`：通过；当前无匹配 voice 单测，结果为 `0 passed; 214 filtered out`。
- `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`：通过，仅有既有 warning。
- `cd pinvou3-app; npm run build -- --bundles msi`：通过，生成 `pinvou3-app/src-tauri/target/release/bundle/msi/pinvou3_0.4.9_x64_en-US.msi`。

**待人工补验**：需要在安装后的 Windows GUI 中使用真实麦克风完成手动 smoke，包括允许/拒绝麦克风权限、正常短句识别、禁用设备/无设备、识别中切换会话、取消录音、普通文本发送和附件上传回归。
