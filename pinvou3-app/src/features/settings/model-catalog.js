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
// The OpenAI default only serves as the legacy migration fallback (the custom
// endpoint identity is unknown; keep the old flagship gpt-5.6-terra instead
// of following the official recommended slot, to avoid implying official
// endorsement).
// Default models (checked against each vendor's official docs on 2026-09-28;
// previous full re-check 2026-09-11):
// - deepseek: V4.1-Flash (deepseek-flash) is still the mainline; the 09-10
//   v4-pro phase-out news was superseded the same day by the changelog
//   decision to keep serving v4-pro with unchanged billing (re-verified
//   2026-09-28).
// - glm: GLM-5.3 launched on 2026-08-19 (China) / 2026-08-18 (z.ai), and is
//   the API enum default on both sites (docs.bigmodel.cn / docs.z.ai).
// - gemini: gemini-3.8-flash launched on 2026-09-02 as the successor
//   (ai.google.dev models page).
// - xai: grok-4.7 holds the official "coding/agent recommended" slot since
//   September 2026 (docs.x.ai models page: "For everything else, including
//   code, use Grok 4.7"); grok-4.6 demotes to previous generation.
// - openai: gpt-6-sol takes the default (2026-09-29): the whole preset rides
//   the OpenAI Responses wire now (bridge engine_route_provider), so the
//   Chat-only restriction that kept terra in the slot is gone. Every catalog
//   id lists `v1/responses` with function calling on its model page
//   (developers.openai.com/api/docs/models/*, re-verified 2026-09-29): the
//   gpt-6 pages' "Chat Completions supports function calling only with
//   reasoning_effort set to none" and astra's "tool calling requires
//   Responses" only constrained the Chat wire the preset used before. sol is
//   the balanced flagship ($2/$10; effort ladder covers every tier the
//   engine's Responses effort mapper sends). Off-tier note: the Responses
//   effort mapper has no "none" on this route (off normalizes to low), so
//   the picker exposes low/medium/high/max for the preset.
// - anthropic: claude-opus-5-5 (2026-09-22) takes the default per the
//   official models overview "start with Claude Opus 5.5 for most workloads".
// - mimo: mimo-v2.6-pro (2026-09-22 release) replaces the default; the whole
//   v2.5 family hard-retires 2026-10-21 with no auto-replacement
//   (mimo.mi.com deprecation page).
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
  mimo:        { baseUrl: 'https://api.xiaomimimo.com/v1',          model: 'mimo-v2.6-pro' },
  openai:      { baseUrl: 'https://api.openai.com/v1',              model: 'gpt-6-sol' },
  // Custom OpenAI Responses endpoints: protocol is /v1/responses (the bridge
  // lands on the foundation's named-custom route with wire="responses"). The
  // default URL/model prefill the official endpoint so it works out of the
  // box; users aiming at an aggregator change the model along with the URL.
  openai_responses: { baseUrl: 'https://api.openai.com/v1',         model: 'gpt-6-sol' },
  anthropic:   { baseUrl: 'https://api.anthropic.com/v1',           model: 'claude-opus-5-5' },
  gemini:      { baseUrl: 'https://generativelanguage.googleapis.com/v1beta/openai', model: 'gemini-3.8-flash' },
  xai:         { baseUrl: 'https://api.x.ai/v1',                    model: 'grok-4.7' },
};
const PROVIDER_KIND_CODING_PLAN = 'coding_plan';
const PROVIDER_KIND_OFFICIAL_API = 'official_api';
const PROVIDER_KIND_CUSTOM = 'custom';
// 模型拼写约定：凡底座（CodeWhale）route 目录收录的模型，列表项 `model` 一律
// always prefer the base models_dev.bundled.json catalog row's exact spelling;
// no case rule may be applied. Custom compatible endpoints (bigmodel.cn Coding
// Plan, the open platforms, Tencent/Alibaba Plans) and the
// modelstudio catalog (qwen_token_plan) keep their own lowercase wire ids.
// z.ai direct's official wire id is confirmed all-lowercase (docs.z.ai API
// enum, checked 2026-09), so its catalog rows use lowercase and keep the
// uppercase legacy; the base bundled assets still hold the uppercase rows
// (GLM-5.2 etc.), but since c0f749731 (2026-08-17, shipped with the current
// gitlink) the resolver provides a case-folding fallback for StrictDirect +
// Deepseek/Zai (exact first, unique hit within the provider), so configs
// saved lowercase can match safely.
// Whenever a catalog row's spelling changes (including case), the old
// spelling must be registered in that item's legacyAliases to keep classifying
// existing configs; everything else stays exact-compare.
const MODEL_CATALOG_SECTIONS = {
  coding_plan: 'Coding Plan',
  official_api: '官方 API',
  aggregator: '聚合平台',
  custom: '自定义兼容接口',
};
// preset key → i18n label key: direct lookup instead of materializing the
// preset option list just to read one label.
const PRESET_LABEL_KEY = {
  local_vllm: 'modelPresetLocalVllm',
  deepseek: 'modelPresetDeepseek',
  kimi: 'modelPresetKimi',
  openai_compatible: 'modelPresetOpenaiCompatible',
  openai_responses: 'modelPresetOpenaiResponses',
  qwen: 'modelPresetQwen',
  doubao: 'modelPresetDoubao',
  minimax: 'modelPresetMinimax',
  glm: 'modelPresetGlm',
  mimo: 'modelPresetMimo',
  openai: 'modelPresetOpenai',
  anthropic: 'modelPresetAnthropic',
  gemini: 'modelPresetGemini',
  xai: 'modelPresetXai',
};
function presetProviderLabel(preset, t) {
  const key = PRESET_LABEL_KEY[preset];
  return (key && t[key]) || preset;
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
  openai_responses: openaiIcon,
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
      // documentation's lowercase wire ids. Official figures re-checked
      // 2026-09-28 (docs.bigmodel.cn/cn/coding-plan/overview):
      // GLM-5.3 / GLM-5.3-Flash are native models on all plans; GLM-5.2 /
      // GLM-5.1 calls auto-switch to GLM-5.3 and GLM-5-Turbo/GLM-4.7
      // auto-switch to GLM-5.3-Flash, so the old rows stay as "legacy model"
      // entries rather than being deleted. GLM-5.3-FlashX is explicitly NOT
      // open on the plan (the official Flash page states the plan does not
      // currently offer GLM-5.3-FlashX).
      items: [
        { model: 'glm-5.3', imageCapable: false, title: 'GLM-5.3', desc: '旗舰编码模型，全套餐支持' },
        { model: 'glm-5.3-flash', imageCapable: true, title: 'GLM-5.3-Flash', desc: '原生多模态编码模型，额度三倍' },
        { model: 'glm-5.2', imageCapable: false, title: 'GLM-5.2', desc: '历史模型，请求自动切换至 GLM-5.3' },
        { model: 'glm-5.1', title: 'GLM-5.1', desc: '历史模型，请求自动切换至 GLM-5.3' },
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
      // z.ai's official wire ids are all-lowercase (docs.z.ai API enum,
      // re-checked 2026-09-28); existing configs may still hold the old
      // uppercase catalog value (GLM-5.2), recognized via legacyAliases.
      // Old models auto-route per z.ai's official figures
      // (docs.z.ai/devpack/overview, re-checked 2026-09-28): GLM-5.2/GLM-5.1
      // requests auto-route to GLM-5.3, GLM-4.7 auto-routes to
      // GLM-5.3-Flash. GLM-5-Turbo is absent from z.ai's current model
      // overview/pricing/API enum (the base bundled assets still hold that
      // uppercase historical row, which is not grounds for inclusion), and its
      // old row is deleted; existing GLM-5-Turbo configs fall back to the
      // custom classification (tier hints unaffected) and must be re-picked
      // as glm-5.3 / glm-5.3-flash manually.
      items: [
        { model: 'glm-5.3', imageCapable: false, title: 'GLM-5.3', desc: '旗舰编码模型，全套餐支持' },
        { model: 'glm-5.3-flash', imageCapable: true, title: 'GLM-5.3-Flash', desc: '原生多模态编码模型，额度三倍' },
        { model: 'glm-5.2', legacyAliases: ['GLM-5.2'], imageCapable: false, title: 'GLM-5.2', desc: '历史模型，请求自动路由至 GLM-5.3' },
        { model: 'glm-5.1', title: 'GLM-5.1', desc: '历史模型，请求自动路由至 GLM-5.3' },
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
      // Row lineup mirrors the live 130060 personal-plan table (re-checked
      // 2026-09-28; the page updated 2026-09-24); GLM-5/GLM-5.1 retire
      // 2026-10-09 per 130060 (the TokenHub platform list 130051 marks the
      // underlying models 2026-10-08, so treat end of 10-08 as the safe
      // cutoff). The old "high-load" flag on hy4-preview is gone from the
      // current 130060 table, so the note is dropped. DeepSeek rows
      // are first-party direct supply without SLA, per 130060.
      items: [
        { model: 'tc-code-latest', title: 'tc-code-latest', desc: '自动模型，智能路由' },
        { model: 'glm-5.3', legacyAliases: ['glm-5-3'], imageCapable: false, title: 'glm-5.3', desc: '旗舰推理与编码' },
        { model: 'glm-5.3-flash', imageCapable: true, title: 'glm-5.3-flash', desc: '多模态高性价比' },
        { model: 'glm-5.2', legacyAliases: ['glm-5-2'], imageCapable: false, title: 'glm-5.2', desc: '上代旗舰推理' },
        { model: 'glm-5.1', legacyAliases: ['glm-5-1'], title: 'glm-5.1', desc: '官方将于 2026-10-09 下线' },
        { model: 'glm-5', legacyAliases: ['glm-5-0'], title: 'glm-5', desc: '通用推理，官方将于 2026-10-09 下线' },
        { model: 'kimi-k3', imageCapable: true, title: 'kimi-k3', desc: 'Kimi 最新旗舰' },
        { model: 'kimi-k2.7-code', imageCapable: true, title: 'kimi-k2.7-code', desc: 'Kimi 编码模型' },
        { model: 'deepseek-v4-pro-202606', legacyAliases: ['deepseek/deepseek-v4-pro-0813', 'deepseek/deepseek-v4-pro'], title: 'deepseek-v4-pro-202606', desc: '高能力模型' },
        { model: 'deepseek-v4-flash-202605', legacyAliases: ['deepseek/deepseek-v4-flash-0731', 'deepseek/deepseek-v4-flash'], title: 'deepseek-v4-flash-202605', desc: '快速响应' },
        { model: 'minimax-m3', legacyAliases: ['minimax-m-3-0'], imageCapable: true, title: 'minimax-m3', desc: 'MiniMax 最新旗舰' },
        { model: 'minimax-m2.7', legacyAliases: ['minimax-m-2-7'], imageCapable: false, title: 'minimax-m2.7', desc: '通用能力' },
        { model: 'hy3', legacyAliases: ['hy3-preview', 'hy3-202608'], title: 'hy3', desc: 'Hy 套餐专属模型' },
        { model: 'hy4-preview', title: 'hy4-preview', desc: 'Hy4 预览' },
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
      // The overseas Kimi Code plan serves https://api.kimi.ai/coding/v1
      // (kimi.com/code/docs, checked 2026-09-28); registered as an
      // alias so saved configs classify into this group — note the alias
      // only classifies; the exact-route K3 effort gate still pins
      // api.kimi.com/coding/v1, so an overseas-host config gets no K3
      // tiers until the base learns the new host.
      endpointAliases: ['https://api.kimi.com/coding/v1/chat/completions', 'https://api.kimi.ai/coding/v1'],
      // kimi-for-coding is a stable alias whose underlying model rolls: it
      // became K2.8 Preview on 2026-09-11 with 1M context on all tiers
      // (kimi.com/code/docs models page), so the old "standard coding model,
      // smaller context" framing is gone. k3 / k3-256k keep their own ids;
      // the [1m] suffix variant is only for Claude Code-style clients.
      items: [
        { model: 'k3', imageCapable: true, title: 'k3', desc: 'K3 长上下文模型' },
        { model: 'k3-256k', imageCapable: true, title: 'k3-256k', desc: 'K3 256K 上下文，价格更低' },
        { model: 'kimi-for-coding', imageCapable: true, title: 'kimi-for-coding', desc: 'K2.8 Preview，全档 1M 上下文' },
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
      // Official figures re-checked on 2026-09-14 (the api-docs.deepseek.com
      // updates and pricing pages agree): V4.1-Flash (deepseek-flash) is the
      // current mainline with native image input and 1M context;
      // deepseek-v4-pro keeps serving past 09-14 per user requests with
      // unchanged billing (updates 2026-09-10 entry + pricing page footnote);
      // the news260910 announcement's "routed to V4.1-Flash billing from
      // 09-14" contradicts those two pages, so the verified two-page account
      // is recorded here. The pricing page states v4-pro Vision: Not
      // supported, so imageCapable:false holds under either account and will
      // be re-checked at the next refresh (when V4.1 Pro ships).
      // deepseek-v4-flash / -vision-exp are retired with temporary routing to
      // V4.1-Flash only, their old rows are deleted, and legacyAliases keeps
      // classifying existing configs.
      // The deepseek-chat / deepseek-reasoner aliases were retired on
      // 2026-07-24 and are no longer listed.
      // api.deepseeki.com is an unofficial domain (never listed in the official
      // documentation; the community reports it does not resolve,
      // deepseek-ai/awesome-deepseek-agent#311) — do not add it to the endpoint
      // allowlist.
      items: [
        { model: 'deepseek-flash', imageCapable: true, legacyAliases: ['deepseek-v4-flash', 'deepseek-v4-flash-vision-exp'], title: 'deepseek-flash', desc: 'V4.1-Flash 主力，1M 上下文，支持图片输入' },
        { model: 'deepseek-v4-pro', imageCapable: false, title: 'deepseek-v4-pro', desc: '在售；官方确认 2026-09-14 后继续提供且计费不变' },
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
      // Official figures re-checked on 2026-09-28 (docs.bigmodel.cn API enum /
      // pricing page): glm-5.3 is the current flagship (the text-model API
      // enum default) with forced thinking (disabled errors out) and effort
      // limited to low/high/max; glm-5.3-flash is the multimodal
      // cost-effective tier and glm-5.3-flashx the new multimodal speed tier
      // (200 tokens/s, 1M context, vision-enum default). GLM-5.2 drops to
      // previous-generation flagship, all other rows are still on sale.
      // GLM-5.1 / 5-Turbo / 4.7 do not support reasoning_effort.
      items: [
        { model: 'glm-5.3', imageCapable: false, title: 'glm-5.3', desc: '最新旗舰，强制思考' },
        { model: 'glm-5.3-flash', imageCapable: true, title: 'glm-5.3-flash', desc: '最新多模态高性价比' },
        { model: 'glm-5.3-flashx', imageCapable: true, title: 'glm-5.3-flashx', desc: '极速多模态，200 tokens/s' },
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
      // z.ai's official wire ids are all-lowercase; GLM-5-Turbo is absent from
      // z.ai's current API enum (the historical uppercase row in the base
      // bundled assets is not grounds for inclusion) and is not listed;
      // existing glm-5-turbo configs fall back to the custom classification
      // (tier hints unaffected), consistent with how this file's
      // glm_coding_plan_global group is handled. glm-5.3-flashx is z.ai's
      // vision-enum default (re-checked 2026-09-28) and, like bigmodel, is
      // not open on the Coding Plan endpoint.
      items: [
        { model: 'glm-5.3', imageCapable: false, title: 'glm-5.3', desc: '最新旗舰，强制思考' },
        { model: 'glm-5.3-flash', imageCapable: true, title: 'glm-5.3-flash', desc: '最新多模态高性价比' },
        { model: 'glm-5.3-flashx', imageCapable: true, title: 'glm-5.3-flashx', desc: '极速多模态，200 tokens/s' },
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
      // Official figures re-checked 2026-09-28 (platform.minimax.cn / .io
      // model intro + chat API reference): M3 stays the PAYG flagship (1M
      // context, native multimodal); MiniMax-M3.1-Flash-Preview is the newer
      // multimodal tier with real reasoning_effort tuning, but it is
      // temporarily only served via the Token Plan and MiniMax Code
      // subscriptions — a plain pay-as-you-go key may not invoke it (no
      // dated announcement page exists; per the current model-intro pages).
      // The whole M2.x family is text-only with thinking always on (M3
      // accepts thinking.type=disabled; effort tuning is M3.1-only). The
      // official China docs' primary domain moved to api.minimax.cn;
      // api.minimaxi.com still answers (401 liveness probe, 2026-09-28) and
      // stays the default because the base tiered route pins it, with the
      // new domain registered as an alias below — note the alias only
      // classifies configs into this group; the exact-route thinking gate
      // still pins the two legacy hosts, so a saved api.minimax.cn config
      // gets no M3 effort tiers until the base learns the new host.
      // International and China use separate account/key systems.
      endpointAliases: ['https://api.minimax.cn/v1'],
      items: [
        { model: 'MiniMax-M3', imageCapable: true, title: 'MiniMax-M3', desc: '最新旗舰，1M 上下文多模态' },
        { model: 'MiniMax-M3.1-Flash-Preview', imageCapable: true, title: 'MiniMax-M3.1-Flash-Preview', desc: '新预览旗舰，仅订阅渠道' },
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
        { model: 'MiniMax-M3.1-Flash-Preview', imageCapable: true, title: 'MiniMax-M3.1-Flash-Preview', desc: '新预览旗舰，仅订阅渠道' },
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
      // Official figures re-checked 2026-09-28 (mimo.mi.com model list +
      // deprecation page): the MiMo-V2.6 series launched 2026-09-22 and all
      // three tiers are omni-modal (text/image/audio/video input) with 1M
      // context; mimo-v2.5-pro is text-only and mimo-v2.5 multimodal. The
      // two v2.5 chat models (both listed below) hard-retire Beijing time
      // 2026-10-21 10:00 with NO system replacement (requests error out),
      // so the v2.6 rows lead and the v2.5 rows carry the retirement
      // notice.
      // Token Plan subscription keys (tp-/ttp-) must switch to the cluster
      // hosts (token-plan-cn / -sgp / -ams.xiaomimimo.com/v1); the console's
      // displayed URL is authoritative.
      items: [
        { model: 'mimo-v2.6-pro', imageCapable: true, title: 'mimo-v2.6-pro', desc: '最新旗舰，1M 上下文' },
        { model: 'mimo-v2.6-flash', imageCapable: true, title: 'mimo-v2.6-flash', desc: '全模态，低成本' },
        { model: 'mimo-v2.6-pro-ultraspeed', imageCapable: true, title: 'mimo-v2.6-pro-ultraspeed', desc: '旗舰效果，极速输出' },
        { model: 'mimo-v2.5-pro', imageCapable: false, title: 'mimo-v2.5-pro', desc: '官方将于 2026-10-21 下线' },
        { model: 'mimo-v2.5', imageCapable: true, title: 'mimo-v2.5', desc: '官方将于 2026-10-21 下线' },
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
      // Official figures as of 2026-09-11 (help.aliyun.com/zh/model-studio):
      // qwen3.8-max is the current flagship (1M, hybrid thinking on by
      // default); qwen3.8-flash is the current fast mainline.
      // qwen3.7-max has moved to the legacy section and is text-only (no
      // image input); qwen3.7-flash is still callable but its featured slot
      // has been taken over by qwen3.8-flash. qwen3.8-max-preview is retired
      // with requests auto-routing to the GA version, so it is not listed.
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
      // The ap-southeast-1 endpoint belongs to the international Token Plan
      // (Singapore region only), a separate subscription from the China
      // edition whose keys (sk-sp-) are not interchangeable. The list was
      // re-checked against the 2026-09-28 personal-plan allowlist
      // (token-plan-personal-overview); the night discount (22:00-08:00) is
      // now 40% off credits for qwen3.8-max AND qwen3.8-flash (was half
      // price, max only), while the DeepSeek rows keep 50% off. The team
      // plan's night list currently has three DeepSeek models
      // (v4-pro-0813 / v4-flash-0731 / v4.1-flash). No official per-model
      // Responses API support list is published, so no protocol claim is
      // made on the deepseek-v4-flash-0731 row. Gateway deployments are not
      // verified one by one, so glm/deepseek rows stay unannotated
      // (image capability falls back to the same-id official-group rows).
      items: [
        { model: 'qwen3.8-max', imageCapable: true, title: 'qwen3.8-max', desc: '正式旗舰，夜间 22:00-08:00 四折（个人版）' },
        { model: 'qwen3.8-flash', imageCapable: true, title: 'qwen3.8-flash', desc: '快速高性价比，夜间同样四折' },
        { model: 'qwen3.7-max', imageCapable: false, title: 'qwen3.7-max', desc: '上代旗舰推理' },
        { model: 'qwen3.7-plus', imageCapable: true, title: 'qwen3.7-plus', desc: '均衡性价比' },
        { model: 'qwen3.6-flash', imageCapable: true, title: 'qwen3.6-flash', desc: '轻量兼容款，支持图像输入' },
        { model: 'auto', title: 'auto', desc: '自动模型，智能路由' },
        { model: 'glm-5.2', title: 'glm-5.2', desc: '上代旗舰' },
        { model: 'glm-5.3', title: 'glm-5.3', desc: '旗舰推理与编码' },
        { model: 'deepseek-v4-pro', title: 'deepseek-v4-pro', desc: '高能力模型' },
        { model: 'deepseek-v4-pro-0813', title: 'deepseek-v4-pro-0813', desc: '高能力模型' },
        { model: 'deepseek-v4.1-flash', title: 'deepseek-v4.1-flash', desc: '快速响应' },
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
      // qwen3.7-flash is listed in the international site's full model list
      // (alibabacloud.com model-studio text-generation-model recommended
      // models section), re-checked and restored on 2026-09-11, re-verified
      // 2026-09-28; the models index page that previously justified deleting
      // the row is a curated 3-per-category page, not the full catalog.
      // qwen3.6-flash is now listed internationally too (legacy section with
      // its dated snapshot), so the row joins this group as well. Alibaba
      // now recommends workspace-dedicated base URLs
      // (https://{WorkspaceId}.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1);
      // this legacy shared domain is still documented as available.
      items: [
        { model: 'qwen3.8-max', imageCapable: true, title: 'qwen3.8-max', desc: '最新旗舰' },
        { model: 'qwen3.8-flash', imageCapable: true, title: 'qwen3.8-flash', desc: '快速高性价比' },
        { model: 'qwen3.7-max', imageCapable: false, title: 'qwen3.7-max', desc: '上代旗舰推理（纯文本）' },
        { model: 'qwen3.7-plus', imageCapable: true, title: 'qwen3.7-plus', desc: '均衡性价比' },
        { model: 'qwen3.7-flash', imageCapable: true, title: 'qwen3.7-flash', desc: '上代快速款' },
        { model: 'qwen3.6-flash', imageCapable: true, title: 'qwen3.6-flash', desc: '轻量兼容款，支持图像输入' },
        { model: '', title: '自定义通义模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'qwen_coding_plan',
      section: 'coding_plan',
      title: '通义千问 Coding Plan / Qwen Coding Plan',
      configTitle: '通义千问 Coding Plan',
      desc: '阿里百炼 Coding Plan 订阅接口',
      preset: 'qwen',
      providerKind: PROVIDER_KIND_CODING_PLAN,
      vendor: 'qwen',
      baseUrl: 'https://coding.dashscope.aliyuncs.com/v1',
      endpointAliases: ['https://coding-intl.dashscope.aliyuncs.com/v1'],
      // Alibaba Model Studio Coding Plan (help.aliyun.com/zh/model-studio/
      // coding-plan, checked 2026-09-28): a fixed-monthly subscription
      // (Pro ¥200/mo; the Lite tier closed to new purchases 2026-03-20)
      // separate from the Token Plan, usable only inside AI coding tools.
      // Keys are also sk-sp- prefixed. The endpoint serves exact-version
      // ids only; the rows are a deliberate subset of the official page
      // list (the page additionally recommends kimi-k2.5 — not mirrored
      // because Moonshot retired it platform-wide on 2026-08-31 — and
      // lists qwen3.5-plus / qwen3-max-2026-01-23 under "more models").
      // Gateway deployments are not verified one by one, so rows stay
      // unannotated.
      items: [
        { model: 'qwen3.7-plus', title: 'qwen3.7-plus', desc: '均衡性价比' },
        { model: 'qwen3.6-plus', title: 'qwen3.6-plus', desc: '均衡性价比' },
        { model: 'glm-5', title: 'glm-5', desc: '高能力模型' },
        { model: 'MiniMax-M2.5', title: 'MiniMax-M2.5', desc: '通用能力' },
        { model: 'qwen3-coder-plus', title: 'qwen3-coder-plus', desc: '代码场景' },
        { model: 'qwen3-coder-next', title: 'qwen3-coder-next', desc: '代码场景' },
        { model: 'glm-4.7', title: 'glm-4.7', desc: '通用能力' },
        { model: '', title: '自定义 Coding Plan 模型', desc: '手动填写 Coding Plan 模型 ID', custom: true },
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
      // Official figures re-checked 2026-09-28 (docs.volcengine.com/ark
      // model list + release announcements 1159178): doubao-seed-evolving is
      // the officially recommended Coding/Agent model — a permanent Model ID
      // whose underlying version auto-updates (no update cadence is
      // documented), so the old "rolls weekly" wording is gone. The 2-1 family gained -260915
      // pro/lite snapshots (2026-09); the -260628 rows and the 2-1 -260915
      // snapshots stay on sale, but every 2-0 -260215 snapshot on the model
      // list now carries a "coming offline soon" badge, so those rows
      // note the coming retirement. The coding-specialized preview
      // doubao-seed-2-0-code-preview-260215 also lists multimodal
      // understanding, so the image capability is annotated. Vendor docs now
      // document seven reasoning_effort modes (none…max; default high for
      // doubao-seed-evolving and the 2-1 generation, medium for 2-0); the
      // base still normalizes to the off/high/max exposure below, so no tier
      // change is made here.
      items: [
        { model: 'doubao-seed-evolving', imageCapable: true, title: 'doubao-seed-evolving', desc: '最新推荐，统一模型 ID 自动升级' },
        { model: 'doubao-seed-2-1-pro-260915', imageCapable: true, title: 'doubao-seed-2-1-pro-260915', desc: '高能力模型' },
        { model: 'doubao-seed-2-1-pro-260628', imageCapable: true, title: 'doubao-seed-2-1-pro-260628', desc: '高能力模型' },
        { model: 'doubao-seed-2-1-turbo-260628', imageCapable: true, title: 'doubao-seed-2-1-turbo-260628', desc: '低成本低时延，效果比肩 2-1-pro' },
        { model: 'doubao-seed-2-1-lite-260915', imageCapable: true, title: 'doubao-seed-2-1-lite-260915', desc: '轻量模型' },
        { model: 'doubao-seed-2-0-code-preview-260215', imageCapable: true, title: 'doubao-seed-2-0-code-preview-260215', desc: '编程特化（预览），官方即将下线' },
        { model: 'doubao-seed-2-0-pro-260215', imageCapable: true, title: 'doubao-seed-2-0-pro-260215', desc: '稳定通用，官方即将下线' },
        { model: 'doubao-seed-2-0-lite-260428', imageCapable: true, title: 'doubao-seed-2-0-lite-260428', desc: '轻量模型' },
        { model: '', title: '自定义豆包模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'volcengine_coding_plan',
      section: 'coding_plan',
      title: '火山方舟 Coding Plan / Volcengine Ark Coding Plan',
      configTitle: '火山方舟 Coding Plan',
      desc: '火山方舟编码套餐专用端点',
      preset: 'openai_compatible',
      providerKind: PROVIDER_KIND_CODING_PLAN,
      vendor: 'doubao',
      baseUrl: 'https://ark.cn-beijing.volces.com/api/coding/v3',
      // Ark Coding Plan (volcengine docs coding-plan-personal-get-started +
      // activity/codingplan, checked 2026-09-28): an Agent/Coding
      // subscription separate from pay-as-you-go /api/v3 — the official
      // quick start explicitly warns NOT to point coding tools at the plain
      // /api/v3 host. ark-code-latest is the console-managed Auto shell
      // model; the plan's real-time switchable model list below mirrors the
      // official page verbatim (Doubao/Kimi/GLM/DeepSeek/MiniMax included).
      // The same page also documents an Anthropic-compatible
      // https://ark.cn-beijing.volces.com/api/coding endpoint, unused by
      // this repo's OpenAI route.
      items: [
        { model: 'ark-code-latest', title: 'ark-code-latest', desc: 'Coding Plan 自动模型' },
        { model: 'doubao-seed-evolving', title: 'doubao-seed-evolving', desc: '最新推荐，统一模型 ID 自动升级' },
        { model: 'doubao-seed-2.1-pro', title: 'doubao-seed-2.1-pro', desc: '高能力模型' },
        { model: 'doubao-seed-2.1-lite', title: 'doubao-seed-2.1-lite', desc: '轻量模型' },
        { model: 'doubao-seed-2.0-mini', title: 'doubao-seed-2.0-mini', desc: '轻量模型' },
        { model: 'kimi-k3', title: 'kimi-k3', desc: 'Kimi 最新旗舰' },
        { model: 'kimi-k2.8-preview', title: 'kimi-k2.8-preview', desc: '代码场景' },
        { model: 'kimi-k2.7-code', title: 'kimi-k2.7-code', desc: '代码场景' },
        { model: 'glm-5.3', legacyAliases: ['glm-latest'], title: 'glm-5.3', desc: '旗舰推理与编码' },
        { model: 'glm-5.3-flash', title: 'glm-5.3-flash', desc: '多模态高性价比' },
        { model: 'deepseek-v4.1-flash', title: 'deepseek-v4.1-flash', desc: '快速响应' },
        { model: 'deepseek-v4-flash', title: 'deepseek-v4-flash', desc: '快速响应' },
        { model: 'deepseek-v4-pro', title: 'deepseek-v4-pro', desc: '高能力模型' },
        { model: 'minimax-m3', title: 'minimax-m3', desc: 'MiniMax 最新旗舰' },
        { model: '', title: '自定义火山方舟 Coding Plan 模型', desc: '手动填写 Coding Plan 模型 ID', custom: true },
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
      // Official figures re-checked 2026-09-29 (developers.openai.com
      // models/pricing/guides + per-model endpoint tables): the GPT-6 family
      // expanded — gpt-6-sol ("built to power complex coding and agentic
      // workflows", $2/$10) and gpt-6-luna ($0.10/$0.50) joined gpt-6-astra.
      // The whole preset rides the OpenAI Responses wire since 2026-09-29
      // (bridge engine_route_provider → foundation named-custom route with
      // wire="responses"): every listed id's model page documents
      // `v1/responses` as Supported with function calling, so the Chat-only
      // restrictions that previously kept gpt-5.6-terra as the default
      // ("Chat Completions supports function calling only with
      // reasoning_effort set to none" on the gpt-6 pages; astra's "tool
      // calling requires Responses") no longer bind this preset. gpt-6-sol
      // is the default (balanced flagship; see the MODEL_PRESET_DEFS note
      // for the effort-ladder caveat: the engine's Responses effort mapper
      // has no "none" here, so the picker exposes low/medium/high/max).
      // The 5.6 family and gpt-5.5 / gpt-5.4-mini are on sale and not
      // deprecated (5.6-sol promo pricing documented through at least
      // 2026-11-21). gpt-5.3-codex remains Responses-only and is not listed
      // (its home is the Codex ACP lane). Hand-typed ids outside the
      // catalog families keep the Chat wire (bridge
      // openai_responses_wire_model), so legacy chat-only ids stay usable;
      // force Responses via the dedicated openai_responses group instead.
      items: [
        { model: 'gpt-6-sol', imageCapable: true, title: 'gpt-6-sol', desc: '编码与 Agent 新旗舰（默认）' },
        { model: 'gpt-6-luna', imageCapable: true, title: 'gpt-6-luna', desc: '低价高效' },
        { model: 'gpt-6-astra', imageCapable: true, title: 'gpt-6-astra', desc: '最强旗舰' },
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
      // Official figures re-checked 2026-09-28 (platform.claude.com models
      // overview + per-model pages): claude-opus-5-5 (released 2026-09-22,
      // "costs 40% less to run than Opus 5") is the new default
      // recommendation ("start with Claude Opus 5.5 for most workloads");
      // claude-fable-5-1 remains the highest-capability tier for demanding
      // reasoning and long-horizon agentic work. claude-fable-5 and
      // claude-opus-5 are both Legacy (retirement floors 2027-06/2027-07).
      // claude-haiku-4-5 keeps its 200K context / no-effort figures; watch
      // item: its retirement floor is 2026-10-15, and Sonnet 5.5 / Haiku 5.5
      // were both announced as "coming weeks" on 2026-09-22 — re-check both
      // slots at the next refresh. Every current model supports image input
      // (the base bundled offline seed recording them as text-only is
      // stale; the official docs win). From the 4.6 generation on, IDs
      // without a date are fixed snapshots (claude-haiku-4-5 predates that
      // and stays an alias).
      items: [
        { model: 'claude-opus-5-5', imageCapable: true, title: 'claude-opus-5-5', desc: '官方默认推荐，复杂 Agent 编码' },
        { model: 'claude-fable-5-1', imageCapable: true, title: 'claude-fable-5-1', desc: '最强旗舰，高难推理与长程 Agent' },
        { model: 'claude-fable-5', imageCapable: true, title: 'claude-fable-5', desc: '上代旗舰，兼容保留' },
        { model: 'claude-opus-5', imageCapable: true, title: 'claude-opus-5', desc: '官方已转 Legacy，兼容保留' },
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
      // Official figures as of 2026-09-11 (ai.google.dev models / deprecations):
      // gemini-3.8-flash (2026-09-02) is the latest Flash; 3.7 (2026-08-13),
      // 3.6 and 3.5 are all Stable and on sale, with 3.5 officially called the
      // legacy baseline; gemini-3.1-pro-preview remains the flagship Pro with
      // a non-GA id (gemini-3-pro-preview was retired on 2026-03-09).
      // Thinking cannot be turned off for the Gemini 3 family on the
      // OpenAI-compatible layer.
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
      // Official figures re-checked 2026-09-28 (docs.x.ai models + release
      // notes): grok-4.7 (September 2026) holds the official recommended
      // slot — "For everything else, including code, use Grok 4.7. It is
      // the most capable model we've built" (500K, effort low/medium/high/
      // xhigh, reasoning cannot be turned off); grok-4.6 demotes to the
      // previous generation. The reasoning guide's summary table still lists
      // xhigh on the grok-4.5/grok-4.6 row, but its caveat is authoritative
      // ("xhigh is available on grok-4.6 and later"; grok-4.5 requests with
      // xhigh are treated as high; the grok-4.5 model page itself lists no
      // reasoning-effort row) — the app follows the caveat and keeps
      // exposing low/medium/high for grok-4.5, matching the base's downgrade.
      // The grok-4.20-0309-* and grok-build-0.1 detail pages all state
      // text, image → text, so image capability is annotated.
      // Note: the base's effort injection (apply_xai_grok_4_6_reasoning_effort)
      // covers only grok-4.6 / grok-4.5, so grok-4.7 gets no tier UI until
      // the base learns it.
      items: [
        { model: 'grok-4.7', imageCapable: true, title: 'grok-4.7', desc: '旗舰，编码与 Agent 默认推荐' },
        { model: 'grok-4.6', imageCapable: true, title: 'grok-4.6', desc: '上代旗舰，编码与 Agent' },
        { model: 'grok-4.5', imageCapable: true, title: 'grok-4.5', desc: '上代旗舰，编码与 Agent' },
        { model: 'grok-4.20-0309-reasoning', imageCapable: true, title: 'grok-4.20-0309-reasoning', desc: '4.20 推理，1M 上下文' },
        { model: 'grok-4.20-0309-non-reasoning', imageCapable: true, title: 'grok-4.20-0309-non-reasoning', desc: '4.20 非推理，1M 上下文' },
        { model: 'grok-4.3', imageCapable: true, title: 'grok-4.3', desc: '快速可靠，强工具调用' },
        { model: 'grok-build-0.1', imageCapable: true, title: 'grok-build-0.1', desc: '代码 Agent，256K 上下文' },
        { model: '', title: '自定义 Grok 模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'openrouter',
      section: 'aggregator',
      title: 'OpenRouter',
      desc: 'OpenRouter 聚合平台官方 API',
      preset: 'openai_compatible',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'openrouter',
      baseUrl: 'https://openrouter.ai/api/v1',
      // OpenRouter (openrouter.ai/docs + rankings, checked 2026-09-28): one
      // key, many vendors; ids are org-prefixed (deepseek/deepseek-v4.1-flash)
      // and rankings shift weekly, so the rows are the current top usage
      // plus stable vendor flagships — treat them as suggestions, the custom
      // row covers everything else. Rows stay unannotated: deployments
      // behind the aggregator are not verified one by one, and their
      // per-deployment context figures are likewise not mirrored (the
      // engine keeps its conservative fallback for these ids). The engine
      // has a dedicated openrouter route (reasoning_effort passthrough low/
      // medium/high, thinking toggle at off), exposed via REASONING_EFFORT_TIERS.
      items: [
        { model: 'deepseek/deepseek-v4.1-flash', title: 'deepseek/deepseek-v4.1-flash', desc: '快速响应' },
        { model: 'deepseek/deepseek-v4-pro', title: 'deepseek/deepseek-v4-pro', desc: '高能力模型' },
        { model: 'deepseek/deepseek-v4-flash', title: 'deepseek/deepseek-v4-flash', desc: '快速响应' },
        { model: 'z-ai/glm-5.3', title: 'z-ai/glm-5.3', desc: '旗舰推理与编码' },
        { model: 'z-ai/glm-5.3-flash', title: 'z-ai/glm-5.3-flash', desc: '多模态高性价比' },
        { model: 'qwen/qwen3.8-flash', title: 'qwen/qwen3.8-flash', desc: '快速高性价比' },
        { model: 'minimax/minimax-m3', title: 'minimax/minimax-m3', desc: 'MiniMax 最新旗舰' },
        { model: 'moonshotai/kimi-k2.7-code', title: 'moonshotai/kimi-k2.7-code', desc: '代码场景' },
        { model: 'openai/gpt-5.6-luna', title: 'openai/gpt-5.6-luna', desc: '低成本高并发' },
        { model: 'openai/gpt-6-luna', title: 'openai/gpt-6-luna', desc: '低成本高并发' },
        { model: '', title: '自定义 OpenRouter 模型', desc: '手动填写模型 ID（org/model 格式）', custom: true },
      ],
    },
    {
      key: 'siliconflow',
      section: 'aggregator',
      title: '硅基流动 SiliconFlow',
      desc: '硅基流动国内站官方 API',
      preset: 'openai_compatible',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'siliconflow',
      baseUrl: 'https://api.siliconflow.cn/v1',
      // SiliconFlow China (docs.siliconflow.cn, checked 2026-09-28): the
      // cheapest hosted DeepSeek/GLM/Kimi/Qwen source with a free tier.
      // Ids are case-sensitive org-prefixed spellings; the Pro/ prefix marks
      // the accelerated tier. The engine has dedicated siliconflow kinds
      // (thinking toggle at off; low/medium/high collapse to high), exposed
      // via REASONING_EFFORT_TIERS. Rows stay unannotated: deployments
      // behind the platform are not verified one by one, and their
      // per-deployment context figures are likewise not mirrored (the
      // engine keeps its conservative fallback for these ids).
      items: [
        { model: 'deepseek-ai/DeepSeek-V4-Pro', title: 'deepseek-ai/DeepSeek-V4-Pro', desc: '高能力模型' },
        { model: 'deepseek-ai/DeepSeek-V4-Flash', title: 'deepseek-ai/DeepSeek-V4-Flash', desc: '快速响应' },
        { model: 'Pro/deepseek-ai/DeepSeek-V4', title: 'Pro/deepseek-ai/DeepSeek-V4', desc: '高能力模型' },
        { model: 'Pro/zai-org/GLM-5.2', title: 'Pro/zai-org/GLM-5.2', desc: '上代旗舰' },
        { model: 'moonshotai/Kimi-K2.7-Code', title: 'moonshotai/Kimi-K2.7-Code', desc: '代码场景' },
        { model: 'Qwen/Qwen3.6-27B', title: 'Qwen/Qwen3.6-27B', desc: '通用能力' },
        { model: '', title: '自定义硅基流动模型', desc: '手动填写模型 ID（org/Model 格式）', custom: true },
      ],
    },
    {
      key: 'siliconflow_global',
      section: 'aggregator',
      title: '硅基流动国际版 / SiliconFlow Global',
      desc: '硅基流动国际站 API（与国内 Key 不通用）',
      preset: 'openai_compatible',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'siliconflow',
      baseUrl: 'https://api.siliconflow.com/v1',
      // SiliconFlow global site (docs.siliconflow.com, checked 2026-09-28):
      // separate account/key system from the China site, modeled as its own
      // group like MiniMax/Kimi. Same case-sensitive id scheme.
      items: [
        { model: 'deepseek-ai/DeepSeek-V4-Pro', title: 'deepseek-ai/DeepSeek-V4-Pro', desc: '高能力模型' },
        { model: 'deepseek-ai/DeepSeek-V4-Flash', title: 'deepseek-ai/DeepSeek-V4-Flash', desc: '快速响应' },
        { model: 'Pro/deepseek-ai/DeepSeek-V4', title: 'Pro/deepseek-ai/DeepSeek-V4', desc: '高能力模型' },
        { model: 'Pro/zai-org/GLM-5.2', title: 'Pro/zai-org/GLM-5.2', desc: '上代旗舰' },
        { model: 'moonshotai/Kimi-K2.7-Code', title: 'moonshotai/Kimi-K2.7-Code', desc: '代码场景' },
        { model: '', title: '自定义硅基流动模型', desc: '手动填写模型 ID（org/Model 格式）', custom: true },
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
    {
      // Custom OpenAI Responses endpoints (/v1/responses protocol). The
      // third custom protocol surface beside Chat Completions
      // (openai_compatible) and Anthropic (Messages); the bridge lands on
      // the foundation's named-custom route with wire="responses"
      // (RESPONSES_ROUTE_PROVIDER), with no model-name matching for the
      // endpoint identity.
      key: 'openai_responses',
      section: 'custom',
      title: 'OpenAI Responses Compatible',
      desc: '自定义 OpenAI Responses 兼容接口',
      preset: 'openai_responses',
      providerKind: PROVIDER_KIND_CUSTOM,
      items: [
        { model: '', title: '自定义 Responses 模型', desc: '手动填写模型 ID 和服务地址', custom: true },
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
    // Custom-kind groups are explicit opt-ins: a saved record only matches the
    // custom group whose preset it actually carries. URL-only matching would
    // re-route a record to a sibling custom group that happens to share the
    // default endpoint (e.g. an `openai_compatible` record at api.openai.com
    // picked up by the `openai_responses` group) and silently re-stamp its
    // preset — and wire — on save.
    const customKindMatch = !provider.providerKind
      || provider.providerKind !== PROVIDER_KIND_CUSTOM
      || provider.preset === (model.preset || '');
    if (compareBase && customKindMatch && urls.includes(compareBase)) return true;
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
// case. Case-only custom ids must stay custom. Case compatibility applies only
// to spellings explicitly listed in legacyAliases — existing values can only
// come from an old catalog row's exact value, or another official parallel
// spelling on the same official page (e.g. Tencent's hyphenated glm-5-3 /
// minimax-m-3-0 forms),
// never a blanket case-insensitive match.
function catalogItemMatchesModel(item, model) {
  if (typeof item.model !== 'string' || typeof model !== 'string') return false;
  return item.model === model || (item.legacyAliases || []).includes(model);
}

// Catalog item vision-capability annotation (imageCapable): entries verified
// as multimodal are annotated true, and flagship rows the official docs
// explicitly call text-only are annotated false; the model form prefills the
// "image input capability" from this. The inclusion principle matches the
// backend builtin verified table (image_capability.rs
// VERIFIED_IMAGE_CAPABLE_MODELS): only annotate figures backed by an in-repo
// preset, public facts, or hands-on verification; leave anything uncertain
// unannotated — unannotated does not mean
// "unsupported", it just falls back to the "auto" chain (builtin table →
// Unknown). When adding catalog entries, decide in the same change whether an
// annotation is needed.
// Existing annotations were verified online entry by entry against each
// vendor's official docs on 2026-09-11: the whole current Claude family, all
// of Gemini, every Grok catalog row (including grok-4.5 / grok-4.20-0309-*,
// whose detail pages state
// text, image → text), the GPT-5.x/6 rows, kimi-k3 / k2.7-code / k2.6 and the
// Kimi Code rows, deepseek-flash(V4.1), MiniMax-M3, qwen3.8-max/3.8-flash/
// 3.7-plus/3.7-flash/3.6-flash, the six Doubao rows (including the
// coding-specialized preview), glm-5.3-flash and
// mimo-v2.5 are officially multimodal; qwen3.7-max, glm-5.2/5.3,
// deepseek-v4-pro, MiniMax-M2.x and mimo-v2.5-pro are officially text-only,
// annotated false.
// Returns the first explicit annotation (true/false) in scan order; an
// unannotated hit does not short-circuit the scan;
// returns null on no hit or when no hit carries an annotation (the "auto"
// chain is the fallback).
// Warning: matching scans groups globally across the bare model id in
// declaration order; when the same id appears in multiple groups, the first
// declared group's explicit annotation wins: qwen_token_plan deliberately
// leaves rows like glm-5.2 / deepseek-v4-pro unannotated (deployments behind
// the Alibaba gateway are not verified one by one), and they are in practice
// covered by the same-id rows in the official glm/deepseek groups — the two
// endpoints agree (same underlying model), so "unannotated = auto" is
// unreachable for such ids within the group; if a future id's capabilities
// diverge between its group members, this function must be changed to prefer
// the current provider group's row (all same-id rows agreed as of the
// 2026-09-14 re-check).
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
  if (provider) return providerLabelForModel(m, t);
  // The custom Responses group is a custom cloud endpoint just like
  // openai_compatible: attribute the sub-label to its own preset instead of
  // labeling every Responses-protocol model as the Chat-compatible group.
  return presetProviderLabel(m.preset === 'openai_responses' ? m.preset : 'openai_compatible', t);
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
  // deepseek: the wire docs nominally accept multiple tiers from low to
  // ultra, but medium/xhigh/minimal are all normalized by the vendor to high
  // (only max is separate), leaving low/high/max as the effective tiers; the
  // base keeps low as the cheaper tier, hence off/low/high/max.
  deepseek: ['off', 'low', 'high', 'max'],
  // volcengine：底座把 low/medium 归一为 high，仅 off/high/max 有区别。
  volcengine: ['off', 'high', 'max'],
  // 只有 thinking 开关的 provider：off/high。
  moonshot: ['off', 'high'],
  zai: ['off', 'high'],
  minimax: ['off', 'high'],
  'xiaomi-mimo': ['off', 'high'],
  // siliconflow (same set on the CN and global sites): the base maps off →
  // thinking.disabled and folds low/medium/high uniformly into
  // reasoning_effort=high + thinking.enabled (client.rs
  // apply_reasoning_effort), so only off/high make a real difference.
  siliconflow: ['off', 'high'],
  // openrouter: the base passes low/medium/high through on OpenRouter's
  // unified scale (off → thinking.disabled); the base also passes max/xhigh
  // through as xhigh verbatim, but only some upstream models behind the
  // aggregator accept xhigh, so the UI stays conservative and exposes only
  // off/low/medium/high.
  openrouter: ['off', 'low', 'medium', 'high'],
  // anthropic native：off 不注入（等价默认），暴露 low/medium/high/max。
  anthropic: ['low', 'medium', 'high', 'max'],
  // openai: only gpt-5.x reasoning-family models get the base injection;
  // off=none. The max tier sends "max" for the gpt-5.6
  // family, while gpt-5.5 / codex models are downgraded by the base to
  // "xhigh" (chat.rs
  // openai_compatible_reasoning_effort); the frontend exposes one unified max
  // label.
  openai: ['off', 'low', 'medium', 'high', 'max'],
  // OpenAI Responses wire (openai preset rows hit by GPT×Responses + the
  // whole openai_responses group; bridge RESPONSES_ROUTE_PROVIDER). The
  // foundation's Responses effort mapper (responses.rs
  // responses_reasoning_effort, non-deepseek arm) has no "none": off
  // normalizes to low, max/xhigh to xhigh, medium passes through. off is
  // hidden because it equals low there (the "no lookalike tiers" rule); max
  // is exposed under the unified label (the wire carries xhigh).
  openai_responses: ['low', 'medium', 'high', 'max'],
  // xai: the base apply_xai_grok_4_6_reasoning_effort injects reasoning_effort
  // only for grok-4.6 / grok-4.5 on the exact api.x.ai/v1
  // endpoint; Grok reasoning cannot be turned off (off is normalized to
  // high), so off is not exposed. The base three tiers, with max added for
  // grok-4.6 (wire sends xhigh) — see
  // reasoningEffortTiersForModel.
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

// Mirrors the model-id prefix half of bridge.rs `openai_responses_wire_model`:
// GPT models for which the Responses API supports function calling under
// OpenAI's official wording (Pinvou catalog ∩ Responses support, verified
// 2026-09-29 against the per-model pages). A hit switches to the Responses
// wire (bridge RESPONSES_ROUTE_PROVIDER); hand-typed ids outside the catalog
// do not match and keep the Chat wire. Prefix set only: the bridge
// additionally gates on the official endpoint host and the operator
// DEEPSEEK_* env pins, which the tier UI does not re-check — a hand-edited
// non-official record may show the Responses tier ladder while the engine
// keeps the Chat wire. A mechanical test pins the two prefix sets equal.
function isOpenaiResponsesWireModel(model) {
  const lower = String((model && model.model) || '').trim().toLowerCase();
  return lower.startsWith('gpt-6')
    || lower.startsWith('gpt-5.6')
    || lower.startsWith('gpt-5.5')
    || lower.startsWith('gpt-5.4-mini');
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
// 2. xai return: both Rust and the frontend return "xai"; the base injects
//    tiers only for grok-4.6 /
//    grok-4.5 on the exact api.x.ai/v1 (chat.rs
//    apply_xai_grok_4_6_reasoning_effort); for other Grok models and unofficial
//    endpoints the frontend falls back to null (no switching).
// 3. qwen/gemini 归类：Rust 将 qwen/tencent/openai/gemini/google 归入 "openai"
//    （wire route 身份）；前端对 qwen/tencent/gemini/google 返回 null（底座无档位），
//    仅 openai vendor 的 reasoning 家族返回 "openai"（对齐底座
//    `model_is_openai_reasoning_family`）。
// 4. zai/moonshot/minimax route-level tiers: the base decides tiered effort by
//    "exact first-party base_url + model name" (zai GLM-5.2/5.3/5.3-Flash,
//    moonshot K3 including k3-256k,
//    MiniMax-M3); the frontend decides by exact endpoint identity too (see
//    reasoningEffortTiersForModel
//    and is_exact_*_base_url). For compatible gateways / same-model
//    misconfigurations the base fails closed (no injection), and the frontend
//    falls back to generic tiers, aligned with the base behavior.
// 对齐 bridge.rs `is_official_deepseek_base_url`：官方 DeepSeek 端点判定
// (trim the trailing slash plus repeated /beta, /v1 suffixes, matching Rust
// trim_end_matches' repeated-strip semantics, lowercased comparison).
// api.deepseeki.com is an unofficial domain (never listed in the official
// documentation; the community reports it does not resolve,
// deepseek-ai/awesome-deepseek-agent#311, checked 2026-09-11) and has been
// removed.
function isOfficialDeepseekBaseUrl(baseUrl) {
  const normalized = String(baseUrl || '')
    .trim()
    .replace(/\/+$/, '') // eslint-disable-line sonarjs/super-linear-regex -- trailing-slash normalization; input is a user-entered URL of bounded length
    .replace(/(?:\/beta)+$/, '')
    .replace(/(?:\/v1)+$/, '')
    .toLowerCase();
  return normalized === 'https://api.deepseek.com';
}

// The base only routes tiered effort for moonshot/zai/minimax/xai via the
// CodeWhale config routes (is_exact_direct_moonshot_k3_route / is_exact_kimi_code_k3_route /
// is_exact_zai_tiered_effort_route / is_exact_minimax_m3_route / is_exact_xai_grok_4_6_route).
// replicates the base `is_exact_https_route` comparison semantics: scheme and
// host are ASCII case-insensitive while the path is case-
// sensitive, and only one trailing slash is tolerated — no extra slash
// stripping and no whole-string lowercasing (paths differing only by case are
// adjacent routes, not official endpoints). For compatible gateways
// misconfigured with the same model the base fails closed (no injection), and
// the frontend narrows its tier exposure accordingly.
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
// Moonshot direct platform endpoint: the base provider.rs
// is_exact_moonshot_platform_route accepts both the international
// https://api.moonshot.ai/v1 and the China https://api.moonshot.cn/v1
// (the Pinvou "Kimi China" group's default endpoint is the latter).
function isExactMoonshotPlatformBaseUrl(baseUrl) {
  return isExactHttpsRoute(baseUrl, 'api.moonshot.ai', 'v1')
    || isExactHttpsRoute(baseUrl, 'api.moonshot.cn', 'v1');
}
// Kimi Code membership-plan endpoint: https://api.kimi.com/coding/v1 (bare
// k3 / k3-256k)
function isExactKimiCodeBaseUrl(baseUrl) {
  return isExactHttpsRoute(baseUrl, 'api.kimi.com', 'coding/v1');
}
// z.ai first-party Chat endpoint (Coding Plan / standard platform / the Zhipu
// open platform's standard host).
// Aligned with the base provider.rs is_exact_zai_chat_route's three hosts
// (since #53,
// open.bigmodel.cn/api/paas/v4 is also a first-party tiered route); bigmodel's
// /api/coding/paas/v4 (auto-switch semantics) is explicitly excluded by the
// base, and both sides consistently offer no tiers.
function isExactZaiChatBaseUrl(baseUrl) {
  return isExactHttpsRoute(baseUrl, 'api.z.ai', 'api/paas/v4')
    || isExactHttpsRoute(baseUrl, 'api.z.ai', 'api/coding/paas/v4')
    || isExactHttpsRoute(baseUrl, 'open.bigmodel.cn', 'api/paas/v4');
}
// MiniMax first-party OpenAI Chat 端点（国际 api.minimax.io / 国内 api.minimaxi.com）。
function isExactMinimaxChatBaseUrl(baseUrl) {
  return isExactHttpsRoute(baseUrl, 'api.minimax.io', 'v1')
    || isExactHttpsRoute(baseUrl, 'api.minimaxi.com', 'v1');
}
// xAI official endpoint (base provider.rs is_exact_xai_platform_route):
// https://api.x.ai/v1
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
  // Aggregators: the base has dedicated openrouter / siliconflow(+CN)
  // routes (the vendor arms in bridge.rs provider()), so the vendor alone
  // decides and no URL gating is needed (consistent with the base's
  // kind-based injection behavior).
  if (vendor === 'openrouter') return 'openrouter';
  if (vendor === 'siliconflow') return 'siliconflow';
  if (['mimo', 'xiaomi', 'xiaomi-mimo'].includes(vendor)) return 'xiaomi-mimo';
  if (vendor === 'doubao' || vendor === 'volcengine') return 'volcengine';
  if (vendor === 'anthropic' || vendor === 'claude') return 'anthropic';
  if (vendor === 'xai' || vendor === 'grok') return 'xai'; // see reasoningEffortTiersForModel for exact-route tiers
  // Official OpenAI vendor: a Responses wire hit (the GPT family) takes the
  // openai_responses tier table; other reasoning families (codex etc. on the
  // Chat route) keep the openai table; non-reasoning families get no tiers
  // (null).
  if (vendor === 'openai') {
    if (isOpenaiResponsesWireModel(model)) return 'openai_responses';
    return isOpenaiReasoningFamilyModel(model) ? 'openai' : null;
  }
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

// Mirrors Rust bridge.rs `base_url_uses_local_or_private`: loopback, RFC1918
// private ranges (10/8, 172.16/12, 192.168/16), and Docker host aliases
// (host.docker.internal etc.). These endpoints usually run on the user's own
// machine/intranet where probing is cheap, so it is worth sending a real
// thinking effort (defaulting to the lowest thinking tier); public
// OpenAI-compatible endpoints are excluded (keep the default high).
// Difference from `baseUrlUsesLoopback`: the latter only gates the
// "auth optional" decision, this predicate covers probing and thinking
// control. The loopback part reuses `baseUrlUsesLoopback`; this function only
// adds Docker aliases and RFC1918, so the two loopback rule sets cannot drift.
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
    case 'openai':
      // Mirrors the bridge's engine_route_provider: a GPT×Responses hit takes
      // the openai_responses tier table; other reasoning families keep the
      // Chat-semantics openai table.
      if (isOpenaiResponsesWireModel(model)) return 'openai_responses';
      return isOpenaiReasoningFamilyModel(model) ? 'openai' : null;
    case 'openai_responses': return 'openai_responses';
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
// - zai: on the first-party Chat endpoints (the two api.z.ai paths +
//   open.bigmodel.cn/api/paas/v4,
//   see isExactZaiChatBaseUrl), GLM-5.2/5.3/5.3-Flash offer tiered effort
//   (off/high/max) while GLM-5.1/GLM-5-Turbo only have the generic thinking
//   switch (off/high);
//   for compatible gateways, unverified models, and bigmodel's coding host
//   (explicitly excluded by the base) the base strips
//   thinking/reasoning_effort (both tiers equivalent) → no switching offered.
//   Wire semantics of the off tier: the vendor docs say the GLM-5.3 family's
//   thinking.type only accepts enabled (disabled errors out); base #52
//   (shipped with the current gitlink; 6ae5b1734 is ae7e3fb36's
//   direct parent) rewrites disabled to
//   enabled + clear_thinking:false and normalizes effort to low on the
//   forced-thinking routes (GLM-5.3/5.3-Flash), so off is on the wire
//   "enabled + low", matching the vendor's figures; no re-check is needed as
//   the gitlink advances.
// - moonshot: K3 (direct kimi-k3 / Kimi Code k3, k3-256k, always-thinking)
//   offers low/high/max (off normalized to low); other moonshot models expose
//   off/high via the generic thinking switch.
// - minimax：仅 first-party MiniMax-M3 提供 off（disabled）/high（adaptive）；M2.7/M2.5
//   与兼容网关底座清空控制字段（两档等效）→ 不提供切换。
// - xai: only grok-4.6 on the exact api.x.ai/v1 (low/medium/high/max, max
//   sends xhigh on the wire)
//   and grok-4.5 (low/medium/high; xhigh/max are downgraded to high by the
//   base so not exposed) offer tiers;
//   Grok reasoning cannot be turned off (the base normalizes off to high), so
//   off is not exposed; other models and unofficial endpoints → null.
// 与底座 `is_exact_zai_tiered_effort_route` / `is_exact_direct_moonshot_k3_route` /
// `is_exact_kimi_code_k3_route` / `is_exact_minimax_m3_route` / `is_exact_xai_grok_4_6_route`
// Alignment: for compatible gateways misconfigured with the same model the
// base fails closed, so the frontend no longer exposes invalid or mutually
// equivalent options.
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

// Base K3 (always-thinking) exact routes: direct-platform kimi-k3
// (api.moonshot.ai / .cn) and Kimi Code's k3 / k3-256k (the base
// is_exact_kimi_code_k3_route covers both).
// Only these "exact endpoint + model name" combinations enter the tiered
// low/high/max routing.
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

// Default thinking effort for a model: local models (vLLM / local loopback
// endpoints) default to the lowest thinking tier (static four-tier table →
// low), everything else high. The local default is no longer off: real-machine
// testing shows local models like the Qwen3.8 family cannot reliably turn
// thinking off, and silent thinking both stalls the first packet and leaks
// reasoning into plain text; off remains available as an explicit choice.
// When Ollama is probed the runtime default is high (the think wire boolean
// only has off/on, on=high; see Rust request_reasoning_effort); this
// function's static low maps to a high highlight via
// reasoningEffortDisplayForTiers on the ['off','high'] probed tier table,
// consistent with the runtime.
function defaultReasoningEffortForModel(model) {
  const provider = reasoningProviderForModel(model);
  if (provider === 'vllm' || provider === 'local') {
    // Always-thinking models with controllable tiers (knowledge table):
    // tiers[0] is the lowest tier the model allows.
    const spec = alwaysThinkingSpecForModel(model && model.model);
    if (spec && spec.tiers) return spec.tiers[0];
    return 'low';
  }
  return reasoningEffortTiersForModel(model) ? 'high' : null;
}

// Thinking-effort reset on model switch: drop the old tier and fall back to
// the default for the new model's route (vllm→low (lowest thinking tier),
// other models with tiers→high; models without tiers→null = not explicitly
// set). After picking off for K2.6 and switching to K3, off is not in K3's
// tier table (low/high/max) and must reset to high — otherwise the UI has no
// highlight and saving keeps the stale value. A separate function so the
// "normalize on model switch" state transition can be behavior-tested.
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
  // OpenAI Responses wire (openai rows hit by GPT×Responses + the whole
  // openai_responses group): the foundation's Responses effort mapper has no
  // none (responses.rs codex_responses_reasoning_effort's off→low), so the
  // route's wire-true equivalent of off is low — the same rule as the K3
  // ladder above. A chat-era stored off falling back to the default high
  // would silently invert the user's "minimal reasoning" intent into the
  // most reasoning.
  if (canonical === 'off' && reasoningProviderForModel(model) === 'openai_responses') {
    canonical = 'low';
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

// Only symbols consumed by other modules stay exported (main.jsx / SettingsView /
// composer-shared / CodexAcpView / ScheduledTasksView; local-server-tiers.jsx consumes the
// reasoning-tier helpers). isPresetModel, localUserNamed,
// defaultReasoningEffortForModel and localProbeTiersForKind are internal-only; the
// vm-based catalog test strips this block and reads the top-level declarations, so it
// does not depend on the export surface.
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
  presetProviderLabel,
  normalizedProviderBaseUrl,
  findCloudProviderForModel,
  providerLabelForModel,
  isCodingPlanModel,
  catalogItemMatchesModel,
  catalogImageCapableForModel,
  groupModelsForSelector,
  selectorMainLabel,
  selectorSubLabel,
  reasoningEffortTiersForModel,
  reasoningEffortForModelSwitch,
  normalizeStoredReasoningEffort,
  alwaysThinkingSpecForModel,
  localReasoningTiers,
  reasoningEffortDisplayForTiers,
  baseUrlUsesLocalOrPrivate,
};
