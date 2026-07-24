#!/usr/bin/env node

const DEFAULT_BASE_URL = 'https://ima.qq.com';
const ERR_PROGRAMMATIC = -100;

function programError(message) {
  const err = new Error(message);
  err.code = ERR_PROGRAMMATIC;
  err.msg = message;
  return err;
}

function redact(value) {
  return String(value || '')
    .replace(/Bearer\s+[A-Za-z0-9._~+/=-]+/gi, 'Bearer [REDACTED]')
    .replace(/(api[_-]?key|client[_-]?id|secret|token)\s*[:=]\s*[^,\s}]+/gi, '$1=[REDACTED]');
}

function loadCredentials(options = {}) {
  const clientId = options.clientId || process.env.IMA_CLIENT_ID || process.env.IMA_OPENAPI_CLIENTID;
  const apiKey = options.apiKey || process.env.IMA_API_KEY || process.env.IMA_OPENAPI_APIKEY;
  if (!clientId || !apiKey) {
    throw programError('未找到 IMA 凭证。请先在 Pinvou 工具商店连接「腾讯 ima」。');
  }
  return { clientId, apiKey };
}

function parseJson(raw, label) {
  if (!raw || !raw.trim()) return {};
  try {
    return JSON.parse(raw);
  } catch {
    throw programError(`${label} 不是合法 JSON。`);
  }
}

async function imaApi(apiPath, body = {}, options = {}) {
  if (!apiPath || typeof apiPath !== 'string') {
    throw programError('缺少必需参数 apiPath。');
  }
  if (!apiPath.startsWith('openapi/')) {
    throw programError('apiPath 必须以 openapi/ 开头。');
  }

  const { clientId, apiKey } = loadCredentials(options);
  const baseUrl = (options.baseUrl || process.env.IMA_BASE_URL || DEFAULT_BASE_URL).replace(/\/+$/, '');
  const skillVersion = options.skillVersion || process.env.IMA_SKILL_VERSION || '1.1.8-pinvou1';

  const response = await fetch(`${baseUrl}/${apiPath}`, {
    method: 'POST',
    headers: {
      'ima-openapi-clientid': clientId,
      'ima-openapi-apikey': apiKey,
      'ima-openapi-ctx': `skill_version=${skillVersion}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(body),
  });
  const text = await response.text();
  if (!response.ok) {
    throw programError(`IMA HTTP ${response.status}: ${redact(text).slice(0, 300)}`);
  }
  return text;
}

async function main() {
  const [, , apiPath, rawBody = '{}', rawOptions = '{}'] = process.argv;
  try {
    const body = parseJson(rawBody, 'body');
    const options = parseJson(rawOptions, 'options');
    const result = await imaApi(apiPath, body, options);
    process.stdout.write(result);
  } catch (err) {
    const code = typeof err.code === 'number' ? err.code : ERR_PROGRAMMATIC;
    const msg = redact(err.msg || err.message || '未知错误');
    process.stderr.write(JSON.stringify({ code, msg }));
    process.exit(1);
  }
}

if (require.main === module) {
  main();
}

module.exports = { imaApi, ERR_PROGRAMMATIC };
