/**
 * 设置页原生 confirm 回归契约（Tauri WebView2 下系统 window.confirm 实测不弹）：
 * SettingsView 的记忆删除与反馈关闭两条流程必须走应用内自绘二级确认弹窗
 * （与 ProviderFormModal / ModelDeleteDialog / SearchDeleteDialog 同款配方），
 * 不得再依赖原生 confirm。静态读源码断言 + 三语词典 parity，照
 * acp_providers_contract.test.js 的 window.confirm 断言模式。
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import { dict } from '../src/shared/i18n-all.js'; // 三语全量断言：浏览器入口用 i18n.js 惰性装载，测试用聚合 shim

const here = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.resolve(here, '..');
const SETTINGS_VIEW = fs.readFileSync(
  path.join(appRoot, 'src', 'features', 'settings', 'SettingsView.jsx'),
  'utf8',
);
const SMOKE = fs.readFileSync(
  path.join(appRoot, 'tests', 'settings_ui_smoke.js'),
  'utf8',
);

/** 截取 startMarker 到其后第一个 endMarker 之间的源码片段。 */
function sliceSource(source, startMarker, endMarker) {
  const start = source.indexOf(startMarker);
  assert.notStrictEqual(start, -1, `source marker missing: ${startMarker}`);
  const end = source.indexOf(endMarker, start);
  assert.notStrictEqual(end, -1, `end marker missing: ${endMarker}`);
  return source.slice(start, end);
}

test('SettingsView no longer calls native window.confirm', () => {
  assert.doesNotMatch(
    SETTINGS_VIEW,
    /window\.confirm\s*\(/,
    '设置页不得调用 window.confirm（Tauri WebView2 下不弹）',
  );
});

test('memory delete routes through the in-app confirm dialog', () => {
  // 应用内确认弹窗必须存在，且确认按钮带稳定 testid
  assert.match(SETTINGS_VIEW, /data-testid="memory-delete-confirm"/, '记忆删除二级确认弹窗必须存在');
  assert.match(SETTINGS_VIEW, /data-testid="memory-delete-confirm-ok"/, '记忆删除确认按钮必须带 testid');

  // deleteItem 只记录待删条目，不得直接删除
  const deleteItem = sliceSource(
    SETTINGS_VIEW,
    'const deleteItem = item => {',
    'const confirmDeleteItem',
  );
  assert.match(deleteItem, /setMemoryDeleteConfirm\(item\)/, 'deleteItem 必须先记录待删条目');
  assert.doesNotMatch(deleteItem, /window\.confirm\s*\(/, 'deleteItem 不得调用原生 confirm');
  assert.doesNotMatch(deleteItem, /deleteMemoryItem\(item\.kind/, 'deleteItem 不得在确认前真正删除');

  // 真正的删除只存在于确认路径 confirmDeleteItem（唯一调用点）
  const confirmHandler = sliceSource(
    SETTINGS_VIEW,
    'const confirmDeleteItem = async item => {',
    'const archiveItem',
  );
  assert.match(
    confirmHandler,
    /await bridge\.memory\.deleteMemoryItem\(item\.kind, item\.id\)/,
    '确认后必须调用 deleteMemoryItem',
  );
  assert.strictEqual(
    (SETTINGS_VIEW.match(/await bridge\.memory\.deleteMemoryItem\(/g) || []).length,
    1,
    'deleteMemoryItem 只允许在确认路径中出现一次',
  );

  // 确认弹窗的 OK 按钮先执行删除再清理状态（镜像 ModelDeleteDialog 的顺序）
  const dialog = sliceSource(SETTINGS_VIEW, 'const MemoryDeleteDialog', 'const SettingsView = (');
  assert.match(
    dialog,
    /onConfirmDelete\(item\);\s*setMemoryDeleteConfirm\(null\);/,
    '确认按钮必须先删除再清理状态',
  );
  assert.doesNotMatch(dialog, /window\.confirm\s*\(/, '确认弹窗不得调用原生 confirm');
});

test('feedback close routes through the in-app confirm layer', () => {
  assert.match(SETTINGS_VIEW, /data-testid="feedback-close-confirm"/, '反馈关闭确认层必须存在');
  assert.match(SETTINGS_VIEW, /data-testid="feedback-close-confirm-ok"/, '反馈关闭确认按钮必须带 testid');

  // 脏草稿首次关闭改为弹应用内确认层，不再依赖原生 confirm
  const closeFeedback = sliceSource(
    SETTINGS_VIEW,
    'const closeFeedback = () => {',
    'const pickFeedbackAttachments',
  );
  assert.match(closeFeedback, /!feedbackCloseConfirm/, 'closeFeedback 必须以应用内确认状态为门');
  assert.match(closeFeedback, /setFeedbackCloseConfirm\(true\)/, '脏草稿首次关闭必须弹出应用内确认层');
  assert.doesNotMatch(closeFeedback, /window\.confirm\s*\(/, 'closeFeedback 不得调用原生 confirm');

  // 确认层盖在反馈面板（z-[100]）之上，且只在面板打开时出现
  assert.match(
    SETTINGS_VIEW,
    /feedbackOpen && feedbackCloseConfirm && \(/,
    '确认层必须与反馈面板同开同关',
  );
  assert.match(
    SETTINGS_VIEW,
    /"feedback-close-confirm" className="fixed inset-0 z-\[110\]/,
    '确认层必须盖在反馈面板（z-[100]）之上',
  );

  // OK 按钮确认后必须真正走 closeFeedback 关闭路径
  assert.match(
    SETTINGS_VIEW,
    /onClick=\{\(\) => \{ setFeedbackCloseConfirm\(false\); closeFeedback\(\); \}\}/,
    '确认按钮必须路由回 closeFeedback 真正关闭',
  );
});

test('the settings smoke no longer stubs native confirm', () => {
  assert.doesNotMatch(
    SMOKE,
    /window\.confirm\s*=/,
    'settings smoke 不得再 stub window.confirm（原生 confirm 复现时必须当场失败）',
  );
});

test('feedback/memory confirm copy exists in zh/en/ja', () => {
  const expectedAnyway = { zh: '仍要关闭', en: 'Close anyway', ja: '閉じる' };
  for (const language of ['zh', 'en', 'ja']) {
    const d = dict[language];
    assert.ok(d.feedbackCloseConfirm, `${language}.feedbackCloseConfirm 必须存在`);
    assert.equal(d.feedbackCloseAnyway, expectedAnyway[language], `${language}.feedbackCloseAnyway 文案不符`);
    assert.ok(d.cancel, `${language}.cancel 必须存在（反馈关闭取消按钮）`);
    assert.ok(d.uiSettingsView, `${language}.uiSettingsView 必须存在`);
    assert.ok(d.uiSettingsView.memoryDeleteConfirm, `${language}.uiSettingsView.memoryDeleteConfirm 必须存在`);
    assert.ok(d.uiSettingsDetail, `${language}.uiSettingsDetail 必须存在`);
    assert.ok(d.uiSettingsDetail.delete, `${language}.uiSettingsDetail.delete 必须存在（记忆删除确认按钮）`);
    assert.ok(d.uiSettingsDetail.cancel, `${language}.uiSettingsDetail.cancel 必须存在（记忆删除取消按钮）`);
  }
});
