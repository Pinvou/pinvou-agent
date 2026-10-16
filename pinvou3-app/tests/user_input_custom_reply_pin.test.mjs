#!/usr/bin/env node
// 自定义回复框代码层钉死回归：
//   1) request_user_input 车道（UserInputCard / NativeUserInputCard 共用的
//      normalizeUserInputQuestions）：模型参数 allow_free_text=false 不再能关掉
//      自由输入框；模型自报的「其他/Other」占位选项恒被内置输入框取代。
//      与 CodeWhale TUI 的 offers_other() 恒真决策对齐（wire 字段仅保留兼容）。
//   2) ACP elicitation 车道（ElicitationCard 共用的 normalizeElicitationQuestions）：
//      选择题（oneOf/enum）即使没有 agent 生成的 other 伴随字段也恒有自定义回复框；
//      非选择题的主输入本身即自由回复，不再叠加第二个输入框。
// 导入方式对齐 codex_acp_timeline.test.mjs：拷贝源文件到临时 module 目录后 file URL 导入。
import assert from 'node:assert/strict';
import { copyFileSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, '..');
const temp = mkdtempSync(path.join(tmpdir(), 'pinvou3-user-input-pin-'));
writeFileSync(path.join(temp, 'package.json'), '{"type":"module"}\n');
copyFileSync(
  path.join(root, 'src', 'shared', 'user-input-shared.js'),
  path.join(temp, 'user-input-shared.js'),
);

try {
  const {
    buildUserInputAnswers,
    normalizeElicitationQuestions,
    normalizeUserInputQuestions,
  } = await import(`${pathToFileURL(path.join(temp, 'user-input-shared.js')).href}?t=${Date.now()}`);

  // ── request_user_input：allow_free_text 不再门控自定义回复框 ──────
  for (const flag of [false, true, undefined]) {
    const [question] = normalizeUserInputQuestions([{
      id: 'q1',
      header: '部署',
      question: '发到哪个环境?',
      options: [
        { label: '测试', description: '先灰度' },
        { label: '生产', description: '全量' },
      ],
      allow_free_text: flag,
    }]);
    assert.equal(question.allowOther, true, `allow_free_text=${flag} 时自定义回复框必须存在`);
    assert.deepEqual(question.options.map(option => option.label), ['测试', '生产']);
  }

  // 模型自报的「其他/Other/その他」占位选项恒被内置输入框取代（含 allow_free_text=false）。
  {
    const [question] = normalizeUserInputQuestions([{
      id: 'q1',
      header: '范围',
      question: '改哪里?',
      options: [
        { label: '仅前端', description: '' },
        { label: '其他', description: '自己说' },
        { label: 'Other (custom)', description: '' },
        { label: '其它（补充）', description: '' },
        { label: 'その他', description: '' },
        { label: 'その他（自由記入）', description: '' },
      ],
      allow_free_text: false,
    }]);
    assert.deepEqual(
      question.options.map(option => option.label),
      ['仅前端'],
      '三语占位式其他选项被内置自定义框取代，普通选项保留',
    );
  }

  // 其余映射不受影响：multi_select、必答、缺省 id/header。
  {
    const [multi, single] = normalizeUserInputQuestions([
      { question: '技能?', options: [{ label: 'A', description: '' }, { label: 'B', description: '' }], multi_select: true },
      { question: '语言?', options: [{ label: 'A', description: '' }, { label: 'B', description: '' }] },
    ]);
    assert.equal(multi.multiSelect, true);
    assert.equal(multi.required, false);
    assert.equal(single.multiSelect, false);
    assert.equal(single.required, true);
    assert.equal(multi.id, 'question-1');
    assert.equal(single.header, 'Q2');
  }

  // 钉死后必然可达的自定义答案提交：other 标记保留、label 换成界面“其他”文案（三语 i18n 键）。
  {
    const answers = buildUserInputAnswers([{
      questionId: 'q1',
      answerKey: 'q1',
      otherAnswerKey: null,
      multiSelect: false,
      answers: [{ label: '其他', value: '自定义内容', other: true }],
    }], 'Other');
    assert.deepEqual(answers, [{ id: 'q1', label: 'Other', value: '自定义内容', other: true }]);
  }

  // ── elicitation：选择题恒有自定义框，伴随字段折叠与元数据映射不变 ──
  {
    const schema = {
      type: 'object',
      required: ['flavor'],
      properties: {
        flavor: {
          type: 'string',
          title: '口味',
          description: '选哪种?',
          oneOf: [
            { const: 'mint', title: '薄荷', description: '凉' },
            { const: 'berry', title: '莓果' },
          ],
          _meta: { codex: { questionId: 'flavor' } },
        },
        flavor_other: {
          type: 'string',
          title: '其他口味',
          _meta: { codex: { isOtherAnswer: true, questionId: 'flavor' } },
        },
        engine: {
          type: 'string',
          title: '引擎',
          enum: ['v8', 'jsc'],
          _meta: { codex: { questionId: 'engine' } },
        },
        amount: {
          type: 'integer',
          title: '数量',
          _meta: { codex: { questionId: 'amount' } },
        },
        token: {
          type: 'string',
          _meta: { codex: { questionId: 'token', isSecret: true } },
        },
      },
    };
    const questions = normalizeElicitationQuestions(schema, '补充说明…');
    assert.deepEqual(
      questions.map(question => question.id),
      ['flavor', 'engine', 'amount', 'token'],
      'other 伴随字段折叠进所属问题，不单独成题',
    );

    const [flavor, engine, amount, token] = questions;
    // 有伴随字段：沿用伴随字段提交与占位文案。
    assert.equal(flavor.allowOther, true);
    assert.equal(flavor.otherAnswerKey, 'flavor_other');
    assert.equal(flavor.otherPlaceholder, '其他口味');
    assert.equal(flavor.required, true);
    assert.deepEqual(flavor.options, [
      { value: 'mint', label: '薄荷', description: '凉' },
      { value: 'berry', label: '莓果', description: undefined },
    ]);

    // 无伴随字段的枚举题（钉死点）：自定义框必须在，提交回落主 answerKey。
    assert.equal(engine.allowOther, true, '无 other 伴随字段的枚举题也必须有自定义回复框');
    assert.equal(engine.otherAnswerKey, undefined, '无伴随字段时不造键，提交时 QuestionChoiceCard 归一为 null');
    assert.equal(engine.otherPlaceholder, '补充说明…');
    assert.deepEqual(engine.options, [
      { value: 'v8', label: 'v8', description: '' },
      { value: 'jsc', label: 'jsc', description: '' },
    ]);

    // 非选择题：主输入即自由回复，不叠加第二个输入框。
    assert.equal(amount.allowOther, false);
    assert.equal(amount.inputType, 'integer');
    assert.equal(token.secret, true);

    // 空 schema 容错。
    assert.deepEqual(normalizeElicitationQuestions(null), []);
    assert.deepEqual(normalizeElicitationQuestions({}), []);
  }

  console.log('user_input_custom_reply_pin: all assertions passed');
} finally {
  rmSync(temp, { recursive: true, force: true });
}
