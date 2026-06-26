# Linux 本地语音识别搭建方案

pinvou3 的语音输入原本是为 Windows 写的（bundle 的 `llama-funasr-sensevoice.exe`），
迁到 Linux 没适配。本文记录 Linux 端打通的全过程 + 一键搭建脚本，并给出生产化建议。

POC 已实测：x86 Linux CPU 上端到端跑通中文识别（录音 → 转码 → 识别 → 写入输入框）。

## 一、三关（代码改动已进本分支）

Linux 上语音输入要过三关，前两关是代码、第三关是引擎：

| 关 | 问题 | 修法 | 位置 |
|---|---|---|---|
| ① 录音权限 | webkit2gtk webview 默认拒绝 `getUserMedia` | setup 里挂 `permission-request`，只放行 UserMedia | `src-tauri/src/lib.rs` + `Cargo.toml`(webkit2gtk) |
| ② 错误文案 | 报错写「请在 Windows 权限中…」、技术黑话 | 改中性大白话 + 失败横条加「去依赖体检」按钮 | `tauri-bridge.js` / `index.html` / `linux_system.rs` |
| ③ 识别引擎 | Linux 没有 ASR 引擎/模型 | 用开源 **SenseVoice.cpp**（llama.cpp 系 gguf）+ shim 接入 | `scripts/asr/` |

## 二、引擎：SenseVoice.cpp

- 引擎源码：`github.com/lovemefan/SenseVoice.cpp`（llama.cpp 系，CPU 可跑；只 clone 一次构建，github 国内偶尔慢，可换镜像）
- 模型：modelscope `lovemefan/SenseVoiceGGUF`（**国内直连，实测 3.4 MB/s**，174MB 约 1 分钟，支持断点续传）
- 引擎二进制 `sense-voice-main` 仅 ~420KB；模型按量化档 174MB(q4_k) / 292MB(q8_0)
- CLI：`sense-voice-main -m 模型.gguf 音频.wav -l auto -itn`

`pinvou-asr`（团队中间层）期望的后端是 `-m -a --vad`，与 SenseVoice.cpp 的 `-m -f`
**参数不同**（是两个引擎），所以这里走 **shim** 适配，而非直接让 pinvou-asr 调它。

### shim 做的三件事（`scripts/asr/pinvou3-asr-shim.py`）
pinvou3 后端通过 `PINVOU3_ASR_CMD` 按 `asr --model X --lang zh --input wav` 调用 shim，shim：
1. **ffmpeg 转码**：浏览器录音多为 48k/立体声，统一转 16k 单声道（不转会加载失败）。
2. **调引擎**识别。
3. **清洗输出**：剥 `[start-end]` 时间戳前缀 + 去 `<|zh|><|NEUTRAL|>` 控制标记
   （否则会被 `parse_local_asr_text` 跳过或污染输入框）。

## 三、一键搭建

```bash
scripts/asr/setup-sensevoice.sh            # 默认 q4_k(174MB)
scripts/asr/setup-sensevoice.sh q8_0       # 更准(292MB)
GGML_CUDA=ON scripts/asr/setup-sensevoice.sh   # GB10 等带 GPU 开 CUDA 加速
```

装到 `~/.pinvou3/asr/`（引擎 + 模型 + shim）。接入：

```bash
PINVOU3_ASR_CMD=~/.pinvou3/asr/pinvou3-asr-shim.py ./pinvou3-app/run-dev.sh
```

点麦克风录音即可。诊断日志 `~/.pinvou3/asr/shim.log`。

## 四、体积与分发建议（不要全打进 .deb）

| 组件 | 体积 | 建议 |
|---|---|---|
| ffmpeg | 库共约 23MB，系统多自带 | **apt recommends**，不打包（同 poppler/pandoc） |
| 模型 | q4_k 174MB / q8 292MB | **按需下载**到 `~/.pinvou3/asr/`（同 bge-m3） |
| 引擎 | 420KB，且**分 CPU 架构** | 随模型按需下载对应架构 |

打进 .deb 会让每个包 +~180MB、连不用语音的人都背、且不跨架构。推荐
**ffmpeg 走 apt + 引擎/模型按需下载**。setup 脚本就是按需下载器的雏形。

## 五、GB10(arm64) 注意

- 引擎需在 arm64 上重新构建（`uname -m` 自动取架构，setup 脚本已处理 cmake 下载）。
- GB10 有 GPU，用 `GGML_CUDA=ON` 构建走 GPU 加速（CPU 也能跑，x86 实测 5.5s 音频 ~0.2s）。

## 六、生产化待办

- [ ] **按需下载器**：app 首次用语音时自动下引擎+模型到 `~/.pinvou3/asr/`（服务端托管对应架构二进制 + gguf），替代手动跑 setup。
- [ ] **shim 逻辑固化**：把 ffmpeg 转码 + 标记清洗固化进 `pinvou-asr`（团队中间层），或让 pinvou3 后端直接支持 SenseVoice.cpp 接口，去掉 shim 这层。
- [ ] **精度**：q4_k 有个别同音字误差（达→打），生产用 q8。
- [ ] ffmpeg 加入依赖体检 + deb recommends。
