import React, { useId, useState } from 'react';
import { MessageCircle } from '../../components/icons.jsx';

function normalizedOptions(question) {
  return (question.options || []).map(option => ({
    value: option.value != null ? option.value : option.label,
    label: option.label != null ? String(option.label) : String(option.value),
    description: option.description || '',
  }));
}

function initialState(questions, initialAnswers, otherAnswerLabel) {
  // 用无原型对象：question.id 后端仅校验非空，constructor/toString/__proto__ 是合法输入；
  // 普通 {} 会让这些键命中 Object.prototype（读函数/写触发 __proto__ setter），导致未选择
  // 被判为已回答、伪造“其他答案”或历史答案无法形成 own property（重挂载丢选中态）。
  const selected = Object.create(null);
  const other = Object.create(null);
  for (const question of questions) {
    const answers = (initialAnswers || []).filter(answer => answer && answer.id === question.id);
    const options = normalizedOptions(question);
    // “其他”答案判定（评审第五轮 P2）：
    //   1) 显式布尔 other 标记优先——包括明确的 false。warm 路径（restoredAnswers）保留 other
    //      标记；若问题 allow_free_text=false 却存在名为“其他”的普通预设项（isFreeTextPlaceholderOption
    //      过滤被 allowOther 跳过、该预设项保留），提交链路会产生 { other: false }，必须尊重它，
    //      否则该预设项重挂载后既不高亮也看不到答案。
    //   2) 冷重载路径后端只持久化 {id,label,value}（UserInputAnswer 无 other 字段，已被剥离）；
    //      仅当该问题允许自由输入（allowOther）时才按 label 命中“其他”文案兼容判定，allowOther=false
    //      时不存在自定义答案，不应把名为“其他”的预设项误判为 other。
    const isOtherAnswer = answer => {
      if (typeof answer.other === 'boolean') return answer.other;
      return Boolean(question.allowOther)
        && otherAnswerLabel != null && otherAnswerLabel !== ''
        && answer.label === otherAnswerLabel;
    };
    // 预设匹配优先按 label：跨语言冷重载时 otherAnswerLabel 随界面语言变化，“其他”答案的 label
    // （如中文“其他”）不再命中英文预设；此时不应回退到 value 匹配，否则撞值的“其他”答案会被
    // 误判为预设（评审第五轮 P2）。仅当 label 缺失时才按 value 回退，兼容 label 缺失的历史数据。
    const findOption = answer => (
      answer.label != null && answer.label !== ''
        ? options.find(option => option.label === answer.label)
        : options.find(option => option.value === answer.value)
    );
    const matchesOption = answer => Boolean(findOption(answer));
    const optionValues = answers
      .filter(answer => !isOtherAnswer(answer) && matchesOption(answer))
      .map(answer => {
        const option = findOption(answer);
        return option && option.value;
      })
      .filter(value => value != null);
    const custom = answers.find(answer => isOtherAnswer(answer) || !matchesOption(answer));
    if (question.multiSelect) {
      if (optionValues.length) selected[question.id] = optionValues;
    } else if (optionValues.length) {
      selected[question.id] = optionValues[0];
    } else if (!options.length && answers.length) {
      selected[question.id] = answers[0].value;
    }
    if (custom) other[question.id] = custom.value || '';
  }
  return { selected, other };
}

function hasOwn(object, key) {
  return Object.prototype.hasOwnProperty.call(object, key);
}

// 状态更新必须保持无原型，否则 spread 回普通对象会让未显式设置的保留键重新命中
// Object.prototype（例如选 constructor 前读取 selected['constructor'] 拿到函数）。
function copyState(state) {
  return Object.assign(Object.create(null), state);
}

export function QuestionChoiceCard({
  title,
  description = '',
  questions = [],
  initialAnswers = [],
  resolved = false,
  submitting = false,
  statusText = '',
  error = false,
  submitLabel = '',
  cancelLabel = '',
  otherPlaceholder = '',
  otherAnswerLabel = '',
  inputPlaceholder = '',
  onSubmit,
  onCancel,
}) {
  const cardId = useId();
  const initial = initialState(questions, initialAnswers, otherAnswerLabel);
  const [selected, setSelected] = useState(initial.selected);
  const [other, setOther] = useState(initial.other);
  const locked = resolved || submitting;

  function choose(question, value) {
    if (locked) return;
    setOther(current => {
      const next = copyState(current);
      delete next[question.id];
      return next;
    });
    setSelected(current => {
      const next = copyState(current);
      if (!question.multiSelect) {
        next[question.id] = value;
        return next;
      }
      const values = Array.isArray(current[question.id]) ? current[question.id] : [];
      next[question.id] = values.includes(value)
        ? values.filter(candidate => candidate !== value)
        : [...values, value];
      return next;
    });
  }

  function changeOther(question, value) {
    if (locked) return;
    setSelected(current => {
      const next = copyState(current);
      delete next[question.id];
      return next;
    });
    setOther(current => {
      const next = copyState(current);
      next[question.id] = value;
      return next;
    });
  }

  function changeValue(question, value) {
    if (locked) return;
    setSelected(current => {
      const next = copyState(current);
      next[question.id] = value;
      return next;
    });
  }

  function answered(question) {
    if (String(other[question.id] || '').trim()) return true;
    if (!hasOwn(selected, question.id)) return false;
    const value = selected[question.id];
    return Array.isArray(value) ? value.length > 0 : value !== '';
  }

  const complete = questions.length > 0 && questions.every(question => (
    question.required === false || answered(question)
  ));

  function submit() {
    if (locked || !complete || !onSubmit) return;
    const groups = questions.map(question => {
      const custom = String(other[question.id] || '').trim();
      if (custom) {
        return {
          questionId: question.id,
          answerKey: question.answerKey || question.id,
          otherAnswerKey: question.otherAnswerKey || null,
          multiSelect: false,
          answers: [{ label: question.otherAnswerLabel || otherAnswerLabel, value: custom, other: true }],
        };
      }
      const values = question.multiSelect
        ? (Array.isArray(selected[question.id]) ? selected[question.id] : [])
        : [selected[question.id]];
      const options = normalizedOptions(question);
      return {
        questionId: question.id,
        answerKey: question.answerKey || question.id,
        otherAnswerKey: question.otherAnswerKey || null,
        multiSelect: Boolean(question.multiSelect),
        answers: values.filter(value => value !== undefined).map(value => {
          const option = options.find(candidate => candidate.value === value);
          return {
            label: option ? option.label : String(value),
            value,
            other: false,
          };
        }),
      };
    });
    onSubmit(groups);
  }

  return (
    <div className="rounded-2xl border border-blue-500/20 bg-blue-500/[0.045] p-4">
      <div className="flex items-start gap-3">
        <MessageCircle size={18} className="mt-0.5 shrink-0 text-blue-500" />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold">{title}</div>
          {description && (
            <div className="mt-1 text-[12px] leading-5 text-gray-500 dark:text-gray-400">
              {description}
            </div>
          )}
          <div className="mt-3 space-y-4">
            {questions.map((question, index) => {
              const options = normalizedOptions(question);
              const selectedValues = question.multiSelect
                ? (Array.isArray(selected[question.id]) ? selected[question.id] : [])
                : [selected[question.id]];
              return (
                <fieldset key={question.id || index} disabled={locked} className="min-w-0">
                  <legend className="text-[12px] font-semibold">
                    {questions.length > 1 ? `${index + 1}. ` : ''}{question.header || question.id}
                  </legend>
                  {question.question && (
                    <div className="mt-1 text-[12px] leading-5 text-gray-500 dark:text-gray-400">
                      {question.question}
                    </div>
                  )}
                  {options.length > 0 ? (
                    <div className="mt-2 grid gap-2">
                      {options.map(option => {
                        const active = selectedValues.includes(option.value);
                        return (
                          <label key={String(option.value)}
                            onClick={(event) => {
                              // 选项整行可点：点击文本/描述区域直接选中（WebView 下 label 隐式
                              // 激活不可靠）；点击圆点本身交给原生 onChange，避免双触发。
                              if (locked) return;
                              if (event.target === event.currentTarget.querySelector('input')) return;
                              event.preventDefault();
                              choose(question, option.value);
                            }}
                            className={`rounded-xl border px-3 py-2.5 cursor-pointer transition-colors ${
                              active
                                ? 'border-blue-500/55 bg-blue-500/[0.08]'
                                : 'border-black/[0.08] dark:border-white/10 hover:bg-black/[0.025] dark:hover:bg-white/[0.04]'
                            }`}>
                            <span className="flex items-start gap-2">
                              <input
                                type={question.multiSelect ? 'checkbox' : 'radio'}
                                name={`${cardId}-${question.id}-choice`}
                                checked={active}
                                onChange={() => choose(question, option.value)}
                                className="mt-0.5 accent-blue-600"
                              />
                              <span>
                                <span className="block text-[12px] font-medium">{option.label}</span>
                                {option.description && (
                                  <span className="mt-0.5 block text-[11px] leading-4 text-gray-500 dark:text-gray-400">
                                    {option.description}
                                  </span>
                                )}
                              </span>
                            </span>
                          </label>
                        );
                      })}
                    </div>
                  ) : question.inputType === 'boolean' ? (
                    <label
                      className="mt-2 flex items-center gap-2 text-[12px]"
                      onClick={(event) => {
                        if (locked) return;
                        if (event.target === event.currentTarget.querySelector('input')) return;
                        event.preventDefault();
                        changeValue(question, !Boolean(selected[question.id]));
                      }}
                    >
                      <input
                        type="checkbox"
                        checked={Boolean(selected[question.id])}
                        onChange={event => changeValue(question, event.target.checked)}
                        className="accent-blue-600"
                      />
                      {question.header || question.id}
                    </label>
                  ) : (
                    <input
                      type={question.secret
                        ? 'password'
                        : ['number', 'integer'].includes(question.inputType) ? 'number' : 'text'}
                      value={selected[question.id] == null ? '' : selected[question.id]}
                      onChange={event => {
                        const raw = event.target.value;
                        changeValue(
                          question,
                          ['number', 'integer'].includes(question.inputType) && raw !== ''
                            ? Number(raw)
                            : raw,
                        );
                      }}
                      placeholder={question.placeholder || inputPlaceholder}
                      className="mt-2 w-full rounded-xl border border-black/[0.08] dark:border-white/10 bg-white/80 dark:bg-white/[0.04] px-3 py-2 text-[12px] outline-none focus:border-blue-500/50"
                    />
                  )}
                  {question.allowOther && (
                    <input
                      type={question.secret ? 'password' : 'text'}
                      value={other[question.id] || ''}
                      onChange={event => changeOther(question, event.target.value)}
                      placeholder={question.otherPlaceholder || otherPlaceholder}
                      className="mt-2 w-full rounded-xl border border-black/[0.08] dark:border-white/10 bg-white/80 dark:bg-white/[0.04] px-3 py-2 text-[12px] outline-none focus:border-blue-500/50"
                    />
                  )}
                </fieldset>
              );
            })}
          </div>
          {!resolved ? (
            <div className="mt-4 flex items-center gap-2">
              <button
                type="button"
                disabled={!complete || submitting}
                onClick={submit}
                className="px-3 py-1.5 rounded-xl bg-blue-600 text-white text-[12px] font-medium hover:bg-blue-700 disabled:opacity-45 disabled:cursor-not-allowed"
              >
                {submitting ? '…' : submitLabel}
              </button>
              {onCancel && (
                <button
                  type="button"
                  disabled={submitting}
                  onClick={onCancel}
                  className="px-3 py-1.5 rounded-xl text-[12px] text-gray-500 hover:bg-black/[0.05] dark:hover:bg-white/[0.07] disabled:opacity-45"
                >
                  {cancelLabel}
                </button>
              )}
            </div>
          ) : null}
          {statusText && (
            <div className={`mt-3 text-[11px] ${error ? 'text-red-500' : 'text-gray-400'}`}>
              {statusText}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
