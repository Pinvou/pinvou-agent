// request_user_input option normalization and answer assembly shared by the main chat (UserInputCard
// in tool-renderers.jsx) and the code session (NativeUserInputCard in CodexAcpView.jsx).
// The two cards submit through different channels (bridge global activeSession vs explicit sessionId), but the
// question normalization shape (allow_free_text / multi_select / "other" placeholder-option filtering) and the
// answer structure must match exactly; it is consolidated here to avoid drift between the two copies.

export function isFreeTextPlaceholderOption(option) {
  const label = String(option?.label || '').trim();
  return /^(?:其他|其它|other)(?:\s*[（(][^()（）]*[)）])?$/i.test(label);
}

export function normalizeUserInputQuestions(questions) {
  return (questions || []).map((question, index) => {
    const allowOther = question.allow_free_text !== false;
    return {
      id: question.id || `question-${index + 1}`,
      header: question.header || `Q${index + 1}`,
      question: question.question || '',
      options: (question.options || [])
        .filter(option => !allowOther || !isFreeTextPlaceholderOption(option))
        .map(option => ({
          value: option.label,
          label: option.label,
          description: option.description || '',
        })),
      allowOther,
      multiSelect: Boolean(question.multi_select),
      required: !question.multi_select,
    };
  });
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
