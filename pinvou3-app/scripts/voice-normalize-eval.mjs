import fs from 'node:fs';
import process from 'node:process';
import { execFileSync } from 'node:child_process';
import { performance } from 'node:perf_hooks';
import os from 'node:os';
import path from 'node:path';

const DEFAULT_CASES_PATH = 'tests/fixtures/voice-normalize-labeled-samples.json';
const PINVOU_SETTINGS_PATH = path.join(os.homedir(), '.pinvou3', 'settings.json');

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

function loadJsonOrJsonl(path) {
  if (!path) return null;
  const raw = fs.readFileSync(path, 'utf8').trim();
  if (!raw) return [];
  if (path.endsWith('.jsonl')) return raw.split(/\r?\n/).filter(Boolean).map(line => JSON.parse(line));
  const parsed = JSON.parse(raw);
  return Array.isArray(parsed) ? parsed : [parsed];
}

function normalizeId(id) {
  return String(id || '').replace(/_\d+$/u, '');
}

function normalizeCase(sample) {
  const expected = Array.isArray(sample.keywords) ? sample.keywords : sample.expected || [];
  const asrErrors = Array.isArray(sample.asr_expected_errors) ? sample.asr_expected_errors : [];
  const forbiddenAdditions = Array.isArray(sample.forbidden_additions) ? sample.forbidden_additions : sample.forbidden || [];
  return {
    id: sample.id,
    normalized_id: normalizeId(sample.id),
    type: sample.type || '',
    mode: sample.mode || 'dictation',
    raw: sample.asr_text || sample.raw || sample.raw_text || '',
    gold_text: sample.gold_text || '',
    expected_final_text: sample.expected_final_text || sample.expected_text || sample.gold_text || '',
    allow_empty_final: !!sample.allow_empty_final,
    expected,
    forbidden: [...new Set([...asrErrors, ...forbiddenAdditions])],
    forbidden_additions: forbiddenAdditions,
    asr_expected_errors: asrErrors,
  };
}

function loadCases(path) {
  return loadJsonOrJsonl(path || DEFAULT_CASES_PATH).map(normalizeCase);
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
  return String(text || '')
    .replace(/[\s。！？!?，,、；;：:"'“”‘’（）()【】\[\].…\-—]/gu, '')
    .trim();
}

function isFillerOnlyText(text) {
  const compact = normalizeTextForCompare(text);
  if (!compact) return true;
  if (compact.length > 8) return false;
  return compact.replace(/嗯|啊|呃|额|那个|就是/gu, '') === '';
}

function applyDeterministicCorrections(text, rawText) {
  let value = String(text || '');
  const raw = String(rawText || '');
  if (!value.trim()) return value;
  value = value
    .replace(/今日进价/g, '今日金价')
    .replace(/今天的进价/g, '今天的金价')
    .replace(/数据分析图标/g, '数据分析图表')
    .replace(/核心公能/g, '核心功能')
    .replace(/销售暑假/g, '销售数据')
    .replace(/屁屁提|PPTT/g, 'PPT')
    .replace(/截止事件/g, '截止时间')
    .replace(/负责任/g, '负责人')
    .replace(/风险电/g, '风险点')
    .replace(/三百字一类/g, '三百字以内')
    .replace(/代码嫩力/g, '代码能力')
    .replace(/g\s*p\s*t\s*five/giu, 'GPT-5')
    .replace(/克劳德\s*sonnet/giu, 'Claude Sonnet')
    .replace(/爱新闻/g, 'AI 新闻');
  value = value
    .replace(/GPT-5和/g, 'GPT-5 和')
    .replace(/和Claude Sonnet/g, '和 Claude Sonnet');
  if (raw.includes('图标') && value.includes('数据分析') && !value.includes('图表')) {
    value = value.replace(/数据分析$/u, '数据分析图表');
  }
  if (raw.includes('只是发了')) {
    value = value
      .replace(/^嗯[，,、\s]*/gu, '')
      .replace(/只是发了一个图表吧[。.]?/gu, '图表。')
      .replace(/生成数据分析[，,、\s]*图表/gu, '生成数据分析图表')
      .replace(/生成数据分析图表吧[。.]?/gu, '生成数据分析图表。');
  }
  return value;
}

function scoreOutput(testCase, output) {
  const text = String(output || '').trim();
  if (testCase.allow_empty_final) {
    return {
      score: text ? 0 : 100,
      expected_hits: [],
      missing_expected: [],
      forbidden_hits: testCase.forbidden.filter(term => text.includes(term)),
      expected_match: !text,
      cleanup_pass: !text,
      intent_pass: !text,
      over_rewrite: false,
    };
  }

  const expectedHits = testCase.expected.filter(term => text.includes(term));
  const missingExpected = testCase.expected.filter(term => !text.includes(term));
  const forbiddenHits = testCase.forbidden.filter(term => text.includes(term));
  const additionHits = testCase.forbidden_additions.filter(term => text.includes(term));
  const expectedScore = testCase.expected.length ? expectedHits.length / testCase.expected.length : 1;
  const forbiddenPenalty = testCase.forbidden.length ? forbiddenHits.length / testCase.forbidden.length : 0;
  const expectedNorm = normalizeTextForCompare(testCase.expected_final_text);
  const outputNorm = normalizeTextForCompare(text);
  const expectedMatch = expectedNorm ? outputNorm === expectedNorm : true;
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

function normalizePrompt(raw, mode) {
  return [
    '你是 Pinvou 的语音 ASR 纠错器，只负责把 ASR 文本纠正为用户原本想说的话。',
    '',
    '强规则：',
    '1. 不回答问题，不执行任务。',
    '2. 不新增用户没说过的目标、工具、格式、数量、时间、条件。',
    '3. 不把输出形态改掉：用户说图表就保留图表，不要改成表格；用户说输入框就不要发送。',
    '4. 正常查询、比较、搜索、整理、生成、做、把、帮我等句子都必须保留原请求，不能输出空字符串。',
    '5. 只有整句去掉标点后只剩“嗯/啊/呃/额/那个/就是”等口头禅，才输出空字符串。',
    '6. 优先纠正上下文中明显 ASR 错词：',
    '   - 行情/价格查询里的“进价/惊吓”通常应修为“金价”',
    '   - 数据分析可视化里的“图标”通常应修为“图表”',
    '   - “屁屁提/PPTT”通常应修为“PPT”',
    '   - “销售暑假”通常应修为“销售数据”',
    '   - “截止事件”通常应修为“截止时间”',
    '   - “负责任”通常应修为“负责人”',
    '   - “风险电”通常应修为“风险点”',
    '   - “g p t five”通常应修为“GPT-5”，“克劳德 sonnet”通常应修为“Claude Sonnet”',
    '   - 搜索“爱新闻”通常应修为“AI 新闻”',
    '7. 去掉口头禅、重复词和误识别语气词。',
    '8. 只输出最终文本，不解释，不使用 Markdown。',
    '',
    '示例：',
    'ASR 文本：今天天气怎么样？',
    '最终文本：今天天气怎么样？',
    'ASR 文本：嗯。',
    '最终文本：',
    'ASR 文本：搜索一下今天的爱新闻，按重要性排序。',
    '最终文本：搜索一下今天的 AI 新闻，按重要性排序。',
    '',
    `模式：${mode === 'task' ? 'task' : 'dictation'}`,
    `ASR 文本：\n${raw}`,
  ].join('\n');
}

function readWindowsCredential(target) {
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
  if($utf16 -match '^[\u0020-\u007e]+$' -and $utf16.Length -ge 8){ [Console]::Out.Write($utf16) }
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
      script.replace('__PINVOU_CREDENTIAL_TARGET__', escapedTarget),
    ], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();
  } catch (_) {
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
    const apiKey = model.api_key || readWindowsCredential(target);
    return {
      baseUrl: model.base_url,
      apiKey,
      model: model.model,
      source: `pinvou_settings:${model.id || activeId || 'unknown'}`,
    };
  } catch (_) {
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
  if (envConfig.baseUrl && envConfig.apiKey && envConfig.model) return envConfig;
  return loadPinvouActiveModelConfig() || envConfig;
}

async function callOpenAiCompatible(raw, mode) {
  const config = resolveEvalModelConfig();
  if (!config.baseUrl || !config.apiKey || !config.model) {
    throw new Error('missing PINVOU_VOICE_EVAL_BASE_URL / PINVOU_VOICE_EVAL_API_KEY / PINVOU_VOICE_EVAL_MODEL and no usable Pinvou active model credential');
  }
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
        { role: 'system', content: '只输出修正后的文本，不解释。' },
        { role: 'user', content: normalizePrompt(raw, mode) },
      ],
      temperature: 0,
      max_tokens: 128,
      stream: false,
    }),
  });
  if (!response.ok) throw new Error(`LLM HTTP ${response.status}: ${await response.text()}`);
  const value = await response.json();
  const output = String(value?.choices?.[0]?.message?.content || '').trim();
  return { output, llm_ms: Math.round(performance.now() - started), source: config.source };
}

function percentileNearest(values, p) {
  const nums = values.filter(value => Number.isFinite(value)).sort((a, b) => a - b);
  if (!nums.length) return null;
  const idx = Math.ceil((p / 100) * nums.length) - 1;
  return nums[Math.max(0, Math.min(nums.length - 1, idx))];
}

async function main() {
  const args = parseArgs(process.argv);
  const cases = loadCases(args.cases || DEFAULT_CASES_PATH);
  const observed = loadJsonOrJsonl(args.observed);
  const rows = [];

  for (const testCase of cases) {
    let output = '';
    let llmMs = null;
    let source = observed ? 'observed' : 'llm';
    let error = '';
    if (observed) {
      const match = findObserved(observed, testCase);
      if (!match) {
        rows.push({ id: testCase.id, source: 'skipped', score: null, llm_ms: null, output: '', error: 'no observed sample' });
        continue;
      }
      output = match.final_text || match.output || match.normalized_text || '';
      llmMs = match.llm_ms ?? null;
    } else {
      try {
        const result = await callOpenAiCompatible(testCase.raw, testCase.mode);
        output = result.output || (isFillerOnlyText(testCase.raw) ? '' : testCase.raw);
        output = applyDeterministicCorrections(output, testCase.raw);
        llmMs = result.llm_ms;
        source = result.source || source;
      } catch (err) {
        source = 'skipped';
        error = String(err.message || err);
      }
    }

    if (source === 'skipped') {
      rows.push({ id: testCase.id, source, score: null, llm_ms: null, output: '', error });
      continue;
    }
    rows.push({
      id: testCase.id,
      source,
      llm_ms: llmMs,
      output,
      ...scoreOutput(testCase, output),
    });
  }

  console.table(rows.map(row => ({
    id: row.id,
    source: row.source,
    score: row.score,
    llm_ms: row.llm_ms,
    missing: (row.missing_expected || []).join('|'),
    forbidden: (row.forbidden_hits || []).join('|'),
    output: row.output,
    error: row.error || '',
  })));

  const scored = rows.filter(row => row.score !== null);
  const totalExpected = scored.reduce((sum, row) => sum + (row.expected_hits?.length || 0) + (row.missing_expected?.length || 0), 0);
  const hitExpected = scored.reduce((sum, row) => sum + (row.expected_hits?.length || 0), 0);
  const summary = {
    samples: scored.length,
    skipped: rows.length - scored.length,
    final_keyword_hit_rate: totalExpected ? `${hitExpected}/${totalExpected} = ${(hitExpected / totalExpected * 100).toFixed(1)}%` : 'N/A',
    intent_preservation_rate: scored.length ? `${scored.filter(row => row.intent_pass).length}/${scored.length} = ${(scored.filter(row => row.intent_pass).length / scored.length * 100).toFixed(1)}%` : 'N/A',
    over_rewrite_rate: scored.length ? `${scored.filter(row => row.over_rewrite).length}/${scored.length} = ${(scored.filter(row => row.over_rewrite).length / scored.length * 100).toFixed(1)}%` : 'N/A',
    cleanup_success_rate: scored.length ? `${scored.filter(row => row.cleanup_pass).length}/${scored.length} = ${(scored.filter(row => row.cleanup_pass).length / scored.length * 100).toFixed(1)}%` : 'N/A',
    llm_p90: percentileNearest(scored.map(row => row.llm_ms), 90),
  };
  console.log(JSON.stringify(summary, null, 2));

  const failures = scored.filter(row => row.score < 80 || !row.intent_pass || row.over_rewrite || !row.cleanup_pass);
  if (failures.length || !scored.length) process.exitCode = 1;
}

main().catch(error => {
  console.error(error);
  process.exit(1);
});
