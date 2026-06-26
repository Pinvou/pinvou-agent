# Quickstart：加密存储大模型 API Key 验证

## 1. 准备

确认当前 feature：

```powershell
Get-Content .specify\feature.json
```

期望 `feature_directory` 指向 `specs/019-encrypt-api-keys`。

## 2. 单元与静态检查

实现后至少执行：

```powershell
cargo test --manifest-path pinvou3-app\src-tauri\Cargo.toml credential
cargo test --manifest-path pinvou3-app\src-tauri\Cargo.toml prefs
cargo check --manifest-path pinvou3-app\src-tauri\Cargo.toml
```

如果测试名称最终不同，需在实现总结中列出实际执行的等价测试。

## 3. 新 Key 保存验证

1. 打开应用设置页，新增或编辑一个需要 API Key 的大模型配置。
2. 输入测试 Key 并保存。
3. 关闭并重启应用。
4. 检查 `~\.pinvou3\settings.json`：
   - 不得出现完整测试 Key。
   - 模型名称、preset、model、base_url、active model 信息仍存在。
5. 使用该模型发起连接测试或一次简单请求。
6. 期望请求能使用已保存凭据。

## 4. 旧明文迁移验证

1. 准备旧版 `settings.json`，在 `advanced.saved_models[*].api_key` 或 `advanced.custom_api_key` 中放入测试 Key。
2. 启动新版本应用。
3. 打开设置页确认模型仍显示“已配置”。
4. 再次检查 `settings.json`：
   - 完整测试 Key 不应保留。
   - 不应出现新的明文副本。
5. 重启应用并验证模型仍可连接。

## 5. 替换和删除验证

### 替换

1. 对已配置模型输入新的测试 Key。
2. 保存并重启。
3. 确认请求使用新 Key。
4. 确认旧 Key 不再出现在 settings、日志或诊断输出中。

### 删除

1. 点击删除/清除 Key。
2. 保存并重启。
3. 确认设置页显示未配置或需重新配置。
4. 发起模型请求时应提示缺少凭据，不得继续使用旧 Key。

## 6. 环境变量覆盖验证

1. 设置 `DEEPSEEK_API_KEY` 为临时测试值。
2. 启动应用并打开设置页。
3. 确认 UI 显示环境变量覆盖状态。
4. 检查 settings 和受保护凭据存储，确认环境变量值没有被自动写入。

## 7. 泄露检查

用测试 Key 的唯一片段扫描常见输出：

```powershell
rg "TEST_KEY_UNIQUE_PART" $env:USERPROFILE\.pinvou3
rg "TEST_KEY_UNIQUE_PART" pinvou3-app\src-tauri\target -g "*.log" -g "*.txt" -g "*.json"
```

期望：

- `settings.json` 不包含完整 Key。
- 日志、错误、诊断输出不包含完整 Key。
- 如果扫描命中受保护凭据存储的系统内部位置，不应通过普通文本文件直接读取到完整 Key。

## 8. 失败路径验证

模拟系统凭据存储不可用或读取失败：

1. 让凭据条目缺失或使用测试替身返回错误。
2. 打开设置页或发起模型请求。
3. 确认应用提示重新配置或凭据不可用。
4. 确认不会把 Key 回写到明文 settings。

## 9. 回归边界

- 本地 vLLM 无 Key 配置仍应可用。
- 搜索 API Key 不在本 feature 范围内，不应被本次迁移误删或改写。
- 反馈服务、MCP marketplace 配置和其它第三方凭据不应被本次迁移触碰。

## 10. 本次实现验证记录

2026-06-26 已执行：

```powershell
cargo test --manifest-path pinvou3-app\src-tauri\Cargo.toml credential
cargo test --manifest-path pinvou3-app\src-tauri\Cargo.toml prefs
cargo check --manifest-path pinvou3-app\src-tauri\Cargo.toml
```

结果：以上命令均通过；输出中仍有项目既有 warning。

补充执行过与本次改动相关的 bridge 回归用例：

```powershell
cargo test --manifest-path pinvou3-app\src-tauri\Cargo.toml openai_compatible
cargo test --manifest-path pinvou3-app\src-tauri\Cargo.toml official_deepseek
cargo test --manifest-path pinvou3-app\src-tauri\Cargo.toml qwen_preset_defaults
cargo test --manifest-path pinvou3-app\src-tauri\Cargo.toml remote_provider_keeps_default_reasoning_effort
```

结果：以上命令均通过。

未执行：第 3-7 节的真实 UI 手工验证仍需在运行中的应用内完成。
