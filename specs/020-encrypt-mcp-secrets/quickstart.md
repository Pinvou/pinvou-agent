# Quickstart：验证 MCP API 密钥不再明文存储

## 1. 静态检查内置资源

```powershell
rg -n "AMAP_KEY|IWENCAI_API_KEY|QCC_API_KEY|Authorization|Bearer|sk-" pinvou3-app/resources/mcp-servers
```

期望：

- 可以出现字段名。
- 不应出现真实密钥值。
- 不应出现 `Authorization: Bearer <真实密钥>`。

## 2. 运行 Rust 检查

```powershell
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml marketplace --lib
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml credential_store --lib
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
```

期望：

- 测试通过。
- 错误输出不包含真实 MCP API 密钥。

## 3. 验证旧配置迁移

准备一个临时 `PINVOU3_HOME`，放入旧版格式：

```text
<PINVOU3_HOME>/bundle/mcp-servers/weather/manifest.json
<PINVOU3_HOME>/bundle/mcp-servers/iwencai/manifest.json
<PINVOU3_HOME>/bundle/mcp-servers/qcc/manifest.json
<PINVOU3_HOME>/bundle/mcp.json
```

启动应用或触发 bundle/marketplace 初始化后检查：

```powershell
rg -n "真实测试密钥|Bearer 真实测试密钥" $env:PINVOU3_HOME
```

期望：

- 搜索结果为空。
- 对应凭据可从系统凭据存储读取。
- 再次启动不会重复迁移或重新写入明文。

## 4. 验证工具安装配置

在工具市场安装或重新启用：

- 高德天气
- 同花顺问财
- 企查查

然后检查：

```powershell
Get-Content -Encoding UTF8 "$env:USERPROFILE\.pinvou3\bundle\mcp.json"
rg -n "AMAP_KEY|IWENCAI_API_KEY|QCC_API_KEY|Authorization|Bearer" "$env:USERPROFILE\.pinvou3\bundle\mcp.json"
```

期望：

- `mcp.json` 不包含真实密钥。
- 如出现敏感字段，只能是凭据引用或非敏感声明。

## 5. 验证缺失凭据反馈

删除某个 MCP 工具对应的系统凭据后，再安装或启用该工具。

期望：

- 应用提示哪个工具、哪个字段缺失。
- 提示文本不包含密钥内容。
- 应用不得回退写入明文密钥。

## 6. Windows smoke

在 Windows 桌面应用中验证：

1. 首次启动不会在用户目录写入真实 MCP 密钥。
2. 旧版用户目录可以迁移并清理。
3. 已配置凭据时，高德天气、同花顺问财、企查查的 MCP 工具仍可正常安装和调用。
4. 未配置凭据时，错误反馈清晰可恢复。

## Verification log

- `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml marketplace --lib`: PASS, 9 passed.
- `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml credential_store --lib`: PASS, 5 passed.
- `cargo test --manifest-path DeepSeek-TUI/crates/tui/Cargo.toml expand_env_placeholders --lib`: PASS, 3 passed.
- `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`: PASS with existing warnings.
- Static scan:
  - `rg -n 'sk-[A-Za-z0-9_-]{10,}' pinvou3-app/resources/mcp-servers pinvou3-app/src-tauri/src specs/020-encrypt-mcp-secrets`
  - `rg -n 'AMAP_KEY|IWENCAI_API_KEY|QCC_API_KEY|Authorization|Bearer' pinvou3-app/resources/mcp-servers pinvou3-app/src-tauri/src/bridge pinvou3-app/src-tauri/src/credential_store.rs specs/020-encrypt-mcp-secrets`
  - Result: no real MCP secret values remain in bundled MCP manifests; remaining hits are field names, runtime env reads, placeholders, tests, or spec examples.
