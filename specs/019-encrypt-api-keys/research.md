# 研究：加密存储大模型 API Key

## 决策 1：使用系统凭据存储，不自研加密密钥

**Decision**：大模型 API Key 写入系统用户级凭据存储。Windows 使用 Credential Manager，macOS 使用 Keychain，Linux 使用 Secret Service。实现层可直接使用 `keyring` crate，或复用 DeepSeek-TUI 已引入的 `codewhale-secrets` 中的 `DefaultKeyringStore` 模式。

**Rationale**：自研“加密配置文件”仍需要安全保存解密密钥，容易把问题从“明文 Key”转移为“明文主密钥”。系统凭据存储天然绑定当前系统用户，符合“当前用户可用、其它用户不可直接读取”的规格假设。`keyring` 官方文档说明其 v1 模式支持 macOS、Windows 和 *nix 平台的密码/secret 读写；项目依赖树中已经存在 `keyring v3.6.3`，DeepSeek-TUI 的 `codewhale-secrets` 也已经实现系统 keyring 封装。

**Alternatives considered**：
- 自行使用 AES 加密 `settings.json`：拒绝。主密钥保存和轮换会成为新的高风险面。
- 仅做 Base64/混淆：拒绝。不能满足“不可直接复制使用”的安全目标。
- 使用 DeepSeek-TUI file secret fallback：拒绝作为默认方案。其 file fallback 仍是本地 JSON secret store，不满足本 feature 对普通配置不明文的安全预期；可在测试或特殊无桌面环境中显式使用替代 store。

## 决策 2：settings.json 保存凭据引用与状态，不保存完整 Key

**Decision**：`SavedModel` 中的 Key 持久化形态改为凭据引用/状态，例如 `credential_ref`、`credential_state` 或等价字段；旧字段 `api_key` 仅作为反序列化迁移入口，保存时不再输出完整明文。`advanced.custom_api_key` 同理作为旧配置迁移入口，保存时清空或不序列化完整值。

**Rationale**：规格要求普通用户配置文件不得出现完整明文 Key。保留非敏感模型配置有利于继续支持手工检查、导入、排障和 UI 展示；把敏感值移出普通配置可最小化改动面。

**Alternatives considered**：
- 从 settings 中完全删除 Key 相关字段：部分可行，但会破坏旧配置迁移和前端状态判断。
- 在 settings 中保存掩码字符串：可作为展示状态，但不能作为真实凭据来源，必须配合受保护凭据引用。

## 决策 3：旧明文 Key 自动迁移，迁移成功后立即脱敏保存

**Decision**：`UserPrefs::load()` 或其调用侧在发现旧配置中存在 `SavedModel.api_key` / `advanced.custom_api_key` 明文时，写入受保护凭据存储；写入成功后更新内存模型为凭据引用，并在下一次保存时移除明文。启动或获取设置时可主动保存一次迁移后的 settings，确保明文不会长期保留。

**Rationale**：已安装用户最可能存在历史明文配置。自动迁移可以避免要求用户重新配置，也能尽快降低落盘风险。迁移失败时必须保守处理：不删除旧 Key 导致不可用之前，需要提示用户重新配置或修复凭据存储，同时不得把 Key 输出到日志。

**Alternatives considered**：
- 只在用户打开设置页时迁移：会延长风险暴露窗口。
- 只在下一次保存设置时迁移：用户可能长期不打开设置，不能满足安全目标。

## 决策 4：前端默认只接收脱敏状态

**Decision**：`get_effective_model_config`、`list_models`、`get_settings` 返回到前端的数据不应包含完整 Key。前端设置页展示未配置、已配置、环境变量覆盖、读取失败、需重新配置等状态。用户保存新 Key 时，前端只在该次提交 payload 中携带明文，后端写入凭据存储后不再回传。

**Rationale**：即使 Key 不在 settings.json 明文存储，如果后端每次初始化都把完整 Key 传给 WebView 状态，也会增加调试、日志、截图和状态快照泄露风险。设置页只需要知道状态，不需要默认看到完整 Key。

**Alternatives considered**：
- 回传完整 Key 以便用户编辑：拒绝。违背默认隐藏完整 Key 的规格。
- 回传固定掩码并由用户覆盖：采用。用户需要替换时输入新 Key；保存空值应表示“不改变”还是“删除”必须由 UI 明确区分。

## 决策 5：环境变量覆盖保持最高优先级

**Decision**：`DEEPSEEK_API_KEY` 仍然覆盖 settings 和受保护凭据。环境变量来源只用于本次运行，不迁移、不写入凭据存储，也不出现在 settings。

**Rationale**：当前项目和 harness 已依赖环境变量覆盖能力。保留该优先级可以避免破坏开发、测试和临时部署路径，同时不扩大落盘风险。

**Alternatives considered**：
- 将环境变量自动写入凭据库：拒绝。环境变量常用于临时覆盖，自动持久化会违背用户预期。
- 让 settings 覆盖环境变量：拒绝。会破坏现有行为和测试假设。

## 参考资料

- `keyring` docs.rs：说明 v1 模式可在 macOS、Windows 和 *nix 平台读写 password/secret。<https://docs.rs/keyring>
- `keyring-rs` README：说明默认 v1 feature 支持 Mac、Windows、*nix 原生安全存储。<https://github.com/open-source-cooperative/keyring-rs>
- 当前依赖树：`cargo tree --manifest-path pinvou3-app\src-tauri\Cargo.toml -i keyring` 显示 `keyring v3.6.3` 已由 `codewhale-secrets` 引入。
