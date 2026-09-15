#!/usr/bin/env node
/**
 * voice-normalize-eval.mjs — Offline evaluation harness for the voice dictation pipeline
 * (transcription + correction + LLM post-processing).
 *
 * Positioning: dev-only manual evaluation; not run in CI; results are not a quality gate.
 *
 * How production alignment works (this file keeps no hand-copied duplicates of production logic):
 * - Correction/classification/validation: the production functions are slice-extracted from
 *   src/platform/tauri/bridge/voice.js via vm (applyVoiceDeterministicCorrections / classifyVoiceText /
 *   validateVoicePostprocessOutput / stripVoicePostprocessFences / normalizeVoiceMode), the same way
 *   tests/voice_input_error_logic.test.js extracts slices. Classification uses the raw ASR text, matching production.
 * - LLM prompt constants and max_tokens: parsed at runtime from src-tauri/src/app/commands/voice.rs
 *   (voice_postprocess_prompt, the r#"…"# constants, and voice_postprocess_max_tokens); parse failures throw
 *   instead of silently falling back to a stale copy. eval does not perform production's one retry and uses the base max_tokens (retry=false).
 * - The user message assembly mirrors voice.rs's voice_postprocess_user_content (Rust code cannot be vm-extracted,
 *   so only the string assembly is mirrored; see the buildEvalUserContent comment).
 * - eval does not apply a second deterministic correction pass to LLM output (explicitly forbidden in production, see voice.js finishVoiceInput:
 *   candidateText takes postprocessResult.text directly); it only sanitizes + runs production validation + falls back to the rule text on failure.
 *
 * LLM credential read scope (needed only by the conditional_llm strategy):
 * - Prefers the PINVOU_VOICE_EVAL_BASE_URL / PINVOU_VOICE_EVAL_API_KEY /
 *   PINVOU_VOICE_EVAL_MODEL environment variables; when all three are set they are used directly and no other credential source is touched.
 * - When the environment variables are incomplete, fall back to the active model config in ~/.pinvou3/settings.json; if its api_key is missing,
 *   then on Windows only, read a single named entry from Credential Manager via Advapi32 CredRead
 *   (target = `model:<id>.pinvou3-model-api-key`, see loadPinvouActiveModelConfig);
 *   the value is used only as the Authorization header of that request — never printed, logged, or written to a file.
 *   Non-Windows platforms read no system credential store.
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

const USAGE = `Usage: node scripts/voice-normalize-eval.mjs [options]  (dev-only, not run in CI)

Options:
  --cases <path>        Labeled corpus, default ${DEFAULT_CASES_PATH}
  --observed <path>     Real observed samples (json/jsonl); when given, strategy defaults to observed
  --strategy <name>     asr_only | rules_only | conditional_llm | observed
                        (default: observed when --observed is given, otherwise conditional_llm)
  --classify-check      Run only the classification comparison against the production classifyVoiceText; no LLM calls; prints every decision
  --help                Print this help

Notes: the correction/classification/validation functions are vm-extracted from ${VOICE_BRIDGE_PATH};
the LLM prompt and max_tokens are parsed from ${VOICE_RUST_PATH}.
conditional_llm requires PINVOU_VOICE_EVAL_BASE_URL / PINVOU_VOICE_EVAL_API_KEY /
PINVOU_VOICE_EVAL_MODEL, otherwise it falls back to the active model in ~/.pinvou3/settings.json.`;

/**
 * Extract the production implementations from voice.js. The slice anchors stay in sync with
 * tests/voice_input_error_logic.test.js; if an anchor or guard rule goes missing, throw immediately
 * (better to make eval unusable than to measure a drifted pipeline).
 */
function extractVoiceProductionLogic() {
  const source = fs.readFileSync(VOICE_BRIDGE_PATH, 'utf8');
  const start = source.indexOf('  function normalizeVoiceMode(mode) {');
  const end = source.indexOf('\n  function logVoicePipeline(', start);
  if (start === -1 || end === -1) {
    throw new Error(`voice.js slice anchors missing (normalizeVoiceMode/logVoicePipeline); cannot align with production: ${VOICE_BRIDGE_PATH}`);
  }
  const fenceStart = source.indexOf('  function stripVoicePostprocessFences(text) {');
  const fenceEnd = source.indexOf('\n  async function postprocessVoiceText(', fenceStart);
  if (fenceStart === -1 || fenceEnd === -1) {
    throw new Error(`voice.js slice anchors missing (stripVoicePostprocessFences/postprocessVoiceText); cannot align with production: ${VOICE_BRIDGE_PATH}`);
  }
  const slice = source.slice(start, end);
  // Mirrors the drift guard in tests/voice_input_error_logic.test.js: the extracted production slice must still
  // contain the boundary-aware Pinvou correction rule; if the production rule is renamed or removed, eval fails
  // immediately instead of continuing to measure the stale behavior.
  if (!/产品名con(?![a-zA-Z])/.test(slice)) {
    throw new Error('voice.js slice is missing the boundary-aware Pinvou correction rule (产品名con(?![a-zA-Z])); the production rule may have changed — sync eval first');
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
 * Parse the production LLM config from voice.rs: the postprocess prompt constants for the three modes
 * (in edit/task/dictation order, matching voice_postprocess_prompt's if/else if/else branches)
 * and the base max_tokens (retry=false).
 */
function extractVoicePostprocessLlmConfig() {
  const rust = fs.readFileSync(VOICE_RUST_PATH, 'utf8');
  const promptStart = rust.indexOf('fn voice_postprocess_prompt(');
  const promptEnd = rust.indexOf('\nfn voice_postprocess_timeout(', promptStart);
  if (promptStart === -1 || promptEnd === -1) {
    throw new Error(`voice.rs is missing voice_postprocess_prompt; cannot align with the production prompt: ${VOICE_RUST_PATH}`);
  }
  const blocks = [...rust.slice(promptStart, promptEnd).matchAll(/r#"([\s\S]*?)"#/g)].map(m => m[1]);
  if (blocks.length !== 3) {
    throw new Error(`voice.rs voice_postprocess_prompt should contain 3 r#"…"# prompt blocks (edit/task/dictation), but parsed ${blocks.length}`);
  }
  const mtStart = rust.indexOf('fn voice_postprocess_max_tokens(');
  if (mtStart === -1) {
    throw new Error(`voice.rs is missing voice_postprocess_max_tokens; cannot align with the production max_tokens: ${VOICE_RUST_PATH}`);
  }
  const mtBody = rust.slice(mtStart, mtStart + 500);
  const edit = mtBody.match(/"edit"\s*=>\s*(\d+)/u);
  const task = mtBody.match(/"task"\s*=>\s*(\d+)/u);
  const fallback = mtBody.match(/_\s*=>\s*(\d+)/u);
  if (!edit || !task || !fallback) {
    throw new Error('failed to parse the edit/task/default branches of voice.rs voice_postprocess_max_tokens');
  }
  return {
    prompts: { edit: blocks[0], task: blocks[1], dictation: blocks[2] },
    maxTokens: { edit: Number(edit[1]), task: Number(task[1]), dictation: Number(fallback[1]) },
  };
}

/**
 * Mirrors the sectioned assembly of voice.rs voice_postprocess_user_content (Rust code cannot be vm-extracted).
 * When the post-rule-correction text differs from the raw ASR, production appends the raw recognition section so
 * the model can undo wrong corrections; eval has no input-box draft, so the DRAFT_TEXT section is omitted.
 * If production changes this function's format, update this mirror.
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
 * Mirrors voice.rs sanitize_voice_postprocess_output: strip a leading paired `<think>` reasoning block
 * (voice.rs strip_leading_thinking_block) + trim + remove the BOM + strip only the outermost pair of each
 * quote kind (lone quotes are kept, an empty wrapping pair strips to empty), then reuse the production
 * stripVoicePostprocessFences (vm-extracted) to strip full fences. Do not replace this with a regex that
 * strips all leading/trailing quotes — that would drift from production on edges like `ends with a quote"`,
 * `''text''`, and `"text`, and eval's validation/scoring strings would then no longer match what actually
 * gets written back into the input box.
 */
function sanitizeEvalLlmOutput(text, production) {
  const withoutThinking = stripLeadingThinkingBlock(String(text || ''));
  const bomStripped = withoutThinking
    .trim()
    .replace(/^[\uFEFF]+|[\uFEFF]+$/g, '')
    .trim();
  const unquoted = stripOneWrappingQuote(stripOneWrappingQuote(bomStripped, '"'), "'");
  // The production chain strips fences twice (Rust strip_voice_markdown_fence once + the frontend
  // stripVoicePostprocessFences once); the mirror strips to a fixpoint, covering at least the production's
  // double strip (fences nested three or more levels deep have not been observed in real output).
  let unFenced = unquoted;
  for (;;) {
    const next = production.stripVoicePostprocessFences(unFenced);
    if (next === unFenced) break;
    unFenced = next;
  }
  return unFenced.trim();
}

// Mirrors voice.rs's trim_start semantics: Rust char::is_whitespace
// (Unicode White_Space) and JS \s use different whitespace classes — \s includes U+FEFF but not U+0085,
// and Rust is the opposite. This mirrors Rust's exact whitespace class (plus the U+FEFF that production
// also tolerates) to avoid drift between the mirror and production on extreme prefixes.
// eslint-disable-next-line no-control-regex -- U+000B/U+000C are White_Space in Rust's char::is_whitespace, which this class mirrors exactly
const RUST_WS_OR_BOM_START = /^(?:[\t\n\u000B\u000C\r\u0020\u0085\u00A0\u1680\u2000-\u200A\u2028\u2029\u202F\u205F\u3000]|\uFEFF)+/;

// Mirrors voice.rs strip_leading_thinking_block: only handles a leading paired
// `<think>…</think>` (tolerating leading whitespace and BOM; the whitespace classes match Rust is_whitespace,
// plus the U+FEFF that production also tolerates). An unclosed block at the start is treated as empty output
// (handed to the empty-output contract), and a bare `<think>` in the middle of the body is treated as literal
// content and left untouched.
function stripLeadingThinkingBlock(text) {
  const trimmed = text.replace(RUST_WS_OR_BOM_START, '');
  if (!trimmed.startsWith('<think>')) return text;
  const rest = trimmed.slice('<think>'.length);
  const end = rest.indexOf('</think>');
  if (end === -1) return '';
  return rest.slice(end + '</think>'.length).replace(RUST_WS_OR_BOM_START, '');
}

// Mirrors voice.rs strip_wrapping_quote: a single character is treated as ambiguous output and kept; strips only
// one outer pair of the same quote kind (prefix first, then suffix — mixed quotes like `'text"` are kept as-is).
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
  // Object shape with file-level _provenance metadata + a samples array
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
    // false means expected_final_text does not match the production prompt's output shape; expected_match is informational only
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
  // expected_match is informational only and does not affect the score/failure verdict: the expected_final_text of
  // dictation samples is a single-sentence ideal, which differs from the Markdown list shape the production dictation
  // prompt emits (noted in the fixture _provenance).
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
  // Read scope: reads only the single Credential Manager entry named by target; the return value is used only as the
  // Authorization header — never printed, logged, or written to a file. Returns an empty string on non-Windows
  // platforms or when the read fails.
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
    // Environment variables win: when PINVOU_VOICE_EVAL_API_KEY is provided, Credential Manager is not touched
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
  // When all three environment variables are set, use them directly: no settings.json read, no Credential Manager access
  if (envConfig.baseUrl && envConfig.apiKey && envConfig.model) return envConfig;
  return loadPinvouActiveModelConfig() || envConfig;
}

/**
 * Mirrors the production openai-compatible request (voice.rs call_voice_postprocess_model):
 * system = the production mode prompt, user = the voice_postprocess_user_content assembly,
 * temperature=0, max_tokens = the production base value (retry=false), stream=false.
 * finish_reason=length is treated as a truncation failure, matching production (throws so the fallback runs).
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
    throw new Error('llm_truncated: finish_reason=length (matches production; truncated output is not written back)');
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
  console.log(`Comparison note: eval's classification decisions are vm-extracted calls of voice.js's production classifyVoiceText — same code as production and constructively identical (${rows.length} samples in this batch; there is no second implementation to compare against, so this is not independent verification).`);
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
      // Matches production: classification is based on the raw ASR text (the comment in voice.js finishVoiceInput explains the silent wrong-correction risk of classifying the corrected text)
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
          // Matches production: LLM output does not go through a second deterministic rule pass; after sanitize it goes straight to production validation, falling back to the rule text on failure
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
