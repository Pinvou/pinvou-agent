# PINVOU Knowledge

`pinvou-knowledge` 是 Pinvou 的可复用知识库核心和自包含服务器。它与桌面应用解耦，但桌面端本地知识库和服务器复用同一套 BGE-M3 加载、向量格式与切块逻辑。

## 能力

- 单服务器共享空间，可建立多个知识集
- 所有者 / 管理 / 只读三级设备权限，设备可单独调整、撤销或移除
- 建服、成员、分享、回收站、模型与升级均由 Pinvou 原生界面管理
- 托管源文件、多个文件夹递归导入、全文与语义混合检索、原文件下载
- 同一知识集内按内容摘要避免重复存储和索引，不同知识集仍可独立收录
- 30 天回收站保留期；所有者可在 Pinvou 中恢复或确认后永久删除
- SQLite + FTS5 + BGE-M3，无需 PostgreSQL、Qdrant 或 Redis

## 本地运行

```bash
cargo run --manifest-path pinvou-knowledge/Cargo.toml --release -- \
  --bind 127.0.0.1:3210 \
  --data-dir ./pinvou-knowledge-data
```

此命令仅用于服务端开发与调试。首次启动会在数据目录写入一次性 `host-owner.claim`，由 Pinvou 原生客户端安全领取后立即删除；产品流程不再使用浏览器后台、管理员密码或初始化密钥。

默认从 Hugging Face 的固定 revision 下载与桌面端本地知识库相同的 BGE-M3 INT8 ONNX 五文件清单，并逐文件验证 SHA-256。服务端可通过 `PINVOU_KNOWLEDGE_HF_BASE_URL` 指定 Hugging Face 兼容镜像；该变量只替换服务根地址，仓库、revision、文件路径和摘要仍由程序固定。桌面端额外优先读取 `PINVOU3_KB_HF_BASE_URL`，未设置时回退到统一变量。完整来源和镜像约定见 [知识库模型来源](../docs/knowledge-model.md)。

模型目录必须包含：

- `model.onnx`（或 `onnx/model_int8.onnx`）
- `tokenizer.json`
- `config.json`
- `special_tokens_map.json`
- `tokenizer_config.json`

## 网络与安全

服务器始终要求每台设备的独立令牌。分享链接默认 24 小时有效、可供多人提交加入申请；所有者通常需要在 Pinvou 中审批，也可在生成链接时仅对只读成员开启自动通过。Pinvou 只把设备令牌写入系统凭据库，不写入连接元数据。所有网络连接都使用由稳定服务身份保护的 HTTPS。分享链接携带服务身份与 CA；局域网发现和手动私网地址只产生未受信候选，客户端在不发送设备凭据的前提下探测服务，并要求用户与宿主界面核对稳定 CA 派生的身份码后再固定身份。Tailnet 地址只允许手动输入，不自动扫描。

TLS 私钥的文件权限边界与产品支持边界一致：受支持的 Linux 托管服务把数据目录设为 `0700`、证书和私钥设为 `0600`；macOS 等 Unix 开发运行同样强制这些模式。Windows 不提供创建共享知识库的产品入口，单独运行开发服务器时只能继承调用方数据目录的 Windows ACL，当前不承诺为任意目录重写安全描述符，因此不得把该入口当作已加固的 Windows 托管部署。

## Linux 服务

正常用户在 Linux 版 Pinvou 的“共享知识库”页面选择“在本机创建”即可。Pinvou 会申请一次系统授权，安装匹配版本的持久 systemd 服务，并自动成为所有者。

下列脚本仅保留为贡献者的独立服务调试入口：

```bash
bash pinvou-knowledge/deploy/install.sh
```

脚本会优先使用 `~/.cargo/bin` 中的 Rust 工具链，编译服务端并安装 systemd 服务。它不是普通用户的产品入口，也不会启动 Web 管理页。

也可以手工执行相同步骤：

```bash
cargo build --locked --manifest-path pinvou-knowledge/Cargo.toml --release
sudo install -m 0755 pinvou-knowledge/target/release/pinvou-knowledge-server /usr/local/bin/
sudo groupadd --system pinvou-knowledge 2>/dev/null || true
id pinvou-knowledge >/dev/null 2>&1 || sudo useradd --system --gid pinvou-knowledge --home-dir /var/lib/pinvou-knowledge --shell /usr/sbin/nologin pinvou-knowledge
sudo install -d -m 0700 -o pinvou-knowledge -g pinvou-knowledge /var/lib/pinvou-knowledge
sudo install -m 0644 pinvou-knowledge/deploy/pinvou-knowledge.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now pinvou-knowledge
```

服务默认监听 `0.0.0.0:3210`。生产环境可通过 systemd override 修改监听地址或模型来源环境变量。

## 可选文档解析器

纯文本、代码和电子表格由进程内解析，文本支持 UTF-8、UTF-16 和 GB18030，并拒绝导入私钥等敏感密钥材料。PDF、Office 与图片 OCR 分别按需调用 `pdftotext`、`pandoc` 和 `tesseract`；Pandoc 不支持的 Office 格式会回退到 LibreOffice，演示文稿会先转为 PDF 再提取文本。缺少对应命令时，该文档会保留并显示解析失败原因，不影响其他文档。
