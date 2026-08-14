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

// ── 思考深度（reasoning effort）档位 ─────────────────────────────
// 每个 provider 只暴露底座 wire 层有实际区别的档位（归一后无区别的档位
// 不展示，避免用户选到"看起来不同、实际相同"的值）。语义与品悟 Rust 侧
// provider() 判定对齐（vendor 优先 + preset 兜底）。
const REASONING_EFFORT_TIERS = {
  // vllm：off/low/medium/high 四档；max 被底座降级为 high，不重复暴露。
  vllm: ['off', 'low', 'medium', 'high'],
  // deepseek：wire 文档只认 low/high/max（无 medium），底座 apply_reasoning_effort
  // 把 low 保留为更便宜档位、medium 归一为 high，故暴露 off/low/high/max。
  deepseek: ['off', 'low', 'high', 'max'],
  // volcengine：底座把 low/medium 归一为 high，仅 off/high/max 有区别。
  volcengine: ['off', 'high', 'max'],
  // 只有 thinking 开关的 provider：off/high。
  moonshot: ['off', 'high'],
  zai: ['off', 'high'],
  minimax: ['off', 'high'],
  'xiaomi-mimo': ['off', 'high'],
  // anthropic native：off 不注入（等价默认），暴露 low/medium/high/max。
  anthropic: ['low', 'medium', 'high', 'max'],
  // openai：仅 gpt-5.x reasoning 系模型底座会注入，off=none。
  openai: ['off', 'low', 'medium', 'high', 'max'],
};

// OpenAI 官方 API 支持「自定义模型」手输模型 ID，因此 reasoning 家族判定必须
// 对齐底座 CodeWhale `model_is_openai_reasoning_family`（models.rs）的完整
// predicate，而不是只覆盖品悟目录收录的 4 个 ID：用户手输 gpt-5.6 / gpt-5.5-pro /
// 日期快照 / gpt-5.3-codex 等模型时底座仍会注入多档 reasoning_effort，前端若
// 返回 null 会隐藏切换，造成「后端注入、前端不可控」的不一致。
function isOpenaiReasoningFamilyModel(model) {
  const lower = String((model && model.model) || '').trim().toLowerCase();
  return isOpenaiGpt55ApiModel(lower)
    || isOpenaiGpt56ApiModel(lower)
    || isOpenaiCodexModel(lower);
}

// 对齐 models.rs `is_openai_gpt_55_api_model`：gpt-5.5 / gpt-5.5-pro 及其日期快照。
function isOpenaiGpt55ApiModel(lower) {
  return lower === 'gpt-5.5' || lower === 'gpt-5.5-pro'
    || hasOpenaiDateSnapshotSuffix(lower, 'gpt-5.5-')
    || hasOpenaiDateSnapshotSuffix(lower, 'gpt-5.5-pro-');
}

// 对齐 models.rs `is_openai_gpt_56_api_model`。
function isOpenaiGpt56ApiModel(lower) {
  return lower === 'gpt-5.6' || lower === 'gpt-5.6-sol'
    || lower === 'gpt-5.6-terra' || lower === 'gpt-5.6-luna';
}

// 对齐 models.rs `is_openai_codex_model`。
const OPENAI_CODEX_MODELS = new Set([
  'gpt-5-codex', 'gpt-5.1-codex', 'gpt-5.1-codex-mini', 'gpt-5.1-codex-max',
  'gpt-5.2-codex', 'gpt-5.3-codex', 'codex-gpt-5.5', 'chatgpt-gpt-5.5',
  'gpt-5.5-codex', 'gpt-5.5-codex-preview', 'codex-gpt-5.5-preview', 'chatgpt-gpt-5.5-preview',
]);

function isOpenaiCodexModel(lower) {
  return OPENAI_CODEX_MODELS.has(lower);
}

// 对齐 models.rs `has_date_snapshot_suffix`：prefix 后须紧跟 YYYY-MM-DD（10 字符，
// 第 5 / 8 位为 '-'，其余为数字），否则不视为日期快照。
function hasOpenaiDateSnapshotSuffix(lower, prefix) {
  if (!lower.startsWith(prefix)) return false;
  const rest = lower.slice(prefix.length);
  if (rest.length !== 10 || rest[4] !== '-' || rest[7] !== '-') return false;
  for (let i = 0; i < 10; i += 1) {
    if (i === 4 || i === 7) continue;
    if (rest[i] < '0' || rest[i] > '9') return false;
  }
  return true;
}

// 品悟 provider 判定（对齐 bridge.rs `provider()`：base_url(deepseek) 优先，
// vendor 优先 + preset 兜底）。
//
// 与 Rust `provider()` 的结构性差异（均为前端「只暴露底座有实际档位区别的
// provider」的刻意裁剪）：
// 1. env(DEEPSEEK_PROVIDER)：Rust 支持环境变量覆盖 provider；前端无 env 概念
//    （GUI 场景极少使用该 env，视为等价）。
// 2. xai 返回：Rust 返回 "xai"（底座有 provider 身份，wire 层需要）；前端返回
//    null——底座对 xai 的 reasoning_effort 是空操作，无档位可切。
// 3. qwen/gemini 归类：Rust 将 qwen/tencent/openai/gemini/google 归入 "openai"
//    （wire route 身份）；前端对 qwen/tencent/gemini/google 返回 null（底座无档位），
//    仅 openai vendor 的 reasoning 家族返回 "openai"（对齐底座
//    `model_is_openai_reasoning_family`）。
// 4. zai/moonshot 路由级档位：底座按「精确 first-party base_url + 模型名」判定
//    tiered effort（zai GLM-5.2/5.3、moonshot K3）；前端同样按精确端点身份判定
//    （见 reasoningEffortTiersForModel 与 is_exact_*_base_url）。兼容网关/中国端点
//    误配同型号时底座 fail-closed（不注入），前端回落通用档位，与底座行为对齐。
// 对齐 bridge.rs `is_official_deepseek_base_url`：官方 DeepSeek 端点判定
// （trim 尾斜杠 + /beta + /v1，小写比较）。
function isOfficialDeepseekBaseUrl(baseUrl) {
  const normalized = String(baseUrl || '')
    .trim()
    .replace(/\/+$/, '')
    .replace(/\/beta$/, '')
    .replace(/\/v1$/, '')
    .toLowerCase();
  return normalized === 'https://api.deepseek.com' || normalized === 'https://api.deepseeki.com';
}

// 底座对 moonshot/zai/minimax 的 tiered effort 只按「精确 first-party base_url + 模型名」
// 路由（CodeWhale config::is_exact_direct_moonshot_k3_route / is_exact_kimi_code_k3_route /
// is_exact_zai_tiered_effort_route / is_exact_minimax_m3_route）。复刻底座 `is_exact_https_route`
// 的比较语义：scheme/host ASCII 大小写不敏感、path 大小写敏感、只容忍一个尾斜杠——不多删
// 斜杠、也不整段转小写（不同大小写的 path 是相邻路由，不是官方端点）。中国端点 / 兼容网关
// 误配同型号时底座 fail-closed（不注入），前端据此收窄档位暴露。
function isExactHttpsRoute(baseUrl, expectedAuthority, expectedPath) {
  const trimmed = String(baseUrl || '').trim();
  // 对齐底座 strip_suffix('/')：只去掉一个尾斜杠，剩下的斜杠仍参与 path 比较。
  const normalized = trimmed.endsWith('/') ? trimmed.slice(0, -1) : trimmed;
  const schemeSep = normalized.indexOf('://');
  if (schemeSep === -1) return false;
  const scheme = normalized.slice(0, schemeSep);
  const authorityAndPath = normalized.slice(schemeSep + 3);
  const slash = authorityAndPath.indexOf('/');
  if (slash === -1) return false;
  const authority = authorityAndPath.slice(0, slash);
  const path = authorityAndPath.slice(slash + 1);
  return scheme.toLowerCase() === 'https'
    && authority.toLowerCase() === expectedAuthority.toLowerCase()
    && path === expectedPath;
}
// Moonshot 直连平台（国际站）端点：https://api.moonshot.ai/v1
function isExactMoonshotPlatformBaseUrl(baseUrl) {
  return isExactHttpsRoute(baseUrl, 'api.moonshot.ai', 'v1');
}
// Kimi Code 会员计划端点：https://api.kimi.com/coding/v1（裸 k3）
function isExactKimiCodeBaseUrl(baseUrl) {
  return isExactHttpsRoute(baseUrl, 'api.kimi.com', 'coding/v1');
}
// z.ai first-party Chat 端点（Coding Plan / 普通平台）。
function isExactZaiChatBaseUrl(baseUrl) {
  return isExactHttpsRoute(baseUrl, 'api.z.ai', 'api/paas/v4')
    || isExactHttpsRoute(baseUrl, 'api.z.ai', 'api/coding/paas/v4');
}
// MiniMax first-party OpenAI Chat 端点（国际 api.minimax.io / 国内 api.minimaxi.com）。
function isExactMinimaxChatBaseUrl(baseUrl) {
  return isExactHttpsRoute(baseUrl, 'api.minimax.io', 'v1')
    || isExactHttpsRoute(baseUrl, 'api.minimaxi.com', 'v1');
}

function reasoningProviderForModel(model) {
  if (!model) return null;
  // 对齐 Rust provider() 优先级：官方 deepseek base_url 优先（即使 preset 是
  // openai_compatible 且无 vendor，只要指向官方 deepseek 端点即按 deepseek 暴露档位）。
  if (isOfficialDeepseekBaseUrl(model.base_url)) return 'deepseek';
  const vendor = (model.vendor || '').trim().toLowerCase();
  const preset = model.preset || '';
  if (preset === 'local_vllm') return 'vllm';
  if (vendor) {
    if (vendor === 'deepseek') return 'deepseek';
    if (vendor === 'kimi' || vendor === 'moonshot') return 'moonshot';
    if (vendor === 'glm' || vendor === 'zai' || vendor === 'zhipu') return 'zai';
    if (vendor === 'minimax') return 'minimax';
    if (vendor === 'mimo' || vendor === 'xiaomi' || vendor === 'xiaomi-mimo') return 'xiaomi-mimo';
    if (vendor === 'doubao' || vendor === 'volcengine') return 'volcengine';
    if (vendor === 'anthropic' || vendor === 'claude') return 'anthropic';
    if (vendor === 'xai' || vendor === 'grok') return null; // 底座空操作，不提供切换
    if (vendor === 'openai') return isOpenaiReasoningFamilyModel(model) ? 'openai' : null;
    return null; // qwen / tencent / gemini / google 无档位
  }
  switch (preset) {
    case 'deepseek': return 'deepseek';
    case 'kimi': return 'moonshot';
    case 'glm': return 'zai';
    case 'minimax': return 'minimax';
    case 'mimo': return 'xiaomi-mimo';
    case 'doubao': return 'volcengine';
    case 'anthropic': return 'anthropic';
    case 'openai': return isOpenaiReasoningFamilyModel(model) ? 'openai' : null;
    default: return null;
  }
}

// 该模型可切换的思考深度档位（无则 null = 不提供切换）。
// 路由/模型级细分（仅品悟目录收录的模型）：
// - zai：first-party z.ai 端点上 GLM-5.2/5.3 提供 tiered effort（off/high/max），
//   GLM-5.1/GLM-5-Turbo 只有 generic thinking 开关（off/high）；中国 open.bigmodel.cn、
//   兼容网关、未验证模型底座会删除 thinking/reasoning_effort（两档等效）→ 不提供切换。
// - moonshot：K3（kimi-k3 / k3，always-thinking）提供 low/high/max（off 归一为 low）；
//   其余 moonshot 模型按 generic thinking 开关暴露 off/high。
// - minimax：仅 first-party MiniMax-M3 提供 off（disabled）/high（adaptive）；M2.7/M2.5
//   与兼容网关底座清空控制字段（两档等效）→ 不提供切换。
// 与底座 `is_exact_zai_tiered_effort_route` / `is_exact_direct_moonshot_k3_route` /
// `is_exact_kimi_code_k3_route` / `is_exact_minimax_m3_route` 对齐：中国端点 / 兼容网关
// 误配同型号时底座 fail-closed，前端不再暴露无效或彼此等效的选项。
function reasoningEffortTiersForModel(model) {
  const provider = reasoningProviderForModel(model);
  if (!provider) return null;
  const tiers = REASONING_EFFORT_TIERS[provider];
  if (!tiers) return null;
  const modelName = String((model && model.model) || '').trim().toLowerCase();
  const baseUrl = (model && model.base_url) || '';
  if (provider === 'zai') {
    if (!isExactZaiChatBaseUrl(baseUrl)) return null;
    if (modelName === 'glm-5.2' || modelName === 'glm-5.3') return ['off', 'high', 'max'];
    if (modelName === 'glm-5.1' || modelName === 'glm-5-turbo') return ['off', 'high'];
    return null;
  }
  if (provider === 'moonshot' && isExactMoonshotK3Route(model, modelName)) {
    return ['low', 'high', 'max'];
  }
  if (provider === 'minimax') {
    if (!isExactMinimaxChatBaseUrl(baseUrl)) return null;
    if (modelName !== 'minimax-m3') return null;
    return ['off', 'high'];
  }
  return tiers;
}

// 底座 K3（always-thinking）精确路由：直连平台 kimi-k3 与 Kimi Code 裸 k3。
// 仅这两个「精确端点 + 模型名」组合会进入 tiered low/high/max 路由。
function isExactMoonshotK3Route(model, modelName) {
  const baseUrl = (model && model.base_url) || '';
  if (modelName === 'kimi-k3') return isExactMoonshotPlatformBaseUrl(baseUrl);
  if (modelName === 'k3') return isExactKimiCodeBaseUrl(baseUrl);
  return false;
}

// 当前模型是否走底座 always-thinking K3 tiered 路由（档位表为 low/high/max）。
function isAlwaysThinkingK3Route(model) {
  if (!model || reasoningProviderForModel(model) !== 'moonshot') return false;
  const modelName = String((model && model.model) || '').trim().toLowerCase();
  return isExactMoonshotK3Route(model, modelName);
}

// 该模型的默认思考深度档位：本地 vLLM 保持 off（防 SSE timeout），其余 high。
function defaultReasoningEffortForModel(model) {
  if (reasoningProviderForModel(model) === 'vllm') return 'off';
  return reasoningEffortTiersForModel(model) ? 'high' : null;
}

// 切换模型时的思考深度重置：丢弃旧档位，按新 model 的 route 回落到默认档位
// （vllm→off，其余支持档位的模型→high；无档位模型→null = 未显式设置）。K2.6 选 off 后
// 切 K3，off 不在 K3 档位表（low/high/max）内，必须重置为 high，否则界面无高亮且保存
// 仍写旧值。单独成函数以便对「模型切换归一」这一状态迁移做行为测试。
function reasoningEffortForModelSwitch(model) {
  return defaultReasoningEffortForModel(model) || null;
}

// 底座 ReasoningEffort::parse_strict 接受的别名 → 规范档位（对齐 as_setting()）。
// 只收录会映射到档位表内档位的别名；auto/automatic 不在 UI 暴露，不收录（回落默认）。
const REASONING_EFFORT_CANONICAL = {
  off: 'off', disabled: 'off', none: 'off', false: 'off',
  low: 'low', minimum: 'low', minimal: 'low', light: 'low',
  medium: 'medium', mid: 'medium',
  high: 'high',
  max: 'max', maximum: 'max', xhigh: 'max', ultra: 'max', ultracode: 'max',
};

// 存量档位归一：用户可能保存过底座归一前的旧值（别名或不在档位表内的档位，
// 如 deepseek 的 medium → 底座归一为 high）。展示与表单初始值都取归一后的档位，
// 避免「档位表不含该值 → 下拉无高亮 / 残留无法选中的脏值」；无档位模型返回 null。
function normalizeStoredReasoningEffort(model, stored) {
  const tiers = reasoningEffortTiersForModel(model) || [];
  if (!tiers.length) return null;
  let canonical = stored
    ? (REASONING_EFFORT_CANONICAL[String(stored).trim().toLowerCase()] || null)
    : null;
  // always-thinking K3：off 在底座 K3 路由里等价于最低档 low（thinking.effort=low /
  // reasoning_effort=low），medium 等价于 high。按路由真实等价值归一，否则 UI 高亮
  // high、请求实际 low，且点击已高亮的 high 会被相等判断短路、无法纠正。
  if (isAlwaysThinkingK3Route(model)) {
    if (canonical === 'off') canonical = 'low';
    else if (canonical === 'medium') canonical = 'high';
  }
  if (canonical && tiers.includes(canonical)) return canonical;
  return defaultReasoningEffortForModel(model) || tiers[0] || null;
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
  reasoningEffortTiersForModel,
  defaultReasoningEffortForModel,
  reasoningEffortForModelSwitch,
  normalizeStoredReasoningEffort,
};
