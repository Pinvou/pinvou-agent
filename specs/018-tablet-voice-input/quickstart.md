# Quickstart：平板语音输入强化验证

## 前置条件

- 当前 feature 指向 `specs/018-tablet-voice-input`。
- 应用已有语音输入能力可用，或至少能进入权限/失败反馈路径。
- Windows 平板、二合一设备、带触摸屏设备，或可模拟平板尺寸的窗口。

## 静态检查

```powershell
cargo check --manifest-path pinvou3-app\src-tauri\Cargo.toml
```

实现后建议同时检查：

```powershell
rg -n "voiceInput|startVoiceInput|cancelVoiceInput|clearVoiceInput|handleSend|handleVoiceClick|inputText" pinvou3-app/src/index.html pinvou3-app/src/tauri-bridge.js
```

## 开发运行

```powershell
cd pinvou3-app
npm run dev
```

## 平板触屏 smoke

1. 在平板尺寸窗口或触屏设备中打开主聊天界面。
2. 确认现有输入框、附件按钮、原语音入口、模型选择、工具入口仍可见或可访问。
3. 确认出现一个更醒目的主语音输入按钮。
4. 不输入任何文本，确认发送和清除按钮不会作为主要操作干扰用户。
5. 输入普通文本，确认发送按钮和清除按钮出现且可触控。
6. 点击清除按钮，确认只清空输入框文本，不删除附件、不取消会话、不影响语音入口。
7. 再次输入文本，点击发送按钮，确认沿用现有发送流程。

## 语音输入 smoke

1. 点击主语音输入按钮。
2. 确认界面立即显示请求权限或录音中状态。
3. 录音中再次点击主语音入口或结束动作，确认进入识别中状态。
4. 识别完成后，确认文本写入输入框且可编辑。
5. 输入框有识别文本时，确认发送和清除按钮可见。
6. 点击发送，确认文本按现有聊天发送流程提交。
7. 再次录音并点击取消，确认不会写入新文本，界面回到可输入状态。

## 失败路径 smoke

- 拒绝麦克风权限，确认出现明确失败反馈，并且文本输入仍可用。
- 在没有可用麦克风的环境中启动语音输入，确认提示可恢复。
- 在录音/识别过程中重复点击主语音入口，确认不会出现多个并发录音状态。

## 桌面回归

1. 在常规桌面大窗口中打开应用。
2. 确认主输入区视觉密度没有明显退化。
3. 确认原有键盘输入、Enter 发送、Shift+Enter 换行、附件添加、模型选择和工具菜单仍可用。
4. 确认新增主语音入口若显示，不遮挡输入框、发送按钮、消息内容或产物入口。

## 横竖屏和尺寸验证

- 平板竖屏：输入框自动增高时，主语音入口和发送/清除按钮不重叠。
- 平板横屏：底部输入区和主语音入口不遮挡最后一条消息。
- 窄窗口：按钮应换行、收纳或降级，但不能覆盖文本。

## 可访问性检查

- 使用键盘 Tab 导航，确认主语音入口、清除按钮、发送按钮均可聚焦。
- 确认按钮具备明确名称，例如“开始语音输入”“结束录音”“清除输入”“发送”。
- 确认录音中、识别中、失败等状态不只依赖颜色表达。

## 实现后验证记录

- 2026-06-26 已执行：`cargo check --manifest-path pinvou3-app\src-tauri\Cargo.toml`。结果：通过；存在既有 warning，未发现本功能引入的 Rust/Tauri 编译错误。
- 2026-06-26 已执行：`rg -n "voiceInput|startVoiceInput|cancelVoiceInput|clearVoiceInput|handleSend|handleVoiceClick|inputText" pinvou3-app/src/index.html pinvou3-app/src/tauri-bridge.js`。结果：通过；语音入口仍复用现有 `tauri-bridge.js` 语音桥接与 `ChatView` 发送入口。
- 2026-06-26 未执行：真实触屏平板 smoke、语音录音 smoke、失败路径 smoke、桌面窗口手动回归。原因：当前回合未启动 Tauri 图形界面和真实麦克风/触屏设备；需要在目标 Windows 设备上按本文件步骤补验。
