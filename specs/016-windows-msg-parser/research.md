# 研究：Windows MSG 邮件解析

## 决策 1：Windows `.msg` 使用 Rust 原生 `msg_parser`

**Decision**：在 Windows `.msg` 解析路径中使用 `msg_parser` 0.3.x，直接读取 Outlook `.msg` 文件并提取邮件头、正文和附件名。

**Rationale**：

- `msg_parser` 面向 Outlook `.msg`（OLE Compound Document）格式，公开说明支持提取 message metadata、body content、recipients、attachments 和 transport headers。
- `Outlook::from_path` 可直接从文件路径解析，结构字段包含 `sender`、`to`、`cc`、`bcc`、`subject`、`body`、`html`、`message_delivery_time` 和 `attachments`。
- 这是 Rust crate，可随 Tauri 后端一起编译，不需要在 Windows MSI 中额外内置 Perl、`msgconvert` 或 Linux 包。
- crate 要求 rustc 1.85+，本项目 Rust 1.88 满足要求。

**Alternatives considered**：

- **继续依赖 `msgconvert`**：Windows 环境没有天然安装路径，用户会看到 Linux 包名，体验差。
- **内置 Strawberry Perl + Email::Outlook::Message**：最接近 Linux 行为，但体积和维护成本高。
- **Python `extract-msg`/`msg-parser` 包**：能力可用，但需要内置 Python runtime 和依赖包，不符合当前 Windows 依赖内置收敛方向。
- **把 `.msg` 转 `.eml` 再复用 `.eml` 解析**：可行但不是用户需求必要条件；本项目只需要为模型生成可读邮件文本，直接解析更简单。

参考：

- `msg_parser` GitHub README：https://github.com/marirs/msg-parser-rs
- `msg_parser::Outlook` docs.rs：https://docs.rs/msg_parser/latest/msg_parser/struct.Outlook.html

## 决策 2：`.eml` 保持现有 Python 标准库解析

**Decision**：本 feature 不替换 `.eml` 解析路径，继续使用当前 `python3 -c` + 标准库 `email` 模块。

**Rationale**：

- 规格要求 `.eml` 行为保持稳定，当前输出已经被用户接受。
- 替换 `.eml` 解析会扩大回归范围，且与 Windows `.msg` 依赖问题无直接关系。
- 后续如需减少 Python 依赖，可单独发起 feature 评估 Rust MIME 解析库。

**Alternatives considered**：

- **同时改为 Rust MIME 解析**：长期方向更一致，但会改变 `.eml` 回归面，不适合本次小步变更。
- **让 `.msg` 生成 `.eml` 后复用 Python**：会引入 MIME 构造复杂度，不如直接生成同格式 markdown。

## 决策 3：Linux 保留现有 `libemail-outlook-message-perl/msgconvert`

**Decision**：Linux `.msg` 路径继续使用现有 `msgconvert`，依赖体检和一键安装仍提示 `libemail-outlook-message-perl`。

**Rationale**：

- 用户需求明确是 Windows 下移除该依赖，不要求替换 Linux 行为。
- Linux 路径已有依赖白名单和安装脚本，保留能降低跨平台回归风险。
- Windows 和 Linux 可在 OS 层暴露不同邮件能力策略，前端按平台显示正确补全方式。

**Alternatives considered**：

- **跨平台全部改为 `msg_parser`**：可减少外部依赖，但会改变 Linux 已验证行为，适合后续独立清理。
- **统一隐藏邮件依赖体检项**：会削弱 Linux 用户的一键安装体验。

## 决策 4：Windows 依赖体检移除 Linux 包名

**Decision**：Windows 上邮件依赖体检不展示 `libemail-outlook-message-perl`、Perl 或 `msgconvert` 安装提示；邮件项应基于 Windows 内置/原生解析能力判断。

**Rationale**：

- Linux 包名在 Windows 上不可执行，展示会误导用户。
- Windows `.msg` 解析不应依赖 PATH 中偶然存在的 `msgconvert`，避免不同机器行为不一致。
- `.eml` 的 Python 依赖是现状，是否继续在 Windows 体检中展示 Python 需要实现时按现有产品策略确认；但 `.msg` 不再由 `msgconvert` 阻塞。

**Alternatives considered**：

- **继续共用单个 email 体检项**：实现简单，但无法区分 Windows `.msg` 原生能力和 Linux `.msgconvert` 能力。
- **拆分 `.eml` 与 `.msg` 两个体检项**：信息更精确，但会改变前端展示，当前需求不要求增加 UI 项。

## 最终实施记录（2026-06-25）

- `Cargo.toml` 使用 `msg_parser = "0.3.0"`，`Cargo.lock` 实际解析为 `msg_parser v0.3.6`。
- Windows `.msg` 解析通过 `msg_parser::Outlook::from_path`，输出字段覆盖发件人、收件人、抄送、密送、主题、日期、正文和附件名。
- Windows `.msg` 路径不再检查或调用 `msgconvert`；Linux `.msg` 路径继续保留 `msgconvert` 转 `.eml`。
- 验证命令：
  - `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`
  - `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib`
- 本机未发现真实 `.msg/.eml` 验收样本，真实样本结论待手动验收补充。
