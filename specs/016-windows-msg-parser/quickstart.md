# Quickstart：Windows MSG 邮件解析验证

## 1. 准备样本

建议准备以下文件：

- `sample-cn.msg`：包含中文发件人、收件人、主题、正文。
- `sample-attachment.msg`：包含至少 1 个附件。
- `sample.eml`：现有 `.eml` 回归样本。
- `broken.msg`：任意非 MSG 文件改名或截断后的损坏样本。

## 2. 开发验证

在仓库根目录执行：

```powershell
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib
```

预期：

- 邮件分类测试通过。
- Windows `.msg` 格式化/解析单测通过。
- `.eml` 回归测试通过。
- 依赖体检平台策略测试通过。

## 3. Windows 手动验收

环境要求：

- 不安装 Perl。
- 不安装 `msgconvert`。
- 保持应用正常运行。

步骤：

1. 打开 Pinvou Windows 应用。
2. 上传 `sample-cn.msg`。
3. 确认结果包含发件人、收件人、主题、日期和正文。
4. 上传 `sample-attachment.msg`。
5. 确认结果至少列出附件文件名。
6. 上传 `broken.msg`。
7. 确认应用不崩溃，并返回可理解 warning。
8. 打开依赖体检页。
9. 确认页面不出现 `libemail-outlook-message-perl`、`msgconvert` 或 `sudo apt install`。

## 4. EML 回归

上传 `sample.eml`。

预期输出结构仍为：

```text
发件人: ...
收件人: ...
抄送: ...
主题: ...
日期: ...

正文:
...

附件: ...
```

## 5. Linux 回归

在 Linux 环境执行依赖体检：

- 缺少依赖时，邮件项仍提示 `python3 libemail-outlook-message-perl`。
- 安装依赖后，`.msg` 仍可通过现有转换链路解析。

## 6. 打包前检查

```powershell
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
```

如涉及前端依赖体检展示调整，再运行应用 smoke：

```powershell
cd pinvou3-app
npm run tauri dev
```

## 7. 实施验证记录（2026-06-25）

- 本机在仓库与 `C:\Users\z27014\Downloads` 中未发现可直接用于验收的 `.msg/.eml` 样本；手动验收仍需准备 `sample-cn.msg`、`sample-attachment.msg`、`sample.eml`。`broken.msg` 可用任意非 MSG 文件改名生成。
- 基线：`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib`，通过 `16 passed; 6 ignored`。
- 依赖：新增 `msg_parser = "0.3.0"`，Cargo 实际锁定 `msg_parser v0.3.6`。
- 实施后：`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib`，通过 `20 passed; 6 ignored`。
- `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml` 已通过，仅保留既有 warnings。
- Windows 依赖体检：新增单测确认 `email` 项 `apt` 为空，且不包含 `libemail-outlook-message-perl` 或 `msgconvert`。
- Windows 损坏 `.msg`：新增单测确认返回 warning，不崩溃，并保留 `basename/path/byte_size`。
- 前端文案：`dep_email` 仅为通用 “邮件（.eml / .msg）” 标签，无需新增 key。
- 打包资源：本 feature 使用 Rust crate 随后端编译，不新增 MSI 外部 runtime/resource。
