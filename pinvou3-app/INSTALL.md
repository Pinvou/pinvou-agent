# pinvou3 v0.2.0 安装说明（本地 LLM 版）

> 本包为 **arm64 (aarch64)** 架构，仅限 ARM64 Linux（如 NVIDIA Jetson、Raspberry Pi 5、Apple Silicon Linux VM 等）。
> 默认连接 **本机 127.0.0.1:8000** 的 vLLM 服务，**不依赖外网**。

---

## 1. 安装

```bash
sudo dpkg -i pinvou3_0.2.0_arm64.deb
# 若报依赖缺失，自动补装：
sudo apt-get install -f
```

`dpkg` 会自动处理以下依赖：
- `libwebkit2gtk-4.1-0`、`libgtk-3-0`（Tauri UI 运行时）
- `poppler-utils`、`tesseract-ocr`、`tesseract-ocr-chi-sim`、`pandoc`、`p7zip-full`、`python3`（文档/图片处理工具）
- 推荐（非强制）：`libreoffice`、`libemail-outlook-message-perl`

---

## 2. 启动本地 LLM（vLLM）

pinvou3 默认向本机 `http://127.0.0.1:8000/v1` 发送请求。接收方需**先自行启动 vLLM**，模型名必须包含 `_256k` 后缀（底座据此派生 256K 上下文窗口）。

示例启动命令（供参考，请按实际环境调整）：

```bash
# 假设模型路径为 /opt/models/qwen3.6-35b-a3b-fp8
vllm serve /opt/models/qwen3.6-35b-a3b-fp8 \
  --served-model-name qwen36_35b_256k \
  --max-model-len 262144 \
  --tensor-parallel-size 1 \
  --gpu-memory-utilization 0.95
```

关键约束：
- `--served-model-name` **推荐** 设为 `qwen36_35b_256k`（或至少包含 `_256k`），底座据此派生 256K 上下文窗口与 compaction 阈值。
- 若使用其他模型名（如 `Qwen2.5-72B-Instruct`），底座也能识别 Qwen 系列并派生 128K 窗口；如仍想获得 256K 阈值，请在模型名中附加 `_256k` 后缀。
- 若 vLLM 绑在其他端口（如 `8080`），见下节「自定义后端地址」。

---

## 3. 启动 pinvou3

图形界面启动方式（任选其一）：

```bash
# 命令行
pinvou3

# 或从桌面菜单查找 "pinvou3 智能助手"
```

首次启动会自动在 `~/.pinvou3/` 下创建配置目录并解包内置技能。

---

## 4. 自定义后端地址与模型（可选）

### 方式 A：环境变量（临时 / 开发调试）

```bash
export DEEPSEEK_BASE_URL="http://192.168.1.100:8000/v1"
export DEEPSEEK_API_KEY="local-no-auth"   # 无鉴权时保持此值
export DEEPSEEK_MODEL="qwen36_35b_256k"
pinvou3
```

环境变量优先级最高，适合 run-dev.sh 或临时切换。

### 方式 B：`~/.pinvou3/settings.json`（持久化）

手改 `~/.pinvou3/settings.json` 的 `advanced` 字段：

```json
{
  "advanced": {
    "model_preset": "custom_local",
    "custom_model_name": "my-local-qwen",
    "custom_base_url": "http://192.168.1.100:8000/v1",
    "custom_api_key": "local-no-auth"
  }
}
```

支持的 `model_preset`：
- `local_vllm` — 默认本地 qwen36_35b_256k（无需改配置）
- `custom_local` — 自定义本地 vLLM 模型名/地址
- `remote_openai` — OpenAI 官方 / 兼容 API（如 GPT-4o、自托管 proxy）
- `remote_deepseek` — DeepSeek 官方 API
- `remote_moonshot` — Moonshot / Kimi

`custom_model_name`、`custom_base_url`、`custom_api_key` 在 `custom_local` 和全部 `remote_*` 模式下生效。

也可写入 `~/.bashrc` 或创建启动脚本持久化。

---

## 5. Windows 软件更新（Windows 版）

Windows 版应用内更新默认使用 H3C OTA 服务 `https://api.intcloud.h3c.com`：

1. 查询更新：`POST /ota/pkg/package/upgrade/check`
2. 下载更新包：HTTP 下载 zip 到 `~/.pinvou3/updates/`
3. 解析安装包：解压下载 zip，读取 `OtaInfo.json` 并定位 `.msi` 或 NSIS `.exe`
4. 启动安装：MSI 使用系统被动安装；NSIS EXE 使用 `/P /UPDATE` 显示安装进度并自动开始，无需手动点击按钮。安装器启动后当前 pinvou3 进程退出
5. 反馈结果：下次启动读取 `~/.pinvou3/updates/update-feedback.json` 并调用 `/ota/pkg/package/updateLog`

可选环境变量：

```powershell
$env:PINVOU3_OTA_HOST = "https://api.intcloud.h3c.com"
$env:PINVOU3_OTA_SN = "device-sn"
$env:PINVOU3_OTA_SOFTWARE_ID = "Pinvou3_Win"
```

- `PINVOU3_OTA_HOST` 可覆盖 Windows 更新源；未配置时使用 `https://api.intcloud.h3c.com`。
- `PINVOU3_OTA_SN` 默认读取 Windows `COMPUTERNAME`，仍为空时使用 `UNKNOWN`。
- `PINVOU3_OTA_SOFTWARE_ID` 默认 `Pinvou3_Win`。
- `PINVOU3_HOME` 可重定位用户数据根目录，更新暂存目录随之移动。
- 升级反馈失败会保留本地记录，并在下次启动后重试。

Linux `.deb` 更新链路仍使用原有 `latest.json`、sha256 校验和 `pkexec apt-get install` 流程。

---

## 6. 卸载

```bash
sudo apt remove pinvou3
```

用户数据（`~/.pinvou3/`）不会自动删除，如需清理：

```bash
rm -rf ~/.pinvou3
```

---

## 7. 外发文件清单

| 文件 | 说明 |
|------|------|
| `pinvou3_0.2.0_arm64.deb` | 主安装包（14 MB） |
| `INSTALL.md` | 本安装说明 |
