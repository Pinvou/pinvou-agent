import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import '../../src/styles/base.css';
import { QuestionChoiceCard } from '../../src/features/conversation/QuestionChoiceCard.jsx';

// 三张卡：单选（含描述）、多选、已提交锁定卡。window.__submits 记录提交 payload 供断言。
window.__submits = [];

const questions = [
  {
    id: 'q-lang',
    header: '语言',
    question: '用什么语言？',
    options: [
      { label: 'Python', description: '通用脚本' },
      { label: 'Go', description: '并发友好' },
    ],
    multiSelect: false,
  },
  {
    id: 'q-skill',
    header: '技能',
    question: '擅长哪些？',
    options: [
      { label: '前端', description: '界面' },
      { label: '后端', description: '服务' },
      { label: '运维', description: '部署' },
    ],
    multiSelect: true,
  },
];

const Fixture = () => {
  const [resolved, setResolved] = useState(false);
  return (
    <div className="max-w-md mx-auto p-6">
      <QuestionChoiceCard
        title="请选择"
        questions={questions}
        submitLabel="提交"
        cancelLabel="取消"
        onSubmit={(groups) => {
          window.__submits.push(groups);
          setResolved(true);
        }}
        onCancel={() => setResolved(true)}
      />
      {resolved && (
        <QuestionChoiceCard
          title="已提交（锁定）"
          questions={questions}
          initialAnswers={[
            { id: 'q-lang', label: 'Python', value: 'Python' },
            { id: 'q-skill', label: '前端', value: '前端' },
          ]}
          resolved
          statusText="已提交"
        />
      )}
      {/* 评审 P2 回归：其他值 == 预设 value 时，重挂载应还原为“其他”而非高亮预设。 */}
      <div data-testid="other-collision-card">
        <QuestionChoiceCard
          title="其他值与预设值相同（锁定）"
          questions={[{
            id: 'q-other-collision',
            header: '选择',
            question: '选一个？',
            options: [{ label: 'A' }, { label: 'B' }],
            allowOther: true,
            multiSelect: false,
          }]}
          initialAnswers={[{ id: 'q-other-collision', label: '其他', value: 'A' }]}
          otherAnswerLabel="其他"
          resolved
          statusText="已提交"
        />
      </div>
      <button type="button" data-testid="reset" onClick={() => setResolved(false)}>重置</button>
    </div>
  );
};

createRoot(document.getElementById('root')).render(<Fixture />);
