// 「添加模型」云端/本地模型目录与预设模板（自 SettingsView.jsx 抽离）。
// 纯数据 + 纯函数：不含组件、不依赖 React；品牌图标映射随目录一并归位。
// 目录条目的 en/ja 文案 overlay 由 ./settings-i18n.js 在模块初始化时挂到 shared/i18n.js 的 dict。
import './settings-i18n.js';
import deepseekIcon from '../../brand-icons/deepseek.svg';
import doubaoIcon from '../../brand-icons/doubao.svg';
import claudeIcon from '../../brand-icons/claude.png';
import geminiIcon from '../../brand-icons/gemini.svg';
import glmIcon from '../../brand-icons/glm.svg';
import kimiIcon from '../../brand-icons/kimi.svg';
import mimoIcon from '../../brand-icons/mimo.svg';
import minimaxIcon from '../../brand-icons/minimax.svg';
import openaiIcon from '../../brand-icons/openai.svg';
import qwenIcon from '../../brand-icons/qwen.svg';
import tencentCloudIcon from '../../brand-icons/tencentcloud.svg';
import xaiIcon from '../../brand-icons/xai.svg';

// ── 「添加模型」方案:模型快切 chip + 添加/编辑弹窗 ─────────────────
// 各预设默认 baseUrl/model 模板(与 bridge/prefs.rs 对齐),添加模型时自动填充。
// openai_compatible 为纯自定义模板,前端刻意不留默认地址/模型,Rust 侧的
// OpenAI 默认值仅服务 legacy 迁移兜底。
const MODEL_PRESET_DEFS = {
  local_vllm:  { baseUrl: 'http://127.0.0.1:8000/v1',                model: 'qwen36_35b_256k' },
  deepseek:    { baseUrl: 'https://api.deepseek.com',                model: 'deepseek-v4-pro' },
  kimi:        { baseUrl: 'https://api.moonshot.cn/v1',              model: 'kimi-k3' },
  // 自定义兼容接口:地址与模型完全由用户填写,不再预填 OpenAI 官方样板。
  openai_compatible: { baseUrl: '',                                 model: '' },
  qwen:        { baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', model: 'qwen3.8-max' },
  doubao:      { baseUrl: 'https://ark.cn-beijing.volces.com/api/v3', model: 'doubao-seed-evolving' },
  minimax:     { baseUrl: 'https://api.minimaxi.com/v1',            model: 'MiniMax-M3' },
  glm:         { baseUrl: 'https://open.bigmodel.cn/api/paas/v4',   model: 'glm-5.2' },
  mimo:        { baseUrl: 'https://api.xiaomimimo.com/v1',          model: 'mimo-v2.5-pro' },
  openai:      { baseUrl: 'https://api.openai.com/v1',              model: 'gpt-5.6-terra' },
  anthropic:   { baseUrl: 'https://api.anthropic.com/v1',           model: 'claude-sonnet-5' },
  gemini:      { baseUrl: 'https://generativelanguage.googleapis.com/v1beta/openai', model: 'gemini-3.6-flash' },
  xai:         { baseUrl: 'https://api.x.ai/v1',                    model: 'grok-4.3' },
};
const PROVIDER_KIND_CODING_PLAN = 'coding_plan';
const PROVIDER_KIND_OFFICIAL_API = 'official_api';
const PROVIDER_KIND_CUSTOM = 'custom';
const MODEL_CATALOG_SECTIONS = {
  coding_plan: 'Coding Plan',
  official_api: '官方 API',
  custom: '自定义兼容接口',
};
function presetOptionsI18n(t) {
  return [
    { key: 'local_vllm', label: t.modelPresetLocalVllm },
    { key: 'deepseek', label: t.modelPresetDeepseek },
    { key: 'kimi', label: t.modelPresetKimi },
    { key: 'openai_compatible', label: t.modelPresetOpenaiCompatible },
    { key: 'qwen', label: t.modelPresetQwen },
    { key: 'doubao', label: t.modelPresetDoubao },
    { key: 'minimax', label: t.modelPresetMinimax },
    { key: 'glm', label: t.modelPresetGlm },
    { key: 'mimo', label: t.modelPresetMimo },
    { key: 'openai', label: t.modelPresetOpenai },
    { key: 'anthropic', label: t.modelPresetAnthropic },
    { key: 'gemini', label: t.modelPresetGemini },
    { key: 'xai', label: t.modelPresetXai },
  ];
}
function presetProviderLabel(preset, t) {
  const m = {};
  presetOptionsI18n(t).forEach(o => { m[o.key] = o.label; });
  return m[preset] || preset;
}

const BRAND_ICON_BY_PRESET = {
  deepseek: deepseekIcon,
  kimi: kimiIcon,
  glm: glmIcon,
  qwen: qwenIcon,
  doubao: doubaoIcon,
  minimax: minimaxIcon,
  mimo: mimoIcon,
  openai: openaiIcon,
  openai_compatible: openaiIcon,
  anthropic: claudeIcon,
  gemini: geminiIcon,
  xai: xaiIcon,
};
const BRAND_ICON_BY_VENDOR = {
  glm: glmIcon,
  kimi: kimiIcon,
  deepseek: deepseekIcon,
  qwen: qwenIcon,
  doubao: doubaoIcon,
  minimax: minimaxIcon,
  mimo: mimoIcon,
  openai: openaiIcon,
  anthropic: claudeIcon,
  gemini: geminiIcon,
  xai: xaiIcon,
  tencent: tencentCloudIcon,
};

const MODEL_CATALOG = {
  local: [
    {
      key: 'local',
      title: '本地模型',
      preset: 'local_vllm',
      items: [
        { model: 'qwen36_35b_256k', title: 'qwen36_35b_256k', desc: '本地服务默认模型' },
        { model: '', title: '自定义本地模型', desc: '填写本地服务暴露的模型 ID', custom: true },
      ],
    },
  ],
  cloud: [
    {
      key: 'glm_coding_plan',
      section: 'coding_plan',
      title: '智谱 Coding Plan / GLM Coding Plan',
      configTitle: '智谱 Coding Plan',
      desc: '智谱编码与 Agent 场景专用接口',
      preset: 'openai_compatible',
      providerKind: PROVIDER_KIND_CODING_PLAN,
      vendor: 'glm',
      baseUrl: 'https://open.bigmodel.cn/api/coding/paas/v4',
      endpointAliases: ['https://open.bigmodel.cn/api/coding/paas/v4/chat/completions'],
      items: [
        { model: 'glm-5.2', title: 'GLM-5.2', desc: '旗舰编码模型' },
        { model: 'glm-5-turbo', title: 'GLM-5-Turbo', desc: '高性能编码模型' },
        { model: 'glm-4.7', title: 'GLM-4.7', desc: '日常编码模型' },
        { model: '', title: '自定义 GLM Coding Plan 模型', desc: '手动填写 Coding Plan 模型 ID', custom: true },
      ],
    },
    {
      key: 'glm_coding_plan_global',
      section: 'coding_plan',
      title: '智谱 Coding Plan 国际版 / GLM Coding Plan Global',
      configTitle: '智谱 Coding Plan 国际版',
      desc: 'z.ai 编码与 Agent 场景专用接口',
      preset: 'openai_compatible',
      providerKind: PROVIDER_KIND_CODING_PLAN,
      vendor: 'glm',
      baseUrl: 'https://api.z.ai/api/coding/paas/v4',
      endpointAliases: ['https://api.z.ai/api/coding/paas/v4/chat/completions'],
      items: [
        { model: 'glm-5.2', title: 'GLM-5.2', desc: '旗舰编码模型' },
        { model: 'glm-5-turbo', title: 'GLM-5-Turbo', desc: '高性能编码模型' },
        { model: 'glm-4.7', title: 'GLM-4.7', desc: '日常编码模型' },
        { model: '', title: '自定义 GLM Coding Plan 模型', desc: '手动填写 Coding Plan 模型 ID', custom: true },
      ],
    },
    {
      key: 'tencent_coding_plan',
      section: 'coding_plan',
      title: '腾讯云 Coding Plan / Tencent Cloud Coding Plan',
      configTitle: '腾讯云 Coding Plan',
      desc: '腾讯云编码计划接口',
      preset: 'openai_compatible',
      providerKind: PROVIDER_KIND_CODING_PLAN,
      vendor: 'tencent',
      baseUrl: 'https://api.lkeap.cloud.tencent.com/coding/v3',
      endpointAliases: ['https://api.lkeap.cloud.tencent.com/coding/v3/chat/completions'],
      items: [
        { model: 'tc-code-latest', title: 'tc-code-latest', desc: 'Coding Plan 自动模型' },
        { model: '', title: '自定义腾讯云 Coding Plan 模型', desc: '手动填写 Coding Plan 模型 ID', custom: true },
      ],
    },
    {
      key: 'kimi_coding_plan',
      section: 'coding_plan',
      title: 'Kimi Coding Plan',
      configTitle: 'Kimi Coding Plan',
      desc: 'Kimi 编码场景专用接口',
      preset: 'openai_compatible',
      providerKind: PROVIDER_KIND_CODING_PLAN,
      vendor: 'kimi',
      baseUrl: 'https://api.kimi.com/coding/v1',
      endpointAliases: ['https://api.kimi.com/coding/v1/chat/completions'],
      items: [
        { model: 'k3', title: 'k3', desc: 'K3 长上下文模型' },
        { model: 'k3-256k', title: 'k3-256k', desc: 'K3 256K 上下文，价格更低' },
        { model: 'kimi-for-coding', title: 'kimi-for-coding', desc: '标准编码模型' },
        { model: 'kimi-for-coding-highspeed', title: 'kimi-for-coding-highspeed', desc: '高速编码模型' },
        { model: '', title: '自定义 Kimi Coding Plan 模型', desc: '手动填写 Coding Plan 模型 ID', custom: true },
      ],
    },
    {
      key: 'deepseek',
      section: 'official_api',
      title: '深度求索 / DeepSeek',
      configTitle: 'DeepSeek',
      desc: 'DeepSeek 官方 API',
      preset: 'deepseek',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'deepseek',
      items: [
        { model: 'deepseek-v4-pro', title: 'deepseek-v4-pro', desc: '高能力模型' },
        { model: 'deepseek-v4-flash', title: 'deepseek-v4-flash', desc: '快速响应' },
        { model: '', title: '自定义 DeepSeek 模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'kimi',
      section: 'official_api',
      title: 'Kimi 中国版 / Kimi China',
      configTitle: 'Kimi',
      desc: 'Moonshot 官方 API',
      preset: 'kimi',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'kimi',
      items: [
        { model: 'kimi-k3', title: 'kimi-k3', desc: '最新通用模型' },
        { model: 'kimi-k2.7-code', title: 'kimi-k2.7-code', desc: '代码场景' },
        { model: 'kimi-k2.7-code-highspeed', title: 'kimi-k2.7-code-highspeed', desc: '高速代码场景' },
        { model: 'kimi-k2.6', title: 'kimi-k2.6', desc: '稳定可用' },
        { model: '', title: '自定义 Kimi 模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'kimi_global',
      section: 'official_api',
      title: 'Kimi 国际版 / Kimi Global',
      configTitle: 'Kimi 国际版',
      desc: 'Moonshot 国际站 API',
      preset: 'kimi',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'kimi',
      baseUrl: 'https://api.moonshot.ai/v1',
      items: [
        { model: 'kimi-k3', title: 'kimi-k3', desc: '最新通用模型' },
        { model: 'kimi-k2.7-code', title: 'kimi-k2.7-code', desc: '代码场景' },
        { model: 'kimi-k2.7-code-highspeed', title: 'kimi-k2.7-code-highspeed', desc: '高速代码场景' },
        { model: 'kimi-k2.6', title: 'kimi-k2.6', desc: '稳定可用' },
        { model: '', title: '自定义 Kimi 模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'glm',
      section: 'official_api',
      title: '智谱开放平台 / GLM API',
      configTitle: 'GLM API',
      desc: '智谱开放平台普通 API',
      preset: 'glm',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'glm',
      items: [
        { model: 'glm-5.2', title: 'glm-5.2', desc: '最新推荐' },
        { model: 'glm-5.1', title: 'glm-5.1', desc: '兼容保留' },
        { model: 'glm-5-turbo', title: 'glm-5-turbo', desc: '高性价比' },
        { model: 'glm-4.7', title: 'glm-4.7', desc: '通用能力' },
        { model: '', title: '自定义 GLM 模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'glm_global',
      section: 'official_api',
      title: '智谱国际版 / GLM API (z.ai)',
      configTitle: 'GLM 国际版 (z.ai)',
      desc: '智谱国际站 z.ai API',
      preset: 'glm',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'glm',
      baseUrl: 'https://api.z.ai/api/paas/v4',
      items: [
        { model: 'glm-5.2', title: 'glm-5.2', desc: '最新推荐' },
        { model: 'glm-5.1', title: 'glm-5.1', desc: '兼容保留' },
        { model: 'glm-5-turbo', title: 'glm-5-turbo', desc: '高性价比' },
        { model: 'glm-4.7', title: 'glm-4.7', desc: '通用能力' },
        { model: '', title: '自定义 GLM 模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'minimax',
      section: 'official_api',
      title: 'MiniMax 中国版 / MiniMax China',
      configTitle: 'MiniMax',
      desc: 'MiniMax 官方 API',
      preset: 'minimax',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'minimax',
      items: [
        { model: 'MiniMax-M3', title: 'MiniMax-M3', desc: '最新推荐' },
        { model: 'MiniMax-M2.7', title: 'MiniMax-M2.7', desc: '通用能力' },
        { model: 'MiniMax-M2.7-highspeed', title: 'MiniMax-M2.7-highspeed', desc: '高速响应' },
        { model: 'MiniMax-M2.5', title: 'MiniMax-M2.5', desc: '官方已转 Legacy，兼容保留' },
        { model: 'MiniMax-M2.5-highspeed', title: 'MiniMax-M2.5-highspeed', desc: '官方已转 Legacy，兼容高速' },
        { model: '', title: '自定义 MiniMax 模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'minimax_global',
      section: 'official_api',
      title: 'MiniMax 国际版 / MiniMax Global',
      configTitle: 'MiniMax 国际版',
      desc: 'MiniMax 国际站 API（与国内 Key 不通用）',
      preset: 'minimax',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'minimax',
      baseUrl: 'https://api.minimax.io/v1',
      items: [
        { model: 'MiniMax-M3', title: 'MiniMax-M3', desc: '最新推荐' },
        { model: 'MiniMax-M2.7', title: 'MiniMax-M2.7', desc: '通用能力' },
        { model: 'MiniMax-M2.7-highspeed', title: 'MiniMax-M2.7-highspeed', desc: '高速响应' },
        { model: 'MiniMax-M2.5', title: 'MiniMax-M2.5', desc: '官方已转 Legacy，兼容保留' },
        { model: 'MiniMax-M2.5-highspeed', title: 'MiniMax-M2.5-highspeed', desc: '官方已转 Legacy，兼容高速' },
        { model: '', title: '自定义 MiniMax 模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'mimo',
      section: 'official_api',
      title: 'MiMo',
      desc: '小米 MiMo 官方 API',
      preset: 'mimo',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'mimo',
      items: [
        { model: 'mimo-v2.5-pro', title: 'mimo-v2.5-pro', desc: '最新推荐' },
        { model: 'mimo-v2.5', title: 'mimo-v2.5', desc: '通用能力' },
        { model: '', title: '自定义 MiMo 模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'qwen',
      section: 'official_api',
      title: '通义千问',
      desc: '阿里云 DashScope 兼容 API',
      preset: 'qwen',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'qwen',
      items: [
        { model: 'qwen3.8-max', title: 'qwen3.8-max', desc: '最新旗舰' },
        { model: 'qwen3.7-max', title: 'qwen3.7-max', desc: '上代旗舰推理' },
        { model: 'qwen3.7-plus', title: 'qwen3.7-plus', desc: '均衡性价比' },
        { model: 'qwen3.7-flash', title: 'qwen3.7-flash', desc: '快速高性价比' },
        { model: 'qwen3.6-flash', title: 'qwen3.6-flash', desc: '兼容保留' },
        { model: '', title: '自定义通义模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'qwen_token_plan',
      section: 'official_api',
      title: '通义千问 Token Plan',
      configTitle: '通义千问 Token Plan',
      desc: '阿里 Token Plan 订阅专用网关',
      preset: 'qwen',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'qwen',
      baseUrl: 'https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1',
      endpointAliases: ['https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1'],
      items: [
        { model: 'qwen3.8-max', title: 'qwen3.8-max', desc: '正式旗舰，夜间五折' },
        { model: 'qwen3.8-max-preview', title: 'qwen3.8-max-preview', desc: '2.4T 旗舰预览，Token Plan 专属，预览结束将下线或替换' },
        { model: 'qwen3.7-max', title: 'qwen3.7-max', desc: '上代旗舰推理' },
        { model: 'qwen3.7-plus', title: 'qwen3.7-plus', desc: '均衡性价比' },
        { model: 'qwen3.6-flash', title: 'qwen3.6-flash', desc: '兼容保留' },
        { model: 'glm-5.2', title: 'glm-5.2', desc: '最新推荐' },
        { model: 'deepseek-v4-pro', title: 'deepseek-v4-pro', desc: '高能力模型' },
        { model: 'deepseek-v4-flash-0731', title: 'deepseek-v4-flash-0731', desc: '快速响应' },
        { model: '', title: '自定义 Token Plan 模型', desc: '手动填写 Token Plan 模型 ID', custom: true },
      ],
    },
    {
      key: 'qwen_global',
      section: 'official_api',
      title: '通义千问国际版 / Qwen International',
      configTitle: '通义千问国际版',
      desc: '阿里云 Model Studio 国际站 API',
      preset: 'qwen',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'qwen',
      baseUrl: 'https://dashscope-intl.aliyuncs.com/compatible-mode/v1',
      items: [
        { model: 'qwen3.8-max', title: 'qwen3.8-max', desc: '最新旗舰' },
        { model: 'qwen3.7-max', title: 'qwen3.7-max', desc: '上代旗舰推理' },
        { model: 'qwen3.7-plus', title: 'qwen3.7-plus', desc: '均衡性价比' },
        { model: 'qwen3.7-flash', title: 'qwen3.7-flash', desc: '快速高性价比' },
        { model: 'qwen3.6-flash', title: 'qwen3.6-flash', desc: '兼容保留' },
        { model: '', title: '自定义通义模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'doubao',
      section: 'official_api',
      title: '豆包',
      desc: '火山方舟官方 API',
      preset: 'doubao',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'doubao',
      items: [
        { model: 'doubao-seed-evolving', title: 'doubao-seed-evolving', desc: '最新推荐' },
        { model: 'doubao-seed-2-1-pro-260628', title: 'doubao-seed-2-1-pro-260628', desc: '高能力模型' },
        { model: 'doubao-seed-2-1-turbo-260628', title: 'doubao-seed-2-1-turbo-260628', desc: '快速响应' },
        { model: 'doubao-seed-2-0-pro-260215', title: 'doubao-seed-2-0-pro-260215', desc: '稳定通用' },
        { model: 'doubao-seed-2-0-lite-260428', title: 'doubao-seed-2-0-lite-260428', desc: '轻量模型' },
        { model: '', title: '自定义豆包模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'openai',
      section: 'official_api',
      title: 'OpenAI',
      configTitle: 'OpenAI',
      desc: 'OpenAI 官方 API',
      preset: 'openai',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'openai',
      baseUrl: 'https://api.openai.com/v1',
      items: [
        { model: 'gpt-5.6-sol', title: 'gpt-5.6-sol', desc: '旗舰推理与编码' },
        { model: 'gpt-5.6-terra', title: 'gpt-5.6-terra', desc: '均衡智能与成本' },
        { model: 'gpt-5.6-luna', title: 'gpt-5.6-luna', desc: '低成本高并发' },
        { model: 'gpt-5.5', title: 'gpt-5.5', desc: '上代旗舰' },
        { model: 'gpt-5.4-mini', title: 'gpt-5.4-mini', desc: '快速经济' },
        { model: '', title: '自定义 OpenAI 模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'anthropic',
      section: 'official_api',
      title: 'Anthropic Claude',
      configTitle: 'Anthropic Claude',
      desc: 'Anthropic 官方 API（Messages 原生协议）',
      preset: 'anthropic',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'anthropic',
      baseUrl: 'https://api.anthropic.com/v1',
      items: [
        { model: 'claude-fable-5', title: 'claude-fable-5', desc: '最强旗舰，长程 Agent' },
        { model: 'claude-opus-5', title: 'claude-opus-5', desc: '复杂 Agent 编码' },
        { model: 'claude-sonnet-5', title: 'claude-sonnet-5', desc: '速度与智能均衡' },
        { model: 'claude-haiku-4-5', title: 'claude-haiku-4-5', desc: '最快，接近旗舰' },
        { model: '', title: '自定义 Claude 模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'gemini',
      section: 'official_api',
      title: 'Google Gemini',
      configTitle: 'Google Gemini',
      desc: 'Gemini API（OpenAI 兼容端点）',
      preset: 'gemini',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'gemini',
      baseUrl: 'https://generativelanguage.googleapis.com/v1beta/openai',
      items: [
        { model: 'gemini-3.6-flash', title: 'gemini-3.6-flash', desc: '最新 Flash，均衡高性价比' },
        { model: 'gemini-3.5-flash', title: 'gemini-3.5-flash', desc: '均衡' },
        { model: 'gemini-3.5-flash-lite', title: 'gemini-3.5-flash-lite', desc: '快速经济' },
        { model: 'gemini-3.1-pro-preview', title: 'gemini-3.1-pro-preview', desc: '旗舰推理（预览）' },
        { model: '', title: '自定义 Gemini 模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'xai',
      section: 'official_api',
      title: 'xAI Grok',
      configTitle: 'xAI Grok',
      desc: 'xAI 官方 API',
      preset: 'xai',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'xai',
      baseUrl: 'https://api.x.ai/v1',
      items: [
        { model: 'grok-4.20-0309-reasoning', title: 'grok-4.20-0309-reasoning', desc: '4.20 推理' },
        { model: 'grok-4.20-0309-non-reasoning', title: 'grok-4.20-0309-non-reasoning', desc: '4.20 非推理' },
        { model: 'grok-4.5', title: 'grok-4.5', desc: '旗舰编码与 Agent' },
        { model: 'grok-4.3', title: 'grok-4.3', desc: '通用推理，默认推荐' },
        { model: 'grok-build-0.1', title: 'grok-build-0.1', desc: '代码 Agent' },
        { model: '', title: '自定义 Grok 模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'openai_compatible',
      section: 'custom',
      title: 'OpenAI Compatible',
      desc: '自定义 OpenAI 兼容接口',
      preset: 'openai_compatible',
      providerKind: PROVIDER_KIND_CUSTOM,
      items: [
        { model: '', title: '自定义兼容模型', desc: '手动填写模型 ID 和服务地址', custom: true },
      ],
    },
  ],
};

const CLOUD_MODEL_PROVIDERS = MODEL_CATALOG.cloud;
function normalizeEndpointUrl(value) {
  const raw = String(value || '').trim();
  if (!raw) return '';
  return raw.replace(/\/+$/, '');
}
function normalizeOpenAiBaseUrl(value) {
  const trimmed = normalizeEndpointUrl(value);
  return trimmed.replace(/\/chat\/completions$/i, '');
}
function providerBaseUrl(provider) {
  if (!provider) return '';
  return provider.baseUrl || (MODEL_PRESET_DEFS[provider.preset] && MODEL_PRESET_DEFS[provider.preset].baseUrl) || '';
}
function normalizedProviderBaseUrl(provider) {
  const base = providerBaseUrl(provider);
  if (provider && provider.endpointMode === 'full_chat_completions') return normalizeEndpointUrl(base);
  return normalizeOpenAiBaseUrl(base);
}
function findCloudProviderForModel(model) {
  if (!model) return null;
  const providerKind = model.provider_kind || model.providerKind;
  const vendor = model.vendor;
  const base = normalizeEndpointUrl(model.base_url || model.baseUrl || '');
  return CLOUD_MODEL_PROVIDERS.find(provider => {
    if (providerKind && provider.providerKind !== providerKind) return false;
    if (vendor && provider.vendor !== vendor) return false;
    const urls = [providerBaseUrl(provider), ...(provider.endpointAliases || [])]
      .map(url => provider.endpointMode === 'full_chat_completions' ? normalizeEndpointUrl(url) : normalizeOpenAiBaseUrl(url));
    const compareBase = provider.endpointMode === 'full_chat_completions' ? base : normalizeOpenAiBaseUrl(base);
    if (compareBase && urls.includes(compareBase)) return true;
    return !providerKind && !vendor && provider.preset === model.preset && provider.items.some(item => !item.custom && item.model === model.model);
  }) || null;
}
function providerLabelForModel(model, t) {
  const provider = findCloudProviderForModel(model);
  if (provider) {
    const overrides = (t && t.uiSettingsDetail && t.uiSettingsDetail.providerCatalog) || {};
    const override = overrides[provider.key];
    return (override && override.title) || provider.title;
  }
  return presetProviderLabel(model && model.preset, t);
}
function isCodingPlanModel(model) {
  const providerKind = model && (model.provider_kind || model.providerKind);
  return providerKind === PROVIDER_KIND_CODING_PLAN || !!(model && findCloudProviderForModel(model)?.providerKind === PROVIDER_KIND_CODING_PLAN);
}

// ── 模型选择器:预设/自定义分组与可区分标注(纯函数,显示期计算) ─────
// 分类判据:模型是否命中其实际 provider 的非 custom 目录项。自定义兼容接口即使
// 使用目录中已有的模型 ID,也必须保持为自定义,避免多个聚合服务模型再次同名。
function isPresetModel(m) {
  if (!m || !m.model) return false;
  if (m.preset === 'local_vllm') {
    return (MODEL_CATALOG.local || []).some(group =>
      (group.items || []).some(item => !item.custom && item.model === m.model));
  }
  const providerKind = m.provider_kind || m.providerKind;
  if (providerKind === PROVIDER_KIND_CUSTOM) return false;
  const provider = findCloudProviderForModel(m);
  return !!provider && provider.providerKind !== PROVIDER_KIND_CUSTOM
    && (provider.items || []).some(item => !item.custom && item.model === m.model);
}

// 保留各组在入参中的原顺序。
function groupModelsForSelector(models) {
  const preset = [];
  const custom = [];
  (models || []).forEach(m => { (isPresetModel(m) ? preset : custom).push(m); });
  return { preset, custom };
}

// 本地模型默认名会持久化。切换界面语言后仍须识别中英日历史默认值,不能把它
// 误判为用户命名;这些字符串只用于兼容已持久化值,不会直接渲染。
function localUserNamed(m, localModelNameFn) {
  if (!m || m.preset !== 'local_vllm') return false;
  if (typeof localModelNameFn !== 'function') return false;
  if (!m.name) return false;
  const model = String(m.model || '');
  const defaults = new Set([
    localModelNameFn(model),
    model ? `本地 ${model}` : '本地模型',
    model ? `Local ${model}` : 'Local model',
    model ? `ローカル ${model}` : 'ローカルモデル',
  ]);
  return !defaults.has(m.name);
}

function selectorMainLabel(m, t) {
  if (!m) return '';
  const localModelNameFn = t && t.uiSettingsDetail && t.uiSettingsDetail.localModelName;
  if (localUserNamed(m, localModelNameFn)) return m.name;
  if (m.preset === 'local_vllm' && isPresetModel(m) && typeof localModelNameFn === 'function') {
    return localModelNameFn(m.model);
  }
  return isPresetModel(m) ? (m.name || m.model) : (m.model || m.name);
}

function selectorSubLabel(m, t) {
  if (!m) return '';
  const localModelNameFn = t && t.uiSettingsDetail && t.uiSettingsDetail.localModelName;
  if (localUserNamed(m, localModelNameFn)) return m.model;   // 主=name -> 副=model
  if (isPresetModel(m)) return providerLabelForModel(m, t);  // 主=name/title -> 副=provider 归属
  // 自定义:主=model -> 副=provider 归属
  if (m.preset === 'local_vllm') return localModelNameFn ? localModelNameFn(m.model) : m.model;
  const provider = findCloudProviderForModel(m);
  return provider ? providerLabelForModel(m, t) : presetProviderLabel('openai_compatible', t);
}

export {
  MODEL_PRESET_DEFS,
  PROVIDER_KIND_CODING_PLAN,
  PROVIDER_KIND_OFFICIAL_API,
  PROVIDER_KIND_CUSTOM,
  MODEL_CATALOG_SECTIONS,
  MODEL_CATALOG,
  CLOUD_MODEL_PROVIDERS,
  BRAND_ICON_BY_PRESET,
  BRAND_ICON_BY_VENDOR,
  presetOptionsI18n,
  presetProviderLabel,
  normalizedProviderBaseUrl,
  findCloudProviderForModel,
  providerLabelForModel,
  isCodingPlanModel,
  isPresetModel,
  groupModelsForSelector,
  localUserNamed,
  selectorMainLabel,
  selectorSubLabel,
};
