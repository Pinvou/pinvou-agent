#!/usr/bin/env node
/**
 * voice-normalize-eval.mjs — 语音听写「转写+纠错+LLM 整理」链路的离线评测脚本。
 *
 * 定位：dev-only 手工评测，不进 CI，结果不构成质量门禁。
 *
 * 生产对齐方式（本文件不维护任何生产逻辑的手抄副本）：
 * - 纠错/分类/校验：用 vm 从 src/platform/tauri/bridge/voice.js 切片抽取生产函数
 *   （applyVoiceDeterministicCorrections / classifyVoiceText / validateVoicePostprocessOutput /
 *   stripVoicePostprocessFences / normalizeVoiceMode），方式同
 *   tests/voice_input_error_logic.test.js 的切片抽取。分类与生产一致地基于原始 ASR 文本。
 * - LLM prompt 常量与 max_tokens：运行时从 src-tauri/src/app/commands/voice.rs 解析
 *   voice_postprocess_prompt（r#"…"# 常量）与 voice_postprocess_max_tokens；解析失败直接报错，
 *   不静默回退到旧副本。eval 不做生产的一次重试，使用 retry=false 的基础 max_tokens。
 * - user 消息拼装镜像 voice.rs 的 voice_postprocess_user_content（Rust 代码无法 vm 抽取，
 *   仅做字符串拼装镜像，见 buildEvalUserContent 注释）。
 * - eval 不对 LLM 输出做二次确定性纠正（生产明令禁止，见 voice.js finishVoiceInput：
 *   candidateText 直接取 postprocessResult.text），只做 sanitize + 生产校验 + 失败回退规则文本。
 *
 * LLM 凭据读取范围（仅 conditional_llm 策略需要）：
 * - 优先读环境变量 PINVOU_VOICE_EVAL_BASE_URL / PINVOU_VOICE_EVAL_API_KEY /
 *   PINVOU_VOICE_EVAL_MODEL；三者齐全即直接使用，不触任何其他凭据来源。
 * - 环境变量不全时回退读 ~/.pinvou3/settings.json 的 active model 配置；其中 api_key 缺失时，
 *   仅在 Windows 上通过 Advapi32 CredRead 读取凭据管理器中单个指定条目
 *   （target = `model:<id>.pinvou3-model-api-key`，见 loadPinvouActiveModelConfig），
 *   读到的值只作为当次请求的 Authorization 头使用，不打印、不记录、不写文件。
 *   非 Windows 平台不读任何系统凭据存储。
 */
import fs from 'node:fs';
import process from 'node:process';
import { execFileSync } from 'node:child_process';
import { performance } from 'node:perf_hooks';
import os from 'node:os';
import path from 'node:path';
import vm from 'node:vm';

const DEFAULT_CASES_PATH = 'tests/fixtures/voice-normalize-labeled-samples.json';
const VOICE_BRIDGE_PATH = 'src/platform/tauri/bridge/voice.js';
const VOICE_RUST_PATH = 'src-tauri/src/app/commands/voice.rs';
const PINVOU_SETTINGS_PATH = path.join(os.homedir(), '.pinvou3', 'settings.json');

const USAGE = `用法: node scripts/voice-normalize-eval.mjs [选项]  （dev-only，不进 CI）

选项:
  --cases <path>        标注语料，默认 ${DEFAULT_CASES_PATH}
  --observed <path>     真实观测样本（json/jsonl）；提供后默认 strategy=observed
  --strategy <name>     asr_only | rules_only | conditional_llm | observed
                        （默认：有 --observed 则 observed，否则 conditional_llm）
  --classify-check      只跑生产 classifyVoiceText 的分类对照，不调 LLM，打印每条决策
  --help                打印本说明

说明: 纠错/分类/校验函数 vm 抽取自 ${VOICE_BRIDGE_PATH}；
LLM prompt 与 max_tokens 解析自 ${VOICE_RUST_PATH}。
conditional_llm 需要 PINVOU_VOICE_EVAL_BASE_URL / PINVOU_VOICE_EVAL_API_KEY /
PINVOU_VOICE_EVAL_MODEL，否则回退 ~/.pinvou3/settings.json 的 active model。`;

/**
 * 从 voice.js 抽取生产实现。切片锚点与 tests/voice_input_error_logic.test.js 保持一致；
 * 锚点或守卫规则缺失时直接抛错（宁可使 eval 不可用，也不衡量一条漂移的管线）。
 */
function extractVoiceProductionLogic() {
  const source = fs.readFileSync(VOICE_BRIDGE_PATH, 'utf8');
  const start = source.indexOf('  function normalizeVoiceMode(mode) {');
  const end = source.indexOf('\n  function logVoicePipeline(', start);
  if (start === -1 || end === -1) {
    throw new Error(`voice.js 切片锚点缺失（normalizeVoiceMode/logVoicePipeline），无法对齐生产: ${VOICE_BRIDGE_PATH}`);
  }
  const fenceStart = source.indexOf('  function stripVoicePostprocessFences(text) {');
  const fenceEnd = source.indexOf('\n  async function postprocessVoiceText(', fenceStart);
  if (fenceStart === -1 || fenceEnd === -1) {
    throw new Error(`voice.js 切片锚点缺失（stripVoicePostprocessFences/postprocessVoiceText），无法对齐生产: ${VOICE_BRIDGE_PATH}`);
  }
  const slice = source.slice(start, end);
  // 与 tests/voice_input_error_logic.test.js 的漂移守卫对齐：抽取到的生产切片必须仍含
  // 边界版 Pinvou 纠错规则；若生产规则改名/删除，eval 立即报错而不是继续衡量旧行为。
  if (!/产品名con(?![a-zA-Z])/.test(slice)) {
    throw new Error('voice.js 切片缺少边界版 Pinvou 纠错规则（产品名con(?![a-zA-Z])），生产规则可能已变化，请先同步 eval');
  }
  const context = { performance: { now: () => 0 }, Date };
  vm.createContext(context);
  vm.runInContext(
    `${slice}\n${source.slice(fenceStart, fenceEnd)}
this.normalizeVoiceMode = normalizeVoiceMode;
this.classifyVoiceText = classifyVoiceText;
this.applyVoiceDeterministicCorrections = applyVoiceDeterministicCorrections;
this.validateVoicePostprocessOutput = validateVoicePostprocessOutput;
this.stripVoicePostprocessFences = stripVoicePostprocessFences;`,
    context,
    { filename: VOICE_BRIDGE_PATH },
  );
  return context;
}

/**
 * 从 voice.rs 解析生产 LLM 配置：三个 mode 的 postprocess prompt 常量（顺序 edit/task/dictation，
 * 与 voice_postprocess_prompt 的 if/else if/else 分支一致）和基础 max_tokens（retry=false）。
 */
function extractVoicePostprocessLlmConfig() {
  const rust = fs.readFileSync(VOICE_RUST_PATH, 'utf8');
  const promptStart = rust.indexOf('fn voice_postprocess_prompt(');
  const promptEnd = rust.indexOf('\nfn voice_postprocess_timeout(', promptStart);
  if (promptStart === -1 || promptEnd === -1) {
    throw new Error(`voice.rs 缺少 voice_postprocess_prompt，无法对齐生产 prompt: ${VOICE_RUST_PATH}`);
  }
  const blocks = [...rust.slice(promptStart, promptEnd).matchAll(/r#"([\s\S]*?)"#/g)].map(m => m[1]);
  if (blocks.length !== 3) {
    throw new Error(`voice.rs voice_postprocess_prompt 应含 3 个 r#"…"# prompt 块（edit/task/dictation），实际解析到 ${blocks.length} 个`);
  }
  const mtStart = rust.indexOf('fn voice_postprocess_max_tokens(');
  if (mtStart === -1) {
    throw new Error(`voice.rs 缺少 voice_postprocess_max_tokens，无法对齐生产 max_tokens: ${VOICE_RUST_PATH}`);
  }
  const mtBody = rust.slice(mtStart, mtStart + 500);
  const edit = mtBody.match(/"edit"\s*=>\s*(\d+)/u);
  const task = mtBody.match(/"task"\s*=>\s*(\d+)/u);
  const fallback = mtBody.match(/_\s*=>\s*(\d+)/u);
  if (!edit || !task || !fallback) {
    throw new Error('voice.rs voice_postprocess_max_tokens 的 edit/task/default 分支解析失败');
  }
  return {
    prompts: { edit: blocks[0], task: blocks[1], dictation: blocks[2] },
    maxTokens: { edit: Number(edit[1]), task: Number(task[1]), dictation: Number(fallback[1]) },
  };
}

/**
 * 镜像 voice.rs voice_postprocess_user_content 的分段拼装（Rust 代码无法 vm 抽取）。
 * 生产在规则纠错后文本与原始 ASR 不同时会附带原始识别段，供模型撤销误纠；
 * eval 无输入框草稿，省略 DRAFT_TEXT 段。若生产改动该函数格式需同步此处。
 */
function buildEvalUserContent(correctedText, rawText) {
  const corrected = String(correctedText || '').trim();
  const asrRaw = String(rawText || '').trim();
  const sections = [];
  if (asrRaw && asrRaw !== corrected) {
    sections.push(`原始 ASR 识别（第一段为原始识别，第二段为规则纠错后文本；纠错可能有误，可参考原始识别恢复）：\n<<<ASR_RAW>>>\n${asrRaw}\n<<<END>>>`);
  }
  sections.push(`ASR 文本（规则纠错后）：\n<<<ASR_TEXT>>>\n${corrected}\n<<<END>>>`);
  return sections.join('\n\n');
}

/**
 * 镜像 voice.rs sanitize_voice_postprocess_output：剥开头成对 `<think>` 推理段
 * （voice.rs strip_leading_thinking_block）+ trim + 去 BOM + 每种引号只剥最外层
 * 一对（孤立引号保留、空包裹对剥为空），再复用生产 stripVoicePostprocessFences
 * （vm 抽取）剥整包围栏。不得用剥所有首尾引号的正则替代——那会与生产在
 * `以引号结尾"`、`''text''`、`"text` 等边界上漂移，eval 的校验/评分字符串就和
 * 实际写回输入框的内容不一致了。
 */
function sanitizeEvalLlmOutput(text, production) {
  const withoutThinking = stripLeadingThinkingBlock(String(text || ''));
  const bomStripped = withoutThinking
    .trim()
    .replace(/^[\uFEFF]+|[\uFEFF]+$/g, '')
    .trim();
  const unquoted = stripOneWrappingQuote(stripOneWrappingQuote(bomStripped, '"'), "'");
  // 生产链剥围栏两次(Rust strip_voice_markdown_fence 一次 + 前端
  // stripVoicePostprocessFences 一次);镜像剥到不动点,至少覆盖生产的
  // 双重剥离(三层以上嵌套围栏在真实输出中未观测到)。
  let unFenced = unquoted;
  for (;;) {
    const next = production.stripVoicePostprocessFences(unFenced);
    if (next === unFenced) break;
    unFenced = next;
  }
  return unFenced.trim();
}

// 镜像 voice.rs 的 trim_start 语义:Rust char::is_whitespace
// (Unicode White_Space)与 JS \s 的空白类不同——\s 含 U+FEFF 不含 U+0085,
// Rust 相反。这里用 Rust 的精确空白类(另含生产侧一并容忍的 U+FEFF)镜像,
// 避免极端前缀上镜像与生产漂移。
// eslint-disable-next-line no-control-regex -- U+000B/U+000C are White_Space in Rust's char::is_whitespace, which this class mirrors exactly
const RUST_WS_OR_BOM_START = /^(?:[\t\n\u000B\u000C\r\u0020\u0085\u00A0\u1680\u2000-\u200A\u2028\u2029\u202F\u205F\u3000]|\uFEFF)+/;

// 镜像 voice.rs strip_leading_thinking_block：只处理开头的成对
// `<think>…</think>`（容忍前导空白与 BOM，空白类与 Rust is_whitespace 一致，
// 另含生产侧一并容忍的 U+FEFF），开头未闭合视为空输出（交给空输出
// 契约），正文中段的裸 `<think>` 视为字面内容不动。
function stripLeadingThinkingBlock(text) {
  const trimmed = text.replace(RUST_WS_OR_BOM_START, '');
  if (!trimmed.startsWith('<think>')) return text;
  const rest = trimmed.slice('<think>'.length);
  const end = rest.indexOf('</think>');
  if (end === -1) return '';
  return rest.slice(end + '</think>'.length).replace(RUST_WS_OR_BOM_START, '');
}

// 镜像 voice.rs strip_wrapping_quote：单字符视为歧义输出保留；只剥同种引号
// 的一对外层配对（先剥前缀再剥后缀，`'text"` 这类混用引号原样保留）。
function stripOneWrappingQuote(text, quote) {
  if ([...text].length === 1) return text;
  if (!text.startsWith(quote) || !text.endsWith(quote)) return text;
  return text.slice(1, -1);
}

function parseArgs(argv) {
  const args = {};
  for (let i = 2; i < argv.length; i += 1) {
    const arg = argv[i];
    if (!arg.startsWith('--')) continue;
    const key = arg.slice(2);
    const next = argv[i + 1];
    if (next && !next.startsWith('--')) {
      args[key] = next;
      i += 1;
    } else {
      args[key] = true;
    }
  }
  return args;
}

function loadJsonOrJsonl(filePath) {
  if (!filePath) return null;
  const raw = fs.readFileSync(filePath, 'utf8').trim();
  if (!raw) return [];
  if (filePath.endsWith('.jsonl')) return raw.split(/\r?\n/).filter(Boolean).map(line => JSON.parse(line));
  const parsed = JSON.parse(raw);
  if (Array.isArray(parsed)) return parsed;
  // 文件级 _provenance 元信息 + samples 数组的对象形态
  if (parsed && Array.isArray(parsed.samples)) return parsed.samples;
  return [parsed];
}

function normalizeId(id) {
  return String(id || '').replace(/_\d+$/u, '');
}

function normalizeCase(sample) {
  const expected = Array.isArray(sample.keywords) ? sample.keywords : sample.expected || [];
  const asrErrors = Array.isArray(sample.asr_expected_errors) ? sample.asr_expected_errors : [];
  const forbiddenAdditions = Array.isArray(sample.forbidden_additions) ? sample.forbidden_additions : sample.forbidden || [];
  const forbidden = [...new Set([...asrErrors, ...forbiddenAdditions])]
    .filter(term => !expected.some(expectedTerm => normalizedEquivalent(term, expectedTerm)));
  return {
    id: sample.id,
    normalized_id: normalizeId(sample.id),
    type: sample.type || '',
    mode: sample.mode || 'dictation',
    raw: sample.asr_text || sample.raw || sample.raw_text || '',
    gold_text: sample.gold_text || '',
    expected_final_text: sample.expected_final_text || sample.expected_text || sample.gold_text || '',
    allow_empty_final: !!sample.allow_empty_final,
    // false 表示 expected_final_text 与生产 prompt 输出形态不一致，expected_match 仅作信息展示
    expected_match_scored: sample.expected_match_scored !== false,
    expected,
    forbidden,
    forbidden_additions: forbiddenAdditions,
    asr_expected_errors: asrErrors,
  };
}

function loadCases(filePath) {
  return loadJsonOrJsonl(filePath || DEFAULT_CASES_PATH).map(normalizeCase);
}

function findObserved(observed, testCase) {
  if (!observed) return null;
  return observed.find(item => (
    item.id === testCase.id ||
    normalizeId(item.id) === testCase.normalized_id ||
    item.raw_text === testCase.raw ||
    item.asr_text === testCase.raw
  )) || null;
}

function normalizeTextForCompare(text) {
  const value = String(text || '')
    .replace(/ＡＩ/giu, 'AI')
    .replace(/ｐｄｆ/giu, 'PDF')
    .replace(/\brest\s+api\b/giu, 'REST API')
    .replace(/AI新闻/gu, 'AI 新闻')
    .replace(/pDF/gu, 'PDF')
    .replace(/三百/gu, '300')
    .replace(/三点/gu, '3点')
    .replace(/三条/gu, '3条')
    .replace(/三个/gu, '3个')
    .replace(/各列3条/gu, '各列三条');
  return value
    .toLowerCase()
    .replace(/[\s。！？!?，,、；;：:"'“”‘’（）()【】\][.…—-]/gu, '')
    .trim();
}

function normalizedEquivalent(a, b) {
  return normalizeTextForCompare(a) === normalizeTextForCompare(b);
}

function normalizedIncludes(text, term) {
  const normalizedText = normalizeTextForCompare(text);
  const normalizedTerm = normalizeTextForCompare(term);
  return !!normalizedTerm && normalizedText.includes(normalizedTerm);
}

function forbiddenIncludes(text, term) {
  const value = String(text || '');
  const rawTerm = String(term || '');
  if (!rawTerm.trim()) return false;
  return value.includes(rawTerm) || value.toLowerCase().includes(rawTerm.toLowerCase());
}

function scoreOutput(testCase, output) {
  const text = String(output || '').trim();
  if (testCase.allow_empty_final) {
    return {
      score: text ? 0 : 100,
      expected_hits: [],
      missing_expected: [],
      forbidden_hits: testCase.forbidden.filter(term => forbiddenIncludes(text, term)),
      expected_match: testCase.expected_match_scored ? !text : null,
      cleanup_pass: !text,
      intent_pass: !text,
      over_rewrite: false,
    };
  }

  const expectedHits = testCase.expected.filter(term => normalizedIncludes(text, term));
  const missingExpected = testCase.expected.filter(term => !normalizedIncludes(text, term));
  const forbiddenHits = testCase.forbidden.filter(term => forbiddenIncludes(text, term));
  const additionHits = testCase.forbidden_additions.filter(term => forbiddenIncludes(text, term));
  const expectedScore = testCase.expected.length ? expectedHits.length / testCase.expected.length : 1;
  const forbiddenPenalty = testCase.forbidden.length ? forbiddenHits.length / testCase.forbidden.length : 0;
  const expectedNorm = normalizeTextForCompare(testCase.expected_final_text);
  const outputNorm = normalizeTextForCompare(text);
  // expected_match 仅作信息展示，不参与 score/失败判定：dictation 样本的 expected_final_text
  // 是单句理想值，与生产 dictation prompt 的 Markdown 列表输出形态不同（fixture _provenance 已标注）。
  const expectedMatch = testCase.expected_match_scored && expectedNorm ? outputNorm === expectedNorm : null;
  const score = Math.max(0, Math.round((expectedScore - forbiddenPenalty) * 100));
  return {
    score,
    expected_hits: expectedHits,
    missing_expected: missingExpected,
    forbidden_hits: forbiddenHits,
    expected_match: expectedMatch,
    cleanup_pass: forbiddenHits.length === 0,
    intent_pass: missingExpected.length === 0 && additionHits.length === 0,
    over_rewrite: additionHits.length > 0,
  };
}

function readWindowsCredential(target) {
  // 读取范围：仅读凭据管理器中 target 指定的单条凭据；返回值只用作 Authorization 头，
  // 不打印、不记录、不写文件。非 Windows 平台或读取失败返回空串。
  if (process.platform !== 'win32' || !target) return '';
  const escapedTarget = String(target).replace(/'/g, "''");
  const script = String.raw`
$Target='__PINVOU_CREDENTIAL_TARGET__'
Add-Type -Namespace WinCred -Name Native -MemberDefinition @"
[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential, CharSet=System.Runtime.InteropServices.CharSet.Unicode)]
public struct CREDENTIAL {
  public UInt32 Flags;
  public UInt32 Type;
  public string TargetName;
  public string Comment;
  public System.Runtime.InteropServices.ComTypes.FILETIME LastWritten;
  public UInt32 CredentialBlobSize;
  public System.IntPtr CredentialBlob;
  public UInt32 Persist;
  public UInt32 AttributeCount;
  public System.IntPtr Attributes;
  public string TargetAlias;
  public string UserName;
}
[System.Runtime.InteropServices.DllImport("Advapi32.dll", EntryPoint="CredReadW", CharSet=System.Runtime.InteropServices.CharSet.Unicode, SetLastError=true)]
public static extern bool CredRead(string target, int type, int reservedFlag, out System.IntPtr credentialPtr);
[System.Runtime.InteropServices.DllImport("Advapi32.dll", SetLastError=true)]
public static extern void CredFree(System.IntPtr buffer);
"@
$ptr=[IntPtr]::Zero
if(-not [WinCred.Native]::CredRead($Target,1,0,[ref]$ptr)){ exit 1 }
try {
  $cred=[Runtime.InteropServices.Marshal]::PtrToStructure($ptr,[type][WinCred.Native+CREDENTIAL])
  $bytes=New-Object byte[] $cred.CredentialBlobSize
  [Runtime.InteropServices.Marshal]::Copy($cred.CredentialBlob,$bytes,0,$bytes.Length)
  $utf16=[Text.Encoding]::Unicode.GetString($bytes).TrimEnd([char]0)
  $utf8=[Text.Encoding]::UTF8.GetString($bytes).TrimEnd([char]0)
  if($utf16 -match '^[ -~]+$' -and $utf16.Length -ge 8){ [Console]::Out.Write($utf16) }
  else { [Console]::Out.Write($utf8) }
} finally {
  [WinCred.Native]::CredFree($ptr)
}
`;
  try {
    return execFileSync('powershell.exe', [
      '-NoProfile',
      '-ExecutionPolicy',
      'Bypass',
      '-Command',
      script.replace('__PINVOU_CREDENTIAL_TARGET__', () => escapedTarget),
    ], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();
  } catch {
    return '';
  }
}

function loadPinvouActiveModelConfig() {
  if (!fs.existsSync(PINVOU_SETTINGS_PATH)) return null;
  try {
    const settings = JSON.parse(fs.readFileSync(PINVOU_SETTINGS_PATH, 'utf8'));
    const advanced = settings?.advanced || {};
    const models = Array.isArray(advanced.saved_models) ? advanced.saved_models : [];
    const activeId = advanced.active_model_id || models[0]?.id;
    const model = models.find(item => item?.id === activeId) || models.find(item => item?.base_url && item?.model);
    if (!model || !model.base_url || !model.model) return null;
    const credentialRef = model.credential_ref || {};
    const service = credentialRef.service || 'pinvou3-model-api-key';
    const account = credentialRef.account || `model:${model.id}`;
    const target = `${account}.${service}`;
    // 环境变量优先：PINVOU_VOICE_EVAL_API_KEY 已提供时不触凭据管理器
    const apiKey = process.env.PINVOU_VOICE_EVAL_API_KEY || model.api_key || readWindowsCredential(target);
    return {
      baseUrl: model.base_url,
      apiKey,
      model: model.model,
      source: `pinvou_settings:${model.id || activeId || 'unknown'}`,
    };
  } catch {
    return null;
  }
}

function resolveEvalModelConfig() {
  const envConfig = {
    baseUrl: process.env.PINVOU_VOICE_EVAL_BASE_URL,
    apiKey: process.env.PINVOU_VOICE_EVAL_API_KEY,
    model: process.env.PINVOU_VOICE_EVAL_MODEL,
    source: 'env',
  };
  // 环境变量三者齐全即直接使用，不读 settings.json、不触凭据管理器
  if (envConfig.baseUrl && envConfig.apiKey && envConfig.model) return envConfig;
  return loadPinvouActiveModelConfig() || envConfig;
}

/**
 * 对齐生产 openai-compatible 请求（voice.rs call_voice_postprocess_model）：
 * system = 生产 mode prompt，user = voice_postprocess_user_content 拼装，
 * temperature=0，max_tokens=生产基础值（retry=false），stream=false。
 * finish_reason=length 与生产一致视为截断失败（抛错走回退）。
 */
async function callOpenAiCompatible(correctedText, rawText, mode, production, llmConfig) {
  const config = resolveEvalModelConfig();
  if (!config.baseUrl || !config.apiKey || !config.model) {
    throw new Error('missing PINVOU_VOICE_EVAL_BASE_URL / PINVOU_VOICE_EVAL_API_KEY / PINVOU_VOICE_EVAL_MODEL and no usable Pinvou active model credential');
  }
  const normalizedMode = production.normalizeVoiceMode(mode);
  const started = performance.now();
  const response = await fetch(`${config.baseUrl.replace(/\/$/, '')}/chat/completions`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${config.apiKey}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      model: config.model,
      messages: [
        { role: 'system', content: llmConfig.prompts[normalizedMode] },
        { role: 'user', content: buildEvalUserContent(correctedText, rawText) },
      ],
      temperature: 0,
      max_tokens: llmConfig.maxTokens[normalizedMode],
      stream: false,
    }),
  });
  if (!response.ok) throw new Error(`LLM HTTP ${response.status}: ${await response.text()}`);
  const value = await response.json();
  const choice = value?.choices?.[0];
  if (choice?.finish_reason === 'length') {
    throw new Error('llm_truncated: finish_reason=length（与生产一致，截断输出不写回）');
  }
  const output = String(choice?.message?.content || '').trim();
  return { output, llm_ms: Math.round(performance.now() - started), source: config.source };
}

function percentileNearest(values, p) {
  const nums = values.filter(value => Number.isFinite(value)).sort((a, b) => a - b);
  if (!nums.length) return null;
  const idx = Math.ceil((p / 100) * nums.length) - 1;
  return nums[Math.max(0, Math.min(nums.length - 1, idx))];
}

function runClassifyCheck(cases, production) {
  const rows = cases.map(testCase => {
    const classified = production.classifyVoiceText(testCase.raw, testCase.mode);
    return {
      id: testCase.id,
      mode: testCase.mode,
      strategy: classified.strategy,
      reason: classified.reason,
      suspicious: classified.suspicious_terms.join('|'),
    };
  });
  console.table(rows);
  const counts = {};
  for (const row of rows) counts[row.strategy] = (counts[row.strategy] || 0) + 1;
  console.log(JSON.stringify({ samples: rows.length, strategy_counts: counts }, null, 2));
  console.log(`对照说明: eval 的分类决策即 voice.js 生产 classifyVoiceText 的 vm 抽取调用，与生产同代码、构造性一致（本批 ${rows.length} 条，无第二份实现可比对，不构成独立验证）。`);
}

async function main() {
  const args = parseArgs(process.argv);
  if (args.help) {
    console.log(USAGE);
    return;
  }
  const production = extractVoiceProductionLogic();
  const cases = loadCases(args.cases || DEFAULT_CASES_PATH);
  if (args['classify-check']) {
    runClassifyCheck(cases, production);
    return;
  }
  const observed = loadJsonOrJsonl(args.observed);
  const evalStrategy = String(args.strategy || (observed ? 'observed' : 'conditional_llm'));
  const llmConfig = evalStrategy === 'conditional_llm' ? extractVoicePostprocessLlmConfig() : null;
  const rows = [];

  for (const testCase of cases) {
    let output;
    let llmMs = null;
    let source = observed ? 'observed' : evalStrategy;
    let error = '';
    let decision = '';
    let fallbackReason = '';
    let llmCalled = false;
    if (observed) {
      const match = findObserved(observed, testCase);
      if (!match) {
        rows.push({ id: testCase.id, source: 'skipped', score: null, llm_ms: null, output: '', error: 'no observed sample' });
        continue;
      }
      output = match.final_text || match.output || match.normalized_text || '';
      llmMs = match.llm_ms ?? null;
    } else {
      const rawText = testCase.raw;
      const ruleText = production.applyVoiceDeterministicCorrections(rawText, rawText);
      // 与生产一致：分类基于原始 ASR 文本（voice.js finishVoiceInput 的注释说明了对纠错后文本分类的静默误纠风险）
      const classified = production.classifyVoiceText(rawText, testCase.mode);
      decision = classified.reason || classified.strategy;
      if (evalStrategy === 'asr_only') {
        output = rawText;
      } else if (evalStrategy === 'rules_only') {
        output = ruleText;
      } else if (evalStrategy === 'conditional_llm' && classified.strategy === 'skip_empty') {
        output = '';
        source = 'rules_skip_empty';
      } else if (evalStrategy === 'conditional_llm' && classified.strategy === 'use_asr') {
        output = ruleText;
        source = `rules_${classified.reason}`;
      } else {
        try {
          llmCalled = true;
          const result = await callOpenAiCompatible(ruleText, rawText, testCase.mode, production, llmConfig);
          // 与生产一致：LLM 输出不再过第二遍确定性规则，sanitize 后直接走生产校验，失败回退规则文本
          const candidate = sanitizeEvalLlmOutput(result.output, production);
          const valid = production.validateVoicePostprocessOutput(rawText, ruleText, candidate, testCase.mode);
          output = valid ? candidate : ruleText;
          fallbackReason = valid ? '' : 'llm_output_invalid';
          llmMs = result.llm_ms;
          source = result.source || source;
        } catch (err) {
          output = ruleText;
          fallbackReason = 'llm_error';
          error = String(err.message || err);
        }
      }
    }

    if (source === 'skipped') {
      rows.push({ id: testCase.id, source, score: null, llm_ms: null, output: '', error });
      continue;
    }
    rows.push({
      id: testCase.id,
      source,
      decision,
      fallback_reason: fallbackReason,
      llm_called: llmCalled,
      llm_ms: llmMs,
      output,
      ...scoreOutput(testCase, output),
    });
  }

  console.table(rows.map(row => ({
    id: row.id,
    source: row.source,
    decision: row.decision || '',
    score: row.score,
    llm_ms: row.llm_ms,
    fallback: row.fallback_reason || '',
    missing: (row.missing_expected || []).join('|'),
    forbidden: (row.forbidden_hits || []).join('|'),
    output: row.output,
    error: row.error || '',
  })));

  const scored = rows.filter(row => row.score !== null);
  const totalExpected = scored.reduce((sum, row) => sum + (row.expected_hits?.length || 0) + (row.missing_expected?.length || 0), 0);
  const hitExpected = scored.reduce((sum, row) => sum + (row.expected_hits?.length || 0), 0);
  const usableCount = scored.filter(row => row.intent_pass && row.cleanup_pass && !row.over_rewrite).length;
  const llmCalledCount = scored.filter(row => row.llm_called).length;
  const fallbackCount = scored.filter(row => row.fallback_reason).length;
  const summary = {
    strategy: evalStrategy,
    samples: scored.length,
    skipped: rows.length - scored.length,
    final_keyword_hit_rate: totalExpected ? `${hitExpected}/${totalExpected} = ${(hitExpected / totalExpected * 100).toFixed(1)}%` : 'N/A',
    intent_preservation_rate: scored.length ? `${scored.filter(row => row.intent_pass).length}/${scored.length} = ${(scored.filter(row => row.intent_pass).length / scored.length * 100).toFixed(1)}%` : 'N/A',
    over_rewrite_rate: scored.length ? `${scored.filter(row => row.over_rewrite).length}/${scored.length} = ${(scored.filter(row => row.over_rewrite).length / scored.length * 100).toFixed(1)}%` : 'N/A',
    cleanup_success_rate: scored.length ? `${scored.filter(row => row.cleanup_pass).length}/${scored.length} = ${(scored.filter(row => row.cleanup_pass).length / scored.length * 100).toFixed(1)}%` : 'N/A',
    first_pass_usable_rate: scored.length ? `${usableCount}/${scored.length} = ${(usableCount / scored.length * 100).toFixed(1)}%` : 'N/A',
    estimated_manual_edit_rate: scored.length ? `${scored.length - usableCount}/${scored.length} = ${((1 - usableCount / scored.length) * 100).toFixed(1)}%` : 'N/A',
    llm_call_rate: scored.length ? `${llmCalledCount}/${scored.length} = ${(llmCalledCount / scored.length * 100).toFixed(1)}%` : 'N/A',
    fallback_rate: scored.length ? `${fallbackCount}/${scored.length} = ${(fallbackCount / scored.length * 100).toFixed(1)}%` : 'N/A',
    llm_p90: percentileNearest(scored.map(row => row.llm_ms), 90),
  };
  console.log(JSON.stringify(summary, null, 2));

  const failures = scored.filter(row => row.score < 80 || !row.intent_pass || row.over_rewrite || !row.cleanup_pass);
  if (failures.length || !scored.length) process.exitCode = 1;
}

await main().catch(error => {
  console.error(error);
  process.exit(1);
});
