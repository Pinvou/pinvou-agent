import React, { useId, useState } from 'react';
import { MessageCircle } from '../../components/icons.jsx';

function normalizedOptions(question) {
  return (question.options || []).map(option => ({
    value: option.value != null ? option.value : option.label,
    label: option.label != null ? String(option.label) : String(option.value),
    description: option.description || '',
  }));
}

function initialState(questions, initialAnswers) {
  const selected = {};
  const other = {};
  for (const question of questions) {
    const answers = (initialAnswers || []).filter(answer => answer && answer.id === question.id);
    const options = normalizedOptions(question);
    const optionValues = answers
      .filter(answer => options.some(option => (
        option.label === answer.label || option.value === answer.value
      )))
      .map(answer => {
        const option = options.find(candidate => (
          candidate.label === answer.label || candidate.value === answer.value
        ));
        return option && option.value;
      })
      .filter(value => value != null);
    const custom = answers.find(answer => !options.some(option => (
      option.label === answer.label || option.value === answer.value
    )));
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
  const initial = initialState(questions, initialAnswers);
  const [selected, setSelected] = useState(initial.selected);
  const [other, setOther] = useState(initial.other);
  const locked = resolved || submitting;

  function choose(question, value) {
    if (locked) return;
    setOther(current => {
      const next = { ...current };
      delete next[question.id];
      return next;
    });
    setSelected(current => {
      if (!question.multiSelect) return { ...current, [question.id]: value };
      const values = Array.isArray(current[question.id]) ? current[question.id] : [];
      return {
        ...current,
        [question.id]: values.includes(value)
          ? values.filter(candidate => candidate !== value)
          : [...values, value],
      };
    });
  }

  function changeOther(question, value) {
    if (locked) return;
    setSelected(current => {
      const next = { ...current };
      delete next[question.id];
      return next;
    });
    setOther(current => ({ ...current, [question.id]: value }));
  }

  function changeValue(question, value) {
    if (locked) return;
    setSelected(current => ({ ...current, [question.id]: value }));
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
                    <label className="mt-2 flex items-center gap-2 text-[12px]">
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
