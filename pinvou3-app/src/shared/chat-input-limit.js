const CHAT_INPUT_MAX_LENGTH = 100000;

function constrainChatInput(value, maxLength = CHAT_INPUT_MAX_LENGTH) {
  const text = String(value ?? '');
  const safeMaxLength = Number.isFinite(maxLength) && maxLength > 0
    ? Math.floor(maxLength)
    : CHAT_INPUT_MAX_LENGTH;

  return {
    text: text.slice(0, safeMaxLength),
    limitReached: text.length >= safeMaxLength,
    truncated: text.length > safeMaxLength,
  };
}

export { CHAT_INPUT_MAX_LENGTH, constrainChatInput };
