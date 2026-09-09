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
// Bare generic words no longer take over unconditionally: local tool
// errors (git/ssh/npm/docker/redis/script exit codes) say timeout/
// connection refused/server error just the same; only a base model-call
// prefix (SSE stream/Chat API) or an API context counts as a model
// service error.
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
// Billing words on third-party payment/hosting platforms still hit when
// an "API" context is present (the billing semantics are right even if
// the provider label may be off); bare local billing strings without an
// API context do not take over.
assert.equal(modelErrors.isModelServiceError('Stripe API error: HTTP 402 payment required'), true);
// The bare "API" in "GitHub API:" is not an api key/model service level
// context signal, so third-party platform rate limits keep the raw
// error display.
assert.equal(modelErrors.isModelServiceError('GitHub API: HTTP 403 rate limit exceeded'), false);
assert.equal(modelErrors.isModelServiceError('billing service rejected the request'), false);
// Letting apiSignal through alone would hijack local errors that carry
// no error semantics: text containing "api key"/"model service" but no
// ambiguous error word and no model-like status code must not take over;
// the strong "invalid api key" (STRONG list) and "api key + status code"
// combinations still must.
assert.equal(modelErrors.isModelServiceError('failed to save api key to config: disk full'), false);
assert.equal(modelErrors.isModelServiceError('wrote model service name to settings.json'), false);
assert.equal(modelErrors.isModelServiceError('invalid api key'), true);
assert.equal(modelErrors.isModelServiceError('api key rejected: HTTP 401'), true);
// Fixed base prefixes and API contexts must still hit.
assert.equal(modelErrors.isModelServiceError('SSE stream request failed: HTTP 402'), true);
assert.equal(modelErrors.isModelServiceError('SSE stream idle timeout after 30s — no data received'), true);
assert.equal(modelErrors.isModelServiceError('Stream read error: connection reset by peer'), true);
assert.equal(modelErrors.isModelServiceError('Failed to call DeepSeek Chat API: HTTP 401'), true);
assert.equal(modelErrors.isModelServiceError('invalid api key'), true);
assert.equal(modelErrors.isModelServiceError('quota exhausted'), true);
assert.equal(modelErrors.isModelServiceError('model service HTTP 503 Service Unavailable'), true);
// A bare 503 without model context does not take over (local MCP/vLLM/
// scripts can emit 5xx too); real base errors always carry an SSE
// stream/Chat API prefix and are unaffected.
assert.equal(modelErrors.isModelServiceError('HTTP 503 Service Unavailable'), false);
assert.doesNotMatch(
  modelErrors.build('HTTP 402 payment required', {
    language: 'en',
    provider: { preset: 'openai_compatible' },
  }).title,
  /当前模型服务/,
);
// The provider label prefers the vendor signal in the error text over
// the current session model config (the two can differ when historical
// turns are rebuilt).
assert.match(
  modelErrors.build('SSE stream request failed: connect to api.deepseek.com: HTTP 402', { language: 'zh-Hans' }).title,
  /DeepSeek/,
);
// The provider label follows the UI language: Chinese brand names must
// not leak into en copy; short keys (zai/xai/glm/kimi) match on word
// boundaries and must not hit unrelated substrings (e.g. "xaio").
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
// Fuzzy matching chain (vendor field): google_gemini / xai families must
// also resolve to a label.
assert.equal(
  modelErrors.build('SSE stream request failed: HTTP 500', { language: 'en', provider: { vendor: 'google_gemini' } }).providerLabel,
  'Gemini',
);
assert.equal(
  modelErrors.build('SSE stream request failed: HTTP 500', { language: 'en', provider: { vendor: 'xai' } }).providerLabel,
  'xAI',
);
// Classification order: OOM no longer falls into quota; a 403 plus rate
// limit co-occurring classifies as rate limiting.
assert.equal(modelErrors.classify('HTTP 500: worker killed 内存耗尽 (OOM)').kind, 'server');
assert.equal(modelErrors.classify('HTTP 403 forbidden: rate limit exceeded').kind, 'rate_limit');
// English OOM shares its shape with gRPC RESOURCE_EXHAUSTED: local
// VRAM/memory exhaustion must not be answered with "check your API quota";
// genuine quota errors (no memory wording) are unaffected.
assert.equal(modelErrors.isModelServiceError('Resource exhausted: OOM when allocating tensor with shape[8192,8192]'), false);
assert.equal(modelErrors.isModelServiceError('CUDA out of memory. Tried to allocate 2.00 GiB'), false);
assert.notEqual(modelErrors.classify('Resource exhausted: OOM when allocating tensor').kind, 'quota');
assert.equal(modelErrors.isModelServiceError('google.rpc RESOURCE_EXHAUSTED: quota exceeded'), true);
assert.equal(modelErrors.classify('RESOURCE_EXHAUSTED: Insufficient Quota').kind, 'quota');
// Billing strong words (quota etc.) are a standalone gate channel and must
// take over unconditionally, so prefix-only tests cannot silently break the
// list. "insufficient balance" now shares the tier of its Chinese
// equivalents: the bare words also appear in local wallet/payment errors and
// require API/provider context; DeepSeek 402 still gates via prefixes and
// status codes.
assert.equal(modelErrors.isModelServiceError('quota has been exceeded'), true);
assert.equal(modelErrors.isModelServiceError('insufficient balance'), false);
assert.equal(modelErrors.isModelServiceError('Payment failed: insufficient balance'), false);
assert.equal(modelErrors.isModelServiceError('DeepSeek API error: insufficient balance'), true);
assert.equal(modelErrors.classify('SSE stream request failed: HTTP 402 insufficient balance').kind, 'billing');
// Generic Chinese payment phrases (账户余额/余额不足/欠费) are equally
// common in local payment/transfer errors: the bare wording no longer
// hijacks, and with an API/provider context they remain a strong billing
// signal.
assert.equal(modelErrors.isModelServiceError('支付失败:账户余额不足'), false);
assert.equal(modelErrors.isModelServiceError('转账出错:余额不足'), false);
assert.equal(modelErrors.isModelServiceError('GLM 400 1113 "余额不足"'), true);
// Local CLI tool shapes are excluded before the keyword lists: a vendor
// name inside a remote URL/hostname must not hijack.
assert.equal(modelErrors.isModelServiceError("fatal: unable to access 'https://github.com/openai/whisper.git/': Failed to connect to github.com port 443: Connection refused"), false);
assert.equal(modelErrors.isModelServiceError('ssh: connect to host git.openai.com port 22: Connection refused'), false);
assert.equal(modelErrors.isModelServiceError('error: failed to push some refs to https://github.com/deepseek-ai/models.git: read timed out'), false);
assert.equal(modelErrors.isModelServiceError('collect2: fatal error: ld terminated with signal 11'), false);
assert.equal(modelErrors.isModelServiceError('user quota exceeded on /home volume'), false);
// Package manager CLI output headers (npm ERR! / ERR_PNPM_*): the
// package name in the registry URL (.../openai) masquerades as a provider
// signal, which combined with the ambiguous "connection refused" once
// hijacked a local registry failure into a model service network card.
assert.equal(modelErrors.isModelServiceError('npm ERR! network request to https://registry.npmjs.org/openai failed, reason: connect ECONNREFUSED 127.0.0.1:4873'), false);
assert.equal(modelErrors.isModelServiceError('ERR_PNPM_NO_NETWORK  request to https://registry.npmjs.org/openai failed'), false);
// Missed-takeover fixes: provider names / bare 401 + unauthorized,
// "Incorrect API key" (OpenAI's real wording), gemini-cli's
// "[API Error: ...]" shell, and the claude vendor name.
assert.equal(modelErrors.isModelServiceError('OpenAI API error: 401 Unauthorized'), true);
assert.equal(modelErrors.isModelServiceError('deepseek: 401 unauthorized'), true);
// Fake keys are built by concatenation: a full literal would trigger
// GitHub push protection's secret-pattern scan (all corpus values are
// synthetic, not real credentials).
const FAKE_PROJ_KEY = 'sk-proj-' + 'abcdefghijklmnop1234567890';
assert.equal(modelErrors.isModelServiceError('Incorrect API key provided: ' + FAKE_PROJ_KEY), true);
assert.equal(modelErrors.isModelServiceError('[API Error: 429 Too Many Requests]'), true);
assert.equal(modelErrors.isModelServiceError('claude API timeout after 30s'), true);
assert.equal(modelErrors.classify('OpenAI API error: 401 Unauthorized').kind, 'auth');
// The real error string of the base aborting an oversized response
// (chat.rs: SSE buffer exceeded) must be taken over by the fixed prefix.
assert.equal(modelErrors.isModelServiceError('SSE buffer exceeded 10485760 bytes — aborting stream'), true);
// Status codes with a version segment and axios wording must also parse:
// 429 -> rate_limit.
assert.equal(modelErrors.classify('HTTP/1.1 429 Too Many Requests').httpStatus, 429);
assert.equal(modelErrors.classify('Request failed with status code 429').kind, 'rate_limit');
assert.equal(modelErrors.classify('HTTP/2 503').httpStatus, 503);
// The HTTPS shape (with the S) must parse too.
assert.equal(modelErrors.classify('HTTPS 502 Bad Gateway').httpStatus, 502);
// Direct assertion for the permission kind: 403/forbidden without
// rate-limit words classifies as permission.
assert.equal(modelErrors.classify('SSE stream request failed: HTTP 403 forbidden').kind, 'permission');
// LlmError Display lead-ins (transport-level immediate failures - DNS/
// connection refused/TLS - propagated without an SSE prefix) must take
// over; the colon/parenthesis anchoring cannot collide with local tool
// wording - the gh CLI's "API rate limit exceeded for ..." has no colon
// and must not hit.
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
// The gate provider list aligns with the label list: home vendors like
// GLM/MiniMax used to be missed.
assert.equal(modelErrors.isModelServiceError('GLM-4 API timeout'), true);
assert.equal(modelErrors.isModelServiceError('MiniMax server error'), true);
// Gemini resource exhaustion classifies with quota semantics.
assert.equal(modelErrors.isModelServiceError('Gemini API Error: RESOURCE_EXHAUSTED'), true);
assert.equal(modelErrors.classify('Gemini API Error: RESOURCE_EXHAUSTED').kind, 'quota');
// POSIX disk quota exhaustion (EDQUOT's standard strerror is "Disk
// quota exceeded") is excluded before the billing strong words and must
// not produce a "top up" prompt.
assert.equal(modelErrors.isModelServiceError('cp: cannot create regular file: Disk quota exceeded'), false);
// Word boundaries: "chat api" must not hit "chat apiary", "api key"
// must not hit "api-keys.yaml" ("api keys" after normalization).
assert.equal(modelErrors.isModelServiceError('chat apiary server down'), false);
assert.equal(modelErrors.isModelServiceError('Error reading /etc/app/api-keys.yaml: connection refused'), false);
// Redaction: besides the placeholder being present, the raw credential
// text must vanish.
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
// Bare Basic/Digest base64 without an Authorization header name and a
// lowercase bare bearer must also be redacted.
const bareBasicRedacted = modelErrors.redactTechnicalDetail('proxy replied: Basic dXNlcjpwYXNzd29yZA==');
assert.doesNotMatch(bareBasicRedacted, /dXNlcjpwYXNzd29yZA==/, 'bare Basic credentials must be redacted');
const lowercaseBearerRedacted = modelErrors.redactTechnicalDetail('error with bearer eyJhbGciOiJIUzI1NiJ9.abc123def456');
assert.doesNotMatch(lowercaseBearerRedacted, /eyJhbGciOiJIUzI1NiJ9/, 'lowercase bare bearer tokens must be redacted');
// Two-token Authorization values with non-standard schemes (including
// quoted JSON forms) must be swallowed whole: enumerating a scheme
// whitelist used to leave the credential part of API-Key/Token-style
// schemes fully intact.
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
// Underscore compound credential keys (\b does not hold next to an
// underscore, hence the exhaustive enumeration) must be redacted.
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('refresh_token: rt_live_abc123def456ghi789'),
  /rt_live_abc123/,
  'compound credential keys must be redacted',
);
// Cloud key ids and vendor token compound names: access_key (AWS AKIA)
// is the most common.
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('access_key: AKIAIOSFODNN7EXAMPLE'),
  /AKIAIOSFODNN7EXAMPLE/,
  'access_key compound key must be redacted',
);
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('api_token: pat_abcdefgh1234'),
  /pat_abcdefgh1234/,
  'api_token compound key must be redacted',
);
// The 12-char bare Basic form (dXNlcjpwYXNz = user:pass) must be covered;
// usage counters stay untouched.
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('proxy replied: Basic dXNlcjpwYXNz'),
  /dXNlcjpwYXNz/,
  'short bare Basic credentials must be redacted',
);
assert.equal(modelErrors.redactTechnicalDetail('token: 15000'), 'token: 15000');
// Digest parameter blobs (comma params beyond the quote-truncated generic
// value) must be swallowed whole.
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('Authorization: Digest username="x", realm=y'),
  /realm=y/,
  'Digest parameter blob must be redacted',
);
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('client_secret: GOCSPX-abcdef1234567890'),
  /GOCSPX/,
  'client_secret values must be redacted',
);
// Bare JWTs (three segments without a key name or Bearer prefix) must be
// redacted.
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('upstream replied eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c before dying'),
  /eyJhbGciOiJIUzI1NiJ9/,
  'bare JWTs must be redacted',
);
// Digit-leading session credentials (Cookie sessionid) must be
// redacted.
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('Set-Cookie: sessionid=8f3k9d2l1a4b7c6e5f9a; Path=/'),
  /8f3k9d2l/,
  'session cookie values must be redacted',
);
// Strong credential keys: quoted values may contain spaces and are
// swallowed whole.
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('"password": "correct horse battery staple"'),
  /horse/,
  'space-separated passwords must be fully redacted',
);
// Unquoted multi-word passphrases are swallowed whole too (to end of
// line or the first ,/;/&), never just the first word.
const unquotedPasswordRedacted = modelErrors.redactTechnicalDetail('password: correct horse battery staple');
assert.doesNotMatch(unquotedPasswordRedacted, /horse/, 'unquoted multi-word passwords must be fully redacted');
assert.match(unquotedPasswordRedacted, /\[敏感信息已隐藏\]/);
// Digit-leading / hyphenated UUID-shaped session credentials must be
// redacted.
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
// Bare Gemini API keys (AIza prefix) must be redacted.
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('request failed, key=AIzaSyB3dEfGhIjKlMnOpQrStUvWxYz012345'),
  /AIzaSy/,
  'bare Gemini API keys must be redacted',
);
// Generic Cookie/Set-Cookie headers must be redacted: multiple
// credential pairs are "; "-separated and the second pair onwards
// (csrftoken etc.) must not leak - the earlier corpus used theme=light
// for the second pair, masking the defect.
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('Cookie: sessionid=abc123def456; csrftoken=zyx987wv654'),
  /abc123def456|zyx987wv654/,
  'multi-pair Cookie header values must all be redacted',
);
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('Set-Cookie: SID=AbCdEf123456; HSID=Xy987612345; SSID=Qw5432167890'),
  /AbCdEf123456|Xy987612345|Qw5432167890/,
  'multi-pair Set-Cookie header values must all be redacted',
);
// The redaction placeholder follows the UI language: technical details
// in English interfaces must not mix in Chinese.
const enRedacted = modelErrors.redactTechnicalDetail('Authorization: Bearer sk-deepseek-secret-token-123', 'en');
assert.match(enRedacted, /\[redacted\]/);
assert.doesNotMatch(enRedacted, /敏感信息/);
// Non-credential values are not swallowed: a digit-less short value for
// a bare "key" (model-name) is preserved.
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('{"key": "model-name"}'),
  /\[敏感信息已隐藏\]/,
  'non-credential bare "key" values must not be redacted',
);
// Structured payload (kind/title/message passthrough): a valid kind is
// kept, an invalid kind falls back to unknown, and fields are
// re-redacted.
const structuredNotice = modelErrors.build(
  { kind: 'billing', title: '余额不足', message: '请充值', technicalDetail: 'Bearer sk-zzz-abc123def456' },
  { language: 'zh-Hans' },
);
assert.equal(structuredNotice.kind, 'billing');
assert.doesNotMatch(structuredNotice.technicalDetail, /sk-zzz-abc123def456/, 'structured passthrough must redact technical detail');
// The structured passthrough's title/message must be redacted as well
// (AIza is caught by the bare-key rule).
const structuredKeyed = modelErrors.build(
  { kind: 'auth', title: 'invalid key AIzaSyB3dEfGhIjKlMnOpQrStUvWxYz012345', message: 'check key Bearer sk-zzz-abc123def456' },
  { language: 'zh-Hans' },
);
assert.doesNotMatch(structuredKeyed.title, /AIzaSy/, 'structured passthrough must redact title');
assert.doesNotMatch(structuredKeyed.message, /sk-zzz-abc123def456/, 'structured passthrough must redact message');
assert.match(structuredNotice.technicalDetail, /\[敏感信息已隐藏\]/);
// Escaped-JSON shapes (a gateway body embedded in an outer envelope):
// the kv rule must cross the \" on both sides of the key name to swallow
// the value, or the credential value leaks in full.
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('{\\"password\\": \\"sup3rSecretValue\\"}'),
  /sup3rSecretValue/,
  'backslash-escaped JSON password values must be redacted',
);
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('{\\"api_key\\": \\"4f8a2b6c9d1e3f5a\\"}'),
  /4f8a2b6c9d1e3f5a/,
  'backslash-escaped JSON api_key values must be redacted',
);
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('{\\"authorization\\": \\"Basic dXNlcjpwYXNz\\"}'),
  /dXNlcjpwYXNz/,
  'backslash-escaped JSON authorization headers must be redacted',
);
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('{\\"cookie\\": \\"SID=aaa.bbb; HSID=ccc.ddd\\"}'),
  /HSID=ccc/,
  'backslash-escaped JSON cookie headers must be redacted',
);
// Env-var shapes: the key name is an underscore-prefixed compound (\b
// does not hold next to an underscore - these used to leak whole), and
// values may contain / (Google OAuth refresh tokens "1//0...") and +
// (base64).
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('OPENAI_API_KEY=1a2b3c4d5e6f7g8h9i0j'),
  /1a2b3c4d5e6f/,
  'underscore-prefixed env-style api keys must be redacted',
);
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('ZHIPUAI_API_KEY=abc123def456ghijkl'),
  /abc123def456/,
  'provider-prefixed env-style keys must be redacted',
);
assert.doesNotMatch(
  modelErrors.redactTechnicalDetail('refresh_token=1//0abcDEFghiJKL'),
  /1\/\/0abcDEF/,
  'slash-bearing OAuth refresh tokens must be redacted',
);
// Idempotency: running the same text through the forwarder's and the
// JS-side redaction, or re-redacting an already-built card via build(),
// must be stable - the placeholder must never be re-consumed into
// "[R][R]" (that drifts the dedup key and yields two contradictory cards
// for one error).
const onceRedacted = modelErrors.redactTechnicalDetail(
  'Authorization: Basic dXNlcjpwYXNz; password: "two words here"; OPENAI_API_KEY=1a2b3c4d5e6f7g8h9i0j; sk-abcdef1234567890; Cookie: SID=aaa',
  'en',
);
assert.equal(
  modelErrors.redactTechnicalDetail(onceRedacted, 'en'),
  onceRedacted,
  'redaction must be idempotent: already-redacted text passes through unchanged',
);
const pipelineErrorForBuild = 'Invalid API-key provided: Authorization: Basic dXNlcjpwYXNz, session_id: 8f3k9d2l-4abc-1234';
const builtCard = modelErrors.build('SSE stream request failed: HTTP 402 ' + pipelineErrorForBuild, { language: 'en' });
const rebuiltCard = modelErrors.build(builtCard, { language: 'en' });
assert.equal(rebuiltCard.technicalDetail, builtCard.technicalDetail, 're-building a built card must keep technicalDetail stable');
assert.equal(rebuiltCard.title, builtCard.title, 're-building a built card must keep the title stable');
assert.doesNotMatch(rebuiltCard.technicalDetail, /\[redacted\]\[redacted\]/, 'placeholder must never be re-masked into a doubled placeholder');
assert.equal(modelErrors.build({ kind: 'nope', title: 't', message: 'm' }, { language: 'zh-Hans' }).kind, 'unknown', 'unknown structured kinds must fall back to unknown');
const cleanupState = { settings: { language: 'en' }, chatItems: [] };
const addCleanupItem = (text, metadata) => cleanupState.chatItems.push({ text, ...metadata });
messageSandbox.window.PinvouBridgeMessages.showShellCleanupFailure(
  { shell_cleanup_failed: true },
  cleanupState,
  addCleanupItem,
);
assert.equal(cleanupState.chatItems.length, 1);
assert.match(cleanupState.chatItems[0].text, /background tasks/);
messageSandbox.window.PinvouBridgeMessages.showShellCleanupFailure(
  { shell_cleanup_failed: true },
  cleanupState,
  addCleanupItem,
);
assert.equal(cleanupState.chatItems.length, 1, 'cleanup warning must be deduplicated');
// The redactRawError facade: gate-missed error texts are redacted
// unconditionally in front of display; credential values are swallowed
// while error semantics ("unauthorized"/status codes) survive for logic
// like the 401 model-config refresh.
const zhState = { settings: { language: 'zh-Hans' } };
const rawLeak = 'Incorrect API key provided: ' + FAKE_PROJ_KEY;
const redacted = messageSandbox.window.PinvouBridgeMessages.redactRawError(rawLeak, zhState);
assert.doesNotMatch(redacted, new RegExp(FAKE_PROJ_KEY), 'gate-missed raw fallback must be redacted');
assert.match(redacted, /\[敏感信息已隐藏\]/);
assert.doesNotMatch(
  messageSandbox.window.PinvouBridgeMessages.redactRawError('Incorrect API key provided', zhState),
  /\[敏感信息已隐藏\]/,
  'plain text without credentials must pass through redaction unchanged',
);
// Degrades to returning the input unchanged when the helper is missing
// (no throw).
const noHelperSandbox = { window: {} };
vm.runInNewContext(bridgeMessagesSource, noHelperSandbox, { filename: 'bridge-messages.js' });
assert.equal(
  noHelperSandbox.window.PinvouBridgeMessages.redactRawError(rawLeak, zhState),
  rawLeak,
  'redactRawError must degrade to identity when the helper is missing',
);
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
  // Simulates an error-bearing timeline terminal record already written
  // by recordTurnCompleted: only its presence lets the terminal bubble be
  // hidden (the timeline error card takes over).
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
// A transient -> done sequence within one turn: the transient notice is
// listed first with recoverable wording; when done arrives it must
// upgrade the same item by error identity (kind + technicalDetail)
// instead of appending a second one over differing terminal wording (the
// old exact-text dedup always produced two contradictory bubbles).
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
// Double-build regression for done: recordTurnCompleted first writes the
// build #1 card back into the payload's user_error field, then
// addModelServiceErrorNotice builds again from the raw error (the raw
// branch of bridge-messages.js modelServiceUserError). With non-idempotent
// redaction the Authorization placeholder was re-consumed and the
// technicalDetail drifted, missing the identity dedup - one error showing
// both a transient and a terminal card with contradictory wording. After
// the fix the second build must be field-stable, hit the dedup, and
// upgrade the original item.
const doubleBuildError = 'SSE stream request failed: HTTP 402 {"error":{"message":"Insufficient Balance"},"headers":{"Authorization":"Basic dXNlcjpwYXNz"}}';
const doubleBuildState = { settings: { language: 'zh-Hans' }, chatItems: [] };
const pushToDoubleBuild = (text, metadata) => doubleBuildState.chatItems.push({ text, ...metadata });
messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
  { error: doubleBuildError },
  doubleBuildState,
  pushToDoubleBuild,
  false,
);
assert.equal(doubleBuildState.chatItems.length, 1);
const donePayload = { error: doubleBuildError };
donePayload.user_error = messageSandbox.window.PinvouBridgeMessages.modelServiceUserError(donePayload, doubleBuildState);
assert.ok(donePayload.user_error, 'recordTurnCompleted-style build must produce a card');
messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
  donePayload,
  doubleBuildState,
  pushToDoubleBuild,
  true,
  { error: doubleBuildError },
);
assert.equal(
  doubleBuildState.chatItems.length,
  1,
  're-built card must dedupe against the transient card (stable technicalDetail)',
);
assert.match(doubleBuildState.chatItems[0].text, /本次回复已停止/);
assert.doesNotMatch(
  JSON.stringify(doubleBuildState.chatItems[0].userError),
  /\[敏感信息已隐藏\]\[敏感信息已隐藏\]/,
  'placeholder must never be doubled by re-redaction',
);
// A transient/done sequence with different identities: transient
// (network) vs done (billing) differ in text, kind and technical detail,
// so identity dedup necessarily misses; when the terminal arrives with an
// error-bearing timeline record, every model-service transient bubble of
// the turn must hide together, or the leftover "will keep retrying"
// bubble contradicts the terminal "has stopped" card.
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
// Silent-swallow regression: when the terminal arrives but
// recordTurnCompleted wrote no timeline record (missing openStart/turnId,
// passed as null), the bubble must stay visible.
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
// The transient/terminal wording split: the {stop} placeholder must be
// present in both language templates for every retryable kind
// (rate_limit/server/network/unknown), and transient and terminal
// wordings must never be identical.
for (const language of ['en', 'zh-Hans']) {
  for (const kind of [
    'HTTP 429 too many requests',
    'SSE stream request failed: HTTP 500 internal error',
    'SSE stream request failed: connection reset by peer',
    'Chat API call failed with an unexpected error',
    'HTTP 402 insufficient balance',
    'HTTP 429 quota exceeded',
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
assert.doesNotMatch(modelErrors.build('HTTP 429 too many requests', { language: 'zh-Hans', terminal: true }).message, /\{stop\}/);

// Legacy-list fix, billing vocabulary gap (Anthropic's official 402
// wording / Tongyi's Arrearage error code, R3 M5 residual): bare strings
// without a status code (ACP lane / subagent panel) must also take over
// and classify as billing.
assert.equal(modelErrors.isModelServiceError('Your credit balance is too low'), true);
assert.equal(modelErrors.classify('Your credit balance is too low').kind, 'billing');
assert.equal(modelErrors.isModelServiceError('Arrearage: account suspended, please recharge'), true);
assert.equal(modelErrors.classify('Arrearage: account suspended, please recharge').kind, 'billing');
assert.equal(
  modelErrors.classify('HTTP 402 {"type":"billing_error","message":"Your credit balance is too low"}').kind,
  'billing',
);

// Legacy-list fix, content-policy rejections (R6 residual:
// deterministic failures still told users to retry later):
// model-service-specific semantics take over unconditionally; the new
// content kind is a deterministic failure (retryable=false), has no
// {stop} placeholder in any language, and transient matches terminal
// wording so users are never told to retry later.
assert.equal(modelErrors.isModelServiceError('content policy violation: request blocked'), true);
assert.equal(modelErrors.isModelServiceError('Your request was rejected as a result of our safety system'), true);
assert.equal(modelErrors.isModelServiceError('output blocked by content filtering policy'), true);
const contentEn = modelErrors.build('content policy violation: request blocked', { language: 'en', terminal: true });
assert.equal(contentEn.kind, 'content');
assert.equal(contentEn.retryable, false);
assert.doesNotMatch(contentEn.message, /Try again later/);
assert.doesNotMatch(contentEn.message, /\{stop\}/);
const contentZh = modelErrors.build('content policy violation: request blocked', { language: 'zh-Hans', terminal: true });
assert.equal(contentZh.kind, 'content');
assert.doesNotMatch(contentZh.message, /稍后重试/);
assert.equal(
  modelErrors.build('content policy violation: request blocked', { language: 'en', terminal: false }).message,
  modelErrors.build('content policy violation: request blocked', { language: 'en', terminal: true }).message,
  'content-policy failures are deterministic: transient and terminal wording match',
);

// Legacy-list fix, doubled word with the zh default provider plus the
// server kind (R3 MINOR 4):
assert.equal(
  modelErrors.build('SSE stream request failed: HTTP 503 Service Unavailable', { language: 'zh-Hans', terminal: true }).title,
  '当前模型服务暂时不可用',
);
assert.equal(
  modelErrors.build('SSE stream request failed: HTTP 503 Service Unavailable', { language: 'zh-Hans', terminal: true, providerLabel: 'DeepSeek' }).title,
  'DeepSeek服务暂时不可用',
  'brand labels keep the 服务 suffix',
);

// Legacy-list fix, a successful done settling transient claims (R7
// follow-up): once the turn has recovered, the "will keep retrying"
// promise is stale and hidden uniformly; bare-string fallbacks are
// statements about errors that did happen and stay visible; already
// hidden items are not resurrected.
const settleState = { settings: { language: 'zh-Hans' }, chatItems: [] };
messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
  { error: 'SSE stream idle timeout after 30s — no data received' },
  settleState,
  (text, metadata) => settleState.chatItems.push({ text, ...metadata }),
  false,
);
settleState.chatItems.push({ text: '⚠️ shell command failed: permission denied', turnErrorNotice: true });
settleState.chatItems.push({ text: '⚠️ 旧终态', turnErrorNotice: true, legacyConversationOnly: true, userError: { kind: 'billing' } });
assert.equal(messageSandbox.window.PinvouBridgeMessages.settleModelServiceErrorNotices(settleState), true);
assert.equal(settleState.chatItems[0].legacyConversationOnly, true, 'successful done must settle the same-turn transient claim');
assert.equal(settleState.chatItems[1].legacyConversationOnly, undefined, 'bare fallback notices are factual statements and stay visible');
assert.equal(settleState.chatItems[2].legacyConversationOnly, true, 'already-hidden items are not resurrected');
assert.equal(
  messageSandbox.window.PinvouBridgeMessages.settleModelServiceErrorNotices({ settings: {}, chatItems: [] }),
  false,
  'nothing to settle returns false',
);

// Legacy-list fix, forwarder passthrough of structured code/category
// (R3 M3 / R4 P1): the controlled semantics are kept on the user card
// for diagnostics and later consumption; they must not drive gating or
// classification (the streaming path emits mostly the generic
// "transient", so string gating remains the source of truth).
const codedState = { settings: { language: 'en' }, chatItems: [] };
messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
  { error: 'SSE stream request failed: HTTP 402 insufficient balance', code: 'transient', category: 'internal' },
  codedState,
  (text, metadata) => codedState.chatItems.push({ text, ...metadata }),
  false,
);
assert.equal(codedState.chatItems[0].userError.errorCode, 'transient');
assert.equal(codedState.chatItems[0].userError.errorCategory, 'internal');
const uncodedState = { settings: { language: 'en' }, chatItems: [] };
messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
  { error: 'SSE stream request failed: HTTP 402 insufficient balance' },
  uncodedState,
  (text, metadata) => uncodedState.chatItems.push({ text, ...metadata }),
  false,
);
assert.equal('errorCode' in uncodedState.chatItems[0].userError, false, 'no code field stays absent');

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
  conversationItemsForMode([legacyFinalError]),
  [],
  'the timeline already renders the final error; the compatibility bubble must not be projected again',
);

const doneSection = chatEventsSource.slice(
  chatEventsSource.indexOf('listen("chat:done"'),
  chatEventsSource.indexOf('listen("chat:usage"'),
);
assert.match(doneSection, /legacyConversationOnly: timelineTakesOver/);
assert.match(bridgeMessagesSource, /payload\.shell_cleanup_failed/);
assert.match(doneSection, /messages\.addModelServiceErrorNotice/);
assert.match(doneSection, /typeof messages\.addModelServiceErrorNotice === "function"/);
// A successful terminal (no error) must settle the turn's transient
// bubbles (R7 follow-up); error terminals keep going through
// addModelServiceErrorNotice's upgrade/hide path - the two are mutually
// exclusive.
assert.match(doneSection, /messages\.settleModelServiceErrorNotices/);
assert.match(doneSection, /typeof messages\.settleModelServiceErrorNotices === "function"/);
assert.match(bridgeMessagesSource, /settleModelServiceErrorNotices: function/);
assert.match(
  webBridgeSource.slice(
    webBridgeSource.indexOf('listen("chat:done"'),
    webBridgeSource.indexOf('listen("chat:usage"'),
  ),
  /settleModelServiceErrorNotices/,
);
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
assert.match(chatViewSource, /conversationItemsForMode\(visibleChatItems\)/);

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
  (bridgeMessagesSource.match(/^ {4}(zh|en):/gm) || []).length,
  2,
  'Shell cleanup warning must provide zh/en translations',
);

// The wave-2 split moved event forwarding (including Event::TurnComplete
// handling) from engine.rs into forwarder.rs. The contract checks event
// processing order, so forwarder.rs is concatenated first (it owns the
// TurnComplete -> timing -> emit order); on unsplit main there is no
// forwarder.rs and this falls back to engine.rs alone.
let engineSource = read('src-tauri', 'src', 'features', 'assistant', 'engine.rs');
try {
  engineSource =
    read('src-tauri', 'src', 'features', 'assistant', 'forwarder.rs') + engineSource;
} catch {
  // main (unsplit) has no forwarder.rs
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
// Error text crosses the webview boundary through the Rust redaction
// layer first; the transient payload retains the base's controlled
// code/category (R3 M3 / R4 P1: the frontend must not rely on string
// guessing alone).
const errorEventSection = engineSource.slice(
  engineSource.indexOf('Event::Error { envelope'),
  engineSource.indexOf('Event::CompactionFailed', engineSource.indexOf('Event::Error { envelope')),
);
assert.match(
  errorEventSection,
  /redact_secret\(&envelope\.message\)/,
  'transient error text must pass the Rust redaction layer before crossing to the webview',
);
assert.match(errorEventSection, /"code": envelope\.code/);
assert.match(errorEventSection, /"category": envelope\.category\.to_string\(\)/);
assert.match(
  turnCompleteSection,
  /redact_secret\(&error\)/,
  'chat:done terminal error text must pass the Rust redaction layer too',
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
