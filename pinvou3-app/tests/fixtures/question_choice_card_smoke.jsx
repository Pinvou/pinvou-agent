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
        <div data-testid="locked-restore-card">
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
        </div>
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
      {/* 评审第五轮 P2 回归 A：allowOther=false + 预设项名为“其他” + 显式 other:false。
          重挂载必须高亮预设“其他”项，不得被 label 兼容判定误归为自定义（无自定义输入可渲染）。 */}
      <div data-testid="preset-named-other-card">
        <QuestionChoiceCard
          title="预设项名为其他且禁用自由输入（锁定）"
          questions={[{
            id: 'q-preset-other',
            header: '选择',
            question: '选一个？',
            options: [{ label: '其他' }, { label: '常规' }],
            allowOther: false,
            multiSelect: false,
          }]}
          initialAnswers={[{ id: 'q-preset-other', label: '其他', value: '其他', other: false }]}
          otherAnswerLabel="其他"
          resolved
          statusText="已提交"
        />
      </div>
      {/* 评审第五轮 P2 回归 B：跨语言冷重载——other 标记被后端剥离，label 仍为中文“其他”，
          但界面已切英文（otherAnswerLabel='Other'）。不得按 value 回退把撞值的“其他”答案
          误判为预设 A，应还原为自定义输入。 */}
      <div data-testid="cross-lang-cold-card">
        <QuestionChoiceCard
          title="跨语言冷重载（锁定）"
          questions={[{
            id: 'q-cross-lang',
            header: '选择',
            question: '选一个？',
            options: [{ label: 'A' }, { label: 'B' }],
            allowOther: true,
            multiSelect: false,
          }]}
          initialAnswers={[{ id: 'q-cross-lang', label: '其他', value: 'A' }]}
          otherAnswerLabel="Other"
          resolved
          statusText="已提交"
        />
      </div>
      <button type="button" data-testid="reset" onClick={() => setResolved(false)}>重置</button>
    </div>
  );
};

createRoot(document.getElementById('root')).render(<Fixture />);
