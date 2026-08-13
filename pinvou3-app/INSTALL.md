# pinvou3 安装说明（本地 LLM 版）

> 本包为 **arm64 (aarch64)** 架构，仅限 ARM64 Linux（如 NVIDIA Jetson、Raspberry Pi 5、Apple Silicon Linux VM 等）。
> 默认连接 **本机 127.0.0.1:8000** 的 vLLM 服务，**不依赖外网**。

---

## 1. 安装

```bash
sudo dpkg -i pinvou3_0.6.2_arm64.deb
# 若报依赖缺失，自动补装：
sudo apt-get install -f
```

安装包会声明 Tauri UI 运行时依赖，并推荐以下文档处理工具：
- `libwebkit2gtk-4.1-0`、`libgtk-3-0`（Tauri UI 运行时）
- 推荐（非强制）：`poppler-utils`、`tesseract-ocr`、`tesseract-ocr-chi-sim`、`pandoc`、`p7zip-full`、`python3`（文档/图片处理工具）、`libreoffice`、`libemail-outlook-message-perl`

---

## 2. 启动本地 LLM（vLLM）

pinvou3 默认向本机 `http://127.0.0.1:8000/v1` 发送请求。接收方需**先自行启动 vLLM**，模型名建议包含 `_256k` 后缀（底座据此派生 256K 上下文窗口）。

示例启动命令（供参考，请按实际环境调整）：

```bash
# 假设模型路径为 /opt/models/qwen3.6-35b-a3b-fp8
vllm serve /opt/models/qwen3.6-35b-a3b-fp8 \
  --served-model-name qwen36_35b_256k \
  --max-model-len 262144 \
  --tensor-parallel-size 1 \
  --gpu-memory-utilization 0.95 \
  --enable-auto-tool-choice \
  --tool-call-parser qwen3_coder \
  --reasoning-parser qwen3
```

关键约束：
- Qwen3.6 官方建议使用 `vllm>=0.19.0`；旧版本可能不支持对应模型或无法识别下述 parser 参数，遇到未注册错误时请先升级 vLLM。
- `--served-model-name` **推荐** 设为 `qwen36_35b_256k`（或至少包含 `_256k`），底座据此派生 256K 上下文窗口与 compaction 阈值。
- `--enable-auto-tool-choice --tool-call-parser qwen3_coder` **必须**带上：缺了 vLLM 不会把模型输出解析成标准 `tool_calls`，工具调用会直接失效或漂移。注意 Qwen3.6 的 tool-call parser 名是 `qwen3_coder`；其他模型可能使用 `hermes` 等不同 parser，升级或更换模型时必须按对应模型卡核对。`qwen3` 是 `--reasoning-parser` 的注册名，写给 `--tool-call-parser` 会因未注册而启动失败。
- `--reasoning-parser qwen3` 建议按官方模型卡带上，由服务端把 thinking 内容解析到独立字段；缺省时 `<think>` 内容可能泄漏进正文。思考深度档位（reasoning effort）现由品悟按模型配置透传（SavedModel.reasoning_effort），不再使用 `DEEPSEEK_REASONING_EFFORT` 环境变量。
- 若使用其他模型名（如 `Qwen2.5-72B-Instruct`），底座也能识别 Qwen 系列并派生 128K 窗口；如仍想获得 256K 阈值，请在模型名中附加 `_256k` 后缀。
- 若 vLLM 绑在其他端口（如 `8080`），见下节「自定义后端地址」。

---

## 3. 启动 pinvou3

图形界面启动方式（任选其一）：

```bash
# 命令行
pinvou3

# 或从桌面菜单查找 "PINVOU 智能助手"
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

### 方式 B：应用设置（持久化，推荐）

打开「设置 → 模型与后端」，新增或编辑模型，填写模型名、API 地址和密钥后设为当前模型。应用会把模型保存到 `advanced.saved_models`，密钥单独存入系统凭证库；不要把明文密钥手写进 `settings.json`。

支持的模型预设：
- `local_vllm` — 默认本地 qwen36_35b_256k（无需改配置）
- `openai_compatible` — OpenAI 官方 / 兼容 API（如 GPT-4o、自托管 proxy、自定义本地 vLLM）
- `deepseek` — DeepSeek 官方 API
- `kimi` — Moonshot / Kimi
- `qwen` — 通义千问
- `doubao` — 豆包（火山方舟）
- `minimax` — MiniMax
- `glm` — 智谱 GLM
- `mimo` — 小米 MiMo

需要用环境变量持久覆盖时，也可将方式 A 的变量写入启动脚本；桌面菜单启动通常不会读取 `~/.bashrc`。

---

## 5. 社区版更新

社区版安装包通过 GitHub Releases 发布；当前不提供应用内更新。升级时请下载
新版本安装包并覆盖安装，`PINVOU3_HOME` 下的用户数据不会被安装器删除。

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
| `pinvou3_<版本>_arm64.deb` | 主安装包 |
| `INSTALL.md` | 本安装说明 |
