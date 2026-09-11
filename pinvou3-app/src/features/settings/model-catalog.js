// 「添加模型」云端/本地模型目录与预设模板（自 SettingsView.jsx 抽离）。
// 纯数据 + 纯函数：不含组件、不依赖 React；品牌图标映射随目录一并归位。
// 目录条目的三语文案已按语言并入 shared/i18n/{zh,en,ja}.js(原 settings-i18n.js 拆分),
// 随 i18n.js 聚合/惰性装载一体维护,此处不再需要副作用 import。
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
// 默认模型（2026-09-11 按各厂商官方文档核对）：
// - deepseek：V4.1-Flash（deepseek-flash）为当前主力；deepseek-v4-pro 将于
//   2026-09-14 起被官方路由到 V4.1-Flash 计费（api-docs.deepseek.com/updates）。
// - glm：GLM-5.3 已于 2026-08-19（中国）/2026-08-18（z.ai）上线，双站 API enum
//   默认值即 glm-5.3（docs.bigmodel.cn / docs.z.ai）。
// - gemini：gemini-3.8-flash 于 2026-09-02 发布接棒（ai.google.dev models 页）。
// - xai：grok-4.6 为官方「编码/Agent 推荐」位（docs.x.ai models 页）。
// - openai 保持 gpt-5.6-terra：官方推荐起点虽已是 gpt-6-astra，但其 Chat
//   Completions 不支持函数调用（工具调用需 Responses 协议），品悟 openai 预设
//   走 Chat wire，故默认不换。
const MODEL_PRESET_DEFS = {
  local_vllm:  { baseUrl: 'http://127.0.0.1:8000/v1',                model: 'qwen36_35b_256k' },
  deepseek:    { baseUrl: 'https://api.deepseek.com',                model: 'deepseek-flash' },
  kimi:        { baseUrl: 'https://api.moonshot.cn/v1',              model: 'kimi-k3' },
  // 自定义兼容接口:地址与模型完全由用户填写,不再预填 OpenAI 官方样板。
  openai_compatible: { baseUrl: '',                                 model: '' },
  qwen:        { baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', model: 'qwen3.8-max' },
  doubao:      { baseUrl: 'https://ark.cn-beijing.volces.com/api/v3', model: 'doubao-seed-evolving' },
  minimax:     { baseUrl: 'https://api.minimaxi.com/v1',            model: 'MiniMax-M3' },
  glm:         { baseUrl: 'https://open.bigmodel.cn/api/paas/v4',   model: 'glm-5.3' },
  mimo:        { baseUrl: 'https://api.xiaomimimo.com/v1',          model: 'mimo-v2.5-pro' },
  openai:      { baseUrl: 'https://api.openai.com/v1',              model: 'gpt-5.6-terra' },
  anthropic:   { baseUrl: 'https://api.anthropic.com/v1',           model: 'claude-sonnet-5' },
  gemini:      { baseUrl: 'https://generativelanguage.googleapis.com/v1beta/openai', model: 'gemini-3.8-flash' },
  xai:         { baseUrl: 'https://api.x.ai/v1',                    model: 'grok-4.6' },
};
const PROVIDER_KIND_CODING_PLAN = 'coding_plan';
const PROVIDER_KIND_OFFICIAL_API = 'official_api';
const PROVIDER_KIND_CUSTOM = 'custom';
// 模型拼写约定：凡底座（CodeWhale）route 目录收录的模型，列表项 `model` 一律
// 优先使用底座 models_dev.bundled.json 目录行的原样拼写，不可套用大小写规律；
// 自定义兼容端点（bigmodel.cn Coding Plan、开放平台、腾讯/阿里 Plan）与
// modelstudio 目录（qwen_token_plan）保持各自的小写 wire id。z.ai 直连的官方
// wire id 已确认为全小写（docs.z.ai API enum，2026-09 核查），目录行据此改用
// 小写并留大写 legacy；底座 bundled 资产里仍是大写行（GLM-5.2 等），但 resolver
// 自 c0f749731（2026-08-17，已随当前 gitlink 发布）起对 StrictDirect + Deepseek/Zai
// 提供大小写折叠回退（精确优先、provider 内唯一命中），小写保存配置可安全命中。
// 凡目录行拼写发生变更（含大小写），旧拼写仍须登记到该项 legacyAliases 以兼容
// 存量配置的目录归类；其余情况保持精确比较。
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
      // bigmodel 是底座 zai kind 的自定义端点：模型名原样透传，必须用厂商
      // 文档的小写 wire id。2026-09 官方口径（docs.bigmodel.cn/cn/coding-plan/
      // overview）：GLM-5.3 / GLM-5.3-Flash 为全套餐原生模型；GLM-5.2/GLM-5.1
      // 调用自动切换至 GLM-5.3，GLM-5-Turbo/GLM-4.7 自动切换至 GLM-5.3-Flash，
      // 故旧模型行保留为「历史模型」入口而非删除。
      items: [
        { model: 'glm-5.3', title: 'GLM-5.3', desc: '旗舰编码模型，全套餐支持' },
        { model: 'glm-5.3-flash', imageCapable: true, title: 'GLM-5.3-Flash', desc: '原生多模态编码模型，额度三倍' },
        { model: 'glm-5.2', title: 'GLM-5.2', desc: '历史模型，请求自动切换至 GLM-5.3' },
        { model: 'glm-5-turbo', title: 'GLM-5-Turbo', desc: '历史模型，自动切换至 GLM-5.3-Flash' },
        { model: 'glm-4.7', title: 'GLM-4.7', desc: '历史模型，自动切换至 GLM-5.3-Flash' },
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
      // z.ai 官方 wire id 为全小写（docs.z.ai API enum，2026-09 核查）；存量
      // 配置可能保存旧大写目录值（GLM-5.2），以 legacyAliases 兼容识别。
      // GLM-5-Turbo 从未在 z.ai 发布（不在模型总览/定价/API enum），旧行已删；
      // 存量 GLM-5-Turbo 配置会回落为自定义归类（档位提示不受影响），须自行
      // 改选 glm-5.3 / glm-5.3-flash。
      items: [
        { model: 'glm-5.3', title: 'GLM-5.3', desc: '旗舰编码模型，全套餐支持' },
        { model: 'glm-5.3-flash', imageCapable: true, title: 'GLM-5.3-Flash', desc: '原生多模态编码模型，额度三倍' },
        { model: 'glm-5.2', legacyAliases: ['GLM-5.2'], title: 'GLM-5.2', desc: '历史模型，请求自动路由至 GLM-5.3' },
        { model: 'glm-4.7', title: 'GLM-4.7', desc: '历史模型，自动路由至 GLM-5.3-Flash' },
        { model: '', title: '自定义 GLM Coding Plan 模型', desc: '手动填写 Coding Plan 模型 ID', custom: true },
      ],
    },
    // The two Tencent Cloud subscription tiers are modeled separately per the
    // official docs (TokenHub product 1823):
    // - Coding Plan: https://cloud.tencent.com/document/product/1823/130092
    //   (checked 2026-09-11) OpenAI-compatible base URL /coding/v3; the catalog
    //   lists all of its model rows (Auto + GLM-5). GLM-5 retires 2026-10-09
    //   (130092 / 130060). The same page also offers an Anthropic-compatible
    //   /coding/anthropic endpoint for Claude Code-style tools; this repo's
    //   OpenAI route does not use it. Per 130092, Coding Plan models do not
    //   support multimodal (image) input, so no imageCapable flags here.
    // - Token Plan: https://cloud.tencent.com/document/product/1823/130119
    //   (checked 2026-09-11) OpenAI-compatible base URL /plan/v3 (access guide
    //   in 130075, plan overview in 130060); its general and Hy tiers have
    //   different lineups, and it is a different subscription from Coding Plan
    //   (plan keys are sk-tp-*, coding keys sk-sp-*, not interchangeable).
    // Both endpoints are identified as vendor=tencent coding_plan by
    // identify_coding_plan_endpoint on the Rust side, so both groups must keep
    // providerKind=CODING_PLAN to stay consistent with the metadata read back
    // after saving. Model names pass through the generic OpenAI-compatible
    // route verbatim and must use the official lowercase wire ids; the other
    // parallel official spellings on the same page (e.g. glm-5-3, the
    // deepseek/deepseek-v4-* forms, minimax-m-3-0) are registered in
    // legacyAliases so stored configs match with any of them.
    // kimi-k2.5 was removed from both groups: Tencent announce 2414 retired it
    // platform-wide on 2026-08-31 00:00 (plan users auto-switch to Auto /
    // tc-code-latest), verified against the live 130092/130119 tables.
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
        { model: 'glm-5', legacyAliases: ['glm-5-0'], title: 'glm-5', desc: '旗舰编码模型，官方将于 2026-10-09 下线' },
        { model: '', title: '自定义腾讯云 Coding Plan 模型', desc: '手动填写 Coding Plan 模型 ID', custom: true },
      ],
    },
    {
      key: 'tencent_token_plan',
      section: 'coding_plan',
      title: '腾讯云 Token Plan / Tencent Cloud Token Plan',
      configTitle: '腾讯云 Token Plan',
      desc: '腾讯云 TokenHub Token 订阅接口',
      preset: 'openai_compatible',
      providerKind: PROVIDER_KIND_CODING_PLAN,
      vendor: 'tencent',
      baseUrl: 'https://api.lkeap.cloud.tencent.com/plan/v3',
      endpointAliases: ['https://api.lkeap.cloud.tencent.com/plan/v3/chat/completions'],
      // Row lineup mirrors the live 130119 general-tier table (checked
      // 2026-09-11); GLM-5/GLM-5.1 retire 2026-10-09 per 130060. hy4-preview is
      // flagged by Tencent as high-load (may be rate-limited at peak).
      items: [
        { model: 'tc-code-latest', title: 'tc-code-latest', desc: '自动模型，智能路由' },
        { model: 'glm-5.3', legacyAliases: ['glm-5-3'], title: 'glm-5.3', desc: '旗舰推理与编码' },
        { model: 'glm-5.3-flash', imageCapable: true, title: 'glm-5.3-flash', desc: '多模态高性价比' },
        { model: 'glm-5.2', legacyAliases: ['glm-5-2'], title: 'glm-5.2', desc: '上代旗舰推理' },
        { model: 'glm-5.1', legacyAliases: ['glm-5-1'], title: 'glm-5.1', desc: '官方将于 2026-10-09 下线' },
        { model: 'glm-5', legacyAliases: ['glm-5-0'], title: 'glm-5', desc: '通用推理，官方将于 2026-10-09 下线' },
        { model: 'kimi-k3', imageCapable: true, title: 'kimi-k3', desc: 'Kimi 最新旗舰' },
        { model: 'kimi-k2.7-code', imageCapable: true, title: 'kimi-k2.7-code', desc: 'Kimi 编码模型' },
        { model: 'deepseek-v4-pro-202606', legacyAliases: ['deepseek/deepseek-v4-pro-0813', 'deepseek/deepseek-v4-pro'], title: 'deepseek-v4-pro-202606', desc: '高能力模型' },
        { model: 'deepseek-v4-flash-202605', legacyAliases: ['deepseek/deepseek-v4-flash-0731', 'deepseek/deepseek-v4-flash'], title: 'deepseek-v4-flash-202605', desc: '快速响应' },
        { model: 'minimax-m3', legacyAliases: ['minimax-m-3-0'], imageCapable: true, title: 'minimax-m3', desc: 'MiniMax 最新旗舰' },
        { model: 'minimax-m2.7', legacyAliases: ['minimax-m-2-7'], title: 'minimax-m2.7', desc: '通用能力' },
        { model: 'hy3', legacyAliases: ['hy3-preview', 'hy3-202608'], title: 'hy3', desc: 'Hy 套餐专属模型' },
        { model: 'hy4-preview', title: 'hy4-preview', desc: 'Hy4 预览，高峰期可能限频' },
        { model: '', title: '自定义腾讯云 Token Plan 模型', desc: '手动填写 Token Plan 模型 ID', custom: true },
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
        { model: 'k3', imageCapable: true, title: 'k3', desc: 'K3 长上下文模型' },
        { model: 'k3-256k', imageCapable: true, title: 'k3-256k', desc: 'K3 256K 上下文，价格更低' },
        { model: 'kimi-for-coding', imageCapable: true, title: 'kimi-for-coding', desc: '标准编码模型' },
        { model: 'kimi-for-coding-highspeed', imageCapable: true, title: 'kimi-for-coding-highspeed', desc: '高速编码模型' },
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
      // 2026-09-11 官方口径（api-docs.deepseek.com/updates 与 pricing）：
      // V4.1-Flash（deepseek-flash）为当前主力，原生图片输入、1M 上下文；
      // deepseek-v4-pro 自 2026-09-14 12:00 起被路由到 V4.1-Flash 并按其计费
      // （至 V4.1 Pro 发布）；deepseek-v4-flash / -vision-exp 已退役、仅临时
      // 路由，旧行删除并以 legacyAliases 兼容存量配置归类。
      // deepseek-chat / deepseek-reasoner 别名已于 2026-07-24 停用，不再收录。
      // api.deepseeki.com 非官方域名（官方文档从未出现，社区按 typosquat
      // 处理），勿加入端点白名单。
      items: [
        { model: 'deepseek-flash', imageCapable: true, legacyAliases: ['deepseek-v4-flash', 'deepseek-v4-flash-vision-exp'], title: 'deepseek-flash', desc: 'V4.1-Flash 主力，1M 上下文，支持图片输入' },
        { model: 'deepseek-v4-pro', imageCapable: false, title: 'deepseek-v4-pro', desc: '2026-09-14 起自动路由至 deepseek-flash 计费' },
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
        { model: 'kimi-k3', imageCapable: true, title: 'kimi-k3', desc: '最新通用模型' },
        { model: 'kimi-k2.7-code', imageCapable: true, title: 'kimi-k2.7-code', desc: '代码场景' },
        { model: 'kimi-k2.7-code-highspeed', imageCapable: true, title: 'kimi-k2.7-code-highspeed', desc: '高速代码场景' },
        { model: 'kimi-k2.6', imageCapable: true, title: 'kimi-k2.6', desc: '稳定可用' },
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
        { model: 'kimi-k3', imageCapable: true, title: 'kimi-k3', desc: '最新通用模型' },
        { model: 'kimi-k2.7-code', imageCapable: true, title: 'kimi-k2.7-code', desc: '代码场景' },
        { model: 'kimi-k2.7-code-highspeed', imageCapable: true, title: 'kimi-k2.7-code-highspeed', desc: '高速代码场景' },
        { model: 'kimi-k2.6', imageCapable: true, title: 'kimi-k2.6', desc: '稳定可用' },
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
      // 2026-09-11 官方口径（docs.bigmodel.cn API enum / 定价页）：glm-5.3 为
      // 当前旗舰（API enum 默认值），强制思考（disabled 会报错），effort 仅
      // low/high/max；glm-5.3-flash 为多模态高性价比档。GLM-5.2 降为上代旗舰，
      // 其余行均在售。GLM-5.1 / 5-Turbo / 4.7 不支持 reasoning_effort。
      items: [
        { model: 'glm-5.3', imageCapable: false, title: 'glm-5.3', desc: '最新旗舰，强制思考' },
        { model: 'glm-5.3-flash', imageCapable: true, title: 'glm-5.3-flash', desc: '最新多模态高性价比' },
        { model: 'glm-5.2', imageCapable: false, title: 'glm-5.2', desc: '上代旗舰' },
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
      // z.ai 官方 wire id 全小写；GLM-5-Turbo 不存在于 z.ai（API enum 无此
      // 行），不收录。
      items: [
        { model: 'glm-5.3', imageCapable: false, title: 'glm-5.3', desc: '最新旗舰，强制思考' },
        { model: 'glm-5.3-flash', imageCapable: true, title: 'glm-5.3-flash', desc: '最新多模态高性价比' },
        { model: 'glm-5.2', imageCapable: false, title: 'glm-5.2', desc: '上代旗舰' },
        { model: 'glm-5.1', title: 'glm-5.1', desc: '兼容保留' },
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
      // 2026-09-11 官方口径（platform.minimaxi.com / platform.minimax.io）：
      // M3 为当前旗舰（1M 上下文、原生多模态）；M2.x 全系纯文本且思考不可关。
      // 官方文档现行中国域名为 api.minimax.cn/v1，api.minimaxi.com 仍存活
      // （同构 401 探活，2026-09-11），保持现状并在此备注；国际站 api.minimax.io
      // 与国内为两套独立账号/Key 体系。
      items: [
        { model: 'MiniMax-M3', imageCapable: true, title: 'MiniMax-M3', desc: '最新旗舰，1M 上下文多模态' },
        { model: 'MiniMax-M2.7', imageCapable: false, title: 'MiniMax-M2.7', desc: '通用能力' },
        { model: 'MiniMax-M2.7-highspeed', imageCapable: false, title: 'MiniMax-M2.7-highspeed', desc: '高速响应' },
        { model: 'MiniMax-M2.5', imageCapable: false, title: 'MiniMax-M2.5', desc: '官方已转 Legacy，兼容保留' },
        { model: 'MiniMax-M2.5-highspeed', imageCapable: false, title: 'MiniMax-M2.5-highspeed', desc: '官方已转 Legacy，兼容高速' },
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
        { model: 'MiniMax-M3', imageCapable: true, title: 'MiniMax-M3', desc: '最新旗舰，1M 上下文多模态' },
        { model: 'MiniMax-M2.7', imageCapable: false, title: 'MiniMax-M2.7', desc: '通用能力' },
        { model: 'MiniMax-M2.7-highspeed', imageCapable: false, title: 'MiniMax-M2.7-highspeed', desc: '高速响应' },
        { model: 'MiniMax-M2.5', imageCapable: false, title: 'MiniMax-M2.5', desc: '官方已转 Legacy，兼容保留' },
        { model: 'MiniMax-M2.5-highspeed', imageCapable: false, title: 'MiniMax-M2.5-highspeed', desc: '官方已转 Legacy，兼容高速' },
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
      // 2026-09-11 官方口径（mimo.mi.com 模型清单）：mimo-v2.5-pro 纯文本
      // （1M 上下文，默认深度思考）；多模态（图/音/视频理解）在 mimo-v2.5 上。
      // Token Plan 订阅 Key（tp-）须改用 https://token-plan-cn.xiaomimimo.com/v1。
      items: [
        { model: 'mimo-v2.5-pro', imageCapable: false, title: 'mimo-v2.5-pro', desc: '最新旗舰，1M 上下文' },
        { model: 'mimo-v2.5', imageCapable: true, title: 'mimo-v2.5', desc: '全模态理解（图片/视频）' },
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
      // 2026-09-11 官方口径（help.aliyun.com/zh/model-studio）：qwen3.8-max 为
      // 当前旗舰（1M，混合思考默认开启）；qwen3.8-flash 为当前快速主力。
      // qwen3.7-max 已归旧版分区且纯文本（不支持图像）；qwen3.7-flash 仍可调
      // 但主推位已由 qwen3.8-flash 接棒。qwen3.8-max-preview 已下线请求自动
      // 路由至正式版，不收录。
      items: [
        { model: 'qwen3.8-max', imageCapable: true, title: 'qwen3.8-max', desc: '最新旗舰' },
        { model: 'qwen3.8-flash', imageCapable: true, title: 'qwen3.8-flash', desc: '快速高性价比' },
        { model: 'qwen3.7-max', imageCapable: false, title: 'qwen3.7-max', desc: '上代旗舰推理（纯文本）' },
        { model: 'qwen3.7-plus', imageCapable: true, title: 'qwen3.7-plus', desc: '均衡性价比' },
        { model: 'qwen3.7-flash', imageCapable: true, title: 'qwen3.7-flash', desc: '上代快速款' },
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
      // ap-southeast-1 端点属于国际站 Token Plan（仅新加坡地域），与中国版是
      // 独立订阅、Key（sk-sp-）不互通。行单按 2026-09-11 个人版/团队版白名单
      // 核对；「夜间五折」（22:00–次日 08:00）为个人版文档口径，团队版当前
      // 仅列 DeepSeek 两款。deepseek-v4-flash-0731 暂不支持 Responses API。
      items: [
        { model: 'qwen3.8-max', imageCapable: true, title: 'qwen3.8-max', desc: '正式旗舰，夜间 22:00-08:00 五折（个人版）' },
        { model: 'qwen3.8-flash', imageCapable: true, title: 'qwen3.8-flash', desc: '快速高性价比' },
        { model: 'qwen3.7-max', imageCapable: false, title: 'qwen3.7-max', desc: '上代旗舰推理' },
        { model: 'qwen3.7-plus', imageCapable: true, title: 'qwen3.7-plus', desc: '均衡性价比' },
        { model: 'qwen3.6-flash', imageCapable: true, title: 'qwen3.6-flash', desc: '轻量兼容款，支持图像输入' },
        { model: 'glm-5.2', title: 'glm-5.2', desc: '最新推荐' },
        { model: 'deepseek-v4-pro', title: 'deepseek-v4-pro', desc: '高能力模型' },
        { model: 'deepseek-v4-flash-0731', title: 'deepseek-v4-flash-0731', desc: '快速响应，暂不支持 Responses API' },
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
      // qwen3.7-flash 未见于国际站目录（2026-09-11 核查），不收录。
      items: [
        { model: 'qwen3.8-max', imageCapable: true, title: 'qwen3.8-max', desc: '最新旗舰' },
        { model: 'qwen3.8-flash', imageCapable: true, title: 'qwen3.8-flash', desc: '快速高性价比' },
        { model: 'qwen3.7-max', imageCapable: false, title: 'qwen3.7-max', desc: '上代旗舰推理（纯文本）' },
        { model: 'qwen3.7-plus', imageCapable: true, title: 'qwen3.7-plus', desc: '均衡性价比' },
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
      // 2026-09-11 官方口径（volcengine docs 82379/1330310、2549861）：五个
      // 现役行拼写逐字一致、能力列均含多模态理解；doubao-seed-evolving 是一个
      // Model ID 周级滚动升级的官方首推 Coding/Agent 模型；无 2-2 系。新增
      // 编程特化预览 doubao-seed-2-0-code-preview-260215（多模态未见明确口径，
      // 不标图片能力）。思考控制走 thinking.type + reasoning_effort。
      items: [
        { model: 'doubao-seed-evolving', imageCapable: true, title: 'doubao-seed-evolving', desc: '最新推荐，周级滚动升级' },
        { model: 'doubao-seed-2-1-pro-260628', imageCapable: true, title: 'doubao-seed-2-1-pro-260628', desc: '高能力模型' },
        { model: 'doubao-seed-2-1-turbo-260628', imageCapable: true, title: 'doubao-seed-2-1-turbo-260628', desc: '低成本低时延，效果比肩 2-1-pro' },
        { model: 'doubao-seed-2-0-code-preview-260215', title: 'doubao-seed-2-0-code-preview-260215', desc: '编程特化（预览）' },
        { model: 'doubao-seed-2-0-pro-260215', imageCapable: true, title: 'doubao-seed-2-0-pro-260215', desc: '稳定通用' },
        { model: 'doubao-seed-2-0-lite-260428', imageCapable: true, title: 'doubao-seed-2-0-lite-260428', desc: '轻量模型' },
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
      // 2026-09-11 官方口径（developers.openai.com/api/docs/models）：gpt-6-astra
      // 为当前最强旗舰，但其 Chat Completions 不支持函数调用（工具调用需
      // Responses 协议）——品悟 openai 预设走 Chat wire，故收录但不作默认、
      // desc 明示限制。gpt-5.6-sol/terra/luna 定位与官方一致；gpt-5.5 /
      // gpt-5.4-mini 在售未弃用。gpt-5.3-codex 为 Responses 专用，不收录。
      items: [
        { model: 'gpt-6-astra', imageCapable: true, title: 'gpt-6-astra', desc: '最强旗舰；仅 Responses 协议支持函数调用' },
        { model: 'gpt-5.6-sol', imageCapable: true, title: 'gpt-5.6-sol', desc: 'GPT-5.6 家族旗舰，推理与编码' },
        { model: 'gpt-5.6-terra', imageCapable: true, title: 'gpt-5.6-terra', desc: '均衡智能与成本' },
        { model: 'gpt-5.6-luna', imageCapable: true, title: 'gpt-5.6-luna', desc: '低成本高并发' },
        { model: 'gpt-5.5', imageCapable: true, title: 'gpt-5.5', desc: '上代旗舰' },
        { model: 'gpt-5.4-mini', imageCapable: true, title: 'gpt-5.4-mini', desc: '快速经济' },
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
      // 2026-09-11 官方口径（platform.claude.com models overview）：
      // claude-fable-5-1 为当前最高旗舰，claude-fable-5 降为上代（在售至少到
      // 2027-06）；全部现役模型支持图片输入（底座 bundled 离线种子记为纯文本
      // 属过时，以官方为准）。4.6 代起无日期 ID 即固定快照；haiku-4-5 仍是
      // 200K 上下文、不支持 effort（仅 extended thinking）。
      items: [
        { model: 'claude-fable-5-1', imageCapable: true, title: 'claude-fable-5-1', desc: '最强旗舰，高难推理与长程 Agent' },
        { model: 'claude-fable-5', imageCapable: true, title: 'claude-fable-5', desc: '上代旗舰，兼容保留' },
        { model: 'claude-opus-5', imageCapable: true, title: 'claude-opus-5', desc: '复杂 Agent 编码，默认推荐' },
        { model: 'claude-sonnet-5', imageCapable: true, title: 'claude-sonnet-5', desc: '速度与智能均衡' },
        { model: 'claude-haiku-4-5', imageCapable: true, title: 'claude-haiku-4-5', desc: '最快，200K 上下文' },
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
      // 2026-09-11 官方口径（ai.google.dev models / deprecations）：
      // gemini-3.8-flash（2026-09-02）为最新 Flash；3.7（2026-08-13）/3.6/3.5
      // 均 Stable 在售，3.5 已被官方称为 legacy 基线；gemini-3.1-pro-preview
      // 仍是旗舰 Pro 且 ID 未 GA 化（gemini-3-pro-preview 已于 2026-03-09 下线）。
      // Gemini 3 系在 OpenAI 兼容层不能关思考。
      items: [
        { model: 'gemini-3.8-flash', imageCapable: true, title: 'gemini-3.8-flash', desc: '最新 Flash，均衡高性价比' },
        { model: 'gemini-3.7-flash', imageCapable: true, title: 'gemini-3.7-flash', desc: '上一代 Flash' },
        { model: 'gemini-3.6-flash', imageCapable: true, title: 'gemini-3.6-flash', desc: '兼容保留' },
        { model: 'gemini-3.5-flash', imageCapable: true, title: 'gemini-3.5-flash', desc: '基线速度，兼容保留' },
        { model: 'gemini-3.5-flash-lite', imageCapable: true, title: 'gemini-3.5-flash-lite', desc: '快速经济' },
        { model: 'gemini-3.1-pro-preview', imageCapable: true, title: 'gemini-3.1-pro-preview', desc: '旗舰推理（预览）' },
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
      // 2026-09-11 官方口径（docs.x.ai models 与各模型详情页）：grok-4.6 为
      // 「包括代码在内的一切」推荐旗舰（500K，effort low/medium/high/xhigh，
      // 推理不可关）；grok-4.5 降为上代；grok-4.20-0309-* 与 grok-build-0.1
      // 详情页均明示 text, image → text，图片能力可标。
      items: [
        { model: 'grok-4.6', imageCapable: true, title: 'grok-4.6', desc: '旗舰，编码与 Agent 默认推荐' },
        { model: 'grok-4.5', imageCapable: true, title: 'grok-4.5', desc: '上代旗舰，编码与 Agent' },
        { model: 'grok-4.20-0309-reasoning', imageCapable: true, title: 'grok-4.20-0309-reasoning', desc: '4.20 推理，1M 上下文' },
        { model: 'grok-4.20-0309-non-reasoning', imageCapable: true, title: 'grok-4.20-0309-non-reasoning', desc: '4.20 非推理，1M 上下文' },
        { model: 'grok-4.3', imageCapable: true, title: 'grok-4.3', desc: '快速可靠，强工具调用' },
        { model: 'grok-build-0.1', imageCapable: true, title: 'grok-build-0.1', desc: '代码 Agent，256K 上下文' },
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
  return raw.replace(/\/+$/, ''); // eslint-disable-line sonarjs/super-linear-regex -- trailing-slash normalization; input is a user-entered URL of bounded length
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
    return !providerKind && !vendor && provider.preset === model.preset && provider.items.some(item => !item.custom && catalogItemMatchesModel(item, model.model));
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
// 目录命中默认精确比较:本地 vLLM 等服务的模型 ID 是不透明字符串、可能区分
// 大小写,case-only 的自定义 ID 必须保持自定义。大小写兼容只对发生过「目录
// 拼写迁移」的目录项生效,且以 legacyAliases 显式列出历史拼写(存量值只能
// 来自旧目录行的精确值,精确别名即可覆盖),不做全量 case-insensitive。
function catalogItemMatchesModel(item, model) {
  if (typeof item.model !== 'string' || typeof model !== 'string') return false;
  return item.model === model || (item.legacyAliases || []).includes(model);
}

// 目录项视觉能力标注(imageCapable):已验证多模态的条目标注 true,官方明示纯
// 文本的旗舰行标注 false,模型表单按此预填「图片输入能力」。收录原则与后端
// 内置已验证表(image_capability.rs VERIFIED_IMAGE_CAPABLE_MODELS)一致:只标
// 有仓内 preset/公开事实/实测佐证的明确口径;拿不准的不标——未标注不等于
// 「不支持」,只是回退「自动处理」链(内置表→Unknown)。新增目录条目时请同步
// 评估是否需要标注。
// 现有标注于 2026-09-11 按各厂商官方文档逐条联网核查:Claude 现役全系、
// Gemini 全系、Grok 全部目录行(含 grok-4.5 / grok-4.20-0309-* 详情页明示
// text, image → text)、GPT-5.x/6 目录行、kimi-k3 / k2.7-code / k2.6 / Kimi
// Code 四行、deepseek-flash(V4.1)、MiniMax-M3、qwen3.8-max/3.8-flash/
// 3.7-plus/3.7-flash/3.6-flash、豆包五行现役模型、glm-5.3-flash、mimo-v2.5
// 均为官方多模态;qwen3.7-max、glm-5.2/5.3、deepseek-v4-pro、MiniMax-M2.x、
// mimo-v2.5-pro 官方明示纯文本,标 false。
// 返回 true/false(条目显式标注)/null(未命中或未标注)。
function catalogImageCapableForModel(model) {
  if (typeof model !== 'string' || !model) return null;
  for (const scope of ['local', 'cloud']) {
    for (const group of MODEL_CATALOG[scope] || []) {
      for (const item of group.items || []) {
        if (item.custom || !catalogItemMatchesModel(item, model)) continue;
        if (item.imageCapable === true) return true;
        if (item.imageCapable === false) return false;
      }
    }
  }
  return null;
}

function isPresetModel(m) {
  if (!m || !m.model) return false;
  if (m.preset === 'local_vllm') {
    return (MODEL_CATALOG.local || []).some(group =>
      (group.items || []).some(item => !item.custom && catalogItemMatchesModel(item, m.model)));
  }
  const providerKind = m.provider_kind || m.providerKind;
  if (providerKind === PROVIDER_KIND_CUSTOM) return false;
  const provider = findCloudProviderForModel(m);
  return !!provider && provider.providerKind !== PROVIDER_KIND_CUSTOM
    && (provider.items || []).some(item => !item.custom && catalogItemMatchesModel(item, m.model));
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
  const alias = m.preset === 'local_vllm' ? '' : String(m.alias || '').trim();
  if (alias) return alias;
  const localModelNameFn = t && t.uiSettingsDetail && t.uiSettingsDetail.localModelName;
  if (localUserNamed(m, localModelNameFn)) return m.name;
  if (m.preset === 'local_vllm' && isPresetModel(m) && typeof localModelNameFn === 'function') {
    return localModelNameFn(m.model);
  }
  return isPresetModel(m) ? (m.name || m.model) : (m.model || m.name);
}

function selectorSubLabel(m, t) {
  if (!m) return '';
  if (m.preset !== 'local_vllm' && String(m.alias || '').trim()) return m.model || m.name || '';
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
  // 本地 loopback OpenAI 兼容端点：Rust 探测后走 Ollama think 开关或 vLLM 档位。
  // 四档覆盖 vLLM 语义；Ollama 由底座把 low/medium/high 归一为 think=true（只有开关）。
  local: ['off', 'low', 'medium', 'high'],
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
  // openai：仅 gpt-5.x reasoning 系模型底座会注入，off=none。max 档在 gpt-5.6
  // 系发 "max"，gpt-5.5 / codex 系被底座降级为 "xhigh"（chat.rs
  // openai_compatible_reasoning_effort），前端统一以 max 标签暴露。
  openai: ['off', 'low', 'medium', 'high', 'max'],
  // xai：底座 apply_xai_grok_4_6_reasoning_effort 仅对精确 api.x.ai/v1 的
  // grok-4.6 / grok-4.5 注入 reasoning_effort；Grok 推理不可关（off 被归一为
  // high），故不暴露 off。基础三档，grok-4.6 另加 max（wire 发 xhigh），见
  // reasoningEffortTiersForModel。
  xai: ['low', 'medium', 'high'],
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
  return ['gpt-5.6', 'gpt-5.6-sol', 'gpt-5.6-terra', 'gpt-5.6-luna'].includes(lower);
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
// 2. xai 返回：Rust 与前端都返回 "xai"；底座只对精确 api.x.ai/v1 的 grok-4.6 /
//    grok-4.5 注入档位（chat.rs apply_xai_grok_4_6_reasoning_effort），其余
//    Grok 型号与非官方端点前端回落 null（不提供切换）。
// 3. qwen/gemini 归类：Rust 将 qwen/tencent/openai/gemini/google 归入 "openai"
//    （wire route 身份）；前端对 qwen/tencent/gemini/google 返回 null（底座无档位），
//    仅 openai vendor 的 reasoning 家族返回 "openai"（对齐底座
//    `model_is_openai_reasoning_family`）。
// 4. zai/moonshot/minimax 路由级档位：底座按「精确 first-party base_url + 模型名」
//    判定 tiered effort（zai GLM-5.2/5.3/5.3-Flash、moonshot K3 含 k3-256k、
//    MiniMax-M3）；前端同样按精确端点身份判定（见 reasoningEffortTiersForModel
//    与 is_exact_*_base_url）。兼容网关/误配同型号时底座 fail-closed（不注入），
//    前端回落通用档位，与底座行为对齐。
// 对齐 bridge.rs `is_official_deepseek_base_url`：官方 DeepSeek 端点判定
// （trim 尾斜杠 + /beta + /v1，小写比较）。api.deepseeki.com 非官方域名
// （官方文档从未出现，社区按 typosquat 处理，2026-09-11 核查），已移除。
function isOfficialDeepseekBaseUrl(baseUrl) {
  const normalized = String(baseUrl || '')
    .trim()
    .replace(/\/+$/, '') // eslint-disable-line sonarjs/super-linear-regex -- trailing-slash normalization; input is a user-entered URL of bounded length
    .replace(/\/beta$/, '')
    .replace(/\/v1$/, '')
    .toLowerCase();
  return normalized === 'https://api.deepseek.com';
}

// 底座对 moonshot/zai/minimax/xai 的 tiered effort 只按「精确 first-party base_url + 模型名」
// 路由（CodeWhale config::is_exact_direct_moonshot_k3_route / is_exact_kimi_code_k3_route /
// is_exact_zai_tiered_effort_route / is_exact_minimax_m3_route / is_exact_xai_grok_4_6_route）。
// 复刻底座 `is_exact_https_route` 的比较语义：scheme/host ASCII 大小写不敏感、path 大小写
// 敏感、只容忍一个尾斜杠——不多删斜杠、也不整段转小写（不同大小写的 path 是相邻路由，
// 不是官方端点）。兼容网关误配同型号时底座 fail-closed（不注入），前端据此收窄档位暴露。
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
// Moonshot 直连平台端点：底座 provider.rs is_exact_moonshot_platform_route 同时
// 接受国际站 https://api.moonshot.ai/v1 与中国站 https://api.moonshot.cn/v1
//（品悟「Kimi 中国版」组默认端点即后者）。
function isExactMoonshotPlatformBaseUrl(baseUrl) {
  return isExactHttpsRoute(baseUrl, 'api.moonshot.ai', 'v1')
    || isExactHttpsRoute(baseUrl, 'api.moonshot.cn', 'v1');
}
// Kimi Code 会员计划端点：https://api.kimi.com/coding/v1（裸 k3 / k3-256k）
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
// xAI 官方端点（底座 provider.rs is_exact_xai_platform_route）：https://api.x.ai/v1
function isExactXaiPlatformBaseUrl(baseUrl) {
  return isExactHttpsRoute(baseUrl, 'api.x.ai', 'v1');
}

// vendor 在已知列表 → 返回其 provider（可能为 null = 底座无档位）；
// vendor 未知（如用户给本地服务填了自定义 vendor）→ 返回 undefined，落到 preset 兜底
// （与 Rust provider() 的 vendor→preset 回退一致）。
// Sentinel: unknown vendor (callers use it to distinguish "known but no tiers
// (null)" from "unknown → preset fallback").
const VENDOR_UNHANDLED = Symbol('vendor-unhandled');
function vendorReasoningProvider(vendor, model) {
  if (vendor === 'deepseek') return 'deepseek';
  if (['kimi', 'moonshot'].includes(vendor)) return 'moonshot';
  if (['glm', 'zai', 'zhipu'].includes(vendor)) return 'zai';
  if (vendor === 'minimax') return 'minimax';
  if (['mimo', 'xiaomi', 'xiaomi-mimo'].includes(vendor)) return 'xiaomi-mimo';
  if (vendor === 'doubao' || vendor === 'volcengine') return 'volcengine';
  if (vendor === 'anthropic' || vendor === 'claude') return 'anthropic';
  if (vendor === 'xai' || vendor === 'grok') return 'xai'; // 精确路由细分见 reasoningEffortTiersForModel
  if (vendor === 'openai') return isOpenaiReasoningFamilyModel(model) ? 'openai' : null;
  if (['qwen', 'tencent', 'gemini', 'google'].includes(vendor)) {
    return null; // 底座无档位
  }
  return VENDOR_UNHANDLED; // unknown vendor → preset fallback
}

// 对齐 Rust bridge.rs `base_url_uses_loopback`：localhost / 127.0.0.0/8 /
// ::1（含 `::` 展开形式）。注意 `0.0.0.0` 不是回环地址（Rust
// `IpAddr::is_loopback()` 对 0.0.0.0 为 false），此处与 Rust 保持一致不视为
// 本地；IPv6 仅 `::1/128` 是回环，`::ffff:127.x`（IPv4-mapped）同样不是。
// 空地址或解析失败按非本地处理。
function baseUrlUsesLoopback(baseUrl) {
  if (!baseUrl) return false;
  try {
    // eslint-disable-next-line unicorn/prefer-string-replace-all -- strips at most one trailing dot; replaceAll with a global regex is equivalent but the rule mis-fires on the anchored pattern
    const host = new URL(baseUrl).hostname.replace(/^\[|\]$/g, '').replace(/\.$/, '');
    if (host.toLowerCase() === 'localhost') return true;
    if (host.includes(':')) return isIpv6Loopback(host);
    const octets = host.split('.').map(Number);
    return octets.length === 4
      && octets.every((n) => Number.isSafeInteger(n) && n >= 0 && n <= 255)
      && octets[0] === 127;
  } catch {
    return false;
  }
}

// 对齐 Rust bridge.rs `base_url_uses_local_or_private`：loopback / RFC1918 私网
// （10/8、172.16/12、192.168/16）/ Docker 宿主别名（host.docker.internal 等）。
// 这些端点通常跑在用户自己的机器/内网，探测成本低且值得默认关思考；公网
// OpenAI 兼容端点不在此列（保持默认 high）。与 `baseUrlUsesLoopback` 的区别：
// 后者仅用于「允许无鉴权」判定，本判定覆盖探测与思考控制范围。
// 回环部分复用 `baseUrlUsesLoopback`，本函数只补 Docker 别名与 RFC1918，
// 避免两份回环规则漂移。
function baseUrlUsesLocalOrPrivate(baseUrl) {
  if (baseUrlUsesLoopback(baseUrl)) return true;
  if (!baseUrl) return false;
  try {
    // eslint-disable-next-line unicorn/prefer-string-replace-all -- strips at most one trailing dot; replaceAll with a global regex is equivalent but the rule mis-fires on the anchored pattern
    const host = new URL(baseUrl).hostname.replace(/^\[|\]$/g, '').replace(/\.$/, '');
    const lower = host.toLowerCase();
    if (lower === 'host.docker.internal'
      || lower === 'host.lima.internal'
      || lower === 'host.orbstack.internal'
      || lower.endsWith('.docker.internal')) return true;
    const octets = host.split('.').map(Number);
    if (octets.length !== 4 || octets.some((n) => !(Number.isSafeInteger(n) && n >= 0 && n <= 255))) {
      return false;
    }
    return octets[0] === 10
      || (octets[0] === 172 && octets[1] >= 16 && octets[1] <= 31)
      || (octets[0] === 192 && octets[1] === 168);
  } catch {
    return false;
  }
}

// IPv6 回环判定：把 `::` 展开为完整 8 组十六进制后与 `::1` 的完整形式
// （0000:0000:0000:0000:0000:0000:0000:0001）比较——与 Rust
// `IpAddr::is_loopback()`（仅 ::1/128）对齐。展开采用 pad 形式，`::0001`
// 等带前导零的合法写法同样命中。
function isIpv6Loopback(host) {
  const expanded = expandIpv6(host);
  const IPV6_LOOPBACK_EXPANDED = '0000:0000:0000:0000:0000:0000:0000:0001'; // eslint-disable-line sonarjs/no-hardcoded-ip -- exact expanded ::1 form is the defined loopback value being compared against, not a routable hard-coded address
  return expanded === IPV6_LOOPBACK_EXPANDED;
}

// 把 IPv6 地址展开为完整 8 组小写十六进制；`::` 按 RFC 4291 用零组补齐。
// 解析失败（组数非法/非 IPv6）返回 null。
function expandIpv6(host) {
  if (host.includes('::')) {
    const [left, right] = host.split('::');
    const l = left ? left.split(':') : [];
    const r = right ? right.split(':') : [];
    if (l.length + r.length >= 8) return null;
    const zeros = Array.from({ length: 8 - l.length - r.length }, () => '0');
    return [...l, ...zeros, ...r]
      .map((g) => g.padStart(4, '0').toLowerCase())
      .join(':');
  }
  const groups = host.split(':');
  if (groups.length !== 8) return null;
  return groups.map((g) => g.padStart(4, '0').toLowerCase()).join(':');
}

// 品悟 provider 判定（对齐 bridge.rs `provider()`：vendor 优先 + preset 兜底）。
function reasoningProviderForModel(model) {
  if (!model) return null;
  // 对齐 Rust provider() 优先级：官方 deepseek base_url 优先（即使 preset 是
  // openai_compatible 且无 vendor，只要指向官方 deepseek 端点即按 deepseek 暴露档位）。
  if (isOfficialDeepseekBaseUrl(model.base_url)) return 'deepseek';
  const vendor = (model.vendor || '').trim().toLowerCase();
  const preset = model.preset || '';
  if (preset === 'local_vllm') return 'vllm';
  if (vendor) {
    const provider = vendorReasoningProvider(vendor, model);
    if (provider !== VENDOR_UNHANDLED) return provider;
    // 未知 vendor：继续走 preset 兜底（与 Rust provider() 的 vendor→preset 回退一致）。
  }
  switch (preset) {
    case 'deepseek': return 'deepseek';
    case 'kimi': return 'moonshot';
    case 'glm': return 'zai';
    case 'minimax': return 'minimax';
    case 'mimo': return 'xiaomi-mimo';
    case 'doubao': return 'volcengine';
    case 'anthropic': return 'anthropic';
    case 'xai': return 'xai';
    case 'openai': return isOpenaiReasoningFamilyModel(model) ? 'openai' : null;
    case 'openai_compatible':
      // 本地/私网端点（loopback、RFC1918、host.docker.internal 等）：Rust
      // 探测后 Ollama/vLLM 思考控制真正生效（Ollama→think 开关、vLLM→档位）；
      // LM Studio/通用端点 wire 层空操作（前端按探测结果另行提示）。
      // 远端自定义 OpenAI 兼容端点不提供切换。
      return baseUrlUsesLocalOrPrivate(model.base_url || model.baseUrl) ? 'local' : null;
    default: return null;
  }
}

// Local-deployment knowledge table (fallback) for "thinking always on" models.
// Only applies to local routes (vllm / local loopback endpoints / probed
// ollama); exact cloud routes do not use this table.
// Basis:
// - Kimi K3 is always-thinking, official effort tiers low/high/max; the engine
//   wire layer clamps max to high, so only low/high are exposed.
// - GLM-5.3 / GLM-4.7 are always-thinking and likewise expose low/high.
// - GPT-OSS thinking cannot be disabled; officially only three tiers low/medium/high.
// - kimi-k2-thinking / kimi-k2.5-thinking / kimi-k2.7 / deepseek-r1 /
//   minimax-m2 / qwen3 thinking family: thinking cannot be disabled and has no
//   tier control (noControl).
// When a framework (probe result) explicitly reports thinking can be disabled,
// the framework wins — this table only overlays as a fallback during effort
// tier resolution on local routes; it never overrides exact cloud routes.
// modelId is lowercased with `_`/whitespace normalized to `-`, then substring-matched.
// Accepted risk: substring matching also covers future names (e.g. a future
// kimi-k3.5 matches the kimi-k3 entry); entries are re-reviewed as new models ship.
// GLM-5.3 scope note: this table applies to local routes only; the cloud exact
// route (z.ai first-party) deliberately keeps its own ['off','high','max'] tiers
// from the hosted API contract.
function alwaysThinkingSpecForModel(modelId) {
  const normalized = String(modelId || '').trim().toLowerCase().replaceAll(/[\s_]+/g, '-');
  if (!normalized) return null;
  if (normalized.includes('kimi-k3')) return { tiers: ['low', 'high'] };
  if (normalized.includes('glm-5.3') || normalized.includes('glm-4.7')) return { tiers: ['low', 'high'] };
  if (normalized.includes('gpt-oss')) return { tiers: ['low', 'medium', 'high'] };
  if (normalized.includes('kimi-k2-thinking')
    || normalized.includes('kimi-k2.5-thinking')
    || normalized.includes('kimi-k2.7')
    || normalized.includes('deepseek-r1')
    || normalized.includes('minimax-m2')
    || (normalized.includes('qwen3') && normalized.includes('thinking'))) {
    return { noControl: true };
  }
  return null;
}

// 该模型可切换的思考深度档位（无则 null = 不提供切换）。
// 路由/模型级细分（仅品悟目录收录的模型）：
// - zai：first-party z.ai 端点上 GLM-5.2/5.3/5.3-Flash 提供 tiered effort（off/high/max），
//   GLM-5.1/GLM-5-Turbo 只有 generic thinking 开关（off/high）；中国 open.bigmodel.cn、
//   兼容网关、未验证模型底座会删除 thinking/reasoning_effort（两档等效）→ 不提供切换。
//   注意：厂商文档称 GLM-5.3 系 thinking.type 仅接受 enabled（disabled 报错），底座
//   仍把 off 映射为 thinking disabled，属底座与厂商的分歧，前端按底座实际行为暴露。
// - moonshot：K3（直连 kimi-k3 / Kimi Code k3、k3-256k，always-thinking）提供 low/high/max
//   （off 归一为 low）；其余 moonshot 模型按 generic thinking 开关暴露 off/high。
// - minimax：仅 first-party MiniMax-M3 提供 off（disabled）/high（adaptive）；M2.7/M2.5
//   与兼容网关底座清空控制字段（两档等效）→ 不提供切换。
// - xai：仅精确 api.x.ai/v1 的 grok-4.6（low/medium/high/max，max 在 wire 上发 xhigh）
//   与 grok-4.5（low/medium/high，xhigh/max 被底座降级为 high 故不暴露）提供档位；
//   Grok 推理不可关（底座把 off 归一为 high），不暴露 off；其余型号与非官方端点 → null。
// 与底座 `is_exact_zai_tiered_effort_route` / `is_exact_direct_moonshot_k3_route` /
// `is_exact_kimi_code_k3_route` / `is_exact_minimax_m3_route` / `is_exact_xai_grok_4_6_route`
// 对齐：兼容网关误配同型号时底座 fail-closed，前端不再暴露无效或彼此等效的选项。
function reasoningEffortTiersForModel(model) {
  const provider = reasoningProviderForModel(model);
  if (!provider) return null;
  const tiers = REASONING_EFFORT_TIERS[provider];
  if (!tiers) return null;
  const modelName = String((model && model.model) || '').trim().toLowerCase();
  const baseUrl = (model && model.base_url) || '';
  // Local routes (vllm preset / local loopback openai_compatible endpoint) that
  // hit the always-thinking knowledge table: noControl → null (reusing the
  // "not adjustable" semantic exit); tiers → the knowledge-table tiers replace
  // the default tier table. Exact cloud routes (zai/moonshot/minimax) are
  // unaffected. (Probed ollama does not go through here: tiers for local
  // compatible endpoints are issued by localReasoningTiers, which overlays the
  // knowledge table on the probed kind.)
  const localSpec = ['vllm', 'local'].includes(provider)
    ? alwaysThinkingSpecForModel(model && model.model)
    : null;
  if (localSpec) return localSpec.noControl ? null : localSpec.tiers;
  if (provider === 'zai') {
    if (!isExactZaiChatBaseUrl(baseUrl)) return null;
    if (['glm-5.2', 'glm-5.3', 'glm-5.3-flash'].includes(modelName)) {
      return ['off', 'high', 'max'];
    }
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
  if (provider === 'xai') {
    if (!isExactXaiPlatformBaseUrl(baseUrl)) return null;
    if (modelName === 'grok-4.6') return [...tiers, 'max'];
    if (modelName === 'grok-4.5') return tiers;
    return null;
  }
  return tiers;
}

// 底座 K3（always-thinking）精确路由：直连平台 kimi-k3（api.moonshot.ai / .cn）与
// Kimi Code 的 k3 / k3-256k（底座 is_exact_kimi_code_k3_route 同时收录两者）。
// 仅这些「精确端点 + 模型名」组合会进入 tiered low/high/max 路由。
function isExactMoonshotK3Route(model, modelName) {
  const baseUrl = (model && model.base_url) || '';
  if (modelName === 'kimi-k3') return isExactMoonshotPlatformBaseUrl(baseUrl);
  if (modelName === 'k3' || modelName === 'k3-256k') return isExactKimiCodeBaseUrl(baseUrl);
  return false;
}

// 当前模型是否走底座 always-thinking K3 tiered 路由（档位表为 low/high/max）。
function isAlwaysThinkingK3Route(model) {
  if (!model || reasoningProviderForModel(model) !== 'moonshot') return false;
  const modelName = String((model && model.model) || '').trim().toLowerCase();
  return isExactMoonshotK3Route(model, modelName);
}

// 该模型的默认思考深度档位：本地模型（vLLM / 本地 loopback 端点）默认 off
// （防 SSE timeout / 思考 trace 抢占首包），其余 high。
function defaultReasoningEffortForModel(model) {
  const provider = reasoningProviderForModel(model);
  if (provider === 'vllm' || provider === 'local') {
    // Always-thinking models with controllable tiers (knowledge table): off is
    // unavailable, default to the lowest tier.
    const spec = alwaysThinkingSpecForModel(model && model.model);
    if (spec && spec.tiers) return spec.tiers[0];
    return 'off';
  }
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
// Local always-thinking knowledge-table models (tiers from alwaysThinkingSpecForModel)
// need no special case: stored values outside spec.tiers (e.g. off) fall
// through to the trailing default tier (spec.tiers[0]).
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

// 本地 OpenAI 兼容端点按探测服务类型可切换的档位。与 Rust
// `probe_local_server_kind` 结果对齐：
// - vllm → 底座 `chat_template_kwargs` 支持四档
// - sglang / llamacpp / koboldcpp / lmdeploy / dockermodelrunner → thinking
//   control wire is structurally identical to vLLM (chat_template_kwargs +
//   reasoning_effort passthrough); the tier set is the same as vllm
// - ollama → 底座 `think` 布尔开关：off=think:false，其余档位一律归一 think=true，
//   只暴露 off/high 避免「看起来不同、实际相同」的误导
// - lmstudio / generic → 底座 openai wire route 对 reasoning_effort 是空操作，
//   返回 null（前端显示「该端点不支持思考档位调节」提示，不提供切换）
// - null（尚未探测/探测失败）→ 返回默认四档，前端在探测完成前不提供误导档位
function localProbeTiersForKind(kind) {
  switch (kind) {
    case 'vllm':
    case 'sglang':
    case 'llamacpp':
    case 'koboldcpp':
    case 'lmdeploy':
    case 'dockermodelrunner':
      return ['off', 'low', 'medium', 'high'];
    case 'ollama': return ['off', 'high'];
    case 'lmstudio':
    case 'generic':
      return null;
    default:
      return ['off', 'low', 'medium', 'high'];
  }
}

// Overlay of probed tiers × the model knowledge table (shared by the
// SettingsView model-edit dialog and the chat input model popover): when the
// model hits the always-thinking knowledge table, the table wins — noControl →
// null (frontend shows the "thinking always on" hint rather than "probe
// unsupported"); tiers → override the probed tiers.
// Exception: on lmstudio/generic endpoints the engine openai wire route treats
// reasoning_effort as a no-op, so the knowledge-table tiers would equally
// change nothing — fall back to the probe result (null), no switching offered.
// On a miss, tiers are issued per the probe result.
function localReasoningTiers(modelId, probedKind) {
  const spec = alwaysThinkingSpecForModel(modelId);
  if (spec) {
    if (spec.noControl) return null;
    if (probedKind === 'lmstudio' || probedKind === 'generic') {
      return localProbeTiersForKind(probedKind);
    }
    // The engine ollama wire only has the boolean think (off=think:false, all
    // other tiers normalize to think:true) and never sends a tier string; for
    // always-thinking models on the ollama route the only meaningful exposure
    // is high (e.g. GPT-OSS only accepts the low/medium/high strings —
    // true/false is ignored, and thinking cannot be disabled anyway).
    if (probedKind === 'ollama') return ['high'];
    return spec.tiers;
  }
  return localProbeTiersForKind(probedKind);
}

// Visual fallback of stored tiers against the probed tier table: normalization
// uses the static four-tier table (local), but once ollama is probed only the
// off/high tiers render, so a stored low/medium would land on no button.
// Ollama's think boolean normalizes every non-off tier to the same wire value
// (think:true), semantically equal to high, so the highlight maps to the
// nearest tier, high (same for max; the core normalizes max to high).
// This only affects display comparison and never changes stored values (the
// original value survives switching back to a four-tier endpoint). Returns
// null when no tier was ever picked, the table is missing/empty, or the table
// truly has no high to land on (no highlight shown).
function reasoningEffortDisplayForTiers(effort, tiers) {
  if (!effort || !Array.isArray(tiers) || !tiers.length) return null;
  if (tiers.includes(effort)) return effort;
  return tiers.includes('high') ? 'high' : null;
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
  catalogItemMatchesModel,
  catalogImageCapableForModel,
  isPresetModel,
  groupModelsForSelector,
  localUserNamed,
  selectorMainLabel,
  selectorSubLabel,
  reasoningEffortTiersForModel,
  defaultReasoningEffortForModel,
  reasoningEffortForModelSwitch,
  normalizeStoredReasoningEffort,
  alwaysThinkingSpecForModel,
  localProbeTiersForKind,
  localReasoningTiers,
  reasoningEffortDisplayForTiers,
  baseUrlUsesLocalOrPrivate,
};
