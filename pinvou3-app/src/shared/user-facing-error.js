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
  // Mirror the full `connectorErrorCopy` house pattern: guard the prototype
  // chain before the dict lookup ('constructor' passes the snake_case pattern
  // and would otherwise resolve to the inherited Object constructor), and
  // only ever return string copy — i18n dicts also hold function- and
  // array-valued keys that must fall through instead of reaching the DOM.
  if (
    !code || !messages
    || !Object.prototype.hasOwnProperty.call(messages, code)
    || typeof messages[code] !== 'string'
  ) {
    return fallback;
  }
  return messages[code] || fallback;
}

export { errorCode, localizedErrorMessage };
