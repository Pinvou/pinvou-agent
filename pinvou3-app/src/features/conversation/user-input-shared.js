// 主聊天（tool-renderers.jsx 的 UserInputCard）与代码会话（CodexAcpView.jsx 的
// NativeUserInputCard）共用的 request_user_input 选项归一化与答案组装。
// 两张卡的提交通道不同（bridge 全局 activeSession vs 显式 sessionId），但问题的
// 归一化形状（allow_free_text / multi_select / 「其他」占位项过滤）与答案结构
// 必须完全一致，收敛在这里避免两处拷贝漂移。

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
    // 保留 other 标记：QuestionChoiceCard 还原历史答案时据此把“其他”与预设选项区分开，
    // 避免“其他值 == 预设 value”被误判为预设（评审 P2）。
    other: answer.other,
  })));
}
