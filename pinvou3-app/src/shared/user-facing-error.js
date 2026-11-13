const ERROR_CODE_PATTERN = /^[a-z][a-z0-9_]*$/;

function errorCode(value) {
  if (value && typeof value === 'object' && typeof value.code === 'string') {
    const code = value.code.trim();
    if (ERROR_CODE_PATTERN.test(code)) return code;
  }

  const message = typeof value === 'string'
    ? value
    : (value && typeof value.message === 'string' ? value.message : '');
  const separator = message.indexOf(':');
  if (separator <= 0) return '';
  const code = message.slice(0, separator).trim();
  return ERROR_CODE_PATTERN.test(code) ? code : '';
}

function localizedErrorMessage(value, messages, fallback) {
  const code = errorCode(value);
  return (code && messages && messages[code]) || fallback;
}

export { errorCode, localizedErrorMessage };
