// request_user_input option normalization and answer assembly shared by the main chat (UserInputCard
// in tool-renderers.jsx) and the code session (NativeUserInputCard in CodexAcpView.jsx), plus the
// ACP elicitation schema mapping for ElicitationCard.
// The two request_user_input cards submit through different channels (bridge global activeSession
// vs explicit sessionId), but the question normalization shape (multi_select / "other"
// placeholder-option filtering) and the answer structure must match exactly; it is consolidated
// here to avoid drift between the two copies.
//
// Custom-reply policy: the free-text "other" box is pinned open at this layer for every
// choice-style question, regardless of what the model asked for. This mirrors the CodeWhale TUI
// (crates/tui/src/tui/user_input.rs `offers_other()` is unconditionally true): `allow_free_text`
// stays on the wire for backward compatibility but no longer gates the UI, so the user can always
// answer with their own words alongside the offered options.

// Placeholder-option labels across the three UI languages (zh 其他/其它, en Other, ja その他);
// an optional parenthetical suffix (e.g. "Other (custom)" / “其他（自定义）”) stays a placeholder.
export function isFreeTextPlaceholderOption(option) {
  const label = String(option?.label || '').trim();
  return /^(?:其他|其它|other|その他)(?:\s*[（(][^()（）]*[)）])?$/i.test(label);
}

export function normalizeUserInputQuestions(questions) {
  return (questions || []).map((question, index) => {
    return {
      id: question.id || `question-${index + 1}`,
      header: question.header || `Q${index + 1}`,
      question: question.question || '',
      // The built-in custom box always exists now, so a model-supplied option that is just an
      // "其他/Other" placeholder is always dropped in favor of it.
      options: (question.options || [])
        .filter(option => !isFreeTextPlaceholderOption(option))
        .map(option => ({
          value: option.label,
          label: option.label,
          description: option.description || '',
        })),
      allowOther: true,
      multiSelect: Boolean(question.multi_select),
      required: !question.multi_select,
    };
  });
}

// ACP elicitation (codex agent) requestedSchema → QuestionChoiceCard questions. The agent marks
// its free-text companion fields with `_meta.codex.isOtherAnswer` and ties them to the owning
// question via `_meta.codex.questionId`; those companions are folded into the question instead of
// rendering as standalone inputs.
/**
 * @typedef {object} ElicitationMetaCodex
 * @property {boolean} [isOtherAnswer]
 * @property {string} [questionId]
 * @property {boolean} [isOther]
 * @property {boolean} [isSecret]
 */
/**
 * @typedef {object} ElicitationField
 * @property {string} [type]
 * @property {string} [title]
 * @property {string} [description]
 * @property {Array<{ const?: unknown, title?: string, description?: string }>} [oneOf]
 * @property {unknown[]} [enum]
 * @property {{ codex?: ElicitationMetaCodex }} [_meta]
 */
/**
 * @typedef {object} ElicitationSchema
 * @property {unknown[]} [required]
 * @property {Record<string, ElicitationField>} [properties]
 */
/**
 * @param {ElicitationSchema | null} [schema] - raw elicitation requestedSchema (property keys are
 *   model-chosen and only validated non-empty by the agent, so unmapped fields stay loose)
 * @param {string} [otherPlaceholder] - copy fallback for the custom box when the agent's companion
 *   field carries no title
 */
export function normalizeElicitationQuestions(schema, otherPlaceholder = '') {
  const requiredList = schema && schema.required;
  const required = new Set(Array.isArray(requiredList) ? requiredList : []);
  const fields = Object.entries((schema && schema.properties) || {});
  /** @param {ElicitationField} field - schema property being tested */
  const isOtherAnswerField = field => Boolean(
    field && field._meta && field._meta.codex && field._meta.codex.isOtherAnswer,
  );
  const otherFields = new Map(fields
    .filter(([, field]) => isOtherAnswerField(field))
    .map(([id, field]) => [String(field?._meta?.codex?.questionId || ''), { id, field }]));
  return fields
    .filter(([, field]) => !isOtherAnswerField(field))
    .map(([id, field]) => {
      const other = otherFields.get(id);
      const options = elicitationChoices(field);
      return {
        id,
        answerKey: id,
        otherAnswerKey: other && other.id,
        header: field.title || id,
        question: field.description || '',
        options,
        // Pinned open even when the agent schema shipped no companion other-answer field: the
        // custom value then submits through the main answerKey (see buildElicitationContent).
        // Non-choice fields keep their primary free-text input as the custom reply instead of a
        // duplicate box.
        allowOther: options.length > 0,
        otherPlaceholder: (other && other.field.title) || otherPlaceholder,
        required: required.has(id)
          || Boolean(field && field._meta && field._meta.codex && field._meta.codex.isOther),
        inputType: field.type || 'string',
        secret: Boolean(field && field._meta && field._meta.codex && field._meta.codex.isSecret),
      };
    });
}

/**
 * @param {ElicitationField} field - one requestedSchema property (oneOf/enum choice, or a free-typed input)
 */
function elicitationChoices(field) {
  const oneOf = field && field.oneOf;
  if (Array.isArray(oneOf)) {
    return oneOf.map(option => ({
      value: option && option.const,
      label: option && (option.title || option.const),
      description: option && option.description,
    })).filter(option => option.value != null);
  }
  const enumValues = field && field.enum;
  if (Array.isArray(enumValues)) {
    return enumValues.map(value => ({ value, label: String(value), description: '' }));
  }
  return [];
}

export function buildUserInputAnswers(groups, otherLabel) {
  return (groups || []).flatMap(group => group.answers.map(answer => ({
    id: group.questionId,
    label: answer.other ? otherLabel || answer.label : answer.label,
    value: String(answer.value),
    // Keep the other flag: QuestionChoiceCard relies on it to tell "other" apart from preset options when
    // restoring historical answers, so an "other value == preset value" pair is not mistaken for a preset (review P2).
    other: answer.other,
  })));
}
