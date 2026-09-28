// ACP Provider（第三方中转）预设目录。数据来自 cc-switch 公开预设（MIT）的裁剪，
// 只保留各家官方/主流兼容端点；Rust 侧保持 schema 无关，切换时以用户填写的
// base_url 为准。
//
// The model lists were checked against each vendor's official access docs
// on 2026-09-11 and re-verified on 2026-09-28 (platform.claude.com /
// developers.openai.com / api-docs.deepseek.com / platform.kimi.com /
// docs.bigmodel.cn / help.aliyun.com(model-studio) / platform.minimax.io /
// docs.x.ai). Presets only provide the base URL and protocol; the model is
// filled in by the user (see ProviderFormModal: picking a preset does not
// auto-fill the model). Lists are append-only: relay stations may
// still proxy older models, and tests/acp_providers_ui_smoke.js depends on
// the existing entries.

// wireApi: 'anthropic'（Anthropic 兼容）/ 'openai'（OpenAI 兼容）/ 'kimi'（Kimi 原生）
// models: 该厂商官方在列模型名单（选择预设后模型建议列表按此筛选；
// 空数组表示无固定名单，如豆包按接入点（endpoint）区分，由用户自行填写）。
// models1m: Claude Code 专属 1M 上下文变体（小写 [1m] 后缀，CC 需要显式声明；
// Codex/Kimi 不需要）。仅部分模型支持，按厂商归属挂载，仅 claude 表单展示。
// baseUrlAnthropic: Anthropic 协议专用端点（可选）。Anthropic 客户端会把请求
// 拼成 {baseUrl}/v1/messages，因此该值**不含 /v1 尾**（N9）：Kimi Code 托管
// 为 api.kimi.com/coding（拼出 /coding/v1/messages），DeepSeek 为
// api.deepseek.com/anthropic（官方 Anthropic 兼容端点）。
// nameKey: i18n 键（`uiAcpProviders` 下），展示名按当前语言查表；`name` 仅作
// 缺失 key 时的兜底回退，不直接渲染。
export const ACP_PROVIDER_PRESETS = [
  // claude-opus-5-5 (2026-09-22) prepended as the official default
  // recommendation ("start with Claude Opus 5.5 for most workloads");
  // append-only below it.
  { key: 'anthropic', nameKey: 'presetAnthropic', name: 'Anthropic 官方', baseUrl: 'https://api.anthropic.com', baseUrlAnthropic: 'https://api.anthropic.com', wireApi: 'anthropic', models: ['claude-opus-5-5', 'claude-fable-5-1', 'claude-fable-5', 'claude-opus-5', 'claude-sonnet-5', 'claude-haiku-4-5'] },
  // gpt-6-sol / gpt-6-luna (GPT-6 family expansion, September 2026) appended;
  // gpt-5.3-codex stays valid for Responses-only ACP hosts.
  { key: 'openai', nameKey: 'presetOpenai', name: 'OpenAI 官方', baseUrl: 'https://api.openai.com/v1', wireApi: 'openai', models: ['gpt-6-astra', 'gpt-6-sol', 'gpt-6-luna', 'gpt-5.3-codex', 'gpt-5.6-sol', 'gpt-5.6-terra', 'gpt-5.6-luna', 'gpt-5.5', 'gpt-5.4', 'gpt-5.2'] },
  { key: 'moonshot', nameKey: 'presetMoonshot', name: 'Moonshot Kimi', baseUrl: 'https://api.moonshot.cn/v1', baseUrlAnthropic: 'https://api.moonshot.cn/anthropic', wireApi: 'kimi', models: ['kimi-k3', 'kimi-k2.7-code', 'kimi-k2.6', 'kimi-k2.7-code-highspeed'], models1m: ['kimi-k3[1m]'] },
  // 注意：models 为**发送给 API 的模型 ID**（官方配置中 models."kimi-code/k3"
  // 表名的 kimi-code/ 前缀是别名，实际请求的 model 字段是无前缀的 "k3"）。
  // Kimi Code 托管的 1M 变体仅 k3 一个（k3-256k 等其余档位无 1M 声明；
  // kimi-for-coding 现服务 K2.8 Preview 原生 1M，但无 [1m] 变体声明，不得虚构）。
  { key: 'kimi-code', nameKey: 'presetKimiCode', name: 'Kimi Code', baseUrl: 'https://api.kimi.com/coding/v1', baseUrlAnthropic: 'https://api.kimi.com/coding', wireApi: 'kimi', models: ['k3', 'k3-256k', 'kimi-for-coding', 'kimi-for-coding-highspeed'], models1m: ['k3[1m]'] },
  // models1m only carries officially documented [1m] declarations: the
  // official Claude Code preset page (api-docs.deepseek.com) names just
  // deepseek-flash[1m]; the v4-flash/v4-pro [1m] entries previously listed
  // here were undocumented and have been removed (2026-09-28).
  { key: 'deepseek', nameKey: 'presetDeepseek', name: 'DeepSeek', baseUrl: 'https://api.deepseek.com', baseUrlAnthropic: 'https://api.deepseek.com/anthropic', wireApi: 'openai', models: ['deepseek-flash', 'deepseek-v4-flash', 'deepseek-v4-pro'], models1m: ['deepseek-flash[1m]'] },
  // glm-5.3-flashx appended (2026-09-28): multimodal speed tier, standard
  // API only (not open on the Coding Plan endpoint).
  { key: 'zhipu', nameKey: 'presetZhipu', name: '智谱 GLM', baseUrl: 'https://open.bigmodel.cn/api/paas/v4', wireApi: 'openai', models: ['glm-5.3', 'glm-5.3-flash', 'glm-5.3-flashx', 'glm-5.2', 'glm-5.1', 'glm-5', 'glm-4.7', 'glm-4.6', 'glm-4.7-flash'] },
  { key: 'qwen', nameKey: 'presetQwen', name: '通义千问 Qwen', baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', wireApi: 'openai', models: ['qwen3.8-max', 'qwen3.8-flash', 'qwen3.7-max', 'qwen3.7-plus', 'qwen3.7-flash', 'qwen3-coder-plus'] },
  { key: 'doubao', nameKey: 'presetDoubao', name: '豆包 Doubao', baseUrl: 'https://ark.cn-beijing.volces.com/api/v3', wireApi: 'openai', models: [] },
  { key: 'minimax', nameKey: 'presetMinimax', name: 'MiniMax', baseUrl: 'https://api.minimaxi.com/v1', wireApi: 'openai', models: ['MiniMax-M3', 'MiniMax-M2.7', 'MiniMax-M2.7-highspeed'] },
  // grok-4.20-reasoning is a legacy spelling kept for existing configs
  // (append-only); the official wire id is the -0309- dated
  // spelling (docs.x.ai models page, 2026-09-11), added alongside under the
  // same policy. grok-4.7 appended (2026-09-28): the official
  // coding/agent-recommended flagship.
  { key: 'xai', nameKey: 'presetXai', name: 'xAI Grok', baseUrl: 'https://api.x.ai/v1', wireApi: 'openai', models: ['grok-4.7', 'grok-4.6', 'grok-4.5', 'grok-4.3', 'grok-4.20-reasoning', 'grok-4.20-0309-reasoning', 'grok-4.20-0309-non-reasoning', 'grok-build-0.1'] },
  { key: 'openrouter', nameKey: 'presetOpenrouter', name: 'OpenRouter', baseUrl: 'https://openrouter.ai/api/v1', wireApi: 'openai', models: [] },
  { key: 'siliconflow', nameKey: 'presetSiliconflow', name: '硅基流动 SiliconFlow', baseUrl: 'https://api.siliconflow.cn/v1', wireApi: 'openai', models: [] },
  { key: 'groq', nameKey: 'presetGroq', name: 'Groq', baseUrl: 'https://api.groq.com/openai/v1', wireApi: 'openai', models: [] },
];

// Official on-list model suggestions for the form's model field (datalist;
// other model names can be typed freely).
// As listed in each vendor's official docs as of 2026-09-28 (new tiers are
// added at the top of their vendor group):
export const ACP_MODEL_PRESETS = [
  // Anthropic (platform.claude.com; opus-5-5 (2026-09-22) is the official
  // default recommendation, fable-5-1 the highest-capability tier)
  'claude-opus-5-5', 'claude-fable-5-1', 'claude-fable-5', 'claude-opus-5', 'claude-sonnet-5', 'claude-haiku-4-5',
  // OpenAI (developers.openai.com; the GPT-6 family expanded with
  // gpt-6-sol / gpt-6-luna; gpt-5.3-codex is the current Codex coding model,
  // Responses-only)
  'gpt-6-sol', 'gpt-6-luna', 'gpt-6-astra', 'gpt-5.3-codex', 'gpt-5.6-sol', 'gpt-5.6-terra', 'gpt-5.6-luna', 'gpt-5.5', 'gpt-5.4', 'gpt-5.2',
  // DeepSeek (deepseek-flash = V4.1-Flash is the current mainline; per the
  // official 2026-09-10 confirmation, v4-pro keeps serving past 09-14 with
  // unchanged billing; the deepseek-chat/reasoner old aliases are retired)
  'deepseek-flash', 'deepseek-v4-flash', 'deepseek-v4-pro',
  // Moonshot open platform (platform.kimi.com; kimi-k2.5 and the moonshot-v1
  // series were fully retired on 2026-08-31; kimi-k3 is the flagship pick,
  // kimi-k2.7-code targets coding)
  'kimi-k3', 'kimi-k2.7-code', 'kimi-k2.6', 'kimi-k2.7-code-highspeed',
  // Zhipu GLM (docs.bigmodel.cn; glm-5.3 is the current flagship,
  // glm-5.3-flashx the multimodal speed tier)
  'glm-5.3', 'glm-5.3-flash', 'glm-5.3-flashx', 'glm-5.2', 'glm-5.1', 'glm-5', 'glm-4.7', 'glm-4.6', 'glm-4.7-flash',
  // Qwen (Alibaba Cloud Model Studio; the qwen3.8 family is the current
  // flagship/fast mainline)
  'qwen3.8-max', 'qwen3.8-flash', 'qwen3.7-max', 'qwen3.7-plus', 'qwen3.7-flash', 'qwen3-coder-plus',
  // MiniMax（M3 为最新旗舰，M2.7 现役；M2.5/M2.1/M2 已归历史模型；
  // M3.1-Flash-Preview 暂仅订阅渠道提供，不列入 PAYG 预设建议）
  'MiniMax-M3', 'MiniMax-M2.7', 'MiniMax-M2.7-highspeed',
  // xAI (grok-4.7 is the official coding/agent recommended flagship since
  // September 2026; -0309- is the official wire id
  // spelling, and grok-4.20-reasoning is the legacy spelling, append-only)
  'grok-4.7', 'grok-4.6', 'grok-4.5', 'grok-4.3', 'grok-4.20-reasoning', 'grok-4.20-0309-reasoning', 'grok-4.20-0309-non-reasoning', 'grok-build-0.1',
];

// Claude Code 专属：1M 上下文变体汇总（小写 [1m] 后缀，Codex/Kimi 不需要显式
// 声明）。数据按厂商归属在各预设的 models1m 字段；此处汇总是为「其它」
// （自定义）选项提供全量建议。
export const ACP_MODEL_1M_VARIANTS = ACP_PROVIDER_PRESETS.flatMap(preset => preset.models1m || []);

// Claude Code 细化模型槽位 id（与后端 CLAUDE_MODEL_SLOTS 一一对应）。
// 槽位不填时 CC 的子 agent 会回落官方模型走官方流量，表单将其设为必填。
export const CLAUDE_MODEL_SLOT_IDS = ['opus', 'sonnet', 'haiku', 'fable', 'subagent'];
