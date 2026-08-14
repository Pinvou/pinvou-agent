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

默认下载与桌面端本地知识库相同的 Pinvou BGE-M3 发布包，并验证固定 SHA-256。内网镜像仍可通过 `PINVOU_KNOWLEDGE_MODEL_URL` 指定 `.tar.gz`；镜像内容不同时必须同时用 `PINVOU_KNOWLEDGE_MODEL_SHA256` 指定归档摘要。

模型目录必须包含：

- `model.onnx`（或 `onnx/model_int8.onnx`）
- `tokenizer.json`
- `config.json`
- `special_tokens_map.json`
- `tokenizer_config.json`

## 网络与安全

服务器始终要求每台设备的独立令牌。分享链接默认 24 小时有效、可供多人提交加入申请；所有者通常需要在 Pinvou 中审批，也可在生成链接时仅对只读成员开启自动通过。Pinvou 只把设备令牌写入系统凭据库，不写入连接元数据。所有网络连接都使用由稳定服务身份保护的 HTTPS；分享链接和局域网发现携带服务 CA，手动连接在发送凭据前完成首次身份确认。局域网发现不扫描 Tailscale。

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

纯文本、代码和电子表格由进程内解析。PDF、Office 与图片 OCR 分别按需调用 `pdftotext`、`pandoc` 和 `tesseract`；缺少对应命令时，该文档会保留并显示解析失败原因，不影响其他文档。
