# Coding Plan 模型配置交互方案

更新时间：2026-07-24

## 背景

用户反馈：当前 Pinvou 的普通厂商 API 地址不能覆盖 Coding Plan 场景。以 GLM 为例：

- 普通 GLM API：`https://open.bigmodel.cn/api/paas/v4`
- GLM Coding Plan：`https://open.bigmodel.cn/api/coding/paas/v4`

这两者不是同一个 endpoint，不能让用户在普通 `GLM` 里手动改地址解决。Coding Plan 也不只是 GLM 有，腾讯云、Kimi 等厂商也存在 Coding Plan 类服务。

结论：Coding Plan 应作为云端模型下的一类服务分组，而不是某个厂商的特殊地址，也不应与“云端/本地”处在同一层。

## WorkBuddy 参考

WorkBuddy 的用户模型配置文件位于 `C:\Users\123\.workbuddy\models.json`，示例：

```json
[
  {
    "id": "glm-5",
    "name": "GLM-5",
    "vendor": "GLM Coding Plan",
    "url": "https://open.bigmodel.cn/api/coding/paas/v4",
    "apiKey": "...",
    "supportsToolCall": true,
    "supportsImages": false,
    "supportsReasoning": false
  },
  {
    "id": "tc-code-latest",
    "name": "通用TokenPlan / Auto",
    "vendor": "Tencent Cloud Token Plan",
    "url": "https://api.lkeap.cloud.tencent.com/plan/v3/chat/completions",
    "apiKey": "...",
    "supportsToolCall": true,
    "supportsImages": false,
    "supportsReasoning": true
  }
]
```

WorkBuddy 的添加模型 UI 中，`GLM Coding Plan` 是独立 provider：

- provider id：`glm-coding`
- 展示名：`GLM Coding Plan`
- endpoint：`https://open.bigmodel.cn/api/coding/paas/v4`
- endpoint alias：`https://open.bigmodel.cn/api/coding/paas/v4/chat/completions`
- endpoint 只读
- 模型输入模式：`selectOrInput`
- 高级配置隐藏

GLM Coding Plan 参考模型包括：

- `glm-4.5-air`
- `glm-4.7`
- `glm-5-turbo`
- `glm-5.1`
- `glm-5v-turbo`

## 信息架构

添加模型弹窗顶层只保留两个部署位置：

1. `云端模型`
2. `本地模型`

`Coding Plan` 放在 `云端模型` 内部，作为 section，而不是第二层 tab。

云端模型列表按 section 展示：

1. `Coding Plan`
   - `GLM Coding Plan`
   - `腾讯云 Coding Plan`
   - `Kimi Coding Plan`
2. `官方 API`
   - `DeepSeek`
   - `GLM API`
   - `Kimi API`
   - `通义千问`
   - `豆包`
   - `MiniMax`
   - `Mimo`
3. `自定义兼容接口`
   - `OpenAI Compatible`

不要增加“推荐”分组。`推荐` 含义不稳定，容易和服务类型混淆。

## 当前落地决策

截至 2026-07-24，本分支采用以下交互：

- Coding Plan 服务地址由应用内置维护，用户侧不展示、不编辑。
- 新增 Coding Plan 时不显示“显示名称”，默认名称随所选模型生成。
- 新增本地模型时不显示“显示名”；编辑已有本地模型时保留“显示名”，用于用户改别名。
- 模型下拉使用 iOS 分组列表展开在当前表单内，只展示模型标题、说明和勾选状态。
- 测试连接使用隐藏的 `base_url` 发起，失败文案面向用户解释问题；HTTP 状态等技术细节保留在结构化结果里，不在普通 UI 展示。

## 弹窗交互

### 第一屏：添加模型

沿用当前 `添加模型` 模态，不新增外层弹窗。

顶部 segmented control：

- `云端模型`
- `本地模型`

默认保持当前行为，停在 `云端模型`。

`云端模型` 页使用 iOS 风格分组列表：

- section title：`Coding Plan`
- section title：`官方 API`
- section title：`自定义兼容接口`

每一行/卡片展示：

- 厂商图标
- 名称，例如 `GLM Coding Plan`
- 一行简短描述，例如 `编码与 Agent 场景专用接口`
- 右侧 `+` 或进入箭头

不在列表卡片上展示长 URL。

### 第二屏：配置模型

点击 `GLM Coding Plan` 等 provider 后，不开新 modal，而是在同一个 modal 内切换到配置页。

顶部：

- 左上返回按钮，返回 provider 列表
- 标题：`添加 GLM Coding Plan`
- 副标题：`Coding Plan · 工具调用`

表单使用现有 iOS 分组样式，不做嵌套卡片。

第一组：模型

- `模型`
  - 下拉 + 自定义输入。
  - 推荐模型排前面。
  - 最后一项为 `自定义模型 ID`。

第二组：认证

- `API Key`
  - 新增时必填。
  - 编辑时默认显示“已配置”，不回显明文。
  - 编辑时只有用户选择替换 Key 才写入新 key。

第三组：服务

- `连接测试`
  - 可选按钮。
  - 测试内置服务地址 + 当前模型 + API Key。
  - 测试失败不强制阻止保存，但显示风险提示。
  - 不提示用户修改 Coding Plan 服务地址，因为地址不暴露给用户维护。

底部按钮：

- `取消`
- `保存`

保存禁用条件：

- 模型 ID 为空。
- 新增模型时 API Key 为空。
- 编辑已有模型且无已保存 key、也未输入新 key。

### 编辑已有 Coding Plan 模型

编辑时进入同一个配置页。

- provider 不可切换。
- endpoint 固定且不在表单展示。
- model 可从推荐项切换，也可自定义输入。
- key 默认 keep existing。
- 保存后回到模型列表。

## 数据模型建议

当前 Pinvou 的 `SavedModel` 只有：

- `preset`
- `model`
- `base_url`
- `api_key`
- route limit / credential 字段

为了兼容 Coding Plan 多厂商，建议不要给每个 Coding Plan 厂商都新增一个枚举 preset，否则后续会膨胀。

更推荐的长期结构：

- `preset: openai_compatible` 或新增通用 `coding_plan`
- 新增可选字段：
  - `provider_kind: "official_api" | "coding_plan" | "custom"`
  - `vendor: "glm" | "tencent" | "kimi" | ...`
  - `capabilities`

但为了本分支快速落地、降低 Rust 枚举迁移风险，可以第一版采用保守做法：

1. 在前端 catalog 中把 Coding Plan provider 当作预置模板。
2. 保存为 `openai_compatible`。
3. `name` / `base_url` / `model` / route limits 写入具体值。
4. 增加最小元数据字段前，先通过 `base_url` 判断是否为 Coding Plan，用于 UI 展示和迁移。

如果要在本轮做结构化能力，建议新增字段而不是只靠 URL：

```rust
pub provider_kind: Option<ModelProviderKind>,
pub vendor: Option<String>,
```

其中：

```rust
enum ModelProviderKind {
    OfficialApi,
    CodingPlan,
    Custom,
}
```

## Provider 配置表

建议前端维护一个云端 provider catalog，字段包括：

```js
{
  key: 'glm_coding_plan',
  section: 'coding_plan',
  label: 'GLM Coding Plan',
  vendor: 'glm',
  providerKind: 'coding_plan',
  preset: 'openai_compatible',
  baseUrl: 'https://open.bigmodel.cn/api/coding/paas/v4',
  endpointAliases: [
    'https://open.bigmodel.cn/api/coding/paas/v4/chat/completions'
  ],
  defaultModel: 'glm-5-turbo',
  models: [
    { model: 'glm-5-turbo', title: 'GLM-5-Turbo' },
    { model: 'glm-5.1', title: 'GLM-5.1' },
    { model: 'glm-4.7', title: 'GLM-4.7' },
    { model: 'glm-4.5-air', title: 'GLM-4.5-Air' },
    { model: 'glm-5v-turbo', title: 'GLM-5V-Turbo' }
  ],
  supportsToolCall: true,
  supportsImages: false,
  supportsReasoning: true
}
```

腾讯云 Coding Plan 需要区分用户当前说到的两类地址：

- Coding Plan：`https://api.lkeap.cloud.tencent.com/coding/v3/chat/completions`
- Token Plan：`https://api.lkeap.cloud.tencent.com/plan/v3/chat/completions`

本轮如果只解决 Coding Plan，先放 `Tencent Cloud Coding Plan`；Token Plan 可后续独立加入，不要混名。

## 地址归一化

对 Coding Plan provider 保存时做归一化：

- 接受完整 `/chat/completions` URL。
- 保存到 `base_url` 时统一去掉 OpenAI chat completions 后缀，除非该服务实际 endpoint 就要求完整 chat path。

GLM：

- 输入 `https://open.bigmodel.cn/api/coding/paas/v4/chat/completions`
- 保存 `https://open.bigmodel.cn/api/coding/paas/v4`

Kimi：

- 输入 `https://api.kimi.com/coding/v1/chat/completions`
- 保存 `https://api.kimi.com/coding/v1`

腾讯云：

- 如果后端请求拼接逻辑会自动追加 `/chat/completions`，则保存 base。
- 如果腾讯 endpoint 只有完整 path 可用，则 provider 配置要显式标记 `endpointMode: full_chat_completions`，避免重复拼接。

实现前必须确认当前 Rust bridge 对 `base_url` 的拼接规则。

## 迁移与纠错

### 自动识别

如果已有模型满足以下条件，视为 Coding Plan：

- `base_url` 命中已知 Coding Plan endpoint 或 alias。
- 或 `name`/历史字段中明确含 `Coding Plan`，且 endpoint 命中对应厂商。

识别后：

- UI 列表显示为 `Coding Plan`。
- 编辑时进入 Coding Plan 配置页。
- 若新增了 `provider_kind/vendor` 字段，则保存时补齐。

### 保存纠错

如果用户在普通官方 API 表单中输入了 Coding Plan 地址：

- 不要静默保存为普通 API。
- 提示：`该地址属于 Coding Plan 服务，已切换到对应类型。`
- 保存为 Coding Plan 配置。

如果用户在 Coding Plan 表单中粘贴普通 API 地址：

- 阻止保存或提示切换到普通 API。
- 不允许 Coding Plan provider 的 endpoint 被改成普通 API endpoint。

## 展示规则

模型列表中：

- 主标题：显示名称，例如 `GLM-5-Turbo`
- 副标题：`GLM Coding Plan · glm-5-turbo`
- 标签：
  - `云端`
  - `Coding Plan`
  - `工具调用`

普通官方 API：

- 副标题：`GLM API · glm-5.2`
- 标签：`云端`

不要在模型列表直接展示完整 URL。

## 测试连接结果设计

后端 `test_model_connection` 返回结构化结果，前端兼容旧字符串返回。普通 UI 只展示 `message`，`detail` 只保留给日志/调试，不直接展示给用户：

```json
{
  "ok": false,
  "code": "auth_invalid",
  "message": "API Key 无效，请检查后重新填写",
  "detail": "HTTP 401",
  "http_status": 401
}
```

用户可见文案按以下规则：

| 场景 | code | 用户文案 | detail 字段 |
| --- | --- | --- | --- |
| 2xx | `ok` | `连接成功，服务可用` | `HTTP xxx`，UI 不展示 |
| 400 / 422 | `request_invalid` | `请求格式不被服务接受，请检查模型配置` | `HTTP xxx` |
| 401 | `auth_invalid` | `API Key 无效，请检查后重新填写` | `HTTP 401` |
| 403 | `auth_forbidden` | `当前 API Key 没有访问权限` | `HTTP 403` |
| 404 | `endpoint_not_found` | 自定义/本地：`服务地址不正确，或该服务不支持模型列表接口`；Coding Plan：`当前厂商接口暂时无法完成测试，但不影响保存配置` | `HTTP 404` |
| 405 | `method_not_allowed` | 自定义/本地：`服务可以访问，但不支持当前测试方式`；Coding Plan：`当前厂商接口暂时无法完成测试，但不影响保存配置` | `HTTP 405` |
| 429 | `rate_limited` | `请求过于频繁或额度不足，请稍后再试` | `HTTP 429` |
| 5xx | `server_unavailable` | `服务暂时不可用，请稍后再试` | `HTTP xxx` |
| 超时 | `timeout` | `连接超时，请检查网络或本地服务是否启动` | 原始错误 |
| 本机拒绝连接 | `connection_refused` | `无法连接到服务，请确认本地模型服务已启动` | 原始错误 |
| DNS 失败 | `dns_failed` | `无法解析服务地址，请检查网络` | 原始错误 |
| TLS/证书失败 | `tls_error` | `安全证书校验失败，请检查代理或网络环境` | 原始错误 |
| URL 格式错误 | `invalid_url` | `服务地址格式不正确` | 原始错误 |
| 其他网络错误 | `network_error` | `网络连接失败，请检查网络后重试` | 原始错误 |

Coding Plan 地址是官方固定 endpoint，正常情况下不需要用户编辑。若厂商后续变更地址，应通过应用更新维护 provider catalog；不要在默认配置页暴露地址输入框。

## 需要改的模块

预计影响：

- `pinvou3-app/src/features/settings/SettingsView.jsx`
  - 云端模型目录分组。
  - 添加模型第一屏 provider list。
  - Coding Plan 配置页。
  - 保存/编辑时的 provider 识别。
- `pinvou3-app/src/shared/i18n.js`
  - 新增文案。
- `pinvou3-app/src-tauri/src/platform/prefs.rs`
  - 如采用结构化字段，需要扩展 `SavedModel` 和迁移。
  - 如果第一版只靠 `base_url`，这里可少改或不改。
- `pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs`
  - 确认不同 endpoint 拼接规则。
  - 如腾讯云 Coding Plan 需要完整 URL，需做 route 特判。
- 测试：
  - `pinvou3-app/tests/settings_ui_smoke.js`
  - Rust prefs / bridge 单测。

## 验证清单

1. 添加模型弹窗：
   - 顶层只有 `云端模型` / `本地模型`。
   - 云端页有 `Coding Plan` / `官方 API` / `自定义兼容接口` 三个 section。
2. 选择 `GLM Coding Plan`：
   - 表单不展示服务地址。
   - 模型可下拉也可自定义输入。
   - 新增时 API Key 为空不能保存。
3. 保存后：
   - 模型列表显示 `Coding Plan` 标签。
   - 副标题不是普通 `GLM API`。
4. 普通 `GLM API`：
   - 地址仍为 `https://open.bigmodel.cn/api/paas/v4`。
   - 不被 Coding Plan 逻辑误改。
5. 编辑已有 Coding Plan：
   - endpoint 不展示、不可改。
   - Key 可 keep existing。
6. 测试连接：
   - HTTP 401 显示 `API Key 无效，请检查后重新填写`。
   - UI 不展示 `HTTP 401` 等技术细节。
   - Coding Plan 404/405 不提示用户修改服务地址。
7. 回归：
   - `npm run lint:ui -- --quiet`
   - `npm run build:ui`
   - `npm run test:settings-ui`

## 自动化测试方案

仅靠 UI 快照或保存 payload 不够，必须覆盖“用户能配置 Coding Plan，并且聊天实际使用该模型”的链路。建议分四层做自动化测试。

### 1. 设置页 UI smoke

扩展 `pinvou3-app/tests/settings_ui_smoke.js`。

覆盖场景：

1. 打开 `添加模型`。
2. 默认进入 `云端模型`。
3. 云端列表存在三个 section：
   - `Coding Plan`
   - `官方 API`
   - `自定义兼容接口`
4. `Coding Plan` section 下至少存在：
   - `GLM Coding Plan`
   - `腾讯云 Coding Plan`
   - `Kimi Coding Plan`
5. 点击 `GLM Coding Plan` 后进入配置页：
   - 标题为 `添加 GLM Coding Plan`。
   - 不展示服务地址字段。
   - 模型控件默认有推荐项，并允许自定义模型 ID。
   - 新增时 API Key 为空，保存按钮禁用。
6. 输入 API Key，选择/输入模型后保存。
7. 断言 `saveModel` mock 收到的 payload：
   - `provider_kind` 或等价标识为 `coding_plan`。
   - `vendor` 为 `glm`。
   - `preset` 按最终实现约定。
   - `base_url` 为 `https://open.bigmodel.cn/api/coding/paas/v4`。
   - `model` 为用户选择/输入值。
   - `credential_action` 为 `replace`。
8. 保存后模型列表显示：
   - `Coding Plan` 标签。
   - 副标题类似 `GLM Coding Plan · <model>`。
   - 不显示普通 `GLM API`。

编辑场景：

1. 用已有 Coding Plan 模型打开编辑。
2. endpoint 不展示、不可编辑。
3. API Key 显示“已配置”状态，不回显明文。
4. 不输入新 Key 保存时，payload 使用 `credential_action: keep_existing`。

连接测试场景：

1. mock `test_model_connection` 返回 `{ ok: false, code: "auth_invalid", message: "API Key 无效，请检查后重新填写", detail: "HTTP 401" }`。
2. 点击 `测试连接` 后，UI 显示用户可理解文案。
3. UI 不展示 `HTTP 401` 等技术详情。
4. Coding Plan 表单仍不展示服务地址字段。

纠错场景：

1. 普通 `GLM API` 不被误归类为 Coding Plan。
2. 如果保存数据里已有 Coding Plan endpoint，UI 应进入 Coding Plan 编辑页，而不是普通 GLM 编辑页。

### 2. prefs / settings 命令测试

扩展 Rust 测试，位置优先放在：

- `pinvou3-app/src-tauri/src/platform/prefs.rs`
- `pinvou3-app/src-tauri/src/app/commands/settings.rs`

覆盖场景：

1. `SavedModel` 能保存 Coding Plan 元数据。
2. 读取旧配置时能识别 Coding Plan endpoint：
   - `https://open.bigmodel.cn/api/coding/paas/v4`
   - `https://open.bigmodel.cn/api/coding/paas/v4/chat/completions`
3. 归一化后保存为稳定 base URL。
4. 普通 GLM API：
   - `https://open.bigmodel.cn/api/paas/v4`
   - 不被迁移成 Coding Plan。
5. `set_active_model` 指向 Coding Plan 模型后，`get_effective_model_config` 返回：
   - 正确 `model`。
   - 正确 `base_url`。
   - 正确 credential 状态。
   - 正确 provider kind/vendor 信息。

### 3. 请求路由单测

扩展 `pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs` 或相关请求构造测试。

必须覆盖：

1. GLM Coding Plan base URL：
   - 输入 `https://open.bigmodel.cn/api/coding/paas/v4`
   - 请求应发到 `https://open.bigmodel.cn/api/coding/paas/v4/chat/completions`
   - 不重复拼成 `/chat/completions/chat/completions`。
2. GLM Coding Plan alias：
   - 输入 `https://open.bigmodel.cn/api/coding/paas/v4/chat/completions`
   - 请求仍只发到该完整 chat completions URL。
3. 腾讯云 Coding Plan 如果采用完整 endpoint：
   - 保存和请求构造要明确是否追加 `/chat/completions`。
   - 测试必须覆盖“不重复拼接”和“不漏拼接”。
4. 请求体字段：
   - `model` 使用用户选择的 Coding Plan model id。
   - `Authorization` 使用用户保存的 API Key。
   - 工具调用字段按 provider 能力保留。
   - reasoning/thinking 字段按 provider 能力处理，不把普通 GLM API 的特殊逻辑误套到所有 Coding Plan。

### 4. 端到端 mock chat 测试

新增或扩展一个 UI/集成 smoke，使用本地 mock OpenAI-compatible server，目标是验证“用户配置后确实能用该模型聊天”。

建议流程：

1. 测试启动一个本地 HTTP mock server。
2. 在测试 settings 中创建一个 Coding Plan 模型：
   - `provider_kind: coding_plan`
   - `vendor: glm`
   - `base_url: http://127.0.0.1:<port>/api/coding/paas/v4`
   - `model: glm-5-turbo`
   - 凭据使用测试 key。
3. 调用 `set_active_model` 激活该模型。
4. 在聊天页发送一句测试消息。
5. mock server 断言收到：
   - `POST /api/coding/paas/v4/chat/completions`
   - `Authorization: Bearer <test-key>`
   - body 中 `model: "glm-5-turbo"`
   - messages 存在用户输入。
6. mock server 返回 OpenAI-compatible 响应。
7. UI 断言助手消息正常出现。

这层测试可以先只覆盖 GLM Coding Plan。腾讯云/Kimi 的路径差异用请求路由单测覆盖，避免 e2e 太重。

### 自动化验收标准

实现完成后，至少执行：

```powershell
cd C:\Users\123\pinvou3-verify-local-model-sidebar\pinvou3-app
npm run lint:ui -- --quiet
npm run build:ui
npm run test:settings-ui
cargo test -p pinvou3-tauri prefs --lib
cargo test -p pinvou3-tauri get_effective_model_config --lib
cargo test -p pinvou3-tauri coding_plan --lib
```

如果新增 mock chat smoke，则把对应 npm 命令也加入本清单。没有新增前，必须至少用 Rust 请求路由单测证明实际请求 URL 和 body 正确。

## 当前分支状态提示

正确工作区：

```text
C:\Users\123\pinvou3-verify-local-model-sidebar
```

正确分支：

```text
work/model-config-optimization-20260724
```

当前已有未提交改动：

- `pinvou3-app/src/features/settings/SettingsView.jsx`
  - Coding Plan 配置、连接测试文案、本地模型删除、Qwen 本地图标等设置页改动。
- `pinvou3-app/src-tauri/src/app/commands/settings.rs`
  - `test_model_connection` 结构化返回和友好错误分类。
- `pinvou3-app/tests/settings_ui_smoke.js`
  - 设置页 smoke 回归和 Coding Plan 连接测试断言。
- `docs/CodingPlan模型配置交互方案.md`
  - 本文档更新。

继续接手时，先看 `git status` 确认这些改动是否已提交；如未提交，先跑验证再提交。
