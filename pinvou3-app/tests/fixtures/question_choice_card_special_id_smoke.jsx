import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import '../../src/styles/base.css';
import { QuestionChoiceCard } from '../../src/features/conversation/QuestionChoiceCard.jsx';

// 特殊 question id 冒烟：constructor/toString/__proto__ 是后端仅校验非空即可通过的合法输入。
// 卡片状态必须用无原型对象，否则未选择就被判为已回答、提交伪造“其他答案”、历史答案丢选中态。
window.__submits = [];

const questions = [
  { id: 'constructor', header: '构造', question: '选一个？', options: [{ label: 'A' }, { label: 'B' }], multiSelect: false },
  { id: 'toString', header: '字符串', question: '选一个？', options: [{ label: 'C' }, { label: 'D' }], multiSelect: false },
  { id: '__proto__', header: '原型', question: '选一个？', options: [{ label: 'X' }, { label: 'Y' }], multiSelect: false },
];

const Fixture = () => {
  const [resolved, setResolved] = useState(false);
  return (
    <div className="max-w-md mx-auto p-6">
      <div data-testid="special-card">
        <QuestionChoiceCard
          title="特殊 id 选择"
          questions={questions}
          submitLabel="提交"
          cancelLabel="取消"
          onSubmit={(groups) => {
            window.__submits.push(groups);
            setResolved(true);
          }}
          onCancel={() => setResolved(true)}
        />
      </div>
      {resolved && (
        <div data-testid="locked-special-card">
          <QuestionChoiceCard
            title="已提交（锁定）"
            questions={questions}
            initialAnswers={[{ id: '__proto__', label: 'X', value: 'X' }]}
            resolved
            statusText="已提交"
          />
        </div>
      )}
    </div>
  );
};

createRoot(document.getElementById('root')).render(<Fixture />);
