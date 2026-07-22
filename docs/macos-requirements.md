# macOS 运行环境要求

pinvou3 在 macOS 上支持 **Apple Silicon (arm64)** + **macOS 14.0 (Sonoma) 及以上**。

## 系统要求

| 项目 | 要求 |
|------|------|
| 处理器 | Apple Silicon (M1/M2/M3/M4 等 arm64 芯片) |
| macOS 版本 | 14.0 (Sonoma) 或更高 |
| 磁盘空间 | 应用本体约 200MB + 语音模型约 300MB（按需下载） |

> Intel Mac (x86_64) 暂不支持。语音识别引擎 (SenseVoice) 仅提供 darwin-arm64 二进制。

## 内置组件（无需安装）

以下组件已随应用打包，开箱即用：

- **语音识别引擎** — SenseVoice darwin-arm64，随 app 打包为 bundled Mach-O
- **ASR 模型** — 首次使用语音输入时按需下载到 `~/.pinvou3/asr/`

## 可选外部依赖

文件解析、OCR、音频转码等功能依赖外部命令行工具。这些工具**不是必须的**——缺失时对应功能会优雅降级（跳过该格式，不影响其他功能），但安装后可获得完整体验。

### 安装方式一：Homebrew（推荐）

如果已安装 [Homebrew](https://brew.sh)，应用内「设置 → 依赖体检」页面提供一键安装按钮。

```bash
# 安装 Homebrew（如尚未安装）
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"

# 通过应用内「依赖体检」一键安装，或手动执行：
brew install poppler pandoc tesseract p7zip ffmpeg
brew install --cask libreoffice
```

### 安装方式二：手动安装（无需 Homebrew）

不想安装 Homebrew 的用户可从各工具官网下载：

| 功能 | 工具 | 安装方式 | 官网 |
|------|------|----------|------|
| PDF 文本提取 | poppler (`pdftotext`) | Homebrew / 官网 | https://poppler.freedesktop.org |
| PDF 渲染预览 | poppler (`pdftoppm`) | Homebrew / 官网 | https://poppler.freedesktop.org |
| Office 文档转换 | LibreOffice | Homebrew cask / 官网 | https://www.libreoffice.org/download |
| Markdown/文档转换 | pandoc | Homebrew / 官网 | https://pandoc.org/installing.html |
| OCR（扫描件识别） | tesseract | Homebrew / 官网 | https://tesseract-ocr.github.io/tessdoc/Installation.html |
| 压缩包解压 | 7-Zip (`7z`) | Homebrew (`p7zip`) / 官网 | https://www.7-zip.org |
| 音频转码（语音输入） | ffmpeg | Homebrew / 官网 | https://ffmpeg.org/download.html |
| 邮件解析 (.msg) | msgconvert (Perl 模块) | `sudo cpan -i Email::Outlook::Message` | — |
| 邮件解析 (.eml) | Python 3 | macOS 自带（需 Xcode Command Line Tools） | https://www.python.org/downloads |

### Python 3 说明

macOS 自带 `/usr/bin/python3`，但需要安装 **Xcode Command Line Tools**：

```bash
xcode-select --install
```

如果未安装 Command Line Tools，`/usr/bin/python3` 只是一个弹窗 stub，无法实际运行。

## 依赖缺失时的行为

| 功能 | 依赖缺失时的行为 |
|------|------------------|
| PDF 文本提取 | 跳过文本提取，尝试 OCR 兜底 |
| PDF OCR | 跳过 OCR，该 PDF 标记为无法解析 |
| Office 文档 | 跳过转换，返回提示信息 |
| 压缩包 | 跳过解压，返回提示信息 |
| 邮件 (.msg/.eml) | 跳过解析，返回提示信息 |
| 语音输入 | 引擎已内置；缺 ffmpeg 时跳过音频转码，直接喂原始音频（识别质量可能下降） |

**所有缺失均不会导致应用崩溃。** 应用启动时和「设置 → 依赖体检」页面会实时检测已安装的工具状态。

## 应用内依赖体检

打开「设置 → 依赖体检」可查看各功能的依赖安装状态。检测是实时的（不走缓存），安装完工具后重新体检即可反映。

- **Linux**：显示 `sudo apt install ...` 一键安装指引
- **macOS**：检测到 Homebrew 时显示一键安装按钮；未检测到 Homebrew 时显示各工具官网链接
- **Windows**：依赖已内置或需手动安装

## GUI 启动与 PATH

从 DMG、Finder 或 Spotlight 启动的 GUI 应用不继承终端 shell 的 PATH。pinvou3 会在启动时自动探测并将以下目录加入进程 PATH（如果存在）：

- `/opt/homebrew/bin`（Apple Silicon Homebrew）
- `/usr/local/bin`（Intel Homebrew / 手动安装工具）
- `/Applications/LibreOffice.app/Contents/MacOS`（LibreOffice cask 安装位置）

如果工具安装在非标准路径，可通过设置 `PINVOU3_ASR_CMD` 等环境变量指向自定义路径。

## 已知限制（后续 PR 跟进）

以下为当前 macOS 分发链路的已知风险，集中登记以便后续 PR 跟踪：

- **未签名 / ad-hoc 分发**：当前 dmg 为 ad-hoc 签名（`tauri.conf.json` 的 `mac.signingIdentity = "-"`），首次打开会被 Gatekeeper 隔离。用户需手动解除隔离：
  ```bash
  xattr -dr com.apple.quarantine /Applications/pinvou3.app
  ```
  Developer ID + 公证（notarization）凭证将在后续 PR 接入，届时可省去此步并启用 Hardened Runtime（路线见 `src-tauri/entitlements.plist` 顶部注释）。

- **OTA 无独立签名验证**：更新通道的 `latest.json` 仅靠 sha256 自校验 + CFBundleIdentifier 字符串校验 + PlistBuddy 字段校验，没有 minisign / Developer ID 离线签名。攻击者若控制 pinvou.com 分发域名，理论上可投递伪造的 manifest + dmg。计划后续引入 manifest 离线签名 + 客户端验签。

- **凭证明文存储**：为绕开 ad-hoc 签名导致的 Keychain ACL 频繁弹窗，macOS 路径的 API Key 临时以明文存于 `~/.pinvou3/secrets/<service>.json`（目录 `0700` / 文件 `0600`，见 `src-tauri/src/credential_store.rs`）。接入稳定签名身份后将切回 Keychain。

- **Intel Mac 不支持**：仅打包 arm64 语音识别引擎（SenseVoice darwin-arm64），Intel Mac（x86_64）的语音输入不可用（其余功能正常）。详见上方「系统要求」。

