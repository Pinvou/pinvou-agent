import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, '..');
const read = (...parts) => fs.readFileSync(path.join(root, ...parts), 'utf8');

const chatSource = read('src', 'platform', 'tauri', 'bridge', 'chat.js');
const chatEventsSource = read('src', 'platform', 'tauri', 'bridge', 'chat-events.js');
const desktopBridgeSource = read('src', 'platform', 'tauri', 'bridge.js');
const webBridgeSource = read('src', 'platform', 'web', 'bridge.js');
const webTurnTerminalSource = read('src', 'platform', 'web', 'bridge', 'turn-terminal.js');
const chatViewSource = read('src', 'features', 'chat', 'ChatView.jsx');
const modelServiceErrorsSource = read('src', 'shared', 'model-service-errors.js');
const bridgeMessagesSource = read('src', 'shared', 'bridge-messages.js');
const { conversationItemsForMode } = await import(
  '../src/features/conversation/deepseek-conversation.js'
);

const sandbox = { window: {} };
vm.runInNewContext(chatSource, sandbox, { filename: 'chat.js' });
const installChat = sandbox.window.__PINVOU_TAURI_BRIDGE_FEATURES__.chat;

const messageSandbox = { window: {} };
vm.runInNewContext(modelServiceErrorsSource, messageSandbox, { filename: 'model-service-errors.js' });
vm.runInNewContext(bridgeMessagesSource, messageSandbox, { filename: 'bridge-messages.js' });
const modelErrors = messageSandbox.window.PinvouModelServiceErrors;
assert.equal(modelErrors.classify('SSE stream request failed: HTTP 402 insufficient balance').kind, 'billing');
assert.equal(modelErrors.classify('HTTP 429 quota exceeded').kind, 'quota');
assert.equal(modelErrors.classify('HTTP 429 insufficient_quota').kind, 'quota');
assert.equal(modelErrors.classify('insufficient_quota').kind, 'quota');
assert.equal(modelErrors.classify('quota exhausted').kind, 'quota');
assert.equal(modelErrors.classify('HTTP 429 too many requests').kind, 'rate_limit');
assert.equal(modelErrors.classify('HTTP 500 insufficient balance').kind, 'billing');
assert.equal(modelErrors.classify('ECONNREFUSED').kind, 'network');
assert.equal(modelErrors.classify('permission denied while reading local file').kind, 'unknown');
assert.equal(modelErrors.isModelServiceError('permission denied while reading local file'), false);
assert.equal(modelErrors.isModelServiceError('Error: claude.config.json: permission denied'), false);
assert.equal(modelErrors.isModelServiceError('read llm_cache.db failed'), false);
assert.equal(modelErrors.isModelServiceError('insufficient_quota'), true);
// 裸泛化词不再无条件接管:本地工具错误(git/ssh/npm/docker/redis/脚本退出码)
// 同样会说 timeout/connection refused/server error,只有底座模型调用前缀
// (SSE stream/Chat API)或 API 上下文才判定为模型服务错误。
assert.equal(modelErrors.isModelServiceError('timeout'), false);
assert.equal(modelErrors.isModelServiceError('ECONNREFUSED'), false);
assert.equal(modelErrors.isModelServiceError('curl: (28) Operation timed out after 30000ms'), false);
assert.equal(modelErrors.isModelServiceError('tool exec failed: git clone https://example.com/repo.git: curl 28 timeout'), false);
assert.equal(modelErrors.isModelServiceError('ssh: connect to host github.com port 22: Connection refused'), false);
assert.equal(modelErrors.isModelServiceError('npm ERR! network request failed ECONNREFUSED 127.0.0.1:4873'), false);
assert.equal(modelErrors.isModelServiceError('redis: Error 111 connecting to 127.0.0.1:6379. Connection refused.'), false);
assert.equal(modelErrors.isModelServiceError('exit status 500'), false);
assert.equal(modelErrors.isModelServiceError('mcp server httpbin returned HTTP 500'), false);
assert.equal(modelErrors.isModelServiceError('local vllm health probe failed: HTTP 500'), false);
assert.equal(modelErrors.isModelServiceError('HTTP 500: worker killed 内存耗尽 (OOM)'), false);
// 第三方支付/托管平台的计费词带 "API" 上下文时仍会命中(计费语义本身正确,
// 只是 provider 名可能不准);无 API 上下文的纯本地计费串不接管。
assert.equal(modelErrors.isModelServiceError('Stripe API error: HTTP 402 payment required'), true);
// "GitHub API:" 的裸 "API" 不是 api key/model service 级别的上下文信号,
// 第三方平台限流保持原始错误展示。
assert.equal(modelErrors.isModelServiceError('GitHub API: HTTP 403 rate limit exceeded'), false);
assert.equal(modelErrors.isModelServiceError('billing service rejected the request'), false);
// apiSignal 裸放行会劫持无错误语义的本地错误:含 "api key"/"model service"
// 字样但无歧义错误词、无 model-like 状态码的文本不得接管;强语义的
// "invalid api key"(STRONG 词表)与 "api key + 状态码" 组合仍必须接管。
assert.equal(modelErrors.isModelServiceError('failed to save api key to config: disk full'), false);
assert.equal(modelErrors.isModelServiceError('wrote model service name to settings.json'), false);
assert.equal(modelErrors.isModelServiceError('invalid api key'), true);
assert.equal(modelErrors.isModelServiceError('api key rejected: HTTP 401'), true);
// 底座固定前缀与 API 上下文仍必须命中。
assert.equal(modelErrors.isModelServiceError('SSE stream request failed: HTTP 402'), true);
assert.equal(modelErrors.isModelServiceError('SSE stream idle timeout after 30s — no data received'), true);
assert.equal(modelErrors.isModelServiceError('Stream read error: connection reset by peer'), true);
assert.equal(modelErrors.isModelServiceError('Failed to call DeepSeek Chat API: HTTP 401'), true);
assert.equal(modelErrors.isModelServiceError('invalid api key'), true);
assert.equal(modelErrors.isModelServiceError('quota exhausted'), true);
assert.equal(modelErrors.isModelServiceError('model service HTTP 503 Service Unavailable'), true);
// 裸 503 无模型上下文不接管(本地 MCP/vllm/脚本也可能 5xx);底座真实串
// 必带 SSE stream/Chat API 前缀,不受影响。
assert.equal(modelErrors.isModelServiceError('HTTP 503 Service Unavailable'), false);
assert.doesNotMatch(
  modelErrors.build('HTTP 402 payment required', {
    language: 'en',
    provider: { preset: 'openai_compatible' },
  }).title,
  /当前模型服务/,
);
// provider 标签优先取错误文本里的厂商信号,而非当前会话模型配置
// (历史回合重建时两者可能不同)。
assert.match(
  modelErrors.build('SSE stream request failed: connect to api.deepseek.com: HTTP 402', { language: 'zh-Hans' }).title,
  /DeepSeek/,
);
// provider 标签按界面语言:中文品牌名不得混进 en 文案;短键(zai/xai/glm/
// kimi)走词边界匹配,不得被无关子串(如 "xaio")误命中。
assert.match(
  modelErrors.build('SSE stream request failed: connect to dashscope.aliyuncs.com: HTTP 402', { language: 'en' }).title,
  /Qwen/,
);
assert.doesNotMatch(
  modelErrors.build('SSE stream request failed: connect to dashscope.aliyuncs.com: HTTP 402', { language: 'en' }).title,
  /通义千问/,
);
assert.notEqual(
  modelErrors.build('SSE stream request failed: connect to xaio.internal: HTTP 500', { language: 'en' }).providerLabel,
  'xAI',
  'short provider keys must use word-boundary matching',
);
// 模糊匹配链(vendor 字段):google_gemini / xai 系也要能推导出标签。
assert.equal(
  modelErrors.build('SSE stream request failed: HTTP 500', { language: 'en', provider: { vendor: 'google_gemini' } }).providerLabel,
  'Gemini',
);
assert.equal(
  modelErrors.build('SSE stream request failed: HTTP 500', { language: 'en', provider: { vendor: 'xai' } }).providerLabel,
  'xAI',
);
// 分类顺序:OOM(内存耗尽)不再落到 quota;403+rate limit 共存按频控分。
assert.equal(modelErrors.classify('HTTP 500: worker killed 内存耗尽 (OOM)').kind, 'server');
assert.equal(modelErrors.classify('HTTP 403 forbidden: rate limit exceeded').kind, 'rate_limit');
// 计费强词(余额不足/quota 等)是门控的独立通道,必须无条件接管——
// 防止只走底座前缀路径的测试让强词表静默失效。
assert.equal(modelErrors.isModelServiceError('insufficient balance'), true);
assert.equal(modelErrors.isModelServiceError('账户余额不足，请充值'), true);
assert.equal(modelErrors.isModelServiceError('quota has been exceeded'), true);
// 底座大响应 abort 的真实错误串(chat.rs:SSE buffer exceeded)必须被固定前缀接管。
assert.equal(modelErrors.isModelServiceError('SSE buffer exceeded 10485760 bytes — aborting stream'), true);
// 带版本段与 axios 措辞的状态码也能提出状态:429 → rate_limit。
assert.equal(modelErrors.classify('HTTP/1.1 429 Too Many Requests').httpStatus, 429);
assert.equal(modelErrors.classify('Request failed with status code 429').kind, 'rate_limit');
assert.equal(modelErrors.classify('HTTP/2 503').httpStatus, 503);
// HTTPS 形态(带 S)的状态码同样要提出。
assert.equal(modelErrors.classify('HTTPS 502 Bad Gateway').httpStatus, 502);
// permission 分类的直接断言:403/forbidden 无频控词时按权限分。
assert.equal(modelErrors.classify('SSE stream request failed: HTTP 403 forbidden').kind, 'permission');
// LlmError Display 引导词(传输层立即失败,DNS/连接拒绝/TLS,不带 SSE 前缀
// 直接上抛)必须接管;冒号/括号锚定形态与本地工具文案无碰撞——gh CLI 的
// "API rate limit exceeded for ..." 无冒号,不得命中。
assert.equal(modelErrors.isModelServiceError('Rate limit exceeded: Too many requests'), true);
assert.equal(modelErrors.isModelServiceError('Network error: error sending request for url (https://api.deepseek.com/chat/completions)'), true);
assert.equal(modelErrors.isModelServiceError('Request timed out after 30s'), true);
assert.equal(modelErrors.isModelServiceError('Authentication failed: invalid credentials'), true);
assert.equal(modelErrors.isModelServiceError('Server error (500): Internal Server Error'), true);
assert.equal(modelErrors.isModelServiceError('Context length exceeded: maximum context is 8192 tokens'), true);
assert.equal(modelErrors.isModelServiceError('API rate limit exceeded for 1.2.3.4'), false);
assert.equal(modelErrors.classify('Rate limit exceeded: Too many requests').kind, 'rate_limit');
assert.equal(modelErrors.classify('Context length exceeded: maximum context is 8192 tokens').kind, 'context');
assert.equal(modelErrors.classify('Authorization failed: model access denied').kind, 'auth');
// 门控厂商名单与标签名单对齐:GLM/MiniMax 等自家厂商此前漏标。
assert.equal(modelErrors.isModelServiceError('GLM-4 API timeout'), true);
assert.equal(modelErrors.isModelServiceError('MiniMax server error'), true);
// Gemini 资源耗尽按额度语义分类。
assert.equal(modelErrors.isModelServiceError('Gemini API Error: RESOURCE_EXHAUSTED'), true);
assert.equal(modelErrors.classify('Gemini API Error: RESOURCE_EXHAUSTED').kind, 'quota');
// POSIX 磁盘配额满(EDQUOT 的标准 strerror 即 "Disk quota exceeded")先于
// 计费强词排除,不得提示"请充值"。
assert.equal(modelErrors.isModelServiceError('cp: cannot create regular file: Disk quota exceeded'), false);
// 词边界匹配:"chat api" 不得命中 "chat apiary","api key" 不得命中
// "api-keys.yaml"(归一化后 "api keys")。
assert.equal(modelErrors.isModelServiceError('chat apiary server down'), false);
assert.equal(modelErrors.isModelServiceError('Error reading /etc/app/api-keys.yaml: connection refused'), false);
// 脱敏:除占位符存在外,原始凭证实文必须消失。
const basicRedacted = modelErrors.redactTechnicalDetail('Authorization: Basic dXNlcjpwYXNzd29yZA==');
assert.match(basicRedacted, /\[敏感信息已隐藏\]/);
assert.doesNotMatch(basicRedacted, /dXNlcjpwYXNzd29yZA==/, 'Basic credentials must be redacted');
const skRedacted = modelErrors.redactTechnicalDetail('request failed with key sk-proj-abc123defGHIxyz');
assert.doesNotMatch(skRedacted, /sk-proj-abc123defGHIxyz/, 'bare sk- keys must be redacted');
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('input token: 15000 exceeds maximum context length of 8192'),
  /\[敏感信息已隐藏\]/,
  'token usage counters must not be redacted',
);
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('monkey:bar unrelated'),
  /\[敏感信息已隐藏\]/,
  'words merely ending in "key" must not trigger redaction',
);
assert.match(
  modelErrors.redactTechnicalDetail('Authorization: Bearer sk-deepseek-secret-token-123 api_key=sk-abc12345&token=demo'),
  /\[敏感信息已隐藏\]/,
);
// 无 Authorization 头名的裸 Basic/Digest base64 与小写裸 bearer 同样必须脱敏。
const bareBasicRedacted = modelErrors.redactTechnicalDetail('proxy replied: Basic dXNlcjpwYXNzd29yZA==');
assert.doesNotMatch(bareBasicRedacted, /dXNlcjpwYXNzd29yZA==/, 'bare Basic credentials must be redacted');
const lowercaseBearerRedacted = modelErrors.redactTechnicalDetail('error with bearer eyJhbGciOiJIUzI1NiJ9.abc123def456');
assert.doesNotMatch(lowercaseBearerRedacted, /eyJhbGciOiJIUzI1NiJ9/, 'lowercase bare bearer tokens must be redacted');
// 非标 scheme 的两段式 Authorization(含 JSON 引号形态)必须整段吞值:
// 按 scheme 词表枚举时,API-Key/Token 等非标 scheme 的凭证段整体存活。
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('Authorization: API-Key abc123def456xyz'),
  /abc123def456/,
  'non-standard scheme credentials must be redacted',
);
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('{"headers":{"Authorization":"Token abc123def456xyz"}}'),
  /abc123def456/,
  'quoted non-standard scheme credentials must be redacted',
);
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('Proxy-Authorization: HMAC-SPA256 abcdef123456abcdef'),
  /abcdef123456/,
  'proxy-authorization non-standard scheme credentials must be redacted',
);
// 下划线复合凭证键(\b 在下划线旁不成立,必须整体枚举)必须脱敏。
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('refresh_token: rt_live_abc123def456ghi789'),
  /rt_live_abc123/,
  'compound credential keys must be redacted',
);
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('client_secret: GOCSPX-abcdef1234567890'),
  /GOCSPX/,
  'client_secret values must be redacted',
);
// 裸 JWT(无键名、无 Bearer 前缀的三段式)必须脱敏。
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('upstream replied eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c before dying'),
  /eyJhbGciOiJIUzI1NiJ9/,
  'bare JWTs must be redacted',
);
// 数字开头的会话凭证(Cookie sessionid)必须脱敏。
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('Set-Cookie: sessionid=8f3k9d2l1a4b7c6e5f9a; Path=/'),
  /8f3k9d2l/,
  'session cookie values must be redacted',
);
// 强凭证键的引号值允许空格,整段吞掉。
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('"password": "correct horse battery staple"'),
  /horse/,
  'space-separated passwords must be fully redacted',
);
// 未加引号的多词口令同样整段吞掉(吞到行尾或首个 ,/;&),不得只吞首词。
const unquotedPasswordRedacted = modelErrors.redactTechnicalDetail('password: correct horse battery staple');
assert.doesNotMatch(unquotedPasswordRedacted, /horse/, 'unquoted multi-word passwords must be fully redacted');
assert.match(unquotedPasswordRedacted, /\[敏感信息已隐藏\]/);
// 数字开头/带连字符的 UUID 形态会话凭证必须脱敏。
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('session_id: "8f3k9d2l-4abc-def0-1234-567890abcdef"'),
  /8f3k9d2l/,
  'digit-starting hyphenated session ids must be redacted',
);
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('api_key=1234-5678-abcd-ef01'),
  /1234-5678/,
  'digit-starting hyphenated api keys must be redacted',
);
// 裸 Gemini API Key(AIza 前缀)必须脱敏。
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('request failed, key=AIzaSyB3dEfGhIjKlMnOpQrStUvWxYz012345'),
  /AIzaSy/,
  'bare Gemini API keys must be redacted',
);
// 通用 Cookie/Set-Cookie 头必须脱敏。
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('Cookie: sessionid=abc123def456; theme=light'),
  /abc123def456/,
  'Cookie header values must be redacted',
);
// 脱敏占位符按界面语言选择:en/ja 界面的技术详情不得混入中文。
const enRedacted = modelErrors.redactTechnicalDetail('Authorization: Bearer sk-deepseek-secret-token-123', 'en');
assert.match(enRedacted, /\[redacted\]/);
assert.doesNotMatch(enRedacted, /敏感信息/);
assert.match(
  modelErrors.redactTechnicalDetail('Authorization: Bearer sk-deepseek-secret-token-123', 'ja'),
  /\[秘匿済み\]/,
);
// 非凭证值不被误吞:裸 "key" 的无数字短值(model-name)保留。
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('{"key": "model-name"}'),
  /\[敏感信息已隐藏\]/,
  'non-credential bare "key" values must not be redacted',
);
// structured payload(kind/title/message 对象直通):合法 kind 保留、非法 kind
// 兜底 unknown、字段再次脱敏。
const structuredNotice = modelErrors.build(
  { kind: 'billing', title: '余额不足', message: '请充值', technicalDetail: 'Bearer sk-zzz-abc123def456' },
  { language: 'zh-Hans' },
);
assert.equal(structuredNotice.kind, 'billing');
assert.doesNotMatch(structuredNotice.technicalDetail, /sk-zzz-abc123def456/, 'structured passthrough must redact technical detail');
assert.match(structuredNotice.technicalDetail, /\[敏感信息已隐藏\]/);
assert.equal(modelErrors.build({ kind: 'nope', title: 't', message: 'm' }, { language: 'zh-Hans' }).kind, 'unknown', 'unknown structured kinds must fall back to unknown');
const cleanupState = { settings: { language: 'ja' }, chatItems: [] };
const addCleanupItem = (text, metadata) => cleanupState.chatItems.push({ text, ...metadata });
messageSandbox.window.PinvouBridgeMessages.showShellCleanupFailure(
  { shell_cleanup_failed: true },
  cleanupState,
  addCleanupItem,
);
assert.equal(cleanupState.chatItems.length, 1);
assert.match(cleanupState.chatItems[0].text, /バックグラウンドタスク/);
messageSandbox.window.PinvouBridgeMessages.showShellCleanupFailure(
  { shell_cleanup_failed: true },
  cleanupState,
  addCleanupItem,
);
assert.equal(cleanupState.chatItems.length, 1, 'cleanup warning must be deduplicated');
assert.equal(cleanupState.chatItems[0].legacyConversationOnly, true);

const modelErrorState = {
  settings: { language: 'zh-Hans' },
  currentSessionModelId: 'deepseek-main',
  savedModels: [{ id: 'deepseek-main', preset: 'deepseek', model: 'deepseek-chat' }],
  chatItems: [],
};
const addModelErrorItem = (text, metadata) => modelErrorState.chatItems.push({ text, ...metadata });
const rawBillingError = 'SSE stream request failed: HTTP 402 {"error":{"message":"insufficient balance","api_key":"sk-secret"}}';
const billingAdded = messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
  { error: rawBillingError },
  modelErrorState,
  addModelErrorItem,
  true,
  // 模拟 recordTurnCompleted 已写入的带 error 时间线终态记录:只有它存在时
  // 终态气泡才隐藏(时间线错误卡接管)。
  { error: rawBillingError },
);
assert.equal(billingAdded, true);
assert.equal(modelErrorState.chatItems.length, 1);
assert.equal(modelErrorState.chatItems[0].userError.kind, 'billing');
assert.match(modelErrorState.chatItems[0].text, /DeepSeek账户余额不足/);
assert.doesNotMatch(modelErrorState.chatItems[0].text, /SSE stream request failed/);
assert.match(modelErrorState.chatItems[0].userError.technicalDetail, /\[敏感信息已隐藏\]/);
assert.equal(modelErrorState.chatItems[0].legacyConversationOnly, true);
messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
  { error: rawBillingError },
  modelErrorState,
  addModelErrorItem,
  true,
  { error: rawBillingError },
);
assert.equal(modelErrorState.chatItems.length, 1, 'model service notices must be deduplicated');
messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
  { error: 'HTTP 500 OpenAI internal abc' },
  modelErrorState,
  addModelErrorItem,
  true,
);
assert.equal(modelErrorState.chatItems.length, 2);
messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
  { error: 'HTTP 500 OpenAI internal xyz' },
  modelErrorState,
  addModelErrorItem,
  true,
);
assert.equal(
  modelErrorState.chatItems.length,
  3,
  'same friendly title with different technical details must not be deduplicated',
);
assert.equal(
  messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
    { error: 'permission denied while reading local file' },
    modelErrorState,
    addModelErrorItem,
    false,
  ),
  false,
  'non-model-service errors must fall back to the raw chat error notice',
);
assert.equal(modelErrorState.chatItems.length, 3, 'non-model-service errors must not add model service notices');
const transientState = { settings: { language: 'zh-Hans' }, chatItems: [] };
messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
  { error: 'SSE stream request failed: HTTP 503 Service Unavailable' },
  transientState,
  (text, metadata) => transientState.chatItems.push({ text, ...metadata }),
  false,
);
assert.doesNotMatch(transientState.chatItems[0].text, /已停止/);
// 同一回合 transient → done 连续序列:transient 先以 recoverable 措辞入列,
// done 到达时必须按错误身份(kind+technicalDetail)升级同一条目,而不是因
// terminal 措辞不同新增第二条(旧实现按文本全等去重,必然双气泡且措辞矛盾)。
const transientThenDoneState = { settings: { language: 'zh-Hans' }, chatItems: [] };
const pushToSeqState = (text, metadata) => transientThenDoneState.chatItems.push({ text, ...metadata });
messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
  { error: 'SSE stream request failed: HTTP 402 insufficient balance' },
  transientThenDoneState,
  pushToSeqState,
  false,
);
assert.equal(transientThenDoneState.chatItems.length, 1);
assert.match(transientThenDoneState.chatItems[0].text, /继续重试/, 'transient notice uses recoverable wording');
messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
  { error: 'SSE stream request failed: HTTP 402 insufficient balance' },
  transientThenDoneState,
  pushToSeqState,
  true,
  { error: 'SSE stream request failed: HTTP 402 insufficient balance' },
);
assert.equal(transientThenDoneState.chatItems.length, 1, 'terminal notice must upgrade the transient item, not add a second bubble');
const upgraded = transientThenDoneState.chatItems[0];
assert.match(upgraded.text, /本次回复已停止/, 'upgraded item switches to terminal wording');
assert.equal(upgraded.legacyConversationOnly, true, 'upgraded item is hidden from the unified timeline');
assert.equal(upgraded.userError.kind, 'billing');
// 身份不同的 transient/done 序列:transient(network)与 done(billing)文本、
// kind、技术详情均不同,身份去重必然落空;终态到达且时间线记录带 error 时,
// 同回合所有模型服务 transient 气泡必须一并隐藏,否则残留"系统会继续重试"
// 的瞬态气泡与终态"已停止"错误卡措辞矛盾。
const sweepState = { settings: { language: 'zh-Hans' }, chatItems: [] };
const pushToSweepState = (text, metadata) => sweepState.chatItems.push({ text, ...metadata });
messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
  { error: 'SSE stream idle timeout after 30s — no data received' },
  sweepState,
  pushToSweepState,
  false,
);
assert.equal(sweepState.chatItems.length, 1);
assert.equal(sweepState.chatItems[0].userError.kind, 'network');
assert.equal(sweepState.chatItems[0].legacyConversationOnly, false);
messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
  { error: 'SSE stream request failed: HTTP 402 insufficient balance' },
  sweepState,
  pushToSweepState,
  true,
  { error: 'SSE stream request failed: HTTP 402 insufficient balance' },
);
assert.equal(sweepState.chatItems.length, 2, 'different-identity terminal error adds its own notice');
assert.ok(
  sweepState.chatItems.every(item => item.legacyConversationOnly === true),
  'terminal takeover must hide all same-turn model-service transient bubbles',
);
assert.match(sweepState.chatItems[1].text, /本次回复已停止/);
// 静默吞错回归:终态到达但 recordTurnCompleted 未写入时间线记录
// (openStart/turnId 缺失,以 null 传入)时,气泡必须保留可见。
const noTimelineState = { settings: { language: 'zh-Hans' }, chatItems: [] };
messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
  { error: 'SSE stream request failed: HTTP 402 insufficient balance' },
  noTimelineState,
  (text, metadata) => noTimelineState.chatItems.push({ text, ...metadata }),
  true,
  null,
);
assert.equal(noTimelineState.chatItems.length, 1);
assert.equal(
  noTimelineState.chatItems[0].legacyConversationOnly,
  false,
  'terminal notice must stay visible when no timeline record was written',
);
// en/ja 的 transient/terminal 措辞区分:所有 retryable 类别(rate_limit/server/
// network/unknown)的 {stop} 占位符必须在三语模板齐全,瞬态与终态措辞不得相同。
for (const language of ['en', 'ja', 'zh-Hans']) {
  for (const kind of [
    'HTTP 429 too many requests',
    'SSE stream request failed: HTTP 500 internal error',
    'SSE stream request failed: connection reset by peer',
    'Chat API call failed with an unexpected error',
  ]) {
    const transientMessage = modelErrors.build(kind, { language, terminal: false }).message;
    const terminalMessage = modelErrors.build(kind, { language, terminal: true }).message;
    assert.notEqual(
      transientMessage,
      terminalMessage,
      `${language} transient and terminal wording must differ for: ${kind}`,
    );
  }
}
assert.doesNotMatch(modelErrors.build('HTTP 429 too many requests', { language: 'en', terminal: false }).message, /\{stop\}/);
assert.doesNotMatch(modelErrors.build('HTTP 429 too many requests', { language: 'ja', terminal: true }).message, /\{stop\}/);

const terminalSandbox = { window: {}, Date };
vm.runInNewContext(modelServiceErrorsSource, terminalSandbox, { filename: 'model-service-errors.js' });
vm.runInNewContext(bridgeMessagesSource, terminalSandbox, { filename: 'bridge-messages.js' });
vm.runInNewContext(webTurnTerminalSource, terminalSandbox, { filename: 'turn-terminal.js' });
const timelineState = {
  activeTurnTimelineId: 'turn-1',
  turnTimeline: [{ turn_id: 'turn-1', event: 'user_start', ui_turn_index: 2 }],
};
terminalSandbox.window.PinvouWebTurnTerminal.recordCompleted(
  timelineState,
  timelineState.turnTimeline[0],
  { status: 'Interrupted' },
);
assert.equal(timelineState.activeTurnTimelineId, null);
assert.equal(timelineState.turnTimeline[1].event, 'assistant_done');
assert.equal(timelineState.turnTimeline[1].status, 'Interrupted');
assert.equal(timelineState.turnTimeline[1].ui_turn_index, 2);
timelineState.activeTurnTimelineId = 'turn-2';
timelineState.turnTimeline.push({ turn_id: 'turn-2', event: 'user_start', ui_turn_index: 3 });
terminalSandbox.window.PinvouWebTurnTerminal.recordCompleted(
  timelineState,
  timelineState.turnTimeline[2],
  { status: 'Failed', error: rawBillingError },
);
assert.equal(timelineState.turnTimeline[3].user_error.kind, 'billing');
assert.match(timelineState.turnTimeline[3].user_error.message, /充值/);

const state = {
  activeSessionId: 'session-1',
  chatItems: [
    { id: 1, type: 'system', text: '⚠️ 上一轮模型不可用', turnErrorNotice: true },
    { id: 2, type: 'system', text: '保留的会话通知' },
  ],
  messages: [],
  busy: false,
};
const buffer = {
  localTurnOwned: false,
  remoteTurnActive: false,
  remoteTerminalSeen: false,
  deferredRemoteUserEvent: null,
};
let rejectChat = true;
const context = {
  state,
  // eslint-disable-next-line no-unused-vars -- stub keeps the full call signature
  invoke(command) {
    return rejectChat ? Promise.reject(new Error('当前模型不可用')) : Promise.resolve();
  },
  notify() {},
  TAURI: null,
  sessionStates: { 'session-1': buffer },
  turnUsageDirty: {},
  personaPlaceholderTitles: {},
  renderMarkdown(value) { return value; },
  safeConsoleInfo() {},
  bt(key) { return key; },
  runSyncOnSession(_sid, action) { action(); },
  startThinking() {},
  stopThinking() {},
  ensureSessionBufferLoaded() { return Promise.resolve(); },
  ensureSession() { return Promise.resolve('session-1'); },
  getBuffer() { return buffer; },
  reconcileRemoteTurn() { return Promise.resolve(true); },
  markRemoteTurn() {},
  clearAttachments() {},
  isScheduledRunSession() { return false; },
  basename(value) { return path.basename(String(value || '')); },
  extractArtifactPath() { return ''; },
  parseScheduledTaskDraftFromText() { return null; },
  autoCreateScheduledTaskDraft() {},
  pendingAssistantText: '',
  pendingAssistantBlocks: [],
  currentStreamText: '',
  currentStreamId: 0,
  itemIdSeq: 10,
};
const chat = installChat(context);

await chat.doSendFor('session-1', '第一次', '第一次', [], null, false, false);
assert.equal(
  state.chatItems.filter(item => item.turnErrorNotice).length,
  1,
  '发送失败时只保留当前一次临时错误',
);
assert.match(state.chatItems.find(item => item.turnErrorNotice).text, /当前模型不可用/);
assert.ok(state.chatItems.some(item => item.text === '保留的会话通知'));

rejectChat = false;
await chat.doSendFor('session-1', '重试', '重试', [], null, false, false);
assert.equal(
  state.chatItems.some(item => item.turnErrorNotice),
  false,
  '下一轮开始时必须清除上一轮临时错误',
);
assert.ok(state.chatItems.some(item => item.type === 'user' && item.text === '重试'));

const legacyFinalError = {
  id: 3,
  type: 'system',
  text: '⚠️ 最终模型错误',
  turnErrorNotice: true,
  legacyConversationOnly: true,
};
assert.deepEqual(
  conversationItemsForMode([legacyFinalError], false),
  [legacyFinalError],
  '旧版会话界面必须继续显示最终错误',
);
assert.deepEqual(
  conversationItemsForMode([legacyFinalError], true),
  [],
  '新版时间线已呈现最终错误，不应重复投影兼容气泡',
);

const doneSection = chatEventsSource.slice(
  chatEventsSource.indexOf('listen("chat:done"'),
  chatEventsSource.indexOf('listen("chat:usage"'),
);
assert.match(doneSection, /legacyConversationOnly: timelineTakesOver/);
assert.match(bridgeMessagesSource, /payload\.shell_cleanup_failed/);
assert.match(doneSection, /messages\.addModelServiceErrorNotice/);
assert.match(doneSection, /typeof messages\.addModelServiceErrorNotice === "function"/);
assert.match(doneSection, /shellMessages\.showShellCleanupFailure/);
assert.match(doneSection, /typeof shellMessages\.showShellCleanupFailure === "function"/);
assert.match(
  doneSection,
  /refreshAuthoritativeTurnTimeline\(sid\)/,
  '终态必须重新读取权威时间线，补齐后台或恢复会话漏掉的完成状态',
);
assert.match(
  chatEventsSource,
  /invoke\("get_session_timeline", \{ (?:sessionId: sessionId|sessionId) \}\)/,
  '时间线补偿必须读取当前完成会话，而不是依赖全局 active session',
);
assert.match(chatEventsSource, /turnErrorNotice && item\.text === notice/);
assert.match(chatEventsSource, /addSystemItem\(notice, \{ turnErrorNotice: true \}\)/);
assert.match(
  desktopBridgeSource,
  /if \(item\.turnErrorNotice && !item\.legacyConversationOnly\) return false/,
);
assert.match(chatViewSource, /conversationItemsForMode\(visibleChatItems, useUnifiedConversationUi\)/);

assert.match(webBridgeSource, /turnErrorNotice && item\.text === notice/);
assert.match(
  webBridgeSource,
  /if \(item\.turnErrorNotice && !item\.legacyConversationOnly\) return false/,
);
assert.match(
  webBridgeSource.slice(
    webBridgeSource.indexOf('listen("chat:done"'),
    webBridgeSource.indexOf('listen("chat:usage"'),
  ),
  /legacyConversationOnly: timelineTakesOver/,
);
assert.match(bridgeMessagesSource, /payload\.shell_cleanup_failed/);
assert.match(webBridgeSource, /function bridgeMessages\(\)/);
assert.match(webBridgeSource, /typeof messages\.addModelServiceErrorNotice === "function"/);
assert.match(webBridgeSource, /typeof shellMessages\.showShellCleanupFailure === "function"/);
assert.match(webBridgeSource, /typeof terminal\.recordCompleted === "function"/);
assert.equal(
  (bridgeMessagesSource.match(/^ {4}(zh|en|ja):/gm) || []).length,
  3,
  'Shell cleanup warning must provide zh/en/ja translations',
);

// Wave-2 拆分把事件转发(含 Event::TurnComplete 处理)从 engine.rs 移到
// forwarder.rs。契约检查事件处理顺序,故把 forwarder.rs 拼在前面(它含
// TurnComplete→timing→emit 的顺序);未拆分的 main 上没有 forwarder.rs,
// 自动回退为仅 engine.rs。
let engineSource = read('src-tauri', 'src', 'features', 'assistant', 'engine.rs');
try {
  engineSource =
    read('src-tauri', 'src', 'features', 'assistant', 'forwarder.rs') + engineSource;
} catch {
  // main(未拆分)无 forwarder.rs
}
const turnCompleteStart = engineSource.indexOf('Event::TurnComplete');
const turnCompleteSection = engineSource.slice(
  turnCompleteStart,
  engineSource.indexOf('Event::CompactionStarted', turnCompleteStart),
);
assert.ok(
  turnCompleteSection.indexOf('timing::finish_turn_with_usage')
    < turnCompleteSection.indexOf('emit_chat_terminal'),
  '正常完成必须先落权威时间线，再向前端发送 chat:done',
);
const reclaimedSection = engineSource.slice(
  engineSource.indexOf('async fn finish_reclaimed_lifecycle_turn'),
  engineSource.indexOf('impl AppEngine'),
);
assert.ok(
  reclaimedSection.indexOf('timing::finish_turn')
    < reclaimedSection.indexOf('emit_chat_terminal'),
  '回收/中断同样必须先落时间线，再向前端发送终态',
);

vm.runInNewContext(chatEventsSource, sandbox, { filename: 'chat-events.js' });
const installChatEvents = sandbox.window.__PINVOU_TAURI_BRIDGE_FEATURES__['chat-events'];
const authoritativeTimeline = [
  { turn_id: 'disk-old', event: 'user_start', timestamp: 1000 },
  { turn_id: 'disk-old', event: 'assistant_done', timestamp: 4000, status: 'Completed' },
  { turn_id: 'disk-current', event: 'user_start', timestamp: 10000 },
  { turn_id: 'disk-current', event: 'assistant_done', timestamp: 16000, status: 'Completed' },
];
const recoveredTimelineState = {
  turnTimeline: [
    { turn_id: 'ui-current', event: 'user_start', timestamp: 10020, ui_turn_index: 1 },
    { turn_id: 'ui-current', event: 'assistant_done', timestamp: 16020, status: 'Completed', ui_turn_index: 1 },
  ],
};
let timelineNotifyCount = 0;
const chatEvents = installChatEvents({
  state: recoveredTimelineState,
  listen() {},
  invoke(command, args) {
    assert.equal(command, 'get_session_timeline');
    assert.equal(args.sessionId, 'session-recovered');
    return Promise.resolve(authoritativeTimeline);
  },
  runSyncOnSession(sessionId, action) {
    assert.equal(sessionId, 'session-recovered');
    action();
  },
  notify() { timelineNotifyCount += 1; },
  safeConsoleInfo() {},
});
assert.equal(
  await chatEvents.refreshAuthoritativeTurnTimeline('session-recovered'),
  true,
  '后台/恢复会话完成后应接受权威时间线',
);
assert.deepEqual(
  recoveredTimelineState.turnTimeline,
  authoritativeTimeline,
  '权威时间线必须补回本地未见过的早期完成轮次',
);
assert.equal(timelineNotifyCount, 1);

assert.equal(
  chatEvents.authoritativeTimelineMissesKnownCompletion(
    [
      { turn_id: 'turn-current', event: 'user_start', timestamp: 10000 },
      { turn_id: 'turn-current', event: 'assistant_done', timestamp: 11000, status: 'Completed' },
    ],
    [{ turn_id: 'turn-current', event: 'user_start', timestamp: 10000 }],
  ),
  true,
  '短暂滞后的权威快照不得把已完成回合覆盖回执行中',
);

console.log('chat turn error isolation: ok');
