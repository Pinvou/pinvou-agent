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
// openai_compatible 为纯自定义模板,前端刻意不留默认地址/模型;Rust 侧的 OpenAI
// 默认值仅作旧配置迁移兜底——自定义端点身份未知,保留旧旗舰 gpt-5.6-terra
// 而不追随官方推荐位,避免暗示官方背书。
// Default models (checked against each vendor's official docs on 2026-11-03;
// previous full re-check 2026-10-13):
// - deepseek: V4.1-Flash (deepseek-flash) is still the mainline and
//   deepseek-v4-pro keeps serving with unchanged billing (api-docs.deepseek.com
//   updates/pricing re-verified 2026-11-03; since the 2026-10-17 snapshot the
//   wire id serves DeepSeek-V4.1-Flash-1017 with unchanged calling method and
//   pricing, and the changelog's 2026-09-10 entry remains the authoritative
//   v4-pro-continues record — the separate DeepSeek Harness developer preview
//   is a tooling preview, not a model change).
// - glm: GLM-5.3 launched on 2026-08-19 (China) / 2026-08-18 (z.ai), and is
//   still the API enum default on both sites (docs.bigmodel.cn / docs.z.ai,
//   re-verified 2026-11-03; no GLM retirement announced).
// - gemini: gemini-3.8-flash launched on 2026-09-02 and is still the current
//   Flash flagship (ai.google.dev changelog re-verified 2026-11-03; the Oct 8
//   entry deprecated gemini-3.7-flash / gemini-3.5-flash with auto-routing to
//   3.8-flash / 3.6-flash). Gemini 4 Argon (gemini-4-argon) went GA on
//   2026-10-13 as the new overall flagship and joins the gemini group above
//   the Flash rows; the preset default stays the Flash mainline — see the
//   gemini group.
// - xai: grok-4.7 still holds the official "coding/agent recommended" slot
//   (docs.x.ai models page re-verified 2026-11-03: "For everything else,
//   including code, use Grok 4.7"); no newer model exists on the public API
//   (the grok-4.20-multi-agent-0309 row below is a 4.20-generation variant,
//   not a newer model).
// - openai: gpt-6.1-sol takes the default (2026-10-13): OpenAI's own
//   latest-model reference ranks gpt-6-astra for the most demanding work and
//   gpt-6.1-sol as "Near-Astra performance for complex work at a lower cost"
//   (model data 2026-09-29, announced in OpenAI's "Introducing GPT-6.1 Sol"
//   DevDay post the same day: "An upgrade to GPT-6 Sol that nearly matches
//   GPT-6 Astra's intelligence ... at one-fifth of Astra's standard input and
//   output token prices"), so the balanced-flagship slot moves up from
//   gpt-6-sol. Re-checked 2026-11-03: the models page now phrases its
//   where-to-start tip as "use GPT-6 Astra" and gpt-6.1-terra (2026-10-26)
//   joined as the efficient 6.1 tier — the balanced-flagship default slot
//   stays gpt-6.1-sol. The
//   Azure Foundry capability table pins the wire facts (page ms.date
//   2026-09-21, updated 2026-10-29): gpt-6.1-sol (2026-09-29) lists the
//   Responses API with
//   "Functions, tools, and parallel tool calling (Responses API only)",
//   1,050,000 context / 128,000 output — and the whole preset keeps riding
//   the OpenAI Responses wire (bridge engine_route_provider), so the picker
//   exposes low/medium/high/max as before (the app's effort picker surfaces
//   those four; the wire enum also has "none"/"xhigh", which the picker
//   does not expose; an "off" selection normalizes to low). gpt-6-sol / gpt-6-luna stay listed:
//   Azure shows no retirement for the GPT-6 family (the gpt-5.6 rows are
//   scheduled 2028-01-11 on Azure's retirement schedule).
// - anthropic: claude-opus-5-5 (2026-09-22) keeps the default per the
//   official models overview "start with Claude Opus 5.5 for most workloads"
//   (re-verified 2026-11-03). The 09-22 "coming weeks" watch item resolved:
//   claude-sonnet-5-5 shipped 2026-09-28 and claude-haiku-5-5 shipped
//   2026-10-07 (both verified on their anthropic.com announcement pages) —
//   see the anthropic group.
// - minimax: MiniMax-M3 stays the default; the official docs posted an
//   M2.5/M2.7 retirement notice (new-user cutoff 2026-10-19, offline
//   2026-11-18, requests auto-route to M3.1 Flash with the old names kept as
//   compatibility entries — platform.minimax.cn models faq, re-verified
//   2026-11-03); MiniMax-M3.1-Flash went GA on 2026-10-19 and is PAYG-served
//   (the "-Preview" id appears in no official listing any more); see the minimax group.
// - mimo: mimo-v2.6-pro (2026-09-22 release) stays the default; the v2.5
//   family auto-routed to its v2.6 successors from 2026-10-14 18:00 (UTC+8)
//   and the old ids hard-fail since 2026-10-21 10:00 (mimo.mi.com deprecation
//   page re-verified 2026-11-03: v2.5-pro → v2.6-pro, v2.5 → v2.6-flash) —
//   the dead v2.5 catalog rows are removed in this refresh.
// - kimi: kimi-k3.1-pro (2026-10-23) is the official flagship and migration
//   target, but the preset default stays kimi-k3: the base's tiered-effort
//   route only learns the bare K3 ids, and K3.1 Pro's thinking cannot be
//   disabled (the generic off/high ladder would risk wire 400s). See the
//   kimi group.
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
  openai:      { baseUrl: 'https://api.openai.com/v1',              model: 'gpt-6.1-sol' },
  // Custom OpenAI Responses endpoints: protocol is /v1/responses (the bridge
  // lands on the foundation's named-custom route with wire="responses"). The
  // default URL/model prefill the official endpoint so it works out of the
  // box; users aiming at an aggregator change the model along with the URL.
  openai_responses: { baseUrl: 'https://api.openai.com/v1',         model: 'gpt-6.1-sol' },
  anthropic:   { baseUrl: 'https://api.anthropic.com/v1',           model: 'claude-opus-5-5' },
  gemini:      { baseUrl: 'https://generativelanguage.googleapis.com/v1beta/openai', model: 'gemini-3.8-flash' },
  xai:         { baseUrl: 'https://api.x.ai/v1',                    model: 'grok-4.7' },
};
const PROVIDER_KIND_CODING_PLAN = 'coding_plan';
const PROVIDER_KIND_OFFICIAL_API = 'official_api';
const PROVIDER_KIND_CUSTOM = 'custom';
// 模型拼写约定：凡底座（CodeWhale）route 目录收录的模型，列表项 `model` 一律
// 优先采用底座 models_dev.bundled.json 目录行的原样拼写，不套大小写规则；
// 自定义兼容端点（bigmodel.cn Coding Plan、各开放平台、腾讯/阿里 Plan）与
// modelstudio 目录（qwen_token_plan）的行则保留各自的全小写 wire id。
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
      // 2026-11-03 (docs.bigmodel.cn/cn/coding-plan/overview):
      // GLM-5.3 / GLM-5.3-Flash are native models on all plans; GLM-5.2 /
      // GLM-5.1 calls auto-switch to GLM-5.3 and GLM-5-Turbo/GLM-4.7
      // auto-switch to GLM-5.3-Flash, so the old rows stay as "legacy model"
      // entries rather than being deleted. GLM-5.3-FlashX is not offered on
      // the plan: the plan pages (overview/faq/latest-model/usage-notes)
      // enumerate only GLM-5.3/GLM-5.3-Flash plus the legacy routes, and
      // the official GLM-5.3-FlashX model page states the exclusion
      // explicitly ("套餐当前暂未开放 GLM-5.3-FlashX", docs.bigmodel.cn
      // glm-5.3-flash page, re-checked 2026-11-04) — the 2026-11-03
      // "none states the exclusion" softening was wrong on the citation
      // (the statement exists; the conclusion was unaffected).
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
    //   (re-checked 2026-11-03; page updated 2026-10-15) OpenAI-compatible
    //   base URL /coding/v3 on api.lkeap.cloud.tencent.com; the catalog then
    //   listed Auto + GLM-5 (glm-5 has since been removed per announce 2469
    //   — see the group below). GLM-5 retired 2026-10-09 (130092 / 130060).
    //   The same page also offers an Anthropic-compatible /coding/anthropic
    //   endpoint for Claude Code-style tools; this repo's OpenAI route does
    //   not use it. Per 130092, Coding Plan models do not support multimodal
    //   (image) input, so no imageCapable flags here.
    // - Token Plan: https://cloud.tencent.com/document/product/1823/130119
    //   (re-checked 2026-11-03; both 130119 and the 130060 plan overview were
    //   updated 2026-10-22) — the platform migrated off the lkeap domain per
    //   announce 101147 (the intl mirror's id; the CN announcement list
    //   carries the same notice as 2287, whose series also records the old
    //   platform's API shutdown 2026-09-30): the docs now document only
    //   https://tokenhub.tencentmaas.com/plan/v3, plus an Anthropic
    //   /plan/anthropic endpoint and a /plan/v3/responses Responses surface
    //   (both unused by this repo's OpenAI route). The group default moves to
    //   the tokenhub host; the legacy lkeap URL stays registered as an
    //   endpoint alias so stored configs keep classifying into this group
    //   (whether the old host still serves is unannounced). Its general and
    //   Hy tiers have different lineups, and it is a different subscription
    //   from Coding Plan (plan keys are sk-tp-*, coding keys sk-sp-*, not
    //   interchangeable).
    // Both endpoints are identified as vendor=tencent coding_plan by
    // identify_coding_plan_endpoint on the Rust side (which learned the
    // tokenhub host in the same refresh), so both groups must keep
    // providerKind=CODING_PLAN to stay consistent with the metadata read back
    // after saving. Model names pass through the generic OpenAI-compatible
    // route verbatim and must use the official lowercase wire ids; the other
    // parallel official spellings on the same page (e.g. glm-5-3, the
    // deepseek/deepseek-v4-* forms, minimax-m-3-0) are registered in
    // legacyAliases so stored configs match with any of them.
    // kimi-k2.5 was removed from both groups: Tencent announce 2414 retired it
    // platform-wide on 2026-08-31 00:00 (plan users auto-switch to Auto /
    // tc-code-latest), verified against the live 130092/130119 tables.
    // glm-5 was removed from Coding Plan and glm-5.1 / glm-5 from Token Plan:
    // Tencent announce 2469 took GLM-5 / GLM-5-Turbo / GLM-5.1 offline
    // Beijing time 2026-10-09 00:00 (Coding Plan users auto-switch to Auto,
    // Token Plan users to GLM-5.3) — the announcement is the authoritative
    // record (the live 130092 plan-model table carries only the Auto row,
    // re-checked 2026-11-04 round-4 review). The old ids fall back to the
    // custom listing
    // (endpoint classification is URL-based and unaffected), same treatment
    // as kimi-k2.5.
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
      key: 'tencent_token_plan',
      section: 'coding_plan',
      title: '腾讯云 Token Plan / Tencent Cloud Token Plan',
      configTitle: '腾讯云 Token Plan',
      desc: '腾讯云 TokenHub Token 订阅接口',
      preset: 'openai_compatible',
      providerKind: PROVIDER_KIND_CODING_PLAN,
      vendor: 'tencent',
      baseUrl: 'https://tokenhub.tencentmaas.com/plan/v3',
      endpointAliases: [
        'https://api.lkeap.cloud.tencent.com/plan/v3',
        'https://api.lkeap.cloud.tencent.com/plan/v3/chat/completions',
      ],
      // Row lineup mirrors the live 130060 personal-plan table (re-checked
      // 2026-11-03; the page updated 2026-10-22). GLM-5 / GLM-5.1 retired
      // 2026-10-09 per announce 2469 (Token Plan users auto-switch to
      // GLM-5.3), so their rows are removed and old configs fall to the
      // custom listing. Two rows the live table added are mirrored:
      // deepseek/deepseek-flash (DeepSeek-V4.1-Flash 原厂直供) and
      // mimo-v2.6-flash, both left unannotated like the other gateway
      // DeepSeek rows (gateway deployments are not verified one by one).
      // Both still resolve image-capable from first-party facts: the Rust
      // image table matches them via the "deepseek-flash" / "mimo-v2.6"
      // substrings, and on the FE form the global annotation scan (bare
      // model id, cross-group) returns true for mimo-v2.6-flash through the
      // mimo group's annotated row — agreeing with the Rust table — while
      // deepseek/deepseek-flash matches no annotated row (exact-id match
      // only, the official row spells the id without the vendor prefix) and
      // stays unannotated there. minimax-m2.7 goes offline 2026-11-17 per the
      // live 130060/130051 tables' own note (one day ahead of MiniMax's
      // platform-level 2026-11-18 retirement; new-user access already ended
      // 2026-10-19), so the row carries the date. The
      // deepseek/deepseek-v4-flash-0731 legacyAlias went offline Beijing time
      // 2026-10-31 per announce 2496 (eligible post-paid + Token Plan
      // enterprise users auto-switched to the DeepSeek latest) and is dropped
      // from legacyAliases — live Tencent tables still displaying the id are
      // documentation lag, the announcement governs; deepseek/deepseek-v4-flash
      // survives. The old "high-load" flag on hy4-preview is gone from the
      // current 130060 table, so the note is dropped. DeepSeek rows are
      // first-party direct supply without SLA, per 130060.
      items: [
        { model: 'tc-code-latest', title: 'tc-code-latest', desc: '自动模型，智能路由' },
        { model: 'glm-5.3', legacyAliases: ['glm-5-3'], imageCapable: false, title: 'glm-5.3', desc: '旗舰推理与编码' },
        { model: 'glm-5.3-flash', imageCapable: true, title: 'glm-5.3-flash', desc: '多模态高性价比' },
        { model: 'glm-5.2', legacyAliases: ['glm-5-2'], imageCapable: false, title: 'glm-5.2', desc: '上代旗舰推理' },
        { model: 'kimi-k3', imageCapable: true, title: 'kimi-k3', desc: 'Kimi 最新旗舰' },
        { model: 'kimi-k2.7-code', imageCapable: true, title: 'kimi-k2.7-code', desc: 'Kimi 编码模型' },
        { model: 'deepseek/deepseek-flash', title: 'deepseek/deepseek-flash', desc: 'V4.1-Flash 原厂直供' },
        { model: 'deepseek-v4-pro-202606', legacyAliases: ['deepseek/deepseek-v4-pro-0813', 'deepseek/deepseek-v4-pro'], title: 'deepseek-v4-pro-202606', desc: '高能力模型' },
        { model: 'deepseek-v4-flash-202605', legacyAliases: ['deepseek/deepseek-v4-flash'], title: 'deepseek-v4-flash-202605', desc: '快速响应' },
        { model: 'minimax-m3', legacyAliases: ['minimax-m-3-0'], imageCapable: true, title: 'minimax-m3', desc: 'MiniMax 最新旗舰' },
        { model: 'minimax-m2.7', legacyAliases: ['minimax-m-2-7'], imageCapable: false, title: 'minimax-m2.7', desc: '通用能力，官方将于 2026-11-17 下线' },
        { model: 'hy3', legacyAliases: ['hy3-preview', 'hy3-202608'], title: 'hy3', desc: 'Hy 套餐专属模型' },
        { model: 'hy4-preview', title: 'hy4-preview', desc: 'Hy4 预览' },
        { model: 'mimo-v2.6-flash', title: 'mimo-v2.6-flash', desc: 'MiMo 全模态轻量款' },
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
      // (kimi.com/code/docs, checked 2026-09-28; re-checked 2026-11-03);
      // registered as an
      // alias so saved configs classify into this group — note the alias
      // only classifies; the exact-route K3 effort gate still pins
      // api.kimi.com/coding/v1, so an overseas-host config gets no K3
      // tiers until the base learns the new host.
      endpointAliases: ['https://api.kimi.com/coding/v1/chat/completions', 'https://api.kimi.ai/coding/v1'],
      // kimi-for-coding is a stable alias whose underlying model rolls: it
      // became K2.8 Preview on 2026-09-11 (kimi.com/code/docs models page),
      // so the old "standard coding model, smaller context" framing is gone;
      // the 1M window is available from the plan's lowest coding tier (the
      // CC docs' tier table lists 1048576 already at Andante — the "Pro 及
      // 以上支持 1M" gate on the models page attaches to the k3.1-pro / k3
      // rows, not this alias; corrected 2026-11-04 review), the default
      // effort is max, and unknown ids silently
      // fall back to the non-thinking K2.8 Preview. kimi-for-coding-highspeed
      // serves K2.7 Code Highspeed at 256K. K3.1 Pro / K3.1 Pro Highspeed
      // (2026-10-23 per the Kimi Code whats-new — the dated launch entry
      // lives there; the open-platform changelog has no dated K3.1 launch
      // entry, only an October 2026 Batch-API support note; re-checked
      // 2026-11-03 against the
      // models-overview page, which states both spellings carry kimi-k3's
      // constraints verbatim: "参数约束也与 `kimi-k3` 完全一致") are the
      // plan's new
      // flagship tier: 1M context tier-gated by plan, native vision incl.
      // video, thinking cannot be disabled (400), effort low/high/max — the
      // officially documented [1m] Claude Code declarations moved to
      // k3.1-pro[1m] (the k3[1m] entry no longer appears on the CC page; the
      // ACP catalog keeps it as an append-only legacy suggestion). The
      // base's tiered-effort route only learns bare k3 / k3-256k, so the
      // kimi rows expose no tier control at all: the bare k3.1 spellings
      // and the documented [1m] declaration forms (k3.1-pro[1m] /
      // kimi-k3.1-pro[1m], bracket-tolerant in isMoonshotK31ModelName since
      // the round-4 review) all exit to no ladder — the generic off/high
      // switch is unreachable for any K3.1 spelling, stored effort is
      // dropped (frontend normalize + Rust prefs load), and the wire omits
      // the field so the vendor default (max) applies (foundation
      // follow-up: teach the base's route shapers the K3.1 spellings).
      items: [
        { model: 'k3.1-pro', imageCapable: true, title: 'k3.1-pro', desc: 'K3.1 Pro 新旗舰，原生视觉，1M 按档开放' },
        { model: 'k3.1-pro-highspeed', imageCapable: true, title: 'k3.1-pro-highspeed', desc: 'K3.1 Pro 高速款' },
        { model: 'k3', imageCapable: true, title: 'k3', desc: 'K3 长上下文模型' },
        { model: 'k3-256k', imageCapable: true, title: 'k3-256k', desc: 'K3 256K 上下文，价格更低' },
        { model: 'kimi-for-coding', imageCapable: true, title: 'kimi-for-coding', desc: 'K2.8 Preview，最高 1M' },
        { model: 'kimi-for-coding-highspeed', imageCapable: true, title: 'kimi-for-coding-highspeed', desc: 'K2.7 Code 高速款，256K' },
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
      // Official figures re-checked on 2026-11-03 (the api-docs.deepseek.com
      // updates and pricing pages agree): V4.1-Flash (deepseek-flash) is the
      // current mainline with native image input and 1M context — since the
      // 2026-10-17 snapshot it serves DeepSeek-V4.1-Flash-1017 behind the
      // unchanged wire id (news261017; calling method and pricing unchanged).
      // deepseek-v4-pro keeps serving past 09-14 per user requests with
      // unchanged billing — the authoritative record is the updates
      // changelog's 2026-09-10 entry (the pricing-page footnote wording the
      // 2026-10-13 refresh cited is no longer on the live page — the page
      // now corroborates only implicitly, by still listing v4-pro with its
      // own pricing; note news260910's superseded "we're phasing out
      // V4-Pro…" phrasing is still live on that announcement page — the
      // changelog entry governs). The pricing page states v4-pro Vision: Not
      // supported, so imageCapable:false holds under either account and will
      // be re-checked at the next refresh (when V4.1 Pro ships — it has not
      // shipped as of 2026-11-03).
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
      // Re-checked 2026-11-03 (platform.kimi.com/docs/models; the
      // platform.moonshot.cn docs domain now 301s to platform.kimi.com — the
      // api.moonshot.cn API host is unchanged): kimi-k3.1-pro (2026-10-23) is
      // the flagship and the official migration target for every retirement
      // notice ("原生支持视觉理解，100 万 token 上下文窗口"), with
      // kimi-k3.1-pro-highspeed alongside — the models-overview page states
      // the two spellings' "参数约束也与 `kimi-k3` 完全一致" (identical
      // constraints to kimi-k3: same 1M window and native vision; the
      // "同一模型、参数约束完全一致，仅输出速度不同" sentence is that
      // page's K2.7 Code family wording, not K3.1's — corrected in the
      // 2026-11-04 fix pass); kimi-k3 demotes to previous
      // flagship and stays fully served. The whole K2 series stopped onboarding
      // new users 2026-10-31 and the official models page plans to take all
      // K2 models offline "by the end of December 2026" — the K2 rows carry
      // the notice and will be re-checked at the next refresh. The preset
      // default stays kimi-k3: the base's tiered-effort route only learns
      // bare K3 ids, and K3.1 Pro's thinking cannot be disabled (the generic
      // off/high ladder would risk wire 400s) — the default moves once the
      // base covers K3.1 (upstream; see MODEL_PRESET_DEFS).
      items: [
        { model: 'kimi-k3.1-pro', imageCapable: true, title: 'kimi-k3.1-pro', desc: '最新旗舰，原生视觉，1M 上下文' },
        { model: 'kimi-k3.1-pro-highspeed', imageCapable: true, title: 'kimi-k3.1-pro-highspeed', desc: '最新旗舰高速款' },
        { model: 'kimi-k3', imageCapable: true, title: 'kimi-k3', desc: '上代旗舰（当前默认），1M 上下文' },
        { model: 'kimi-k2.7-code', imageCapable: true, title: 'kimi-k2.7-code', desc: '代码场景；K2 系列已停新用户，官方拟 2026 年 12 月底下线' },
        { model: 'kimi-k2.7-code-highspeed', imageCapable: true, title: 'kimi-k2.7-code-highspeed', desc: '高速代码场景；K2 系列拟 2026 年 12 月底下线' },
        { model: 'kimi-k2.6', imageCapable: true, title: 'kimi-k2.6', desc: '稳定可用；K2 系列拟 2026 年 12 月底下线' },
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
      // Mirror of the China group (separate account/key system); the
      // platform.kimi.ai docs list the same lineup (re-checked 2026-11-03).
      // The highspeed K3.1 variant is the same model with identical
      // constraints (see the China group comment).
      items: [
        { model: 'kimi-k3.1-pro', imageCapable: true, title: 'kimi-k3.1-pro', desc: '最新旗舰，原生视觉，1M 上下文' },
        { model: 'kimi-k3.1-pro-highspeed', imageCapable: true, title: 'kimi-k3.1-pro-highspeed', desc: '最新旗舰高速款' },
        { model: 'kimi-k3', imageCapable: true, title: 'kimi-k3', desc: '上代旗舰，1M 上下文' },
        { model: 'kimi-k2.7-code', imageCapable: true, title: 'kimi-k2.7-code', desc: '代码场景；K2 系列已停新用户，官方拟 2026 年 12 月底下线' },
        { model: 'kimi-k2.7-code-highspeed', imageCapable: true, title: 'kimi-k2.7-code-highspeed', desc: '高速代码场景；K2 系列拟 2026 年 12 月底下线' },
        { model: 'kimi-k2.6', imageCapable: true, title: 'kimi-k2.6', desc: '稳定可用；K2 系列拟 2026 年 12 月底下线' },
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
      // Official figures re-checked on 2026-11-03 (docs.bigmodel.cn API enum /
      // pricing page): glm-5.3 is the current flagship (the text-model API
      // enum default) with forced thinking (disabled errors out) and effort
      // limited to low/high/max; glm-5.3-flash is the multimodal
      // cost-effective tier and glm-5.3-flashx the multimodal speed tier
      // (200 tokens/s, 1M context, vision-enum default). GLM-5.2 drops to
      // previous-generation flagship, all other rows are still on sale.
      // GLM-5.1 / 5-Turbo / 4.7 do not support reasoning_effort. A
      // legal-vertical speed tier, glm-5.3-flashx-legal (noted as a legal
      // vertical on both sites' vision enums; priced same as FlashX on both
      // pricing pages — z.ai $0.37/$1.25, CN ¥2/¥7/¥0.57 — while no launch
      // date is stated anywhere), is deliberately not cataloged —
      // niche vertical, reachable via the custom row.
      // glm-5 / glm-4.6 are deliberately not listed as rows: superseded
      // generations both sites still sell (the 2026-10-09 GLM-5 retirement
      // was Tencent-plan-scoped and does not apply here), but every active
      // row above supersedes them; existing configs keep working and fall
      // to the custom listing.
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
      // vision-enum default (re-checked 2026-11-03) and, like bigmodel, is
      // not offered on the Coding Plan endpoint.
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
      // Official figures re-checked 2026-11-03 (platform.minimax.cn model
      // list + Anthropic/Chat API references + the M Plan/Token Plan notice):
      // M3 stays the PAYG flagship (1M context, native multimodal incl.
      // video; the two protocol references state the thinking default
      // differently — the OpenAI-protocol reference this group rides says
      // "The `adaptive` default applies to every model here" with M3's
      // `disabled` skipping thinking, while the Anthropic-protocol reference
      // documents the default off — thinking.type=disabled is accepted in
      // both). MiniMax-M3.1-Flash went GA on
      // 2026-10-19 (official "MiniMax M3.1 Flash" post) and is now
      // PAYG-served (CNY 1.00/4.00 per Mtok) — the "-Preview" id appears in
      // no official listing any more (treated as retired; not independently
      // documented) and lives on in legacyAliases for stored configs; its
      // effort ladder is the vendor's
      // fullest (low/medium/high/xhigh/max, default max) but thinking cannot
      // be disabled there (400 "requires adaptive thinking"). The model docs
      // post the M2.5/M2.7 retirement notice (dates re-verified 2026-11-03):
      // new-user access ended 2026-10-19, the series goes offline
      // 2026-11-18; from 10-19 M Plan + Token Plan users' M2.x requests
      // auto-route to M3.1 Flash with the old names kept as compatibility
      // entries, while PAYG users keep direct M2.x access until 11-18 (the
      // same page's "retired" table already lists M2.x, and PAYG price rows
      // are gone — treat them as end-of-sale with the 11-18 wire cutoff).
      // The whole M2.x family is text-only with thinking always on. The
      // official China docs' primary endpoint is api.minimax.cn (now the only
      // documented host, with an /anthropic variant);
      // api.minimaxi.com still answers (live probes 2026-11-04 round-4
      // review: chat/completions answers 401 with body status 1004
      // auth-failed, the legacy text endpoint answers 200 with the same
      // 1004 marker) and stays the default
      // because the base tiered route pins
      // it, with the new domain registered as an alias below — note the
      // alias only classifies configs into this group; the exact-route
      // thinking gate still pins the two legacy hosts, so a saved
      // api.minimax.cn config gets no M3 effort tiers until the base learns
      // the new host (upstream CodeWhale change; switching the default
      // app-side alone would silently drop the M3 tier UI).
      // Output-cap disclosure (2026-11-04 fix pass): the protocol API
      // references (platform.minimax.io / platform.minimax.cn
      // api-reference, re-verified live 2026-11-04) DO publish the figure —
      // max_completion_tokens: "For MiniMax-M3.1-Flash and MiniMax-M3 the
      // recommended value is 131072 (128K) and the maximum is 524288
      // (512K)". The earlier "no published max-output figure" premise here
      // was refuted (the figure-free pages cited were the overview/FAQ/
      // guide pages, not the API reference), as was the "sibling M3 caps at
      // 64K" contrast (the base's bundled snapshot records M3 output
      // 524288). The documented hard cap is anchored in
      // core::model_context (the deepseek-flash precedent: register the
      // documented maximum; the engine's 64K automatic-request intersection
      // governs the effective wire cap exactly as for kimi-k3.1-pro), so
      // the GA id no longer rides the 8192 uncatalogued fail-close.
      // International and China use separate account/key systems.
      endpointAliases: ['https://api.minimax.cn/v1'],
      items: [
        { model: 'MiniMax-M3', imageCapable: true, title: 'MiniMax-M3', desc: '旗舰模型，1M 上下文多模态' },
        { model: 'MiniMax-M3.1-Flash', legacyAliases: ['MiniMax-M3.1-Flash-Preview'], imageCapable: true, title: 'MiniMax-M3.1-Flash', desc: 'GA 新旗舰（2026-10-19），高效多模态' },
        { model: 'MiniMax-M2.7', imageCapable: false, title: 'MiniMax-M2.7', desc: '官方将于 2026-11-18 下线' },
        { model: 'MiniMax-M2.7-highspeed', imageCapable: false, title: 'MiniMax-M2.7-highspeed', desc: '高速款，官方将于 2026-11-18 下线' },
        { model: 'MiniMax-M2.5', imageCapable: false, title: 'MiniMax-M2.5', desc: '官方已转 Legacy，2026-11-18 下线' },
        { model: 'MiniMax-M2.5-highspeed', imageCapable: false, title: 'MiniMax-M2.5-highspeed', desc: '官方已转 Legacy，高速款 2026-11-18 下线' },
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
      // Mirror of the China group (separate account/key system); the global
      // docs list the same lineup (re-checked 2026-11-03) — M3.1-Flash GA
      // with the same wire id, and the M2.x rows carry the same
      // 2026-11-18 retirement date per MiniMax's official notice (see the
      // China group comment). The global docs also expose an /anthropic
      // variant, unused by this repo's OpenAI route.
      items: [
        { model: 'MiniMax-M3', imageCapable: true, title: 'MiniMax-M3', desc: '旗舰模型，1M 上下文多模态' },
        { model: 'MiniMax-M3.1-Flash', legacyAliases: ['MiniMax-M3.1-Flash-Preview'], imageCapable: true, title: 'MiniMax-M3.1-Flash', desc: 'GA 新旗舰（2026-10-19），高效多模态' },
        { model: 'MiniMax-M2.7', imageCapable: false, title: 'MiniMax-M2.7', desc: '官方将于 2026-11-18 下线' },
        { model: 'MiniMax-M2.7-highspeed', imageCapable: false, title: 'MiniMax-M2.7-highspeed', desc: '高速款，官方将于 2026-11-18 下线' },
        { model: 'MiniMax-M2.5', imageCapable: false, title: 'MiniMax-M2.5', desc: '官方已转 Legacy，2026-11-18 下线' },
        { model: 'MiniMax-M2.5-highspeed', imageCapable: false, title: 'MiniMax-M2.5-highspeed', desc: '官方已转 Legacy，高速款 2026-11-18 下线' },
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
      // Official figures re-checked 2026-11-03 (mimo.mi.com model list +
      // deprecation page): the MiMo-V2.6 series launched 2026-09-22 and all
      // three tiers are omni-modal (text/image/audio/video input) with 1M
      // context / 128K max output. The v2.5 ids auto-routed to their v2.6
      // successors from 2026-10-14 18:00 (UTC+8) and hard-fail since
      // 2026-10-21 10:00 (v2.5-pro → v2.6-pro, v2.5 → v2.6-flash; the
      // deprecation page and the site banner agree) — those rows are removed
      // in this refresh and old configs fall to the custom listing
      // (classification is URL-based and unaffected). The official docs now
      // also expose Anthropic-compatible /anthropic variants and a Batch
      // host (batch-api-cn.xiaomimimo.com/v1, ultraspeed unsupported, 50%
      // off), both unused by this repo's OpenAI route. No v2.7 or newer
      // tiers exist as of 2026-11-03.
      // Token Plan subscription keys (tp-/ttp-) must switch to the cluster
      // hosts (token-plan-cn / -sgp / -ams.xiaomimimo.com/v1); the console's
      // displayed URL is authoritative.
      items: [
        { model: 'mimo-v2.6-pro', imageCapable: true, title: 'mimo-v2.6-pro', desc: '最新旗舰，1M 上下文' },
        { model: 'mimo-v2.6-flash', imageCapable: true, title: 'mimo-v2.6-flash', desc: '全模态，低成本' },
        { model: 'mimo-v2.6-pro-ultraspeed', imageCapable: true, title: 'mimo-v2.6-pro-ultraspeed', desc: '旗舰效果，极速输出' },
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
      // Official figures as of 2026-11-03 (help.aliyun.com/zh/model-studio
      // models page, updated 2026-11-02; previous check 2026-09-11):
      // qwen3.8-max is the current flagship (1M context; the pages list it
      // as a thinking model but state no default, so the earlier "thinking
      // on by default" clause is dropped); qwen3.8-flash is the current fast
      // mainline. qwen3.7-max has moved to the legacy section and is
      // text-only (no image input); qwen3.7-flash is still callable but its
      // featured slot has been taken over by qwen3.8-flash.
      // qwen3.8-max-preview is retired with requests auto-routing to the GA
      // version, so it is not listed. No qwen3.9/qwen4 generation exists as
      // of 2026-11-03. The dashscope mainline gateway now also hosts
      // third-party models (kimi-k3, glm-5.3, MiniMax-M3.1-Flash and
      // deepseek-v4.1-flash-1017 confirmed on the live model lists
      // 2026-11-04 round-4 review; mimo-v2.6-pro-ultraspeed also listed,
      // bare id without the xiaomi/ prefix; an antdigital Ling entry was
      // not found on any live page and is not claimed anymore); this group
      // stays scoped to the Qwen
      // line — the third-party lineups are covered by their own vendor
      // groups or the custom row.
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
      // edition whose keys (sk-sp-) are not interchangeable. The text rows
      // mirror the personal-plan allowlist (token-plan-personal-overview,
      // re-checked 2026-11-03 — the page's own updated-on field renders
      // empty on fetch, so no page date is claimed; the official list
      // additionally carries non-text image/audio/video
      // entries this text-only catalog does not mirror). New since the last
      // refresh: deepseek-v4.1-flash-1017 joins below. The night discount
      // (22:00-08:00) is 40%-of-price credits for qwen3.8-max AND
      // qwen3.8-flash, while the DeepSeek night list keeps 50% off and has
      // grown to four models (v4-pro-0813 / v4-flash-0731 / v4.1-flash /
      // v4.1-flash-1017; the team plan's night list matches those four per
      // the team overview updated 2026-10-19). Weekly quota was removed
      // 2026-09-22 (monthly reset only). GLM/DeepSeek rows stay live on
      // Alibaba's own infrastructure — the Tencent-side GLM-5 retirement
      // (announce 2469) and deepseek-v4-flash-0731 removal (announce 2496)
      // have no Alibaba equivalent. Gateway deployments are not verified one
      // by one, so glm/deepseek rows stay unannotated (image capability
      // falls back to the same-id official-group rows). No official
      // per-model Responses API support list is published, so no protocol
      // claim is made on the deepseek rows.
      items: [
        { model: 'qwen3.8-max', imageCapable: true, title: 'qwen3.8-max', desc: '正式旗舰，夜间 22:00-08:00 四折（个人版）' },
        { model: 'qwen3.8-flash', imageCapable: true, title: 'qwen3.8-flash', desc: '快速高性价比，夜间同样四折（个人版）' },
        { model: 'qwen3.7-max', imageCapable: false, title: 'qwen3.7-max', desc: '上代旗舰推理' },
        { model: 'qwen3.7-plus', imageCapable: true, title: 'qwen3.7-plus', desc: '均衡性价比' },
        { model: 'qwen3.6-flash', imageCapable: true, title: 'qwen3.6-flash', desc: '轻量兼容款，支持图像输入' },
        { model: 'auto', title: 'auto', desc: '自动模型，智能路由' },
        { model: 'glm-5.2', title: 'glm-5.2', desc: '上代旗舰' },
        { model: 'glm-5.3', title: 'glm-5.3', desc: '旗舰推理与编码' },
        { model: 'deepseek-v4-pro', title: 'deepseek-v4-pro', desc: '高能力模型' },
        { model: 'deepseek-v4-pro-0813', title: 'deepseek-v4-pro-0813', desc: '高能力模型' },
        { model: 'deepseek-v4.1-flash', title: 'deepseek-v4.1-flash', desc: '快速响应' },
        { model: 'deepseek-v4.1-flash-1017', title: 'deepseek-v4.1-flash-1017', desc: '快速响应（1017 快照）' },
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
      // coding-plan, re-checked 2026-11-03; the CN page was last modified
      // 2026-10-10, intl updated 2026-10-11): a fixed-monthly subscription
      // (Pro ¥200/mo; the Lite tier closed to new purchases 2026-03-20)
      // separate from the Token Plan, usable only inside AI coding tools.
      // Keys are also sk-sp- prefixed. The endpoint serves exact-version ids
      // only; the rows mirror the recommended entries of the current page —
      // the page recommends qwen3.7-plus / qwen3.6-plus / kimi-k2.5 (vision)
      // / glm-5 / MiniMax-M2.5 and lists qwen3.5-plus under "more models".
      // kimi-k2.5 is re-mirrored in this refresh: the 2026-10-13 refresh
      // dropped it citing Moonshot's platform-wide retirement, but both
      // official coding-plan pages still list it and Alibaba serves these
      // gateway deployments on its own infrastructure (like glm-5, which
      // survived Tencent's 2026-10-09 retirement the same way), so the
      // earlier exclusion premise did not hold for this gateway.
      // qwen3-coder-plus / qwen3-coder-next / glm-4.7 are not listed by the
      // page and their rows stay removed (old configs keep the endpoint
      // classification via URL and fall to the custom listing).
      // MiniMax-M2.5 is kept per the live page; MiniMax's upstream
      // M2.5/M2.7 retirement (new-user cutoff 2026-10-19, upstream offline
      // 2026-11-18) remains a watch item — Alibaba publishes no
      // decommission notice for its gateway deployment and the row will be
      // re-checked at the next refresh.
      // Gateway deployments are not verified one by one, so rows stay
      // unannotated.
      items: [
        { model: 'qwen3.7-plus', title: 'qwen3.7-plus', desc: '均衡性价比' },
        { model: 'qwen3.6-plus', title: 'qwen3.6-plus', desc: '均衡性价比' },
        { model: 'qwen3.5-plus', title: 'qwen3.5-plus', desc: '官方更多模型档' },
        { model: 'kimi-k2.5', title: 'kimi-k2.5', desc: 'Kimi 编码模型（官方推荐档）' },
        { model: 'glm-5', title: 'glm-5', desc: '高能力模型' },
        { model: 'MiniMax-M2.5', title: 'MiniMax-M2.5', desc: '通用能力' },
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
      // Official figures re-checked 2026-11-03 (docs.volcengine.com/ark
      // model list, live page stamped updated 2026-11-02; release
      // announcements 1159178):
      // doubao-seed-evolving is still the officially recommended
      // Coding/Agent model — a permanent Model ID whose underlying version
      // auto-updates ("版本升级自动生效，通过统一 Model ID 调用"; the docs
      // still describe the weekly release cadence, so the row desc anchors
      // the durable-ID mechanism instead; comment corrected 2026-11-04
      // review) (1024k context per the live list). The 2-1 family's
      // -260915 pro/lite and
      // -260628 pro/turbo snapshots stay on sale, but every 2-0 -260215
      // snapshot on the model list still carries only the dateless
      // "coming offline soon" badge (no concrete dates posted as of the
      // 11-02 list update — do not invent dates), so those rows keep the
      // coming-retirement note. The coding-specialized preview
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
      // Ark Coding Plan (volcengine docs coding-plan-personal-get-started,
      // re-checked 2026-11-03): an Agent/Coding subscription separate from
      // pay-as-you-go /api/v3 — the official quick start explicitly warns
      // NOT to point coding tools at the plain /api/v3 host.
      // ark-code-latest is the console-managed Auto shell model; the plan's
      // real-time switchable model list below mirrors the official quick
      // start page verbatim (Doubao/Kimi/GLM/DeepSeek included). The same
      // page also documents an Anthropic-compatible
      // https://ark.cn-beijing.volces.com/api/coding endpoint, unused by
      // this repo's OpenAI route. Two rows are removed in this refresh per
      // the Ark Agent/Coding Plan deprecation notice (docs 2578687, live
      // page stamped 2026-11-03; the enterprise-plan twin 2578680 stamped
      // 2026-10-19): kimi-k2.7-code went offline
      // 2026-10-28 14:00 (UTC+8; closed to new plan users 2026-10-13,
      // migrate to kimi-k3) and minimax-m3 went offline 2026-11-02 14:00
      // (migrate to doubao-seed-evolving / kimi-k3) — both are absent from
      // the quick start's switchable list (the plan-overview page still
      // displays both with 即将下线 badges), and their old configs fall to
      // the custom listing (endpoint classification is URL-based and
      // unaffected). One row is added: deepseek-v4.1-flash-economy, present
      // on the live plan list (its PAYG counterpart snapshot is
      // deepseek-v4-1-flash-economy-261017).
      // kimi-k3.1-pro row (2026-11-04 fix pass): the plan-overview page
      // (coding-plan-personal-plan-overview) names kimi-k3.1-pro twice — in
      // the 1M-context support list ("doubao-seed-evolving、glm-5.3、
      // glm-5.3-flash、kimi-k3.1-pro、kimi-k3、kimi-k2.8-preview、… 支持 1M
      // 上下文窗口") and in a dated paid activity ("2026年10月24日…至
      // 2026年11月9日…活动期间，Coding Plan 中 Kimi-K3.1-Pro 的可用量…5 折")
      // — and the quick start's switchable list the rows mirror lists it
      // too (the page was updated 2026-11-02; the earlier "the list still
      // lags behind and does not list it" premise no longer holds — the
      // row is double-confirmed on both pages as of the 2026-11-04 review).
      // Whether plan-side kimi-k3 now aliases the new K3.1 snapshot
      // (the PAYG list gained kimi-k3-1-pro-261024) is undocumented.
      items: [
        { model: 'ark-code-latest', title: 'ark-code-latest', desc: 'Coding Plan 自动模型' },
        { model: 'doubao-seed-evolving', title: 'doubao-seed-evolving', desc: '最新推荐，统一模型 ID 自动升级' },
        { model: 'doubao-seed-2.1-pro', title: 'doubao-seed-2.1-pro', desc: '高能力模型' },
        { model: 'doubao-seed-2.1-lite', title: 'doubao-seed-2.1-lite', desc: '轻量模型' },
        { model: 'doubao-seed-2.0-mini', title: 'doubao-seed-2.0-mini', desc: '轻量模型' },
        { model: 'kimi-k3', title: 'kimi-k3', desc: 'Kimi 上代旗舰' },
        { model: 'kimi-k3.1-pro', title: 'kimi-k3.1-pro', desc: 'Kimi K3.1 旗舰（套餐页 1M 名单）' },
        { model: 'kimi-k2.8-preview', title: 'kimi-k2.8-preview', desc: '代码场景' },
        { model: 'glm-5.3', legacyAliases: ['glm-latest'], title: 'glm-5.3', desc: '旗舰推理与编码' },
        { model: 'glm-5.3-flash', title: 'glm-5.3-flash', desc: '多模态高性价比' },
        { model: 'deepseek-v4.1-flash', title: 'deepseek-v4.1-flash', desc: '快速响应' },
        { model: 'deepseek-v4.1-flash-economy', title: 'deepseek-v4.1-flash-economy', desc: '经济款，生成略慢' },
        { model: 'deepseek-v4-flash', title: 'deepseek-v4-flash', desc: '快速响应' },
        { model: 'deepseek-v4-pro', title: 'deepseek-v4-pro', desc: '高能力模型' },
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
      // Official figures re-checked 2026-11-03 (developers.openai.com models
      // + changelog + deprecations; the Azure Foundry capability table,
      // ms.date 2026-09-21 / updated 2026-10-29, pins the wire facts): gpt-6.1-sol (model
      // data 2026-09-29, announced in the "Introducing GPT-6.1 Sol" DevDay
      // post the same day) keeps the
      // default — "Near-Astra performance
      // for complex work at a lower cost" is the balanced-flagship slot.
      // The models page's where-to-start tip now names GPT-6 Astra
      // ("If you're not sure where to start, use GPT-6 Astra"), so astra is
      // the docs' pick for the strongest general slot while this preset's
      // default keeps the cost-balanced stance recorded above. gpt-6.1-terra
      // (2026-10-26; fast/efficient visual reasoning, computer use, and
      // writing; Responses + Chat Completions; the family's 1.05M/128K
      // figures) joins the rows. gpt-6.1-sol's Azure row lists the Responses
      // API with "Functions, tools, and parallel tool calling (Responses API
      // only)", 1,050,000 context / 128,000 output. The whole preset keeps
      // riding the OpenAI Responses wire since 2026-09-29 (bridge
      // engine_route_provider → foundation named-custom route with
      // wire="responses") and both 6.1 ids match the existing `gpt-6`
      // prefix of openai_responses_wire_model. gpt-6-sol / gpt-6-luna stay
      // listed (no GPT-6 retirement on Azure's schedule; the gpt-5.6 rows
      // are scheduled 2028-01-11, gpt-5.5 2027-10-26 and gpt-5.4-mini
      // 2027-09-21 — all far out). The 5.6 family is not deprecated
      // (5.6-sol's promotional pricing is documented through at least
      // 2026-11-21). Deliberately not listed: gpt-5.3-codex (deprecated
      // 2026-10-01, replacement gpt-6-sol; its home is the Codex ACP lane —
      // the deprecations page is reachable again as of the 2026-11-04
      // review and confirms API removal 2027-04-01 with six months' notice,
      // resolving the earlier "UNVERIFIED / page unreachable" watch item;
      // Azure's own retirement table lists 2027-08-24 Azure-side),
      // gpt-6-rosalind (2026-10-30; Rosalind Research
      // trusted-access program only — same treatment as Anthropic's mythos
      // rows), and gpt-5.6-cyber (on the models page but specialized/gated
      // — out of preset scope; public launch coverage and API pricing
      // exist, so the earlier "no release facts verifiable" framing is
      // dropped). Hand-typed ids outside
      // the catalog families keep the Chat wire (bridge
      // openai_responses_wire_model), so legacy chat-only ids stay usable;
      // force Responses via the dedicated openai_responses group instead.
      items: [
        { model: 'gpt-6.1-sol', imageCapable: true, title: 'gpt-6.1-sol', desc: '近 Astra 旗舰，编码与 Agent（默认）' },
        { model: 'gpt-6.1-terra', imageCapable: true, title: 'gpt-6.1-terra', desc: '6.1 高效款，视觉推理与计算机操作' },
        { model: 'gpt-6-sol', imageCapable: true, title: 'gpt-6-sol', desc: '上代均衡旗舰' },
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
      // Official figures re-checked 2026-11-03 (platform.claude.com models
      // overview + deprecations + anthropic.com news; no new model since
      // haiku-5-5): claude-opus-5-5 (released 2026-09-22, "costs 40% less to
      // run than Opus 5") keeps
      // the default recommendation ("start with Claude Opus 5.5 for most
      // workloads"); claude-fable-5-1 remains the highest-capability tier
      // for demanding reasoning and long-horizon agentic work.
      // claude-fable-5 and claude-opus-5 are both still Active (the
      // deprecations table's floors read "not sooner than" 2027-06-09 and
      // 2027-07-24; the earlier "both Legacy" label misread the table —
      // corrected 2026-11-04 review). claude-sonnet-5-5 shipped 2026-09-28
      // ("a clear upgrade over Claude
      // Sonnet 5, runs 30%+ faster, and costs up to 30% less for most work")
      // and claude-haiku-5-5 shipped 2026-10-07 ("the cheapest, fastest, and
      // most capable small model we've ever released"; priced 90% below
      // Haiku 4.5 for requests up to 100K tokens, 50% below above that);
      // claude-sonnet-5 demotes to the previous generation. The whole 5.5
      // generation is officially 1M context / 128K max output per the
      // models-overview comparison table, anchored in core::model_context.
      // Microsoft Foundry's (Azure) retirement schedule lists
      // claude-haiku-4-5 at 2026-11-15 (unchanged this refresh) —
      // while Anthropic's own
      // deprecations table still lists the id Active; the previous
      // "not sooner than 2026-10-15" floor has now passed with no concrete
      // date posted, so the row keeps scoping the date to Azure.
      // Anthropic's API version 2026-10-21 removes temperature/top_p/top_k
      // from the Messages wire — noted here because no app change is
      // needed: both the host's direct Anthropic paths and the base engine
      // pin `anthropic-version: 2023-06-01` (model_endpoint.rs
      // `ANTHROPIC_VERSION`; the base client pins the same), so requests
      // never opt into the 2026-10-21 semantics. (The earlier draft here
      // claimed "the engine never sends sampling params on this route" —
      // that was false as written: the base's auxiliary paths do send
      // temperature — and named a nonexistent "aux-params alignment
      // policy"; both phrasings are dropped in the 2026-11-04 fix pass.)
      // Image input: the Sonnet
      // 5.5 announcement shows screenshot-driven benchmark play
      // (Chartography 61.6%) and the Haiku 5.5 page lists a Visual
      // reasoning benchmark (Chartography 46.4%); every other current model
      // supports image input (the
      // base bundled offline seed recording them as text-only is stale;
      // the official docs win). From the 4.6 generation on, IDs without a
      // date are fixed snapshots (claude-haiku-4-5 predates that and stays
      // an alias). Deliberately not listed: claude-mythos-5-1 /
      // claude-mythos-5 are Active first-party per the deprecations table,
      // but trusted-access-only (anthropic.com/claude/mythos) and absent
      // from the models-overview recommended lineup, so a preset row would
      // imply servability the docs don't grant (framing re-verified
      // 2026-11-03).
      items: [
        { model: 'claude-opus-5-5', imageCapable: true, title: 'claude-opus-5-5', desc: '官方默认推荐，复杂 Agent 编码' },
        { model: 'claude-sonnet-5-5', imageCapable: true, title: 'claude-sonnet-5-5', desc: '新均衡款，较 Sonnet 5 快 30%+，至多省 30%' },
        { model: 'claude-fable-5-1', imageCapable: true, title: 'claude-fable-5-1', desc: '最强旗舰，高难推理与长程 Agent' },
        { model: 'claude-fable-5', imageCapable: true, title: 'claude-fable-5', desc: '上代旗舰，兼容保留' },
        { model: 'claude-opus-5', imageCapable: true, title: 'claude-opus-5', desc: '上代 Opus，兼容保留' },
        { model: 'claude-sonnet-5', imageCapable: true, title: 'claude-sonnet-5', desc: '上代均衡款，兼容保留' },
        { model: 'claude-haiku-5-5', imageCapable: true, title: 'claude-haiku-5-5', desc: '最新小旗舰，100K 以下请求较 4.5 降价 90%' },
        { model: 'claude-haiku-4-5', imageCapable: true, title: 'claude-haiku-4-5', desc: '上代快档，200K 上下文，Azure 端 2026-11-15 下线' },
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
      // Official figures re-checked 2026-11-03 (ai.google.dev changelog /
      // deprecations / model pages; no new model since 4-argon): Gemini 4
      // Argon (gemini-4-argon) went GA
      // on 2026-10-13 as the new overall flagship ("our most capable model
      // built for complex coding, agentic, math, and science tasks"; model
      // page: unified 1M context, inputs text/image/video/audio/PDF, output
      // 65,536 sync / up to 1M with long decoding, effort low/medium/high).
      // gemini-3.8-flash (2026-09-02) stays the preset default: the slot is
      // the Flash mainline, with Argon the Pro-class flagship above it. The
      // Oct 8 changelog entry deprecated gemini-3.7-flash ("deprecated and
      // has been replaced by gemini-3.8-flash. API requests to
      // gemini-3.7-flash are automatically routed to gemini-3.8-flash") and
      // gemini-3.5-flash ("... replaced by gemini-3.6-flash. API requests to
      // gemini-3.5-flash are automatically routed to gemini-3.6-flash"),
      // neither with a shutdown date on the Gemini API — both rows stay with
      // the auto-route note. Vertex-side concrete dates (Vertex AI's
      // "Model versions and lifecycle" page — the earlier "GEM Enterprise
      // lifecycle page" label was loose, corrected 2026-11-04 review;
      // checked 2026-11-03): gemini-3.6-flash
      // retires 2026-11-19 (successor 3.8-flash; 16 days out) and
      // gemini-3.7-flash retires 2027-01-28, while the Gemini API
      // deprecations page still announces no shutdown dates — the rows scope
      // the dates to Vertex. gemini-3.5-flash-lite is Stable on the Gemini
      // API (Vertex floor "2027-07 or later"); gemini-3.1-pro-preview demotes
      // to the previous flagship preview (gemini-3-pro-preview was retired
      // on 2026-03-09). Thinking cannot be turned off for the
      // Gemini 3 family on the OpenAI-compatible layer. The Nov 2 changelog
      // entry (visible on the check date) additionally deprecates
      // gemini-2.5-pro / gemini-2.5-flash effective 2026-12-01 with
      // auto-routing to gemini-3.8-flash / gemini-3.5-flash-lite — recorded
      // though the group carries no 2.5 rows. gemini-3-flash-preview is live
      // on the models page's preview table and stays uncataloged (preview
      // churn; gemini-3.1-pro-preview is kept because the deprecations page
      // names it as gemini-3-pro-preview's replacement).
      items: [
        { model: 'gemini-4-argon', imageCapable: true, title: 'gemini-4-argon', desc: '最新旗舰，复杂编码与推理' },
        { model: 'gemini-3.8-flash', imageCapable: true, title: 'gemini-3.8-flash', desc: '最新 Flash，均衡高性价比' },
        { model: 'gemini-3.7-flash', imageCapable: true, title: 'gemini-3.7-flash', desc: '已弃用，自动路由至 3.8-flash；Vertex 端 2027-01-28 下线' },
        { model: 'gemini-3.6-flash', imageCapable: true, title: 'gemini-3.6-flash', desc: '上代稳定 Flash；Vertex 端 2026-11-19 下线' },
        { model: 'gemini-3.5-flash', imageCapable: true, title: 'gemini-3.5-flash', desc: '已弃用，请求自动路由至 3.6-flash' },
        { model: 'gemini-3.5-flash-lite', imageCapable: true, title: 'gemini-3.5-flash-lite', desc: '快速经济' },
        { model: 'gemini-3.1-pro-preview', imageCapable: true, title: 'gemini-3.1-pro-preview', desc: '上代旗舰推理（预览）' },
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
      // Official figures re-checked 2026-11-03 (docs.x.ai models + release
      // notes + reasoning guide): grok-4.7 (September 2026) holds the official recommended
      // slot — "For everything else, including code, use Grok 4.7. It is
      // the most capable model we've built" (500K, effort low/medium/high/
      // xhigh, reasoning cannot be turned off); grok-4.6 demotes to the
      // previous generation. The grok-4.7 detail page and the September
      // release notes document its low/medium/high/xhigh ladder (the
      // reasoning guide's summary table still lists only grok-4.6 /
      // grok-4.5 / the multi-agent row — the earlier attribution of the
      // 4.7 ladder to that guide was wrong, corrected 2026-11-04 review),
      // while the guide's caveat still bounds xhigh to "grok-4.6 and later"
      // for the older rows
      // (grok-4.5 requests with xhigh are treated as high) — the app follows
      // the caveat and keeps
      // exposing low/medium/high for grok-4.5, matching the base's downgrade.
      // The grok-4.20-0309-* and grok-build-0.1 detail pages all state
      // text, image → text, so image capability is annotated; the 4.20
      // family also lists grok-4.20-multi-agent-0309 (1M), added below with
      // its distinct semantics — there reasoning.effort controls the
      // parallel-agent count (4/16), not reasoning depth. Its image
      // modality IS documented on the model detail page (docs.x.ai/
      // developers/models/grok-4.20-multi-agent-0309: "Modalities: Text,
      // Image" at a glance, plus the 1M context figure; re-checked
      // 2026-11-04 fix pass — the earlier "not documented" reading was
      // taken before the detail page was reachable), so the row is
      // annotated true like its siblings; the Rust "grok" substring entry
      // already admitted it, so the FE row previously resolved auto while
      // the wire inlined images — annotated true closes that divergence.
      // Note: the base's effort injection (apply_xai_grok_4_6_reasoning_effort)
      // covers only grok-4.6 / grok-4.5, so grok-4.7 gets no tier UI until
      // the base learns it (docs explicitly document the 4.7 ladder —
      // foundation follow-up), and the multi-agent row deliberately gets
      // none.
      items: [
        { model: 'grok-4.7', imageCapable: true, title: 'grok-4.7', desc: '旗舰，编码与 Agent 默认推荐' },
        { model: 'grok-4.6', imageCapable: true, title: 'grok-4.6', desc: '上代旗舰，编码与 Agent' },
        { model: 'grok-4.5', imageCapable: true, title: 'grok-4.5', desc: '上代旗舰，编码与 Agent' },
        { model: 'grok-4.20-0309-reasoning', imageCapable: true, title: 'grok-4.20-0309-reasoning', desc: '4.20 推理，1M 上下文' },
        { model: 'grok-4.20-0309-non-reasoning', imageCapable: true, title: 'grok-4.20-0309-non-reasoning', desc: '4.20 非推理，1M 上下文' },
        { model: 'grok-4.20-multi-agent-0309', imageCapable: true, title: 'grok-4.20-multi-agent-0309', desc: '4.20 多智能体，1M 上下文（effort 控制并行数）' },
        { model: 'grok-4.3', imageCapable: true, title: 'grok-4.3', desc: '快速可靠，强工具调用' },
        { model: 'grok-build-0.1', imageCapable: true, title: 'grok-build-0.1', desc: '代码 Agent，256K 上下文' },
        { model: '', title: '自定义 Grok 模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'mistral',
      section: 'official_api',
      title: 'Mistral AI',
      configTitle: 'Mistral AI',
      desc: 'Mistral 官方 API（欧洲旗舰厂商）',
      preset: 'openai_compatible',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'mistral',
      baseUrl: 'https://api.mistral.ai/v1',
      // New group in the 2026-11-03 refresh (docs.mistral.ai, checked
      // 2026-11-03): Mistral is the major European first-party vendor with a
      // dedicated engine kind (provider_kind Mistral) and a plain
      // Bearer + chat-completions API, so it rides the generic OpenAI
      // wire with the vendor stamped for classification. Rows mirror the
      // live models overview: codestral-latest is the dedicated coding
      // model; mistral-medium-latest (multimodal per the official Medium 3
      // announcement) and mistral-small-latest (native text+image — its
      // figures match the current Small 4 generation, 256k context) are
      // multimodal. Sources reconciled in the 2026-11-04 fix pass:
      // docs.mistral.ai was unreachable from every channel on both passes
      // (still TLS-broken on the 2026-11-04 review), so the generation
      // state comes from the 2026-11-01 Wayback snapshot of the official
      // models overview plus mistral.ai/news announcements. The row descs
      // deliberately carry no context figures: models.dev (the base's
      // reference source, whose engine registry resolves these ids) lists
      // medium/large at 262,144, but no official Mistral page confirms any
      // figure for these ids, so none is claimed (the earlier
      // models.dev-sourced figures were dropped in the 2026-11-04 round-4
      // accuracy pass; re-check at the next refresh). models.dev also
      // lists mistral-large-latest modalities text/image/pdf, so its row
      // is annotated true "per models.dev" (weakly sourced — soften/
      // re-check at the next refresh). Generation state: mistral-medium-3-5
      // and mistral-large-2512 (Large 3) were deprecated 2026-10-31 with
      // retirement 2027-04-30 and "Mistral Large 4" as the documented
      // alternative — the -latest aliases the base resolves
      // (mistral-large-latest → mistral-large-2512 in the base pricing
      // alias map) point at the deprecated generations, so those rows are
      // annotated 已弃用 (still served until retirement per the deprecated
      // table; the current Large 4 preview wire id is unverifiable without
      // an API key and is not cataloged; codestral 25.08 remains the
      // current coding model — no official context figure, so its desc
      // carries none). No effort/tier control exists in the
      // base for this vendor, so no tier UI is exposed. Default drift
      // (upstream follow-up, same class as the Meta muse-spark-1.2 one):
      // the base's DEFAULT_MISTRAL_MODEL is "mistral-code-latest"
      // (provider_defaults.rs), a coding spelling this catalog does not
      // list; the rows stay explicit-id, so app-side resolution is
      // unaffected.
      items: [
        { model: 'codestral-latest', title: 'codestral-latest', desc: '编码特化模型' },
        { model: 'mistral-medium-latest', imageCapable: true, title: 'mistral-medium-latest', desc: '多模态中坚款，已弃用' },
        { model: 'mistral-large-latest', imageCapable: true, title: 'mistral-large-latest', desc: '旗舰推理，已弃用' },
        { model: 'mistral-small-latest', imageCapable: true, title: 'mistral-small-latest', desc: '轻量款，支持图像理解' },
        { model: '', title: '自定义 Mistral 模型', desc: '手动填写模型 ID', custom: true },
      ],
    },
    {
      key: 'meta',
      section: 'official_api',
      title: 'Meta Model API (Muse Spark)',
      configTitle: 'Meta Model API',
      desc: 'Meta 官方模型 API',
      preset: 'openai_compatible',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'meta',
      baseUrl: 'https://api.meta.ai/v1',
      // New group in the 2026-11-03 refresh (dev.meta.ai quickstart +
      // models pages, checked 2026-11-03): Meta's first-party paid Model
      // API (GA, agent-first positioning; the base has a dedicated `meta`
      // kind, aliases muse/muse-spark). muse-wonder-1.0 joins in the
      // 2026-11-04 fix pass: the models page positions Muse Wonder as the
      // most capable model ("Muse Wonder is on Model API and in Muse
      // Code"; the coding-agents guides use `muse-wonder-1.0`), and it had
      // been missing from the lineup without disclosure. muse-spark-1.3 is
      // the current default model: 1,048,576 context / 131,072 output,
      // officially multimodal (text/image/PDF/video input; the quickstart
      // states "context 1048576, output 131072" and modalities verbatim —
      // re-verified live 2026-11-04, closing the earlier "re-verify when
      // reachable" TODOs). The models page states all three Muse Spark
      // versions take text, image, video, audio, and PDF input, so
      // muse-spark-1.2 and muse-spark-1.1 are annotated true like 1.3
      // (the earlier "no per-model modality statement covers 1.1" ground
      // was refuted live 2026-11-04; the quickstart troubleshooting also
      // names muse-spark-1.1 as a served id). The host also
      // exposes Anthropic-Messages and Responses surfaces with
      // encrypted-reasoning replay; this repo's OpenAI route uses plain
      // chat completions, and no tier control exists for this vendor.
      items: [
        { model: 'muse-wonder-1.0', imageCapable: true, title: 'muse-wonder-1.0', desc: '最强旗舰，Agent 与计算机操作' },
        { model: 'muse-spark-1.3', imageCapable: true, title: 'muse-spark-1.3', desc: '当前默认，1M 上下文多模态' },
        { model: 'muse-spark-1.2', imageCapable: true, title: 'muse-spark-1.2', desc: '上一代版本' },
        { model: 'muse-spark-1.1', imageCapable: true, title: 'muse-spark-1.1', desc: '上一代，仍在服务' },
        { model: '', title: '自定义 Meta 模型', desc: '手动填写模型 ID', custom: true },
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
      // OpenRouter (openrouter.ai/docs + rankings + GET /api/v1/models,
      // re-checked 2026-11-03): one
      // key, many vendors; ids are org-prefixed (deepseek/deepseek-v4.1-flash)
      // and rankings shift weekly, so the rows are the current top usage
      // plus stable vendor flagships — treat them as suggestions, the custom
      // row covers everything else. This refresh syncs the weekly-usage
      // top-10 (DeepSeek V4.1 Flash #1/#3, GLM-5.3-Flash #2, GPT-6 Luna #4,
      // Tencent Hy4 Preview #5, Claude Opus 5.5 #6, Step 5 Preview #7,
      // MiniMax M3.1 Flash #8, GLM-5.3 #9, DeepSeek V4 Flash #10) plus the
      // new vendor flagships: note anthropic spells its id with dots
      // (claude-opus-5.5, NOT claude-opus-5-5), minimax/minimax-m3.1-flash
      // (2026-10-19) supersedes m3 in usage while the m3 row stays live,
      // and openai/gpt-6.1-terra is the trending newcomer. Rows stay
      // unannotated: deployments
      // behind the aggregator are not verified one by one, and their
      // per-deployment context figures are likewise not mirrored (the
      // engine keeps its conservative fallback for these ids). The engine
      // has a dedicated openrouter route (reasoning_effort passthrough low/
      // medium/high, thinking toggle at off), exposed via
      // REASONING_EFFORT_TIERS — wire nuance from the models API: a few rows
      // (qwen3.8-flash, minimax-m3, kimi-k2.7-code) list `reasoning`
      // but not `reasoning_effort` in supported_parameters, so the passthrough
      // is a no-op there (minimax-m3.1-flash DOES list reasoning_effort —
      // live models API re-check 2026-11-04, correcting the earlier "minimax
      // pair" wording). Never auto-append `:free`: the docs return 404 for
      // base ids without a dedicated :free entry and none of these rows has
      // one (openrouter/free is the dedicated free-router id).
      items: [
        { model: 'deepseek/deepseek-v4.1-flash', title: 'deepseek/deepseek-v4.1-flash', desc: '快速响应（用量第一）' },
        { model: 'deepseek/deepseek-v4.1-flash-1017', title: 'deepseek/deepseek-v4.1-flash-1017', desc: '快速响应（1017 快照）' },
        { model: 'deepseek/deepseek-v4-pro', title: 'deepseek/deepseek-v4-pro', desc: '高能力模型' },
        { model: 'deepseek/deepseek-v4-flash', title: 'deepseek/deepseek-v4-flash', desc: '快速响应' },
        { model: 'z-ai/glm-5.3', title: 'z-ai/glm-5.3', desc: '旗舰推理与编码' },
        { model: 'z-ai/glm-5.3-flash', title: 'z-ai/glm-5.3-flash', desc: '多模态高性价比（用量第二）' },
        { model: 'qwen/qwen3.8-flash', title: 'qwen/qwen3.8-flash', desc: '快速高性价比' },
        { model: 'moonshotai/kimi-k3.1-pro', title: 'moonshotai/kimi-k3.1-pro', desc: 'Kimi 最新旗舰' },
        { model: 'moonshotai/kimi-k2.7-code', title: 'moonshotai/kimi-k2.7-code', desc: '代码场景' },
        { model: 'minimax/minimax-m3.1-flash', title: 'minimax/minimax-m3.1-flash', desc: 'MiniMax GA 新旗舰' },
        { model: 'minimax/minimax-m3', title: 'minimax/minimax-m3', desc: 'MiniMax 上代旗舰' },
        { model: 'anthropic/claude-opus-5.5', title: 'anthropic/claude-opus-5.5', desc: 'Claude 官方默认推荐（注意点号拼写）' },
        { model: 'google/gemini-4-argon', title: 'google/gemini-4-argon', desc: 'Gemini 最新旗舰' },
        { model: 'openai/gpt-6.1-terra', title: 'openai/gpt-6.1-terra', desc: '6.1 高效款（趋势第一）' },
        { model: 'openai/gpt-5.6-luna', title: 'openai/gpt-5.6-luna', desc: '低成本高并发' },
        { model: 'openai/gpt-6-luna', title: 'openai/gpt-6-luna', desc: '低成本高并发（用量第四）' },
        { model: 'tencent/hy4-preview', title: 'tencent/hy4-preview', desc: '腾讯 Hy4 预览（用量第五）' },
        { model: 'stepfun/step-5-preview', title: 'stepfun/step-5-preview', desc: 'StepFun 预览旗舰（用量第七）' },
        { model: '', title: '自定义 OpenRouter 模型', desc: '手动填写模型 ID（org/model 格式）', custom: true },
      ],
    },
    {
      key: 'together',
      section: 'aggregator',
      title: 'Together AI',
      configTitle: 'Together AI',
      desc: 'Together AI 开源模型托管平台',
      preset: 'openai_compatible',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'together',
      baseUrl: 'https://api.together.xyz/v1',
      // New group in the 2026-11-03 refresh: a mainstream OSS-model hosting
      // platform with a dedicated engine kind (Together) and a plain Bearer
      // + chat-completions API. The rows mirror the platform's live
      // serverless catalog (docs.together.ai/docs/serverless/models,
      // re-fetched 2026-11-04 review pass) cross-checked against the
      // deprecations page (docs.together.ai/docs/deprecations). The earlier
      // "2026-11-04 re-line-up" was verified against neither and shipped six
      // ids the shared endpoints no longer serve — all six carry removal
      // records: deepseek-ai/DeepSeek-V4-Pro (2026-08-27),
      // moonshotai/Kimi-K2.6 (2026-08-19), moonshotai/Kimi-K2.5 (2026-05-21),
      // zai-org/GLM-5.1 (2026-07-10), zai-org/GLM-5 (2026-06-22),
      // openai/gpt-oss-20b (2026-09-15); removed ids leave the shared
      // serverless catalog entirely. Each row below is verbatim from the
      // live catalog's Chat table. The canonical page's remaining Chat rows
      // (deepseek-ai/DeepSeek-V4.1-Flash-1017, MiniMaxAI/MiniMax-M3.1-Flash,
      // Qwen/Qwen3.7-Plus, Qwen/Qwen3.8-Flash, Qwen/Qwen3.5-9B,
      // meta-models/Muse-Glimmer-30B, meta-models/Muse-Spark-Open-816B-A42B,
      // nvidia/NVIDIA-Nemotron-3.5-Super-VL, reflection/Beam-1.0,
      // Prism-ML/Ternary-Bonsai-27B, together/Tev1-4B-experimental,
      // zai-org/GLM-5.2) are a deliberate curated-out subset — the
      // flagship-selection frame excludes them and the custom row covers
      // hand-typed ids. (The earlier "page also serves MiniMax-M2.7 /
      // coder variants / older Llama-Gemma generations" sentence described
      // the OLD page's inventory — corrected 2026-11-04 review.) Verify
      // only against docs.together.ai/docs/serverless/models: the legacy
      // URL docs.together.ai/docs/serverless-models is a stale deployed
      // snapshot that still lists the six retired ids above — both URLs
      // answer on 2026-11-04, and the earlier "fetched live" verification
      // hit the stale one. The base URL matches the engine's provider default
      // (the docs' shared-host value is consistent with it, though not
      // literally printed on the pages fetched this refresh — a US-geo
      // variant us-api.together.ai bills +25%). Rows stay unannotated:
      // gateway deployments are not verified one by one and their context
      // figures are not mirrored, and no tier control exists for this
      // vendor.
      items: [
        { model: 'deepseek-ai/DeepSeek-V4-Pro-0813', title: 'deepseek-ai/DeepSeek-V4-Pro-0813', desc: '高能力模型，1M 上下文' },
        { model: 'deepseek-ai/DeepSeek-V4.1-Flash', title: 'deepseek-ai/DeepSeek-V4.1-Flash', desc: '高速款，1M 上下文' },
        { model: 'moonshotai/Kimi-K3', title: 'moonshotai/Kimi-K3', desc: 'Kimi 旗舰，1M 上下文' },
        { model: 'zai-org/GLM-5.3', title: 'zai-org/GLM-5.3', desc: 'GLM 旗舰' },
        { model: 'zai-org/GLM-5.3-Flash', title: 'zai-org/GLM-5.3-Flash', desc: 'GLM 高速款' },
        { model: 'Qwen/Qwen3.7-Max', title: 'Qwen/Qwen3.7-Max', desc: '通义旗舰托管版' },
        { model: 'Qwen/Qwen3.6-Plus', title: 'Qwen/Qwen3.6-Plus', desc: '通义 1M 托管版' },
        { model: 'openai/gpt-oss-120b', title: 'openai/gpt-oss-120b', desc: '开放权重推理模型' },
        { model: '', title: '自定义 Together 模型', desc: '手动填写模型 ID（org/Model 格式）', custom: true },
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
      // SiliconFlow China (docs.siliconflow.cn model plaza + release notes,
      // re-checked 2026-11-03; previous check 2026-09-28): a hosted
      // DeepSeek/GLM/Kimi/Qwen source with a free tier (the plaza's ¥0 rows
      // are unrelated small models — zai-org/GLM-5.3-Flash below is NOT
      // free: the official CN pricing lists it at ¥0.80 input / ¥2.80
      // output / ¥0.23 cache-hit per Mtok, re-verified 2026-11-04 fix pass,
      // correcting the earlier "¥0/free" reading). Ids are org-prefixed
      // spellings;
      // the Pro/ prefix marks the paid tier (variable rate limits vs the
      // free rows' fixed ones — the docs define it by billing/limits, not
      // "acceleration"), and the two Pro/ rows below are kept for existing
      // configs even though they have left the public plaza — the official
      // API reference names the Pro/ prefix on sibling ids (Pro/zai-org/
      // GLM-4.7 in its examples; Pro/zai-org/GLM-5 and Pro/deepseek-ai/
      // DeepSeek-V3.2 in the enable_thinking list) but neither of these two
      // exact ids any more (the earlier "the official API doc still name
      // them" citation was over-broad, corrected 2026-11-04 review). Since
      // 2026-10-21 deepseek-ai/DeepSeek-V4.1-Flash serves the
      // 1017 snapshot in place (wire id unchanged, vision, 1M) — unlike
      // OpenRouter, which lists -1017 as a separate id. The engine has
      // dedicated siliconflow kinds (thinking toggle at off via
      // enable_thinking; low/medium/high collapse to high; the chat API
      // documents reasoning_effort (high|max, low/medium mapped to high)
      // scoped to deepseek-ai/DeepSeek-V4-Flash only — the earlier
      // "DeepSeek/GLM rows" scope overclaimed; corrected 2026-11-04
      // review), exposed via REASONING_EFFORT_TIERS. Rows stay unannotated:
      // deployments behind the platform are not verified one by one, and
      // their per-deployment context figures are likewise not mirrored (the
      // engine keeps its conservative fallback for these ids).
      items: [
        { model: 'deepseek-ai/DeepSeek-V4.1-Flash', title: 'deepseek-ai/DeepSeek-V4.1-Flash', desc: '快速响应，原厂升级 1017 快照' },
        { model: 'deepseek-ai/DeepSeek-V4-Pro', title: 'deepseek-ai/DeepSeek-V4-Pro', desc: '高能力模型' },
        { model: 'deepseek-ai/DeepSeek-V4-Flash', title: 'deepseek-ai/DeepSeek-V4-Flash', desc: '快速响应' },
        { model: 'Pro/deepseek-ai/DeepSeek-V4', title: 'Pro/deepseek-ai/DeepSeek-V4', desc: '高能力模型（Pro 档）' },
        { model: 'zai-org/GLM-5.3', title: 'zai-org/GLM-5.3', desc: '旗舰推理与编码' },
        { model: 'zai-org/GLM-5.3-Flash', title: 'zai-org/GLM-5.3-Flash', desc: '多模态高性价比' },
        { model: 'Pro/zai-org/GLM-5.2', title: 'Pro/zai-org/GLM-5.2', desc: '上代旗舰（Pro 档）' },
        { model: 'moonshotai/Kimi-K2.7-Code', title: 'moonshotai/Kimi-K2.7-Code', desc: '代码场景' },
        { model: 'Qwen/Qwen3.8-27B', legacyAliases: ['Qwen/Qwen3.6-27B'], title: 'Qwen/Qwen3.8-27B', desc: '通用能力' },
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
      // SiliconFlow global site (siliconflow.com model plaza, re-checked
      // 2026-11-03): separate account/key system from the China site,
      // modeled as its own group like MiniMax/Kimi. Same org-prefix id
      // scheme. The two lineups are NOT mirrors: the global plaza
      // additionally lists Kimi-K3, MiniMax-M3, Qwen3.8-2.4T-A95B, dated
      // DeepSeek V4 variants (V4-Pro-0813 / V4-Flash-0731 / V4-Flash-Vision-
      // Exp), gemma-4 and gpt-oss rows, while the CN-only Qwen/Qwen3.8-27B
      // is absent from the global plaza. Of the global-only rows, three
      // joined below in the 2026-11-04 fix pass (deepseek-ai/DeepSeek-V4.1-
      // Flash — the current Flash mainline, which global users would
      // otherwise miss behind the older DeepSeek-V4-Flash name: the global
      // plaza still lists that name with its April release date and no
      // global page documents a retirement, so the "superseded since 09-10"
      // reading is an inference from the CN release notes' in-place upgrade
      // record, re-check at the next refresh — plus zai-org/GLM-5.3 /
      // zai-org/GLM-5.3-Flash): the global
      // plaza displays them under names equal to the CN wire ids verified
      // against the official CN plaza/pricing, so the "ids unverifiable
      // without a key" rationale does not apply to these three. The
      // remaining global-only rows are still deliberately not cataloged
      // because the marketing plaza shows bare
      // names without org prefixes and /v1/models needs an authenticated
      // key, so their exact wire ids stay unverified. The two Pro/ rows
      // below are NOT global-plaza listings: they are the same
      // API-reference keeps as the CN group (they have left the public
      // plazas; kept for existing configs — the official API reference
      // names the Pro/ prefix on sibling ids but no longer these two exact
      // ids, the same corrected citation the CN group carries). Only the
      // unprefixed deepseek-ai/DeepSeek-V4-Pro
      // / -V4-Flash and moonshotai/Kimi-K2.7-Code rows are verified shared
      // between both plazas.
      items: [
        { model: 'deepseek-ai/DeepSeek-V4.1-Flash', title: 'deepseek-ai/DeepSeek-V4.1-Flash', desc: '快速响应，原厂升级 1017 快照' },
        { model: 'deepseek-ai/DeepSeek-V4-Pro', title: 'deepseek-ai/DeepSeek-V4-Pro', desc: '高能力模型' },
        { model: 'deepseek-ai/DeepSeek-V4-Flash', title: 'deepseek-ai/DeepSeek-V4-Flash', desc: '快速响应' },
        { model: 'Pro/deepseek-ai/DeepSeek-V4', title: 'Pro/deepseek-ai/DeepSeek-V4', desc: '高能力模型' },
        { model: 'zai-org/GLM-5.3', title: 'zai-org/GLM-5.3', desc: '旗舰推理与编码' },
        { model: 'zai-org/GLM-5.3-Flash', title: 'zai-org/GLM-5.3-Flash', desc: '多模态高性价比' },
        { model: 'Pro/zai-org/GLM-5.2', title: 'Pro/zai-org/GLM-5.2', desc: '上代旗舰' },
        { model: 'moonshotai/Kimi-K2.7-Code', title: 'moonshotai/Kimi-K2.7-Code', desc: '代码场景' },
        { model: '', title: '自定义硅基流动模型', desc: '手动填写模型 ID（org/Model 格式）', custom: true },
      ],
    },
    {
      key: 'groq',
      section: 'aggregator',
      title: 'Groq',
      configTitle: 'Groq',
      desc: 'Groq 超低延迟推理平台',
      preset: 'openai_compatible',
      providerKind: PROVIDER_KIND_OFFICIAL_API,
      vendor: 'groq',
      baseUrl: 'https://api.groq.com/openai/v1',
      // New group in the 2026-11-03 refresh (console.groq.com docs,
      // checked 2026-11-03): the low-latency inference platform, already a
      // base setup template and an ACP preset — this group brings the model
      // catalog to parity. Plain Bearer + chat completions. The production
      // catalog is narrow (Llama + gpt-oss rows) and the llama rows are
      // tagged Enterprise on the live models page — console.groq.com/docs/
      // deprecations records the reason: the two llama ids were deprecated
      // 2026-06-17 and shut down for free/developer tiers on 2026-08-16
      // (committed-spend Enterprise contracts unaffected; both dates now
      // past — the earlier "no reachable page states a date" note missed
      // this page, corrected 2026-11-04 review). Image
      // input is not documented on the models page, so rows stay
      // unannotated; no tier control exists for this vendor.
      items: [
        { model: 'llama-3.3-70b-versatile', title: 'llama-3.3-70b-versatile', desc: 'Llama 旗舰档（企业套餐）' },
        { model: 'llama-3.1-8b-instant', title: 'llama-3.1-8b-instant', desc: '极速轻量档（企业套餐）' },
        { model: 'openai/gpt-oss-120b', title: 'openai/gpt-oss-120b', desc: '开放权重推理模型' },
        { model: 'openai/gpt-oss-20b', title: 'openai/gpt-oss-20b', desc: '开放权重轻量推理' },
        { model: '', title: '自定义 Groq 模型', desc: '手动填写模型 ID', custom: true },
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
    // URL hosts are case-insensitive (RFC 3986); the Rust side lowercases
    // before its endpoint matches (identify_coding_plan_endpoint's
    // to_ascii_lowercase), so the frontend classification must fold case
    // the same way or a stored uppercase-host URL gets Rust metadata while
    // this lookup misses its group. Comparison-only: the stored value keeps
    // its original case.
    const urls = [providerBaseUrl(provider), ...(provider.endpointAliases || [])]
      .map(url => {
        const normalized = provider.endpointMode === 'full_chat_completions' ? normalizeEndpointUrl(url) : normalizeOpenAiBaseUrl(url);
        return normalized.toLowerCase();
      });
    const compareBase = (provider.endpointMode === 'full_chat_completions' ? normalizeEndpointUrl(base) : normalizeOpenAiBaseUrl(base)).toLowerCase();
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
// vendor's official docs on 2026-09-11 (snapshot rows that joined on
// 2026-09-28 — e.g. the Doubao -260915 pair — re-verified in that refresh):
// the whole current Claude family, all
// of Gemini, every annotated Grok catalog row (grok-4.5 /
// grok-4.20-0309-*, whose detail pages state
// text, image → text; grok-4.20-multi-agent-0309 joined on 2026-11-03
// unannotated and was annotated true in the 2026-11-04 fix pass once its
// detail page became reachable — "Modalities: Text, Image"), the
// GPT-5.x/6 rows, kimi-k3 / k2.7-code(-highspeed) / k2.6 and the
// Kimi Code rows, deepseek-flash(V4.1), MiniMax-M3, qwen3.8-max/3.8-flash/
// 3.7-plus/3.7-flash/3.6-flash, the eight Doubao rows (including the
// coding-specialized preview) and glm-5.3-flash are officially
// multimodal; qwen3.7-max, glm-5.2/5.3, deepseek-v4-pro and MiniMax-M2.x
// are officially text-only,
// annotated false (the retired mimo rows left the catalog in the
// 2026-11-03 refresh; their Rust-side entries stay for stored configs).
// Later additions carry their own dated basis:
// gpt-6.1-sol (2026-10-13; model page lists input modalities text and
// image), claude-sonnet-5-5 (screenshot-driven
// benchmark play on its announcement page), claude-haiku-5-5 (Visual
// reasoning benchmark), and gemini-4-argon (GA 2026-10-13; model page
// lists text/image/video/audio/PDF inputs); the 2026-11-03 refresh adds
// kimi-k3.1-pro / k3.1-pro and both highspeed spellings (whats-new
// 2026-10-23: native vision incl. video; the highspeed variant is the
// same model per the models-overview page), MiniMax-M3.1-Flash (GA
// 2026-10-19; rides the "minimax-m3" substring entry), gpt-6.1-terra
// (2026-10-26), muse-spark-1.3 (dev.meta.ai: text/image/PDF/video) and
// mistral-medium-latest / mistral-small-latest (docs.mistral.ai models
// overview) — all annotated true on the same public-fact basis. The
// 2026-11-04 fix pass adds grok-4.20-multi-agent-0309 (detail page:
// Text, Image), mistral-large-latest (per models.dev — docs.mistral.ai
// unreachable), and the whole Meta Muse family (muse-wonder-1.0 /
// muse-spark-1.2 / muse-spark-1.1: the models page states all three
// Muse Spark versions take text/image/video/audio/PDF, and Wonder's
// section lists the same modalities).
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
// 2026-11-03 refresh; the same-id pairs it added are uniformly annotated or
// uniformly unannotated — re-verified in the 2026-11-04 fix pass).
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
  if (provider === 'moonshot') return moonshotReasoningLadder(model, modelName, tiers);
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

// Moonshot-route tier resolution: the exact-K3 ladder, the K3.1 family's
// no-control exit, or the provider's generic table.
function moonshotReasoningLadder(model, modelName, genericTiers) {
  if (isExactMoonshotK3Route(model, modelName)) return ['low', 'high', 'max'];
  if (isMoonshotK31ModelName(modelName)) {
    // K3.1 spellings (kimi-k3.1-pro[-highspeed] on the platform base,
    // k3.1-pro[-highspeed] on Kimi Code) are always-reasoning: the vendor
    // contract controls depth via top-level reasoning_effort low/high/max
    // and rejects thinking:{type:"disabled"} with an error (models-overview:
    // "参数约束也与 kimi-k3 完全一致"). The base's exact-K3 shapers only
    // learn bare k3 spellings, so no tier this UI could offer reaches the
    // wire correctly today — exposing the generic off/high switch here let
    // users persist a disable shape the vendor rejects (2026-11-04 review).
    // Expose no control instead: the effort field is omitted, which the
    // vendor documents as valid (可省略) and resolves to the default max.
    // Parked until the foundation's route shapers cover K3.1 (see the
    // kimi group comments).
    return null;
  }
  return genericTiers;
}

// K3.1 family spellings across the kimi groups (platform + Kimi Code).
// Case-folded, and bracket-tolerant on the documented `[1m]` 1M declaration
// forms: k3.1-pro[1m] is the ACP Kimi Code models1m datalist entry and
// kimi-k3.1-pro[1m] is the direct-platform bracket spelling of the same
// models, so a stored bracket id must get the same no-control exit as its
// bare spelling — otherwise the generic off/high ladder would let users
// persist a disable shape the vendor rejects (2026-11-04 review round 4).
// Bracket forms of NON-K3.1 ids (k3[1m] / kimi-k3[1m]) strip to non-K3.1
// spellings and keep their own contracts, mirroring prefs
// is_moonshot_k31_route.
function isMoonshotK31ModelName(modelName) {
  const normalized = String(modelName || '').trim().toLowerCase();
  const bare = normalized.endsWith('[1m]') ? normalized.slice(0, -4) : normalized;
  return ['kimi-k3.1-pro', 'kimi-k3.1-pro-highspeed', 'k3.1-pro', 'k3.1-pro-highspeed']
    .includes(bare);
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
  // 本地路由（vllm preset / 探测驱动的 loopback 兼容端点）的存量 `max` 逐字保留，
  // 不落静态四档表之外的降级路径：探测为 tachyon（驭光）时档表是
  // ['off','low','medium','max']，max 是真实最深档（wire 映射 xhigh）；静态
  // normalization 表无 probe 语境、只认 vllm 四档，若在这里把 max 归一成默认
  // low，设置表单重新打开再保存会把用户选的最深档静默改写为最低档。探测为
  // vllm 等其他本地引擎时 wire 方言把 max 映射为有效值（如 high），同样无需
  // 客户端降级；展示高亮由 probe 档表决定：不含 max 的表按
  // reasoningEffortDisplayForTiers 的最近档规则显示（含 high 的表显示 high，
  // 对 vllm wire 恰好等价），值本身不改。
  // always-thinking 知识表命中（如本地 kimi-k3 的 ['low','high']）不在此例——
  // 表外档维持既有的"归一到最低允许档"语义。
  const storedValueProvider = reasoningProviderForModel(model);
  if (
    canonical === 'max'
    && (storedValueProvider === 'vllm' || storedValueProvider === 'local')
    && !alwaysThinkingSpecForModel(model && model.model)
  ) {
    return 'max';
  }
  if (canonical && tiers.includes(canonical)) return canonical;
  return defaultReasoningEffortForModel(model) || tiers[0] || null;
}

// 本地 OpenAI 兼容端点按探测服务类型可切换的档位。与 Rust
// `probe_local_server_kind` 结果对齐：
// - vllm → 底座 `chat_template_kwargs` 支持四档
// - tachyon（驭光）→ wire 与 vllm 同族但有效档为 none/low/medium/xhigh（high
//   会被服务端 400）；最深档复用既有 `max` 档位 id（与 codex/xai 先例一致，
//   三语 i18n 已有 max 文案），bridge→底座链路把 max 映射为 wire 上的 xhigh
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
    case 'tachyon':
      return ['off', 'low', 'medium', 'max'];
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
    // tachyon（驭光）叠加知识表（如本地 kimi-k3 的 ['low','high']）：表内的
    // high 在 tachyon wire 上由 bridge 归一为 medium 发送（不发 high），最深
    // 档 max 不在知识表内故不可达——与 Rust 侧防御归一口径一致，UI 保留
    // 知识表档位不另行改写。
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
// original value survives switching back to a four-tier endpoint).
// Tachyon's ladder replaces high with max; its wire clamps a legacy high to
// medium, so show that effective tier while preserving the stored value.
// Returns null when there is no selection or no equivalent tier.
function reasoningEffortDisplayForTiers(effort, tiers) {
  if (!effort || !Array.isArray(tiers) || !tiers.length) return null;
  if (tiers.includes(effort)) return effort;
  if (effort === 'high' && tiers.includes('medium') && tiers.includes('max') && !tiers.includes('high')) return 'medium';
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
