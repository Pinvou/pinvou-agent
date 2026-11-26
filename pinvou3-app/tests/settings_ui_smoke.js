#!/usr/bin/env node
/**
 * 设置页全局配置 smoke：加载真实 Vite dist，mock TauriBridge，点击真实 React UI。
 * 覆盖模型/搜索/权限/帮助反馈四个高风险设置面板：
 * - 模型列表默认单选、删除二次确认、本地/云端模型删除、添加模型保存条件、编辑模型回显凭据和同厂商切换
 * - 搜索源交互与模型页一致，添加源未点保存不得持久化，确认重启后才写设置
 * - 高级执行权限失败时回滚并 toast 提示
 * - Community uses a dedicated Settings tab that exposes the QQ group and GitHub Discussions at the top level
 * - 反馈弹窗保持与模型/搜索一致的 iOS 规格，提交成功不使用原生 alert
 */
const fs = require('fs');
const os = require('os');
const path = require('path');
const assert = require('assert');
const { startUiTestServer } = require('./ui_test_server');

const settingsViewSource = fs.readFileSync(
  path.join(__dirname, '..', 'src', 'features', 'settings', 'SettingsView.jsx'),
  'utf8',
);
const localServerTiersSource = fs.readFileSync(
  path.join(__dirname, '..', 'src', 'features', 'settings', 'local-server-tiers.jsx'),
  'utf8',
);
const endpointPickerSource = fs.readFileSync(
  path.join(__dirname, '..', 'src', 'features', 'settings', 'endpoint-model-picker.jsx'),
  'utf8',
);
const appMainSource = fs.readFileSync(
  path.join(__dirname, '..', 'src', 'app', 'main.jsx'),
  'utf8',
);
const settingsI18nSources = ['zh', 'en', 'ja'].map((lang) => fs.readFileSync(
  path.join(__dirname, '..', 'src', 'shared', 'i18n', `${lang}.js`),
  'utf8',
));
// test_model_connection always resolves a serialized result object (the Rust
// side maps transport/HTTP failures into the struct, including the 402
// 'billing' code consumed via connectionMessages below); the FE normalizer
// therefore keeps only the object branch plus an unknown-code reject fallback.
assert.doesNotMatch(
  settingsViewSource,
  /HTTP\\s\+\(\\d\{3\}\)/,
  'the legacy string-protocol HTTP parser must stay deleted',
);
assert.match(
  settingsViewSource,
  /code: 'unknown', message: settingsCopy\.connectionMessages\.unknown, detail: String\(value \|\| ''\)/,
  'settings connection test must degrade unknown rejects to the unknown copy',
);
for (const settingsI18nSource of settingsI18nSources) {
  assert.match(
    settingsI18nSource,
    /connectionMessages:\{[^}]*billing:/,
    'settings billing message must be provided in every UI language',
  );
}
// Chat streaming fade-in row: desktop-only (WebSettingsPatch has no `chat`
// field and would hard-reject the patch), with copy present in every language.
assert.match(
  settingsViewSource,
  /!isWeb && \(\s*<IOSSection title=\{t\.chatSection\}>[\s\S]*?t\.chatStreamingAnimation/,
  'chat streaming fade-in row must exist and stay gated off the web lane',
);
for (const settingsI18nSource of settingsI18nSources) {
  assert.match(
    settingsI18nSource,
    /chatStreamingAnimation:/,
    'chat streaming fade-in copy must be provided in every UI language',
  );
}
// Virtual-display preference row wiring (the R10 review: the row had no
// rendering pin — a revert of the exact-equality capability gate or of the
// preference binding passed every other test). The switch binds to the
// PERSISTED preference (`virtualPref`), never to the derived state; while
// the projection is unknown the switch renders disabled AND unchecked.
assert.match(
  settingsViewSource,
  /checked=\{virtualUnknown \? false : virtualPref\}/,
  'the virtual switch must bind to the persisted preference with the unknown-projection fail-neutral (not to the derived state)',
);
assert.doesNotMatch(
  settingsViewSource,
  /checked=\{[^}]*virtualDisplayState/,
  'the virtual switch must never bind to the derived virtualDisplayState (a live session with the pref off must show the switch OFF)',
);
// Per-platform fidelity note rows: gated on the three-valued semantic
// capability with EXACT equality — a truthy check would render the row for
// unknown/absent capability values and unqualify the page.
for (const notePlatform of ['windows', 'macos', 'linux']) {
  assert.match(
    settingsViewSource,
    new RegExp(`platformCapabilities\\?\\.virtualDisplayNote === '${notePlatform}'`),
    `the ${notePlatform} fidelity note row must gate on exact-equality of the virtualDisplayNote capability`,
  );
}
// The "unavailable" status renders the localized lead line followed by the
// backend's diagnostic detail (the install guidance must stay visible — the
// guard passes "unavailable" through for live physical sessions precisely
// so this branch keeps rendering during a session on a probe-red host).
assert.match(
  settingsViewSource,
  /virtualUnavailable \? \(\s*<span className="block mt-0\.5">\s*<span className="block">\{t\.uiComputerUse\.virtualStateUnavailableLead\}<\/span>\s*\{virtualDetail &&/,
  'the unavailable status branch must render the localized lead line and the backend detail (install guidance)',
);
// R11 + R12: the live-physical hint. A live PHYSICAL session with the
// preference flipped ON mid-session used to render the "disabled" copy
// ("Off —") next to a switch showing ON — the R11 fix replaced that copy
// with a dedicated line, but the line's trailing clause promised future
// isolation unconditionally, which is FALSE under an OFF switch and next
// to the "no isolated display available" lead. R12 splits the copy into
// the always-true FACT and a promise gated on the strict pref projection:
// the "Off —" copy is replaced by fact+promise ONLY on a pref-ON leg,
// appended as the fact on pref-OFF legs, and "unavailable" never promises.
// Fail-neutral on both strict boolean projections.
assert.match(
  settingsViewSource,
  /physicalSessionLive=\{computerUse\.physicalSessionLive\}/,
  'the status-lines component must receive the raw status field',
);
assert.match(
  settingsViewSource,
  /preferVirtual=\{computerUse\.preferVirtualDisplay\}/,
  'the status-lines component must receive the raw persisted preference (R12: the future-isolation promise is gated on it)',
);
assert.match(
  settingsViewSource,
  /const physicalLive = physicalSessionLive === true;/,
  'the live-physical line must bind the strict boolean projection (fail-neutral against an older backend)',
);
assert.match(
  settingsViewSource,
  /const promiseIsolated = preferVirtual === true;/,
  'the future-isolation promise must gate on the STRICT preference projection — promising isolation requires backend truth, not the switch\'s `!== false` default',
);
assert.match(
  settingsViewSource,
  /const replaceDisabledLine = virtualState === 'disabled' && physicalLive && promiseIsolated;/,
  'only a pref-ON live-physical "disabled" leg may replace the "Off —" copy (the R11 artifact); a pref-OFF leg keeps the true "Off —" copy and appends the fact',
);
assert.match(
  settingsViewSource,
  /const showPromise =\s*\n\s*promiseIsolated &&\s*\n\s*physicalLive &&\s*\n\s*!virtualDetail &&\s*\n\s*\(replaceDisabledLine \|\| virtualState === 'active'\);/,
  'the promise must ride ONLY a pref-ON leg that can deliver it — never "unavailable" (the probe is down there), never a pref-OFF "disabled" leg, and never a leg carrying a non-empty backend detail (an "active" row can ride a red snapshot; promising isolation while the next creation resolves physical is the R18 hole)',
);
// R13: the FACT is true whenever a physical session is live — gating it on
// `!replaceDisabledLine` rendered the promise ALONE in the replace-disabled
// leg (pref flipped ON mid-session), hiding the "session on the real
// screen" line the R11/R12 arc exists to keep visible. The regex below
// fails both on the old gate and on any re-narrowing of it.
assert.match(
  settingsViewSource,
  /const showFact = physicalLive;/,
  'the live-physical FACT must render in every leg where a physical session is live — the replace-disabled leg must show fact + promise, not the promise alone',
);
assert.match(
  settingsViewSource,
  /virtualUnavailable \? \(\s*<span className="block mt-0\.5">\s*<span className="block">\{t\.uiComputerUse\.virtualStateUnavailableLead\}<\/span>\s*\{virtualDetail && <span className="block">\{virtualDetail\}<\/span>\}\s*\{physicalLive &&/,
  'the unavailable branch must append the (fact-only) live-physical line when a physical session runs',
);
// R15: the fact survives the WRITE ERROR — a failed preference write used
// to replace ALL status lines, suppressing the "session on the real
// screen" fact while a physical session kept driving. The error branch
// must render the fact line (bounded gap tolerates the explanatory
// comment) before the error text.
assert.match(
  settingsViewSource,
  /virtualError \? \(\s*<span className="block mt-0\.5">[\s\S]{0,800}\{showFact && \(\s*<span className="block">\{t\.uiComputerUse\.virtualStatePhysicalLive\}<\/span>\s*\)\}/,
  'a failed preference write must not suppress the live-physical fact — the error branch renders the fact above the error text',
);
// R12 negative pin: the R11 single-line shape promised isolation
// unconditionally ("the isolated display applies to new sessions") — the
// over-claim must stay gone from every language, in both directions of a
// revert (restoring the old English sentence or re-merging the promise
// into the fact key).
for (const settingsI18nSource of settingsI18nSources) {
  assert.match(
    settingsI18nSource,
    /virtualStatePhysicalLive:/,
    'every language must carry the live-physical fact copy',
  );
  assert.match(
    settingsI18nSource,
    /virtualStatePhysicalNextVirtual:/,
    'every language must carry the gated future-isolation promise copy',
  );
  assert.ok(
    !settingsI18nSource.includes('applies to new sessions'),
    'the unconditioned R11 promise sentence must stay gone (en)',
  );
  assert.ok(
    !settingsI18nSource.includes('隔离显示器将用于新会话'),
    'the unconditioned R11 promise sentence must stay gone (zh)',
  );
  assert.ok(
    !settingsI18nSource.includes('隔離ディスプレイは新しいセッションで有効になります'),
    'the unconditioned R11 promise sentence must stay gone (ja)',
  );
}
// Legacy handleDetect auto-fill path is deleted; only handleLocalDetect remains.
assert.strictEqual(
  settingsViewSource.indexOf('async function handleDetect()'),
  -1,
  'legacy handleDetect auto-fill path must stay deleted (only handleLocalDetect may exist)',
);
const localDetectStart = settingsViewSource.indexOf('async function handleLocalDetect()');
const localDetectEnd = settingsViewSource.indexOf('function startManualLocalModel()', localDetectStart);
assert.notStrictEqual(localDetectStart, -1, 'local picker detect handler must exist');
assert.notStrictEqual(localDetectEnd, -1, 'local picker detect handler boundary must exist');
const localDetectSource = settingsViewSource.slice(localDetectStart, localDetectEnd);
// 复审 minor-14:手动添加本地模型是显式路由切换,与 applyCatalogItem 同一纪律——
// 上一个表单残留的端点探测状态(候选列表/picker/挂起批量/添加提示)必须清空,
// 绝不许在新表单下复用。切片钉住 startManualLocalModel 函数体,漏清任一项即红。
const manualLocalStart = localDetectEnd;
const manualLocalEnd = settingsViewSource.indexOf('const catalogSectionTitleClass', manualLocalStart);
assert.notStrictEqual(manualLocalEnd, -1, 'startManualLocalModel handler boundary must exist');
const manualLocalSource = settingsViewSource.slice(manualLocalStart, manualLocalEnd);
assert.match(
  manualLocalSource,
  /setEndpointListing\(null\);\s*setEndpointPickerOpen\(false\);\s*setAddAllModelsConfirm\(null\);\s*setEndpointAddResult\(null\);/,
  'manual local-model entry must drop the leftover probe state (listing, picker, pending batch, add notice) like applyCatalogItem does',
);
assert.match(
  localDetectSource,
  /customPort/,
  'local picker detect must pass the optional custom port to the discovery command',
);
assert.match(
  localDetectSource,
  /65535/,
  'local picker detect must validate the custom port range before probing',
);
// Safety invariant (moved from the deleted legacy handleDetect guard): detection
// must only record candidate rows — never auto-fill the form — so an unknown-load
// model (JIT loading can be tens of GB) is only ever added by an explicit user click.
assert.match(
  localDetectSource,
  /setLocalDetectResult\(\{ candidates:/,
  'local detection must only record candidate rows (no automatic form fill)',
);
assert.doesNotMatch(
  localDetectSource,
  /setBaseUrl\(|setModel\(/,
  'local detection must not auto-fill the form; the user adds a candidate explicitly',
);
// ── 端点模型自动发现(规格 §2.6)源码守卫 ──────────────────────────────
// 「获取模型列表」只允许显式点击触发:按钮 data-testid + onClick 直挂
// handleFetchModels;禁止 effect/挂载期自动探测(与本地检测面板同一安全不变量)。
assert.match(
  settingsViewSource,
  /data-testid="settings-model-fetch-lists"\s+onClick=\{handleFetchModels\}/,
  'fetch model lists must be an explicit-click button wired to handleFetchModels',
);
assert.doesNotMatch(
  settingsViewSource,
  /useEffect\(\(\) => \{[\s\S]{0,240}probeEndpointModels/,
  'model list probing must never run on mount/effect; explicit button click only',
);
assert.match(
  settingsViewSource,
  /typeof bridge\.models\.probeEndpointModels !== 'function' \|\| fetchingModels \|\| savingModel\) return;/,
  'fetch handler must bail when the probe bridge method is unavailable (old desktop/web builds) or a form save is in flight (r5: write-in-flight visibility)',
);
// 复审 M5(base URL 错配竞态):结果只能属于探测时的 URL。手输改 URL 必须作废
// 既有列表/picker/挂起批量;在飞探测 resolve 时发现 URL 已变也必须静默丢弃。
const baseUrlChangeStart = settingsViewSource.indexOf('function handleBaseUrlChange(');
assert.notStrictEqual(baseUrlChangeStart, -1, 'base URL change handler must exist');
const baseUrlChangeSource = settingsViewSource.slice(baseUrlChangeStart, baseUrlChangeStart + 800);
assert.match(
  baseUrlChangeSource,
  /setEndpointListing\(null\);\s*setEndpointPickerOpen\(false\);\s*setAddAllModelsConfirm\(null\);/,
  'editing the base URL must drop the probed listing, close the picker and discard any pending batch (no URL A results under URL B)',
);
assert.match(
  settingsViewSource,
  /if \(baseUrlRef\.current\.trim\(\) !== probedBaseUrl\) return;/,
  'an in-flight probe whose base URL changed mid-flight must be discarded at resolve time',
);
// 复审 M2(round-3 修订,测试连接的 resolve 时刻守卫):handleTest 快照 base
// URL,resolve 时 URL 已变整份静默丢弃(含失败结果);端点事实按**已保存记录**
// 的 model 字段探得,后端经 fact_model 回带事实所属模型——空字段填充守卫
// 对齐到 fact_model(表单模型 ≠ fact_model 时不填),不再拿表单模型跟自己比
// (旧守卫在编辑表单里未保存的 model-ID 改动下形同虚设)。切片钉住 handleTest
// 函数体:删掉任一守卫或把 applyProbedFacts 挪到 fact_model 比较之外都会变红。
const handleTestStart = settingsViewSource.indexOf('async function handleTest()');
assert.notStrictEqual(handleTestStart, -1, 'connection test handler must exist');
const handleTestSource = settingsViewSource.slice(
  handleTestStart,
  settingsViewSource.indexOf('async function handleFetchModels', handleTestStart),
);
assert.match(
  handleTestSource,
  /const probedBaseUrl = baseUrl\.trim\(\);/,
  'handleTest must snapshot the base URL at click time',
);
assert.match(
  handleTestSource,
  /if \(baseUrlRef\.current\.trim\(\) !== probedBaseUrl\) return;\s*setTestResult\(normalizeConnectionTestResult\(result, isCodingPlan\)\);/,
  'a finished connection test whose base URL moved on must be discarded entirely BEFORE setTestResult on the resolve path (same silent semantics as handleFetchModels)',
);
assert.match(
  handleTestSource,
  /const factModel = typeof result\.fact_model === 'string' \? result\.fact_model\.trim\(\) : '';/,
  'the backend hands back the saved model name the facts actually matched (fact_model) and the fill guard must read it',
);
assert.match(
  handleTestSource,
  /if \(!factModel \|\| modelRef\.current\.trim\(\) === factModel\) applyProbedFacts\(result\);/,
  'probed endpoint facts are per-saved-model: they must fill only while the live wire model still equals fact_model (the click-time form value compared to itself proved vacuous in edit mode)',
);
assert.match(
  settingsViewSource,
  /const modelRef = useRef\(model\);\s*modelRef\.current = model;/,
  'the wire model must be mirrored into a render-time ref (declaration AND the render-mirror assignment) so the async resolve compares the live value — deleting the mirror line freezes the ref at mount and silently disables the fact_model comparison (runtime-covered by ⑥.5k2b/⑥.5k2c since round-4)',
);
// 复审 M4(添加结果的可见性):endpointAddResult 升级为 { ok, text } | null——
// 失败曾是硬编码绿色,且 picker 打开时表单状态行被全屏遮罩挡住。钉四条形状:
// 状态对象、.ok 双色三目、.text 渲染、prop 透传进 picker。
assert.match(
  settingsViewSource,
  /setEndpointAddResult\(\{ ok: true, text: settingsCopy\.addedCount\(1\) \}\)/,
  'a single-row add success must record { ok: true, text } (the status line colors by .ok)',
);
assert.match(
  settingsViewSource,
  /setEndpointAddResult\(\{ ok: false, text: settingsCopy\.imageCapabilitySaveFailed/,
  'a failed row/batch save must record { ok: false, text } so the failure renders red',
);
assert.match(
  settingsViewSource,
  /endpointAddResult\.ok \? 'text-\[#137333\] dark:text-\[#93D5A6\]' : 'text-\[#FF3B30\]'/,
  'the add-result status line must color by .ok (green success / red failure), never hardcoded green',
);
assert.match(
  settingsViewSource,
  /endpointAddResult\.text/,
  'the add-result status line must render the notice text field (object shape, not a bare string)',
);
assert.match(
  settingsViewSource,
  /addNotice=\{endpointAddResult\}/,
  'the chat-line picker must receive the add notice so a failed row/batch is visible while the picker stays open',
);
// 复审 M4(提示生命周期):新一轮探测开始与关闭 picker 都必须清空添加提示,
// 旧结果绝不跨探测/跨开关残留。
assert.match(
  settingsViewSource,
  /setFetchModelsMessage\(null\);\s*setEndpointAddResult\(null\);/,
  'a new fetch must clear the previous add notice (stale result never survives into a fresh probe)',
);
assert.match(
  settingsViewSource,
  /onClose=\{\(\) => \{ setEndpointPickerOpen\(false\); setEndpointAddResult\(null\); \}\}/,
  'closing the picker must clear the add notice so a reopen starts clean',
);
// 复审 M4(picker 内横幅):失败原因在 picker 行列表上方就地可见。
assert.match(
  endpointPickerSource,
  /data-testid="endpoint-model-add-notice"/,
  'the picker must render the add notice as an in-panel banner above the rows (the masked form status line is not enough)',
);
assert.match(
  endpointPickerSource,
  /addNotice\.ok \? 'text-\[#137333\] dark:text-\[#93D5A6\]' : 'text-\[#FF3B30\]'/,
  'the in-picker banner must color by .ok too (red failure while the picker stays open for retry)',
);
// 复审 picker minor:picker 按探测序号重挂(key={endpointFetchSeq}),新列表上
// filterDraft/滚动干净复位。
assert.match(
  settingsViewSource,
  /<EndpointModelPicker\s+key=\{endpointFetchSeq\}/,
  'the endpoint picker must remount per fetch so filter draft and scroll reset with each new listing',
);
// round-3 审 M3(单行添加双击重复落库):handleEndpointModelPick 需要自己的
// 在飞守卫——ref 同步挡同一 tick 的第二次点击,state 驱动 picker 行按钮禁用
// (保存 IPC + 刷新链路的窗口内不给可点的第二下);picker 行按钮按 pickBusy
// 禁用。round-4 M4 补两个交错闭口:单行添加在批量确认挂起时禁入(旧快照),
// 批量入口在单行保存在飞时禁入(快照会把在飞行当未保存)。
const pickStart = settingsViewSource.indexOf('async function handleEndpointModelPick(row)');
assert.notStrictEqual(pickStart, -1, 'single-row endpoint pick handler must exist');
const pickSource = settingsViewSource.slice(pickStart, settingsViewSource.indexOf('async function handleAddAllEndpointModels()', pickStart));
assert.match(
  pickSource,
  /if \(pickInFlightRef\.current \|\| savingModel \|\| addingAllModels \|\| addAllModelsConfirm\) return;/,
  'the pick handler must re-entry-guard on a dedicated in-flight ref AND refuse while a batch confirm is pending (the pending snapshot predates the single-row save — confirming afterwards would double-save that row; round-4 review M4)',
);
assert.match(
  pickSource,
  /pickInFlightRef\.current = true;\s*setPickSavingModel\(true\);/,
  'the pick handler must raise both the sync ref (same-tick double click) and the busy state (row-button disable) before the await',
);
assert.match(
  pickSource,
  /finally \{[\s\S]{0,120}pickInFlightRef\.current = false;[\s\S]{0,120}setPickSavingModel\(false\);[\s\S]{0,120}\}/,
  'the in-flight flags must clear on BOTH the success and failure paths so a retry is possible (the registry unregister may sit alongside them — review P2×2)',
);
assert.match(
  settingsViewSource,
  /const pickInFlightRef = useRef\(false\);/,
  'the pick in-flight flag must be a ref (state alone cannot catch a same-tick double click)',
);
assert.match(
  settingsViewSource,
  /pickBusy=\{pickSavingModel \|\| savingModel \|\| !!localAddBusyKey\}/,
  'the chat-line picker must receive the pick-busy prop',
);
assert.match(
  endpointPickerSource,
  /disabled=\{alreadySaved \|\| pickBusy\}/,
  'the row add button must disable while a pick-save is in flight (round-3 review M3)',
);
assert.match(
  endpointPickerSource,
  /disabled=\{loading \|\| pickBusy \|\| addableCount === 0\}/,
  'the batch add-all button must disable while a single-row save is in flight (a dedup snapshot taken then counts the in-flight row as unsaved and batch-saves it a second time, round-4 review M4)',
);
// round-3 审 minor(提示失效清扫):URL 变更/目录切换/手动本地路由三个失效点
// 都要清探测失败提示,旧 URL 的红色失败提示不许挂在新表单下。
const baseUrlHandlerStart = settingsViewSource.indexOf('function handleBaseUrlChange(value)');
const baseUrlHandlerSource = settingsViewSource.slice(baseUrlHandlerStart, settingsViewSource.indexOf('function applyProbedFacts(', baseUrlHandlerStart));
assert.match(
  baseUrlHandlerSource,
  /setFetchModelsMessage\(null\);/,
  'a base-URL change must clear the stale fetch-failure message (round-3 review minor)',
);
const catalogItemHandlerSource = settingsViewSource.slice(
  settingsViewSource.indexOf('function applyCatalogItem(group, item)'),
  settingsViewSource.indexOf('function handleModelIdChange(value)'),
);
assert.match(
  catalogItemHandlerSource,
  /setFetchModelsMessage\(null\);/,
  'a catalog switch must clear the stale fetch-failure message (round-3 review minor)',
);
const manualLocalHandlerSource = settingsViewSource.slice(
  settingsViewSource.indexOf('function startManualLocalModel()'),
  settingsViewSource.indexOf('const catalogSectionTitleClass', settingsViewSource.indexOf('function startManualLocalModel()')),
);
assert.match(
  manualLocalHandlerSource,
  /setFetchModelsMessage\(null\);/,
  'the manual-local route switch must clear the stale fetch-failure message (round-3 review minor)',
);
// round-3 审 minor(获取失败提示的 resolve 时刻守卫):handleFetchModels 的
// catch 与 try 同一 URL 守卫——URL A 的探测失败不许在 URL B 下渲染。
const fetchHandlerSource = settingsViewSource.slice(
  settingsViewSource.indexOf('async function handleFetchModels()'),
  settingsViewSource.indexOf('function buildEndpointModelPayload(row)'),
);
assert.match(
  fetchHandlerSource,
  /catch \(e\) \{[\s\S]*?if \(baseUrlRef\.current\.trim\(\) !== probedBaseUrl\) return;[\s\S]*?setFetchModelsMessage\(/,
  'the fetch-models reject path must carry the same resolve-time URL guard as its try path (and as handleTest)',
);
// round-3 审 minor(手输模型 ID 换模型):手动改 model 字段要清自动填入小字
// (与目录项切换/chooseModel 同一纪律),旧模型的提示不许挂在新模型下。
const modelIdHandlerSource = settingsViewSource.slice(
  settingsViewSource.indexOf('function handleModelIdChange(value)'),
  settingsViewSource.indexOf('function handleBaseUrlChange(value)'),
);
assert.match(
  modelIdHandlerSource,
  /setFilledFromEndpoint\(false\);/,
  'a manual model-ID edit must clear the filled-from-endpoint hint (explicit model switch)',
);
// 测试连接自动填入:仅空字段、绝不覆盖非空值(规格决策 4)。取 applyProbedFacts
// 片段钉条件,把「填空条件取反」的突变变红。复审 M6:空字段判定必须读
// latestFieldsRef(resolve 时刻的最新输入),而不是点击瞬间捕获的闭包旧值——
// 正则钉住 ref 读取形状,删掉空检查或回退到闭包值都会变红。
const probedFactsStart = settingsViewSource.indexOf('function applyProbedFacts(');
assert.notStrictEqual(probedFactsStart, -1, 'endpoint fact auto-fill helper must exist');
const probedFactsSource = settingsViewSource.slice(probedFactsStart, probedFactsStart + 900);
assert.match(
  settingsViewSource,
  /const latestFieldsRef = useRef\(\{ contextWindow, maxOutput \}\);/,
  'the empty-field judgment must read a latest-values ref (async resolve sees current input, not click-time closure)',
);
assert.match(
  probedFactsSource,
  /!latestFieldsRef\.current\.contextWindow && Number\.isFinite\(probedContext\) && probedContext > 0/,
  'context auto-fill must stay gated on an EMPTY context field read from the latest-values ref (never overwrite, never stale-closure)',
);
assert.match(
  probedFactsSource,
  /!latestFieldsRef\.current\.maxOutput && Number\.isFinite\(probedOutput\) && probedOutput > 0/,
  'max output auto-fill must stay gated on an EMPTY max output field read from the latest-values ref (never overwrite, never stale-closure)',
);
// round-4 #851 收口 + m1-2(server_kind 闸):picker 添加的是全新记录
// (fresh makeModelId()),但保存载荷必须与 doSave 同一口径——
// - 凭据阶梯直接复用 cloudCredentialAction(),api_key 按 noAuthActive 口径
//   丢弃:免鉴权声明生效时 picker 添加的记录不得再带 key/凭据要求(chat 门
//   否则会被声明外的凭据需求挡住);
// - auth_mode 随载荷落库、alias 与表单保存同为 null(字段对齐);
// - listing 带 server_kind(serde snake_case)时 ollama / lm_studio 行的
//   context_window_tokens 是运行时生效值(/api/ps 生效窗口、
//   loaded_context_length),与 buildLocalModelPayload 同一教义只存 null——
//   行内徽章照常显示,仅落库被闸(声明会永久 min-clamp 后续探测)。
const endpointPayloadStart = settingsViewSource.indexOf('function buildEndpointModelPayload(row, overrides)');
assert.notStrictEqual(endpointPayloadStart, -1, 'endpoint row payload builder must exist');
const endpointPayloadSource = settingsViewSource.slice(
  endpointPayloadStart,
  settingsViewSource.indexOf('async function handleEndpointModelPick(row)', endpointPayloadStart),
);
assert.match(
  endpointPayloadSource,
  /const nextKeyAction = isLocalPreset\s*\? \(localKeyEnabled && apiKey\.trim\(\) \? 'replace' : 'keep_existing'\)\s*: cloudCredentialAction\(\);/,
  'the picker row payload must reuse the exact doSave credential ladder (cloudCredentialAction) so a keyless declaration deletes/replaces stored credentials the same way a form save does',
);
assert.match(
  endpointPayloadSource,
  /: \(!noAuthActive && apiKey\.trim\(\) \? apiKey\.trim\(\) : ''\)/,
  'the picker row payload must drop the typed key under the keyless declaration (same noAuthActive caliber as doSave, never the raw noAuth)',
);
assert.match(
  endpointPayloadSource,
  /alias: null,/,
  'picker-added records must carry the same null-alias default as a form save (payload field parity)',
);
assert.match(
  endpointPayloadSource,
  /auth_mode: !isLocalPreset && !isCodingPlan && noAuth \? 'none' : null/,
  'the picker row payload must persist the keyless declaration via auth_mode (otherwise a picker-added record re-demands credentials at the chat gate despite the form declaring the endpoint keyless)',
);
assert.match(
  endpointPayloadSource,
  /const serverKind = endpointListing && typeof endpointListing === 'object' \? endpointListing\.server_kind : null;/,
  'the runtime-effective context gate must read server_kind off the listing the picker received (serde snake_case, no prop threading needed)',
);
assert.match(
  endpointPayloadSource,
  /const runtimeProbedLane = serverKind === 'ollama' \|\| serverKind === 'lm_studio';/,
  'the runtime-effective context lane must be exactly ollama/lm_studio (same set as buildLocalModelPayload)',
);
assert.match(
  endpointPayloadSource,
  /context_window_tokens: runtimeProbedLane \? null : row\.contextWindowTokens,/,
  'picker rows on ollama/lm_studio must persist a null context declaration (the row still DISPLAYS the probed value; only persistence is gated) so it cannot min-clamp later probes',
);
// 批量「全部添加」:应用内 ConfirmDialog 是硬闸门(规格决策 1;原生
// window.confirm 在 Tauri WebView2 不渲染,见 tests/settings_window_confirm.test.mjs),
// 必须先记录挂起批次、确认后才进入逐行保存循环;同时钉去重截断文案来源
// (splitBatchAdd/addAllTruncated)。
const addAllStart = settingsViewSource.indexOf('async function handleAddAllEndpointModels()');
assert.notStrictEqual(addAllStart, -1, 'endpoint add-all handler must exist');
const addAllEnd = settingsViewSource.indexOf('function normalizeImageCapabilityTestResult(', addAllStart);
assert.notStrictEqual(addAllEnd, -1, 'endpoint add-all handler boundary must exist');
const addAllSource = settingsViewSource.slice(addAllStart, addAllEnd);
assert.match(addAllSource, /splitBatchAdd\(/, 'batch add must dedupe/cap through splitBatchAdd (model id + base_url)');
assert.doesNotMatch(
  addAllSource,
  /window\.confirm\s*\(/,
  'batch add must not call the native confirm (does not render in Tauri WebView2)',
);
const confirmHandlerIndex = addAllSource.indexOf('async function confirmAddAllEndpointModels()');
assert.ok(confirmHandlerIndex !== -1, 'the confirmed batch-add handler must exist');
const addAllEntry = addAllSource.slice(0, confirmHandlerIndex);
assert.match(
  addAllEntry,
  /setAddAllModelsConfirm\(\{ addable, truncated, baseUrl: baseUrl\.trim\(\), preset \}\)/,
  'the entry handler must only record the pending batch (with its snapshot base URL/preset, round-5 m1) in the in-app confirm state',
);
assert.doesNotMatch(addAllEntry, /onSave\(/, 'no save may run before the in-app confirm');
assert.match(
  addAllEntry,
  /if \(addingAllModels \|\| pickInFlightRef\.current \|\| pickSavingModel \|\| savingModel \|\| !endpointListing\) return;/,
  'the add-all entry must refuse while a single-row add is in flight (its dedup snapshot would count the in-flight row as unsaved and the confirmed batch would save it a second time, round-4 review M4) — removing any of the in-flight clauses must go red',
);
const addAllConfirmBody = addAllSource.slice(confirmHandlerIndex);
assert.match(
  addAllConfirmBody,
  /for \(const row of addable\)/,
  'the batch save loop must run only in the confirmed path (confirm first, saves after)',
);
// 复审 M9(批量 IPC 风暴):确认批量的逐行保存必须带 deferRefresh 跳过桥内
// 逐行三连 reload,循环后(含失败路径)统一 reloadSavedModels 一次;旧行为
// (逐行全量刷新)只允许在桥缺 reloadSavedModels 时作为回退出现。
assert.match(
  addAllConfirmBody,
  /onSave\(buildEndpointModelPayload\(row, \{ baseUrl: confirmBaseUrl, preset: confirmPreset \}\), batchReloadAvailable \? \{ deferRefresh: true \} : undefined\)/,
  'batch rows must save with deferRefresh against the confirmed snapshot URL so each row skips the three reloads (single reload after the loop; round-5 m1 snapshot)',
);
assert.match(
  addAllConfirmBody,
  /await bridge\.models\.reloadSavedModels\(\)/,
  'the confirmed batch must refresh saved models exactly once via reloadSavedModels after the loop',
);
// 复审 M9(忙态闸门):批量进行中确认弹窗双按钮禁用,防重复提交/中途取消。
assert.match(
  settingsViewSource,
  /confirmLabel=\{addingAllModels \? settingsCopy\.saving : settingsCopy\.addAllModels\}/,
  'the add-all confirm must switch to the busy label while the batch saves',
);
assert.match(
  settingsViewSource,
  /busy=\{addingAllModels\}/,
  'the add-all confirm must disable both buttons while the batch saves',
);
// 确认弹窗本体:同一 ConfirmDialog stacked 配方(#817 B1),testid 供 smoke 驱动;标题/描述
// 仍表达规格语义(数量、去重、截断)。
const addAllDialogIndex = settingsViewSource.indexOf('testid="add-all-models-confirm"');
assert.ok(addAllDialogIndex !== -1, 'the add-all confirm dialog must exist');
const addAllDialogSource = settingsViewSource.slice(Math.max(0, addAllDialogIndex - 400), addAllDialogIndex + 700);
assert.match(addAllDialogSource, /<ConfirmDialog/, 'the add-all confirm must reuse the shared ConfirmDialog (#817 B1)');
assert.match(addAllDialogSource, /addAllConfirm\(/, 'the dialog title must carry the deduped count and dedup notice');
assert.match(addAllDialogSource, /addAllTruncated\(/, 'oversized listings must surface the truncation notice in the confirm');
assert.match(addAllDialogSource, /onConfirm=\{confirmAddAllEndpointModels\}/, 'the dialog OK button must route into the confirmed batch handler');
assert.match(addAllDialogSource, /onCancel=\{\(\) => setAddAllModelsConfirm\(null\)\}/, 'cancel must discard the pending batch without saving');
// 表单保存加入同一 (model id + base_url) 身份契约:逐行/批量添加都按它去重,
// 唯独 doSave 此前不查——探测「全部添加」把端点行(带真实长度事实)落库后
// 再点「保存」,同一条身份会以两个新 id 落库两次(用户实测 deepseek-flash
// 双条目)。守卫禁用保存 + 行内提示 + doSave 早退;编辑态必须排除自身 id。
assert.match(
  settingsViewSource,
  /const formSavedKeys = savedModelKeys\(\(models \|\| \[\]\)\.filter\(saved => saved && saved\.id !== initial\.id\)\);/,
  'the form-save duplicate guard must fold the saved list minus the record being edited (self-exclusion keeps the edit flow saveable) — removing the id filter must go red',
);
assert.match(
  settingsViewSource,
  /const duplicateSaved = formSavedKeys\.has\(formIdentityKey\) \|\| pendingSavedKeys\.has\(formIdentityKey\)\s*\n\s*\|\| \(formIdentityKey !== editingSavedKey && !!frozenListed && frozenListed\.has\(formIdentityKey\)\);/,
  'the form-save duplicate guard must reuse the picker dedup key (modelDuplicationKey, same case/slash/ollama folding as splitBatchAdd) and fold the read-failure frozen mirror (review P2: an emptied list must not retract confirmed identities; the edit-mode own key stays excluded)',
);
assert.match(
  settingsViewSource,
  /if \(!canSave \|\| savingModel \|\| duplicateSaved \|\| localAddInFlightRef\.current\) return;/,
  'doSave must early-return on a duplicated (model id + base_url) identity or a pending local row add — belt-and-braces behind the disabled button so no invocation path can double-book',
);
assert.match(
  settingsViewSource,
  /const rowKey = modelDuplicationKey\(row\.model, row\.base_url\);[\s\S]{0,300}const rowSaved = endpointSavedKeys\.has\(rowKey\) \|\| inFlightModelCreates\.has\(rowKey\) \|\| pendingSavedKeys\.has\(rowKey\)\s*\|\| \(inFlightModelCreates\.listed && inFlightModelCreates\.listed\.has\(rowKey\)\);/,
  'local-detect row adds must reuse the picker dedup key over the same saved set, PLUS the module-level in-flight registry (review P2×2: a pending save from ANY entry — surviving a modal remount — must disable the row the same way a saved identity does)',
);
// 复审 P2×2(跨入口在途互斥):模块级注册表是唯一跨弹窗重开存活的在途
// 身份簿——四个创建入口(doSave/本地行/端点单行/批量确认循环)都必须查册
// +入册+出册;行为层的重开禁用由 ⑥.5b5/b6 的浏览器用例钉住。
assert.match(
  settingsViewSource,
  /const inFlightModelCreates = new Set\(\);/,
  'the module-level in-flight identity registry must exist (refs die with the modal instance — only a module store survives a settings reopen)',
);
assert.match(
  settingsViewSource,
  /const formIdentity = modelDuplicationKey\(model\.trim\(\), baseUrl\.trim\(\)\);\s*if \(inFlightModelCreates\.has\(formIdentity\)\) return;/,
  'doSave must refuse an identity already in flight from another entry (endpoint pick/batch/local row) — the ref-only shape cannot see across entries or remounts',
);
assert.match(
  settingsViewSource,
  /const rowIdentity = modelDuplicationKey\(row\.model, row\.base_url\);\s*if \(inFlightModelCreates\.has\(rowIdentity\)\) return;/,
  'the local row add must refuse an identity already in flight and register its own until the save settles',
);
assert.match(
  settingsViewSource,
  /const pickIdentity = modelDuplicationKey\(row\.id, baseUrl\.trim\(\)\);\s*if \(inFlightModelCreates\.has\(pickIdentity\)\) return;/,
  'the endpoint pick must refuse an identity already in flight (the local-save-pending → endpoint re-add repro) and register its own — note rows key on row.id, the payload model field',
);
assert.match(
  settingsViewSource,
  /if \(inFlightModelCreates\.has\(rowIdentity\) \|\| inFlightModelCreates\.listed\.has\(rowIdentity\)\s*\|\| inFlightModelCreates\.committed\.has\(rowIdentity\)\s*\|\| endpointSavedKeys\.has\(rowIdentity\) \|\| pendingSavedKeys\.has\(rowIdentity\)\) continue;/,
  'the confirmed batch loop must skip rows whose identity another entry put in flight while the dialog was open',
);
// r3 复审补全(刷新失败分支 + 在飞禁关层 + PR 声明两钉):查册在落库落定时
// 出册,而 loadModels 会吞 list_models 失败并清空列表——已提交未确认的身份
// 由模块 committed 簿继续拦截,列表确认后 effect 出册(r5 前由父层 state
// 派生解除);Escape/X 在飞禁关层。
assert.match(
  settingsViewSource,
  /const \[, setJustSavedKeys\] = useState\(\(\) => new Set\(\)\);/,
  'the parent commit-mark state must exist as the render driver only (value elided) — the authority is the module-level committed book; the setState flushes blocked visuals in the same commit as the save',
);
assert.match(
  settingsViewSource,
  /inFlightModelCreates\.committed = new Set\(\);\s*inFlightModelCreates\.absentRetirable = new Set\(\);\s*inFlightModelCreates\.listed = new Set\(\);/,
  'the module-level committed-unlisted book, its absence-floor marker set and the live listed mirror must live on the registry (instance props survive modal remounts and settings reopen; top-level bindings are chunk-boundary hazards)',
);
assert.match(
  settingsViewSource,
  /inFlightModelCreates\.committed\.add\(key\);[\s\S]{0,420}setJustSavedKeys\(prev => \{/,
  'markSaveCommitted must record the identity in the LIVE module book before flushing the render — an unwired book left the refresh-failure branch blind to the batch loop (r5 review: the declared book was never populated)',
);
assert.match(
  settingsViewSource,
  /if \(opts && opts\.absentRetirable\) inFlightModelCreates\.absentRetirable\.add\(key\);/,
  'the absence floor must be granted per booking through markSaveCommitted — only keys whose save chain ran the reload tail may retire on a later absent list (review P2×2 late-completion; deferred batch keys keep the present-only rule)',
);
assert.match(
  settingsViewSource,
  /inFlightModelCreates\.listed = listed;\s*if \(failed\) return;\s*let retired = false;\s*for \(const key of \[\.\.\.inFlightModelCreates\.committed\]\) \{[\s\S]{0,1100}if \(!listed\.has\(key\) && !inFlightModelCreates\.absentRetirable\.has\(key\)\) continue;\s*inFlightModelCreates\.committed\.delete\(key\);\s*inFlightModelCreates\.absentRetirable\.delete\(key\);\s*retired = true;\s*\}[\s\S]{0,700}if \(retired\) setJustSavedKeys\(prev => new Set\(prev\)\);\s*\}\);/,
  'the SettingsView effect must refresh the live listed mirror and retire committed keys on a successful list — present (confirmed, ⑦.8) or absent with the floor (deleted mid-tail, review P2×2: the save chain refresh is a post-upsert read under last-write-wins, so a later successful list lacking the identity means deletion; a stranded reservation would block the legitimate re-add until reload) — with the failed-read freeze preceding both; a retirement must flush one bounded re-render (the retiring commit\'s derived pendingSavedKeys still carries the pre-retirement book, so without the flush the consuming rows stay disabled forever — the r10 absent+floor path proved this live)',
);
assert.match(
  settingsViewSource,
  /const failed = !!\(bs && bs\.modelsListLoadFailed\);\s*const source = failed\s*\? \(\(bs && bs\.lastConfirmedModels\) \|\| \[\]\)\.filter\(model => model && model\.id\)\s*: userModels;\s*const listed = savedModelKeys\(source\);/,
  'the mirror effect must freeze while the models list read is failed AND seed that freeze from the shared last-confirmed snapshot (review P2: the shared catch clears the list, so refreshing the mirror from that empty list retracts confirmed identities; and a failure BEFORE the first Settings mount starts from a never-populated mirror — only the shared last-successful read can seed the first-open guards; ⑦.9/⑦.9e)',
);
// The shared loader must own the last-successful snapshot: success replaces
// it (real deletion refresh keeps the deleted model re-addable) and failure
// retains it while clearing the displayed list.
assert.match(
  fs.readFileSync(path.join(__dirname, '..', 'src', 'shared', 'bridge-shared-helpers.js'), 'utf8'),
  /state\.savedModels = \(v && v\.models\) \|\| \[\];\s*state\.lastConfirmedModels = \[\.\.\.\(\(v && v\.models\) \|\| \[\]\)\];[\s\S]{0,1100}state\.savedModels = \[\]; state\.activeModelId = null;\s*state\.modelsListLoadFailed = true;/,
  'the shared loadModels must retain lastConfirmedModels across a failed read and replace it on the next successful read (review P2 pre-mount boundary; ⑦.9e)',
);
assert.match(
  settingsViewSource,
  /for \(const k of pendingSavedKeys\) keys\.add\(k\);[\s\S]{0,600}for \(const k of inFlightModelCreates\.listed\) keys\.add\(k\);\s*return keys;/,
  'the endpoint picker savedKeys must fold the frozen confirmed identities (review P2 r7: a failed read empties endpointSavedKeys while the mirror keeps the confirmed set — the rows must not show a confirmed identity as addable; ⑦.9c)',
);
assert.match(
  settingsViewSource,
  /if \(inFlightModelCreates\.listed\.has\(pickIdentity\)\) return;/,
  'the endpoint pick entry must mirror the row view on the write side (review P2 r7: the belt rejects frozen confirmed identities so a stale dispatch cannot mint a second id behind the disabled row; ⑦.9c)',
);
assert.match(
  settingsViewSource,
  /for \(const key of inFlightModelCreates\.listed\) batchSavedKeys\.add\(key\);/,
  'the add-all snapshot must fold the frozen mirror so the confirm dialog count agrees with the loop book (review P2 r7; ⑦.9c)',
);
assert.match(
  settingsViewSource,
  /const pendingSavedKeys = new Set\(\[\.\.\.inFlightModelCreates\.committed\]\.filter\(key => !endpointSavedKeys\.has\(key\)\)\);/,
  'just-committed identities must fold from the LIVE module book (the effect retires confirmed keys) — deriving from an add-only state Set resurrects deleted identities as pending forever (r5 review P2-2, live-proven: save→confirm→delete→re-add stayed blocked with a false already-exists hint)',
);
assert.match(
  settingsViewSource,
  /onSaveCommitted=\{markSaveCommitted\} \/>/,
  'the parent must hand the modal the commit mark — each write path registers its identity only after the save resolves (failure paths stay retryable); the book itself travels through the module, not props',
);
assert.match(
  settingsViewSource,
  /const saveBlockedInFlight = inFlightModelCreates\.has\(formIdentityKey\) \|\| !!localAddBusyKey;/,
  'the form Save button must fold the same in-flight signals the doSave guard checks (registry hit from another entry, pending local row add) — the guard silently swallowed clicks while the button looked clickable (r4 review MINOR)',
);
assert.match(
  settingsViewSource,
  /disabled=\{!canSave \|\| savingModel \|\| duplicateSaved \|\| saveBlockedInFlight\}/,
  'the Save disabled state must include the in-flight fold — a dead click on an enabled button is the failure mode',
);
assert.match(
  settingsViewSource,
  /const addAllInFlightRef = useRef\(false\);/,
  'the batch confirm dispatch needs a synchronous latch — addingAllModels is state and a same-tick second dispatch reads it stale (r4 review MINOR: added count split, notices overwritten)',
);
assert.match(
  settingsViewSource,
  /if \(!addAllModelsConfirm \|\| addingAllModels \|\| addAllInFlightRef\.current\) return;\s*addAllInFlightRef\.current = true;/,
  'confirmAddAllEndpointModels must lead with the sync latch and set it before any await',
);
assert.match(
  settingsViewSource,
  /for \(const rowIdentity of batchIdentities\) inFlightModelCreates\.delete\(rowIdentity\);\s*addAllInFlightRef\.current = false;/,
  'the batch latch must release in the finally (success and failure alike)',
);
assert.match(
  settingsViewSource,
  /probeEndpointModels !== 'function' \|\| fetchingModels \|\| savingModel\) return;/,
  'the fetch-models entry must refuse while a form save is in flight — write-in-flight visibility invariant (r4 review NIT)',
);
assert.match(
  appMainSource,
  /onSaveModel=\{\(m, options\) => \{\s*if \(!bridge\.available\) throw new Error\('bridge unavailable'\);\s*return bridge\.models\.saveModel\(m, options\);\s*\}\}/,
  'onSaveModel must throw when the bridge is down — a false resolution was treated as a committed save (modal closed, book marked, batch counted) for a save that never happened (r4 review MINOR)',
);
assert.match(
  settingsViewSource,
  /const outcome = await onSave\(\{[\s\S]{0,2400}bookSaveOutcome\(formIdentity, outcome\);/,
  'doSave must route its settled save through the chain-end verdict — booking an identity the tail list no longer contains strands a reservation no later list can confirm (review P2×2 late-completion) — and otherwise register it as committed-unlisted with the absence floor (the bridge refresh can silently fail and the empty list must not invite a re-add)',
);
assert.match(
  settingsViewSource,
  /const outcome = await onSave\(buildLocalModelPayload\(row\)\);[\s\S]{0,300}bookSaveOutcome\(rowIdentity, outcome\);/,
  'the local row add must route its settled save through the chain-end verdict before booking',
);
assert.match(
  settingsViewSource,
  /const outcome = await onSave\(buildEndpointModelPayload\(row\)\);[\s\S]{0,300}bookSaveOutcome\(pickIdentity, outcome\);/,
  'the endpoint pick must route its settled save through the chain-end verdict before booking',
);
assert.match(
  settingsViewSource,
  /const outcome = await onSave\(buildEndpointModelPayload\(row, \{ baseUrl: confirmBaseUrl, preset: confirmPreset \}\), batchReloadAvailable \? \{ deferRefresh: true \} : undefined\);[\s\S]{0,300}bookSaveOutcome\(rowIdentity, outcome\);\s*added \+= 1;/,
  'the batch loop must register each landed row as committed-unlisted — deferred rows book without the floor (the closing reload may not have run), the no-reload fallback books through the full chain verdict; a failed closing reload keeps landed prefix rows blocked until listed',
);
assert.match(
  settingsViewSource,
  /const bookSaveOutcome = \(key, outcome\) => \{\s*if \(outcome && outcome\.refresh === 'ok' && outcome\.models\s*&& !savedModelKeys\(outcome\.models\)\.has\(key\)\) return;\s*onSaveCommitted\(key, \{[\s\S]{0,700}absentRetirable: !!\(outcome && \(outcome\.refresh === 'ok' \|\| outcome\.refresh === 'failed'\)\),\s*\}\);\s*\};/,
  'the verdict gate must skip booking exactly when the successful accepted tail list lacks the identity (deleted mid-tail) and grant the absence floor for every chain that ran the reload tail including superseded ones — the chain read handed its seq after the upsert completed, so any later accepted read reflects a post-upsert world and absence there is deletion; only deferred batches and verdict-less legacy bridges book present-only',
);
// Bridge-side chain-end verdict (review P2×2 + r10 P2 + r12 P2): the verdict
// is a four-arm contract over the save chain's own loadModels ACCEPTANCE —
// ok (accepted response carries the confirmed post-upsert list), failed
// (accepted failure, book without evidence), superseded (a newer pending
// loadModels discarded our response — last-write-wins publishes nothing, so
// chain-end state can still be the pre-upsert accepted write and absence
// there is NOT deletion), and deferred (batch, no tail). The accepted
// evidence must come from the read's own response (tail.models), never from
// the mutable state at chain end. The superseded classification reads LIVE
// accepted-publication evidence at chain completion (r12: the discarded
// return's published* snapshot cannot see publications accepted during the
// remaining refresh, and the original schedule — N discarded first, N+1 and
// the delete publishing inside the tail — still stranded the reservation).
const verdictArmPin = src => /if \(options && options\.deferRefresh\) return \{ refresh: 'deferred' \};[\s\S]{0,3200}const tail = await loadModels\(\);\s*const superseded = !tail \|\| !tail\.accepted;\s*const listFailed = !superseded && tail\.failed === true;[\s\S]{0,700}if \(superseded\) \{\s*const live = readPublishedModelsEvidence\(\);\s*if \(live\.seq > tail\.requestedSeq && !live\.failed\) \{\s*return \{ refresh: 'ok', models: live\.models \};\s*\}\s*return \{ refresh: 'superseded' \};\s*\}\s*return listFailed \? \{ refresh: 'failed' \} : \{ refresh: 'ok', models: tail\.models \};/.test(src);
assert.equal(
  verdictArmPin(fs.readFileSync(path.join(__dirname, '..', 'src', 'platform', 'tauri', 'bridge', 'settings.js'), 'utf8')),
  true,
  'the tauri saveModel must return the generation-gated chain-end verdict — deferred / ok (tail.models, the accepted post-upsert response) / failed (book without evidence) / superseded-with-live-generation-classification (a newer pending loadModels discarded our read; the classification reads the shared closure CURRENT accepted state at chain completion — live.seq > requestedSeq means a confirmed post-upsert publication whose absence upgrades the verdict to ok per the r11/r12 review schedules; a failed or older publication stays superseded present-only)',
);
assert.equal(
  verdictArmPin(fs.readFileSync(path.join(__dirname, '..', 'src', 'platform', 'web', 'bridge.js'), 'utf8')),
  true,
  'the web saveModel must return the same four-arm chain-end verdict contract as the tauri lane',
);
// Canonical identity fold (review P2: full-URL → canonical list → delete →
// re-add): the Rust persistence arm strips the /chat/completions suffix before
// the record is ever listed, so the frontend dedup key must fold identically
// or a raw-form booking never matches its own canonical record.
assert.match(
  fs.readFileSync(path.join(__dirname, '..', 'src', 'features', 'settings', 'endpoint-model-picker-logic.js'), 'utf8'),
  /if \(text\.toLowerCase\(\)\.endsWith\('\/chat\/completions'\)\) \{\s*text = text\.slice\(0, -'\/chat\/completions'\.length\);\s*\}\s*return text\.toLowerCase\(\);/,
  'normalizeBaseUrlKey must strip the /chat/completions suffix exactly like the Rust persistence arm (strip_chat_completions_suffix) — a raw-form URL and its canonical listed record must share one dedup key',
);
assert.match(
  settingsViewSource,
  /for \(const key of inFlightModelCreates\.committed\) batchSavedKeys\.add\(key\);\s*for \(const key of pendingSavedKeys\) batchSavedKeys\.add\(key\);/,
  'the add-all snapshot must exclude committed-but-unlisted identities, reading the LIVE module book (a swallowed list_models refresh makes the rendered fold and the live mirror blind — only the committed book still knows; r3 review: the empty list invited re-adding everything)',
);
assert.match(
  settingsViewSource,
  /for \(const k of pendingSavedKeys\) keys\.add\(k\);[\s\S]{0,600}return keys;/,
  'the endpoint picker saved-tag set must fold committed-but-unlisted identities — the endpoint row of a settled-but-unlisted identity must not look addable',
);
assert.match(
  settingsViewSource,
  /data-testid="model-form-duplicate-hint" role="status"/,
  'the duplicate hint must render as a passive status live region (the PR body claims this pin; none existed before r3)',
);
assert.match(
  settingsViewSource,
  /\{duplicateSaved && !savingModel && \(/,
  'the duplicate hint must hide while a save is in flight — the mid-save models refresh flips duplicateSaved and would flash a misleading “already exists” (r3 lane F: the M9 mutant survived the previous suite, the gate was behaviorally and statically unpinned)',
);
assert.match(
  settingsViewSource,
  /else if \(endpointPickerOpen && !addingAllModels\s+&& !pickInFlightRef\.current && !localAddInFlightRef\.current\) \{\s*setEndpointPickerOpen\(false\); setEndpointAddResult\(null\);/,
  'Escape must not dismiss the endpoint picker while a write is in flight (single-row pick / pending local row add / batch loop) — closing hides the in-flight write and exposes the pre-filled form Save (r3 review lane B: the old gate only covered the confirm-dialog layer)',
);
assert.match(
  endpointPickerSource,
  /data-testid="endpoint-model-picker-close" onClick=\{onClose\}\s*\n\s*aria-label=\{settingsCopy\.modelListClose\}\s*\n\s*disabled=\{pickBusy\}/,
  'the picker X must not dismiss the listing while a row-add save is in flight — dismissing hides the in-flight write (r3 review: the X had no guard)',
);
assert.match(
  settingsViewSource,
  /disabled=\{!!busyAny \|\| saved\}/,
  'the local-detect row add buttons must disable while ANY row add is in flight (the handler guard is global, so the visual must match it) and when the identity is already saved',
);
assert.match(
  settingsViewSource,
  /busyAny=\{localAddBusyKey\}/,
  'the global in-flight busy key must drive the row disable (not a per-row key — the handler blocks every row during flight, the visual must say so)',
);
assert.match(
  settingsViewSource,
  /\{saved \? settingsCopy\.modelAlreadySaved : settingsCopy\.add\}/,
  'an already-saved local candidate row must read 已保存 instead of a dead 添加 button',
);
assert.match(
  settingsViewSource,
  /if \(localAddInFlightRef\.current \|\| savingModel \|\| pickInFlightRef\.current \|\| addingAllModels \|\| addAllModelsConfirm\) return;/,
  'handleLocalModelAdd must lead its guard with the sync in-flight ref and join the sibling save-path mutex (review r2 live proof: with a busy-key state check alone, two native clicks 80 ms apart both landed onSave and minted two ids for one identity)',
);
assert.match(
  settingsViewSource,
  /const localAddInFlightRef = useRef\(false\);/,
  'the local row-add in-flight latch must be a sync ref (state commits lose the race against fast successive dispatches — the round-3 M3 pickInFlightRef lesson)',
);
assert.match(
  settingsViewSource,
  /localAddInFlightRef\.current = true;/,
  'the local row-add latch must engage synchronously before the await',
);
assert.match(
  settingsViewSource,
  /localAddInFlightRef\.current = false;/,
  'the local row-add latch must clear on settle — a stuck latch silently blocks every later row add and the form save',
);
assert.match(
  settingsViewSource,
  /data-testid="local-model-add-error" role="alert"/,
  'a failed local row add must surface an alert on the picker view itself — saveError renders only in the form body, which the picker early return never reaches (review r2 P2: the failure was invisible on the picker screen)',
);
assert.match(
  settingsViewSource,
  /const \[localAddBusyKey, setLocalAddBusyKey\] = useState\(null\);/,
  'the local row-add in-flight key must exist (drives the disabled visual)',
);
assert.match(
  settingsViewSource,
  /\} finally \{[\s\S]{0,160}localAddInFlightRef\.current = false;[\s\S]{0,160}setLocalAddBusyKey\(null\);[\s\S]{0,160}\}/,
  'the local row-add latch and busy key must clear together in the finally — a success-path-only clear strands the latch on the first failure and silently blocks every later row add (review r2 P2: the loose /setLocalAddBusyKey\\(null\\)/ pin could not see this shape)',
);
for (const settingsI18nSource of settingsI18nSources) {
  assert.match(
    settingsI18nSource,
    /modelDuplicateExists:/,
    'the duplicate-save hint copy must be provided in every UI language',
  );
  assert.match(
    settingsI18nSource,
    /modelDuplicateExistsEdit:/,
    'the edit-mode duplicate hint copy (twin already on disk) must be provided in every UI language',
  );
}
// 添加/编辑模型弹窗的滚动必须收敛在内层 data-testid="model-form-scroll" 容器
// （min-h-0 + flex-1 + overflow-y-auto），弹窗本体（圆角 + max-h 元素）不再自带
// overflow-y-auto：fixed + backdrop-filter 祖先链下，个别 WebKit 内核不会为
// 「同元素 max-h + overflow-y-auto」组合建立滚动区（用户反馈：本地模型列表
// 后半部分无法滚动选择）。内层 flex 滚动区在所有内核行为一致。
const modelFormScrollers = settingsViewSource.match(
  /data-testid="model-form-scroll" className="min-h-0 flex-1 overflow-y-auto overscroll-contain custom-scrollbar"/g,
) || [];
assert.strictEqual(
  modelFormScrollers.length,
  2,
  'both model form dialogs (picker + edit form) must scroll inside the dedicated min-h-0 container',
);
const flexScrollDialogs = settingsViewSource.match(
  /max-h-\[76vh\] flex flex-col overflow-hidden rounded-\[22px\]/g,
) || [];
assert.strictEqual(
  flexScrollDialogs.length,
  2,
  'model form dialog containers must cap height and delegate scrolling to the inner container',
);
for (const settingsI18nSource of settingsI18nSources) {
  assert.match(
    settingsI18nSource,
    /localPortLabel:/,
    'local model port input label must be provided in every UI language',
  );
  assert.match(
    settingsI18nSource,
    /localPortPlaceholder:/,
    'local model port input placeholder must be provided in every UI language',
  );
  assert.match(
    settingsI18nSource,
    /localPortInvalid:/,
    'local model port validation message must be provided in every UI language',
  );
  assert.match(
    settingsI18nSource,
    /noAuthToggleLabel:/,
    'the keyless-endpoint toggle label must be provided in every UI language',
  );
  assert.match(
    settingsI18nSource,
    /noAuthHint:/,
    'the keyless-endpoint hint must be provided in every UI language',
  );
}
// 「此端点无需鉴权」(auth_mode="none") 表单行为守卫：开关必须经
// data-testid="model-form-no-auth-toggle" 驱动，且保存载荷必须携带
// auth_mode（null=默认鉴权规则 / "none"=免鉴权声明，直通底座
// auth_mode_disables_api_key）。缺任一即开关失效或声明丢失。凭据删除
// 动作与全部三条探测路径（连接测试 / 图片能力测试 / 自动思考档位探测）
// 必须与 auth_mode 载荷同口径排除托管档/免鉴权态，
// URL 行的渲染点必须与赋值同在（防止中途加条件把字段藏掉）。
{
  const noAuthToggleSites = settingsViewSource.match(
    /data-testid="model-form-no-auth-toggle"/g,
  ) || [];
  assert.strictEqual(
    noAuthToggleSites.length,
    1,
    'the keyless-endpoint toggle must be rendered exactly once via its testid',
  );
  // The toggle's input wiring is pinned separately from its render: a
  // regression that renders the row but detaches the flip (no onClick) keeps
  // every payload/derivation guard green while the headline interaction
  // breaks inert (round-6 review).
  assert.match(
    settingsViewSource,
    /onClick=\{\(\) => setNoAuth\(v => !v\)\}/,
    'the keyless toggle must actually flip the noAuth state on click',
  );
  // Same caliber for the port row: the row can render correctly while its
  // onChange silently stops rewriting the URL — pin the wiring itself.
  assert.match(
    settingsViewSource,
    /onChange: e => handleBaseUrlPortChange\(e\.target\.value\)/,
    'the port row must actually rewrite the URL through handleBaseUrlPortChange on input',
  );
  assert.match(
    settingsViewSource,
    /auth_mode: !isLocalPreset && !isCodingPlan && noAuth \? 'none' : null/,
    'model form save payload must persist the keyless declaration via auth_mode (never for managed coding plans)',
  );
  assert.match(
    settingsViewSource,
    /initial\.provider_kind !== PROVIDER_KIND_CODING_PLAN && initial\.auth_mode === 'none'/,
    'the keyless toggle state must stay off for managed coding-plan entries',
  );
  assert.match(
    settingsViewSource,
    /if \(!isCodingPlan && noAuth\) return hasSavedKey \? 'delete' : 'replace'/,
    'the stored-credential delete action must stay scoped to non-managed keyless declarations (a stale toggle over a live coding-plan URL must not delete the credential)',
  );
  // The keyless test-key posture exists at exactly four spellings (handleTest
  // + handleImageCapabilityTest + handleFetchModels — the latter closed in the
  // round-5 review: a residual typed key must not ride the fetch-models probe
  // either — now factored into the pre-flight capture and its render mirror
  // for the credential-dimension staleness key, review P2 2026-11-17); a
  // plain assert.match would stay green when only ONE site regresses, so pin
  // the count.
  const keylessTestKeySites = settingsViewSource.match(
    /!isLocalPreset && !isCodingPlan && noAuth\s*\?\s*''\s*:/g,
  ) || [];
  assert.strictEqual(
    keylessTestKeySites.length,
    4,
    'connection test, image-capability test, AND fetch-models (payload + capture + mirror) must all refuse to send a typed key while the keyless declaration is active (a single-site regression must fail here)',
  );
  assert.match(
    settingsViewSource,
    /testModelConnection\(baseUrl\.trim\(\), testKey, initial\.__new \? null : initial\.id, !isLocalPreset && !isCodingPlan && noAuth\)/,
    'the connection test must forward the keyless posture so the backend skips the saved-credential fallback (a pre-save probe must not transmit the stored key to the newly typed URL)',
  );
  assert.match(
    settingsViewSource,
    /testImageInputCapability\(model\.trim\(\), baseUrl\.trim\(\), testKey, initial\.__new \? null : initial\.id, !isLocalPreset && !isCodingPlan && noAuth\)/,
    'the image-capability test must forward the keyless posture so the backend skips the saved-credential fallback',
  );
  // The automatic tier probe is the third probe path in the same form: under
  // the keyless posture it must drop the retained form key AND forward the
  // posture flag (the backend then skips the saved-credential fallback).
  // Both halves are pinned — dropping either reopens the credential-to-keyless-URL leak.
  assert.match(
    settingsViewSource,
    /trimInputs: true,\s*noAuth,\s*\}\)/,
    'the model form must forward its keyless posture into the reasoning-tier probe hook (the automatic tier probe is a third credential path)',
  );
  assert.match(
    localServerTiersSource,
    /const probeApiKey = noAuth \? '' : trimInputs \? apiKey\.trim\(\) : apiKey;/,
    'the tier probe must drop the retained form key while the keyless posture is active',
  );
  assert.match(
    localServerTiersSource,
    /probeLocalServerKind\(probeBaseUrl, probeApiKey, modelId, noAuth\)/,
    'the tier probe must forward the keyless posture flag so the backend skips the saved-credential fallback',
  );
  // The keyless UI derivation must follow the same managed scope as the save
  // payload: with the toggle ON and the URL edited to a managed host the
  // toggle row disappears, so the hint/key-row/reveal-error gates must fall
  // back to the undeclared shape too — otherwise the form deadlocks (hint
  // visible, toggle gone, key row hidden, typed key silently dropped).
  assert.match(
    settingsViewSource,
    /const noAuthActive = !isLocalPreset && !isCodingPlan && noAuth;/,
    'the keyless UI derivation must scope the declaration out of managed endpoints exactly like the save payload does',
  );
  assert.match(
    settingsViewSource,
    /!noAuthActive && renderApiKeyField\(\{ withBorder: false \}\)/,
    'the API key row must reappear when the live URL is a managed endpoint under a stale keyless toggle',
  );
  assert.match(
    settingsViewSource,
    /noAuthActive && <div className="px-4 pb-3/,
    'the keyless hint must disappear when the live URL is a managed endpoint (the payload already stores a null declaration there)',
  );
  assert.match(
    settingsViewSource,
    /showBaseUrlField = !isCodingPlan/,
    'the API base URL field (custom endpoint/port) must be visible for every non-coding-plan preset',
  );
  assert.match(
    settingsViewSource,
    /showBaseUrlField && renderInlineField/,
    'the base URL field gate must hold at the render site, not only at the assignment',
  );
  assert.match(
    settingsViewSource,
    /parsed\.pathname === '\/' \? serialized\.replace/,
    'port rewrites must only strip the slash WHATWG serialization adds, never a user-typed path slash',
  );
  assert.match(
    settingsViewSource,
    /testId: 'model-form-base-url-port'/,
    'the form must expose an explicit port field for the API base URL',
  );
  // The port row shares the base-URL render gate (`showBaseUrlField`); the
  // generic render-site regex above already matches TWO rows, so a plausible
  // regression (the port gate narrowing to `isLocalPreset`) stays green
  // there. Pin the port row with its own label-anchored gate.
  assert.match(
    settingsViewSource,
    /showBaseUrlField && renderInlineField\(\{\s*label: settingsCopy\.localPortLabel/,
    'the port row must render for every non-coding-plan preset via the same base-URL gate (a cloud preset silently losing the port row regresses the headline feature)',
  );
  // The custom-model key row follows the same noAuthActive caliber; without
  // this pin, dropping `&& !noAuthActive` resurrects the "key row visible
  // while the declaration drops the typed key" deadlock for custom entries
  // (the non-custom row is pinned above).
  assert.match(
    settingsViewSource,
    /const showCustomCloudKeyField = !isLocalPreset && customModel && !noAuthActive;/,
    'the custom-model key row must hide while the keyless declaration is active (the save drops a typed key under the declaration)',
  );
  // Both preset-switch paths must reset the keyless declaration; a stale ON
  // toggle from preset A would otherwise save preset B keyless with a
  // credential delete. Count-pinned: either site regressing must fail here.
  const keylessResetSites = settingsViewSource.match(
    /setNoAuth\(false\);\s*setBaseUrlPortError\(null\);/g,
  ) || [];
  assert.strictEqual(
    keylessResetSites.length,
    2,
    'both entry-switch paths (catalog apply + manual local add) must reset the keyless declaration and the port error (a stale ON toggle must not leak into the next entry)',
  );
  // The save-side gates must follow the same noAuthActive caliber as the
  // derivation pin above: raw `noAuth` here would drop a typed key while the
  // form shows it accepted (currently unreachable, latent divergence).
  assert.match(
    settingsViewSource,
    /isLocalPreset \|\| noAuthActive \|\| hasSavedKey \|\| !!apiKey\.trim\(\)/,
    'the can-save gate must follow the same noAuthActive caliber as the save payload',
  );
  assert.match(
    settingsViewSource,
    /: \(!noAuthActive && apiKey\.trim\(\) \? apiKey\.trim\(\) : ''\)/,
    'the saved key payload must follow the same noAuthActive caliber as the derivation (a typed key must never be silently discarded under a stale toggle)',
  );
}
// ── ACP 线(ProviderFormModal,规格 §2.7)源码守卫 ────────────────────────────
// 「获取模型列表」按钮仅 codex/kimi 且 base URL 非空时渲染,onClick 直挂
// handleFetchEndpointModels;claude 的模型槽位编辑器走官方,范围外,整个文件
// 不得出现第二个按钮分支(testid 恒出现一次)。
const providerFormModalSource = fs.readFileSync(
  path.join(__dirname, '..', 'src', 'features', 'settings', 'ProviderFormModal.jsx'),
  'utf8',
);
assert.strictEqual(
  (providerFormModalSource.match(/acp-provider-fetch-models/g) || []).length,
  1,
  'the ACP fetch-models button must exist exactly once (claude gets no such branch)',
);
assert.match(
  providerFormModalSource,
  /\(agent === 'codex' \|\| agent === 'kimi'\) && String\(baseUrl \|\| ''\)\.trim\(\) !== '' && \(\s*<button/,
  'the ACP fetch-models button must render only for codex/kimi agents with a non-empty base URL',
);
assert.match(
  providerFormModalSource,
  /data-testid="acp-provider-fetch-models"\s+onClick=\{handleFetchEndpointModels\}/,
  'the ACP fetch-models button must be an explicit-click button wired to handleFetchEndpointModels',
);
// 按钮探活:bridge.models.probeEndpointModels 缺失(老桌面/web 白名单未放行)时
// 禁用并以三语 title 提示,与聊天线 fetchUnavailable 同一探活语义。
assert.match(
  providerFormModalSource,
  /typeof bridge\?\.models\?\.probeEndpointModels === 'function'/,
  'the ACP line must feature-detect bridge.models.probeEndpointModels before enabling the button',
);
assert.match(
  providerFormModalSource,
  /disabled=\{fetchingModels \|\| !probeEndpointModelsAvailable\}\s+title=\{probeEndpointModelsAvailable \? undefined : copy\.fetchUnavailable\}/,
  'the ACP fetch-models button must disable with a localized title when the probe bridge method is unavailable',
);
// 探测 payload 必须携带表单 wire 协议;禁止 effect/挂载期自动探测(与聊天线同一安全不变量)。
assert.match(
  providerFormModalSource,
  /wire:\s*probedWire,/,
  'the ACP probe must pass the form wire protocol (the pre-flight capture) to probeEndpointModels',
);
assert.doesNotMatch(
  providerFormModalSource,
  /useEffect\(\(\) => \{[\s\S]{0,240}probeEndpointModels/,
  'ACP model list probing must never run on mount/effect; explicit button click only',
);
// onPick:模型 id 走输入框同一 changeModel 通道;contextWindow 仅空字段填入,
// 绝不覆盖非空值(与聊天线 fill-only-when-empty 同一不变量,突变取反即红)。
assert.match(
  providerFormModalSource,
  /changeModel\(row\.id\)/,
  'the ACP pick must fill the model field through the same changeModel channel as the input',
);
assert.match(
  providerFormModalSource,
  /!String\(contextWindow \|\| ''\)\.trim\(\) && Number\.isFinite\(probedContext\) && probedContext > 0/,
  'the ACP pick must fill contextWindow ONLY when the field is empty (never overwrite a non-empty value)',
);
// ACP 线复用共享 picker:不传 onAddAll(批量按钮是聊天线专属)、不传 savedKeys
// (provider 无已保存模型去重语义,组件按「无已保存」处理:行恒可添加)。
const acpPickerStart = providerFormModalSource.indexOf('<EndpointModelPicker');
assert.notStrictEqual(acpPickerStart, -1, 'the ACP line must reuse the shared endpoint model picker');
const acpPickerSource = providerFormModalSource.slice(
  acpPickerStart,
  providerFormModalSource.indexOf('/>', acpPickerStart),
);
assert.match(
  acpPickerSource,
  /onPick=\{handleEndpointModelPick\}/,
  'the ACP picker must route row adds through handleEndpointModelPick',
);
assert.doesNotMatch(
  acpPickerSource,
  /onAddAll=/,
  'the ACP line must not enable the picker batch add-all (chat line only)',
);
assert.doesNotMatch(
  acpPickerSource,
  /savedKeys=/,
  'the ACP line must omit savedKeys so picker rows stay addable (providers have no saved-model dedup)',
);
// 复审 ACP minor(Escape 逐层关闭):确认层打开时 Escape 只关确认层,其次候选
// 面板,最后才是整个表单——上层浮层不得连带下层一起消失。顺序钉死。
// r3 复审修正层级顺序:旧顺序「面板优先」在键盘路径下面板+确认层可同时打开
// (面板无焦点圈禁,Tab 可达面板底下的保存键),一次 Escape 曾连关两层。
// IME 守卫随 #817 B1 复审补齐:组合词窗口中的 Escape(取消候选词)不得关闭任何层。
assert.match(
  providerFormModalSource,
  /if \(event\.key === 'Escape' && !isImeComposing\(event\)\) \{\s*if \(confirmStep\) setConfirmStep\(null\);\s*else if \(endpointPickerOpen\) setEndpointPickerOpen\(false\);\s*else onClose\(\);/,
  'Escape must close the confirm layer first (the visual top), then the endpoint picker, then the whole form — one layer per press even when the picker and the confirm co-open (r3 review), IME-guarded',
);
// 复审 2026-11-20:确认层 portal 到 body 后,表单卡片的 stopPropagation 仍是
// 承载层——React 合成事件沿 React 树冒泡,删掉它确认弹层的背景点击会一路
// 冒到表单根节点的 onClose,连带关掉整个表单。
assert.match(
  providerFormModalSource,
  /data-testid="acp-provider-form-dialog"[\s\S]{0,1000}onClick=\{event => event\.stopPropagation\(\)\}/,
  'the form card must keep its stopPropagation shield (anchored between the form dialog root and the card): the confirm layer portals to body, but React synthetic events still bubble through the React tree to the form root onClose',
);
// 复审 M5(ACP 线同因):手输改 base URL 必须作废既有探测状态。
assert.match(
  providerFormModalSource,
  /const handleBaseUrlChange = value => \{\s*setBaseUrl\(value\);\s*clearEndpointProbeState\(\);/,
  'editing the ACP base URL must clear the probed listing and close the picker (no URL A results under URL B)',
);
// round-4 审 m1-1:探测失败写在本表单共享的 error 条上,失效清扫必须连同
// 清 error——旧 URL 的红色失败条不许挂在新 URL/协议下(与聊天线
// fetchModelsMessage 的 round-3 minor 同因;error 的其余写入方均为
// 「上一次尝试」的过期提示,随失效点清扫无副作用)。运行时覆盖见
// tests/acp_providers_ui_smoke.js ⑥.18c/⑥.18d。
assert.match(
  providerFormModalSource,
  /const clearEndpointProbeState = \(\) => \{\s*setEndpointListing\(null\);\s*setEndpointPickerOpen\(false\);\s*setError\(''\);\s*\};/,
  'the ACP probe-invalidation sweep must also clear the shared error state (the probe-failure banner rides it and must not survive a base-URL/wire/preset edit, round-4 review m1-1)',
);
// round-5 审 m1:resolve 时刻守卫必须同时比对 wire 镜像——kimi 表单是唯一
// 带 wire 切换的面,探测在飞时切 wire,URL 不变但协议变了,旧 wire 的
// listing(乃至 auth_required 语义)不得挂在切换后的表单下。try/catch 两条
// resolve 路径都要有(与 URL 守卫同款双臂)。
assert.strictEqual(
  (providerFormModalSource.match(/if \(wireRef\.current !== probedWire\) return;/g) || []).length,
  2,
  'both the try and catch resolve paths must discard an in-flight probe whose wire toggled (round-5 review m1; a single-arm regression must fail here)',
);
// i18n:ACP 簇的探活禁用提示三语齐备(组件内不得硬编码文案)。
for (const settingsI18nSource of settingsI18nSources) {
  assert.match(
    settingsI18nSource,
    /\.uiAcpProviders,\s*\{[^}]*?fetchUnavailable:/,
    'the ACP fetch-unavailable hint must be provided in every UI language',
  );
}
// i18n(复审 picker minor):筛选空态/截断提示/N 条未显示尾注三语齐备,且死键
// acpNoModelsFound(引用已删)不得残留。
for (const settingsI18nSource of settingsI18nSources) {
  assert.match(
    settingsI18nSource,
    /modelListFilterEmpty:/,
    'the filtered-empty picker copy must be provided in every UI language',
  );
  assert.match(
    settingsI18nSource,
    /modelListTruncated:/,
    'the truncated-listing notice must be provided in every UI language',
  );
  assert.match(
    settingsI18nSource,
    /modelListMoreCount:\s*n\s*=>/,
    'the render-cap footer must interpolate the hidden row count in every UI language',
  );
  assert.match(
    settingsI18nSource,
    /modelListMoreCountPlain:\s*n\s*=>/,
    'the untouched-filter footer variant (no "refine the filter" advice) must exist in every UI language (round-3 review minor)',
  );
  assert.doesNotMatch(
    settingsI18nSource,
    /acpNoModelsFound/,
    'the dead acpNoModelsFound key must stay deleted from every UI language',
  );
  // i18n(复审 minor-8):picker 的 3xx/tls 错误嗓音复用连接测试的
  // connectionMessages.redirect / tls_error(零新增键),三语齐备在此钉死,
  // 防止后续清理误删导致 picker 静默回落到 http_error。
  assert.match(
    settingsI18nSource,
    /connectionMessages:\{[^}]*redirect:/,
    'the redirect voice reused by the picker must exist in every UI language',
  );
  assert.match(
    settingsI18nSource,
    /connectionMessages:\{[^}]*tls_error:/,
    'the tls voice reused by the picker must exist in every UI language',
  );
}

// review P2 2026-11-17 (credential dimension in the probe staleness key):
// a probe's result must be discarded when the effective credentials or the
// protocol preset changed mid-flight — the old account's listing must not
// open under new credentials (the picker's Add saves rows with the CURRENT
// credentials, which may not have access to those models). Both guard paths
// (success and error) are pinned on both forms.
assert.match(
  settingsViewSource,
  /const probeAuthRef = useRef\(''\);\s*probeAuthRef\.current = !isLocalPreset && !isCodingPlan && noAuth/,
  'the chat lane must mirror the effective auth payload so an in-flight probe result can be discarded when the credentials moved on',
);
{
  const guardCount = (settingsViewSource.match(/if \(probeAuthRef\.current !== probedAuth\) return;/g) || []).length;
  assert.strictEqual(
    guardCount, 2,
    `both probe paths (success and error) must check the credential dimension, got ${guardCount}`,
  );
  const presetCount = (settingsViewSource.match(/if \(probePresetRef\.current !== probedPreset\) return;/g) || []).length;
  assert.strictEqual(
    presetCount, 2,
    'both probe paths (success and error) must check the protocol preset dimension',
  );
}
assert.match(
  providerFormModalSource,
  /const apiKeyRef = useRef\(apiKey\);\s*apiKeyRef\.current = apiKey;/,
  'the ACP lane must mirror the live key so an in-flight probe result can be discarded when the credential changed',
);
{
  const guardCount = (providerFormModalSource.match(/if \(\(String\(apiKeyRef\.current \|\| ''\)\.trim\(\) \|\| null\) !== probedKey\) return;/g) || []).length;
  assert.strictEqual(
    guardCount, 2,
    `both ACP probe paths (success and error) must check the key dimension, got ${guardCount}`,
  );
}
// Memory delete / feedback close confirm-dialog wiring (no native
// window.confirm, testid pins) is asserted by tests/settings_window_confirm.test.mjs,
// which runs on every `npm test`; the runtime click flows below (①e/①f/①g/⑰.5)
// still exercise the dialogs end to end in this smoke.

function loadPuppeteer() {
  try { return require('puppeteer-core'); } catch { /* fall through */ }
  const npx = path.join(os.homedir(), '.npm', '_npx');
  if (fs.existsSync(npx)) {
    for (const directory of fs.readdirSync(npx)) {
      const candidate = path.join(npx, directory, 'node_modules', 'puppeteer-core');
      if (fs.existsSync(candidate)) {
        try { return require(candidate); } catch { /* try next */ }
      }
    }
  }
  console.error('SKIP: 找不到 puppeteer-core');
  process.exit(2);
}

const puppeteer = loadPuppeteer();
const CHROME = process.env.CHROME || [
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  '/snap/bin/chromium',
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
  '/usr/bin/google-chrome',
  '/usr/bin/google-chrome-stable',
].find(candidate => fs.existsSync(candidate));
if (!CHROME) {
  console.error('SKIP: 未找到 chromium/chrome，可用 env CHROME=/path/to/chrome 指定');
  process.exit(2);
}
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-settings-'));

function injectSource() {
  return `(function () {
    var handlers = Object.create(null);
    var settings = {
      theme: 'liquid-light',
      color_scheme: 'light',
      language: 'zh-Hans',
      memory_enabled: true,
      notifications: { enabled: true, task_completed: true },
      pet: { enabled: false },
      search: {
        provider: 'bing',
        enabled_providers: ['bing', 'metaso'],
        api_key: null,
        credentials: {
          metaso: { credential_state: 'configured', has_secret: true },
        },
      },
      advanced: {},
    };
    var models = [
      {
        id: 'local-qwen',
        name: '本地 vLLM',
        preset: 'local_vllm',
        model: 'qwen36_35b_256k',
        base_url: 'http://127.0.0.1:8000/v1',
        has_secret: false,
        credential_state: 'missing',
      },
      {
        id: 'cloud-deepseek',
        name: 'DeepSeek',
        preset: 'deepseek',
        model: 'deepseek-v4-flash',
        base_url: 'https://api.deepseek.com',
        // doSave 会把 provider_kind 写回 settings;官方 API 组未显式声明 baseUrl,
        // 曾导致 findCloudProviderForModel 失配、编辑弹窗隐藏配置区与测试连接。
        provider_kind: 'official_api',
        has_secret: true,
        credential_state: 'configured',
      },
      {
        id: 'cloud-kimi-global-custom',
        name: 'Kimi 国际版',
        preset: 'kimi',
        model: 'custom-kimi-id',
        base_url: 'https://api.moonshot.ai/v1',
        vendor: 'kimi',
        reasoning_effort: 'off',
        has_secret: true,
        credential_state: 'configured',
      },
    ];
    var activeModelId = 'local-qwen';
    var superPerm = false;
    var calls = [];
    var updateResponse = { available: false, current_version: '0.6.1', latest_version: '0.6.1', notes: '', platform: 'windows' };
    var updateCheckFailure = null;
    var modelTestResponse = { ok: true, code: 'ok', message: '连接成功，服务可用', detail: 'HTTP 200', http_status: 200 };
    // probe_endpoint_models 候选(规格 §2.6):含一条与 ⑥.5g 已保存的
    // glm-5.3-flash @ https://example.com/v1 重复的行,用于钉「已保存」标记与
    // 批量去重(此前的 cloud-deepseek 已被更早的删除流程移除,不能当重复锚点);
    // 其余为带端点事实的全新候选。
    var probeEndpointListing = {
      status: 'listed',
      http_status: 200,
      transport: null,
      server_kind: null,
      truncated: false,
      models: [
        { id: 'glm-5.3-flash', context_window_tokens: 131072, context_from: 'catalog', max_output_tokens: null, output_from: null, loaded: null, title: null },
        { id: 'glm-5-flash', context_window_tokens: 1048576, context_from: 'endpoint', max_output_tokens: 32768, output_from: 'endpoint', loaded: null, title: 'GLM-5 Flash' },
        { id: 'kimi-k3', context_window_tokens: null, context_from: null, max_output_tokens: null, output_from: null, loaded: null, title: null },
      ],
    };
    var imageTestResponse = { status: 'supported', verified: true, summary: '红色', http_status: 200 };
    var imageTestDelay = 0; // 模拟探测耗时,便于断言行内忙转态
    // save_model 失败注入:非 null 时 reject 该错误,模拟连接/写盘失败。
    var saveModelError = null;
    // ⑦.9f 晚到完成旋钮:true 时 get_settings 挂起排队,releaseGetSettingsHold
    // 一并放行——制造「loadModels 已发布、刷新尾未落定」的窗口。
    var holdGetSettings = false;
    var heldGetSettings = [];
    // save_model 在飞窗口注入(round-4 M4 运行时钉):>0 时 save_model 延迟
    // resolve/reject,拉长单行保存在飞窗口,供「全部添加」禁用断言驱动时序。
    var saveModelDelay = 0;
    var listModelsFailure = false;
    // ⑦.9h 时序旋钮:挂起接下来 N 次 list_models(序号已领取、响应未到),
    // releaseHeldListModels 用当前世界统一放行。
    var listModelsHoldCount = 0;
    var heldListModels = [];
    var dependencyCheckResponse = [];
    var memoryOverview = {
      profile: { version: 1, revision: 3, identity: { call_name: '升级前称呼', assistant_alias: 'PINVOU' }, conventions: {} },
      preferences: [
        { id: 'pref-to-delete', text: '待删除的偏好记忆', status: 'active' },
        { id: 'pref-to-keep', text: '保留的偏好记忆', status: 'active' },
      ],
      work_context: [{ id: 'wc-to-delete', text: '待删除的工作上下文', status: 'active' }], current_focus: [], recent_activity: [], pending: [],
      runtime: null, snapshot_path: '', warnings: [],
      sources: {
        profile: { available: true }, preferences: { available: true }, work_context: { available: true },
        current_focus: { available: true }, recent_activity: { available: true },
        pending: { available: true },
      },
    };
    var failMemoryOverview = false;
    var failMemoryUpdate = false;
    var pendingDownloadResolve = null;
    var pendingDownloadReject = null;
    var probedLocalKind = null;
    function record(cmd, args) { calls.push({ cmd: cmd, args: args || null }); }
    // save_model 的落库体(函数声明提升,save_model 分支同步/延迟两条路径共用)。
    // base_url 镜像 Rust upsert_model 的归一化(normalize_provider_metadata →
    // strip_chat_completions_suffix):列表里的记录永远带 canonical URL,
    // ⑦.9g 的全 URL → canonical 列表 → 删除 → 重加场景靠它成立。
    function canonicalBaseUrl(value) {
      var text = String(value || '').trim();
      while (text.slice(-1) === '/') text = text.slice(0, -1);
      var suffix = '/chat/completions';
      if (text.toLowerCase().slice(-suffix.length) === suffix) text = text.slice(0, text.length - suffix.length);
      return text;
    }
    function applySaveModel(args) {
      var savedModel = Object.assign({}, args.model, {
        base_url: canonicalBaseUrl(args.model.base_url),
        has_secret: !!args.model.api_key,
        credential_state: args.model.preset === 'local_vllm' ? 'missing' : 'configured',
      });
      models = models.filter(function (model) { return model.id !== savedModel.id; }).concat(savedModel);
      return null;
    }
    // Deliberately not stubbing window.confirm here. SettingsView now routes
    // everything through in-app confirm dialogs (the native confirm does not
    // render in Tauri WebView2); if someone reintroduces a native confirm,
    // headless auto-dismiss makes the flow assertions fail loudly instead of
    // being masked by a fake stub.
    window.alert = function (message) { record('window_alert', { message: message }); };
    function emit(name, payload) {
      return Promise.all((handlers[name] || []).slice().map(function (handler) {
        return handler({ payload: payload || {} });
      }));
    }
    function invoke(cmd, args) {
      record(cmd, args);
      switch (cmd) {
        case 'probe_local_server_kind': return Promise.resolve(probedLocalKind);
        case 'get_platform_capabilities': return Promise.resolve({
          os: 'windows',
          showSuperPermissionSettings: false,
          usesBundledDependencyInstaller: true,
          taskCompletionNotificationsDefault: true,
          voiceShortcutNative: true,
        });
        // R16: the computer-use settings section now EXECUTES in this smoke
        // (previously it had only source-regex pins). The capability object
        // above deliberately carries no virtualDisplayNote, so the
        // unknown-platform honesty note must render next to the absolute
        // hint. NB: this whole mock is a template literal — no backticks
        // may appear in comments here.
        case 'computer_use_get_status': return Promise.resolve({
          enabled: true,
          stopped: false,
          platform_supported: true,
          pending_confirm: null,
          virtual_display_state: 'ready',
          virtual_display_detail: null,
          prefer_virtual_display: true,
          physical_session_live: false,
          session_display_target: null,
        });
        case 'computer_use_set_enabled': return Promise.resolve(null);
        case 'computer_use_set_virtual_display_preference': return Promise.resolve(null);
        case 'get_settings':
          // ⑦.9f 晚到完成旋钮:挂起刷新尾(loadSettings 是保存链 save_model →
          // loadModels → get_settings → 有效配置的第三步),让删除在「A 已发布、
          // 链未落定」的窗口内插入——评审 P2×2 的精确时序。
          if (holdGetSettings) {
            return new Promise(function (resolve) { heldGetSettings.push(resolve); });
          }
          return Promise.resolve(settings);
        case 'update_settings':
          settings = Object.assign({}, settings, args.patch || {});
          return Promise.resolve(settings);
        case 'update_search_settings':
          settings = Object.assign({}, settings, { search: args.search });
          return Promise.resolve(settings);
        case 'save_search_settings_and_restart':
          settings = Object.assign({}, settings, { search: args.search });
          return Promise.resolve(null);
        case 'get_effective_model_config': return Promise.resolve({ model: 'qwen36_35b_256k', base_url: 'http://127.0.0.1:8000/v1', preset: 'local_vllm', api_key_set: false });
        case 'list_models':
          // ⑦.4 one-shot refresh-failure injection: the app-side loadModels
          // swallows this rejection and clears the saved list — the exact
          // branch the parent-owned just-committed book must survive (the
          // in-flight registry releases on settle; without the book the
          // settled identity is re-addable over an empty list).
          if (listModelsFailure) { listModelsFailure = false; return Promise.reject(new Error('⑦.4 list_models 注入失败')); }
          // ⑦.9f/⑦.9h 时序旋钮:挂起接下来的 N 次 list_models,让保存链
          // 的刷新尾读取停在在飞(序号已领取),供并发读取抢占序号或窗口
          // 内插入删除。
          if (listModelsHoldCount > 0) {
            listModelsHoldCount -= 1;
            return new Promise(function (resolve) { heldListModels.push(resolve); });
          }
          return Promise.resolve({ models: models.slice(), active_model_id: activeModelId });
        case 'reveal_model_api_key': return Promise.resolve(args.id === 'cloud-deepseek' ? 'sk-saved-deepseek' : null);
        case 'save_model':
          // 延迟窗口内同样按 saveModelError 决定 resolve/reject:错误注入与
          // 在飞窗口两个旋钮可以正交组合(延迟成功 / 延迟失败)。无延迟时
          // 保持既有即时语义。
          if (saveModelDelay) {
            return new Promise(function (resolve, reject) {
              setTimeout(function () {
                if (saveModelError) reject(new Error(saveModelError));
                else resolve(applySaveModel(args));
              }, saveModelDelay);
            });
          }
          if (saveModelError) return Promise.reject(new Error(saveModelError));
          return Promise.resolve(applySaveModel(args));
        case 'delete_model':
          models = models.filter(function (model) { return model.id !== args.id; });
          if (activeModelId === args.id) activeModelId = models[0] && models[0].id;
          return Promise.resolve(null);
        case 'set_active_model': activeModelId = args.id; return Promise.resolve(null);
        case 'test_model_connection': return Promise.resolve(Object.assign({}, modelTestResponse));
        case 'probe_endpoint_models': return Promise.resolve(JSON.parse(JSON.stringify(probeEndpointListing)));
        case 'test_image_input_capability':
          return new Promise(function (resolve) {
            setTimeout(function () { resolve(Object.assign({}, imageTestResponse)); }, imageTestDelay);
          });
        case 'discover_local_vllm': {
          // Port-routed alternate rosters: the candidates mapping keeps
          // max_model_len on the first row only, so each provider's
          // persistence arm in buildLocalModelPayload is pinned by
          // detecting that provider on its own port and adding its
          // index-0 row.
          const alternates = {
            1234: [{
              provider: 'lm_studio',
              label: 'LM Studio',
              base_url: 'http://127.0.0.1:1234/v1',
              status: 'ready',
              model: 'gemma-3-12b',
              models: [{ id: 'gemma-3-12b', loaded: true }],
              max_model_len: 12918,
            }],
            8000: [{
              provider: 'vllm',
              label: 'vLLM',
              base_url: 'http://127.0.0.1:8000/v1',
              status: 'ready',
              model: 'qwen36_35b_256k',
              models: ['qwen36_35b_256k'],
              max_model_len: 262144,
            }],
            // ⑥.5b2 持久化臂的未保存身份:8000 行与种子 local-qwen 同身份,
            // 已被行内去重禁用,故 vLLM 的 max_model_len 持久化断言改走 8100。
            8100: [{
              provider: 'vllm',
              label: 'vLLM',
              base_url: 'http://127.0.0.1:8100/v1',
              status: 'ready',
              model: 'qwen3-next',
              models: ['qwen3-next'],
              max_model_len: 262144,
            }],
            // ⑥.5b5/b6 跨入口闩锁的未保存身份:8200 行在所有既有用例之后才
            // 使用,保证进入用例时它一定可添加。
            8200: [{
              provider: 'vllm',
              label: 'vLLM',
              base_url: 'http://127.0.0.1:8200/v1',
              status: 'ready',
              model: 'qwen3-next-2',
              models: ['qwen3-next-2'],
              max_model_len: 262144,
            }],
            // ⑦.4 刷新失败分支的未保存身份:仅在 ⑦.4 使用,不与既有场景互染。
            8225: [{
              provider: 'vllm',
              label: 'vLLM',
              base_url: 'http://127.0.0.1:8225/v1',
              status: 'ready',
              model: 'refresh-twin-7c',
              models: ['refresh-twin-7c'],
              max_model_len: 8192,
            }],
            // ⑦.9 复审 P2(读取失败冻结)的隔离档:两个候选互不相干,A 用于
            // 先落库并经列表确认,B 用于触发下一次 list_models 失败。仅在
            // ⑦.9 使用,不与既有场景互染。
            8240: [
              {
                provider: 'vllm',
                label: 'vLLM',
                base_url: 'http://127.0.0.1:8240/v1',
                status: 'ready',
                model: 'freeze-twin-a',
                models: ['freeze-twin-a'],
                max_model_len: 8192,
              },
              {
                provider: 'vllm',
                label: 'vLLM',
                base_url: 'http://127.0.0.1:8240/v1',
                status: 'ready',
                model: 'freeze-twin-b',
                models: ['freeze-twin-b'],
                max_model_len: 8192,
              },
            ],
            // ⑦.9f 晚到完成场景的隔离名册:仅在 ⑦.9f 使用,不与既有场景互染。
            8260: [
              {
                provider: 'vllm',
                label: 'vLLM',
                base_url: 'http://127.0.0.1:8260/v1',
                status: 'ready',
                model: 'late-twin-a',
                models: ['late-twin-a'],
                max_model_len: 8192,
              },
              {
                provider: 'vllm',
                label: 'vLLM',
                base_url: 'http://127.0.0.1:8260/v1',
                status: 'ready',
                model: 'late-twin-b',
                models: ['late-twin-b'],
                max_model_len: 8192,
              },
            ],
            // ⑦.9i 代际证据场景的隔离名册:仅在 ⑦.9i 使用。
            8280: [
              {
                provider: 'vllm',
                label: 'vLLM',
                base_url: 'http://127.0.0.1:8280/v1',
                status: 'ready',
                model: 'gen-twin-a',
                models: ['gen-twin-a'],
                max_model_len: 8192,
              },
              {
                provider: 'vllm',
                label: 'vLLM',
                base_url: 'http://127.0.0.1:8280/v1',
                status: 'ready',
                model: 'gen-twin-b',
                models: ['gen-twin-b'],
                max_model_len: 8192,
              },
            ],
            // ⑦.9j 尾窗活体证据场景的隔离名册:仅在 ⑦.9j 使用。
            8290: [
              {
                provider: 'vllm',
                label: 'vLLM',
                base_url: 'http://127.0.0.1:8290/v1',
                status: 'ready',
                model: 'gen2-twin-a',
                models: ['gen2-twin-a'],
                max_model_len: 8192,
              },
              {
                provider: 'vllm',
                label: 'vLLM',
                base_url: 'http://127.0.0.1:8290/v1',
                status: 'ready',
                model: 'gen2-twin-b',
                models: ['gen2-twin-b'],
                max_model_len: 8192,
              },
            ],
            // ⑦.9h 被作废读取场景的隔离名册:仅在 ⑦.9h 使用。
            8270: [
              {
                provider: 'vllm',
                label: 'vLLM',
                base_url: 'http://127.0.0.1:8270/v1',
                status: 'ready',
                model: 'stale-twin-a',
                models: ['stale-twin-a'],
                max_model_len: 8192,
              },
              {
                provider: 'vllm',
                label: 'vLLM',
                base_url: 'http://127.0.0.1:8270/v1',
                status: 'ready',
                model: 'stale-twin-b',
                models: ['stale-twin-b'],
                max_model_len: 8192,
              },
            ],
          };
          const customPort = args && args.request && args.request.customPort;
          return Promise.resolve({ candidates: alternates[customPort] || [
            {
              provider: 'ollama',
              label: 'Ollama',
              base_url: 'http://127.0.0.1:11434/v1',
              status: 'ready',
              model: 'qwen2.5-coder:32b',
              models: [{ id: 'qwen2.5-coder:32b', loaded: true }, { id: 'deepseek-r1:14b', loaded: false }],
              max_model_len: 32768,
            },
            {
              provider: 'vllm',
              label: 'vLLM',
              base_url: 'http://127.0.0.1:8000/v1',
              status: 'ready',
              model: 'qwen36_35b_256k',
              models: ['qwen36_35b_256k'],
              max_model_len: 262144,
            },
          ] });
        }
        case 'get_selected_pet': return Promise.resolve('lingling');
        case 'list_sessions': return Promise.resolve([]);
        case 'get_super_permission_status': return Promise.resolve(superPerm);
        case 'set_super_permission': return Promise.reject(new Error('pkexec unavailable'));
        case 'list_personas': return Promise.resolve([]);
        case 'get_backend_status': return Promise.resolve({ online: true, ok: true, status: 'online' });
        case 'check_for_update':
          return updateCheckFailure
            ? Promise.reject(new Error(updateCheckFailure))
            : Promise.resolve(Object.assign({}, updateResponse));
        case 'download_update': return new Promise(function (resolve, reject) { pendingDownloadResolve = resolve; pendingDownloadReject = reject; });
        case 'install_update': return Promise.resolve(null);
        case 'find_resumable_run': return Promise.resolve(null);
        case 'check_dependencies': return Promise.resolve(dependencyCheckResponse.slice());
        case 'install_dependencies': return Promise.resolve(null);
        // Community edition: the backend does not accept online feedback; the submit ends with failed_validation (see ⑰).
        case 'submit_feedback': return Promise.resolve({ status: 'failed_validation', message: '反馈提交失败，请稍后重试。' });
        case 'list_marketplace_tools': return Promise.resolve([]);
        case 'get_mode_state': return Promise.resolve({ mode: 'yolo', plan_phase: 'none' });
        case 'get_active_persona': return Promise.resolve(null);
        case 'list_workspace_files': return Promise.resolve([]);
        case 'list_scheduled_tasks': return Promise.resolve([]);
        case 'list_scheduled_task_recent_runs': return Promise.resolve([]);
        case 'get_app_version': return Promise.resolve('0.6.1');
        case 'get_memory_overview':
          return failMemoryOverview ? Promise.reject(new Error('snapshot locked')) : Promise.resolve(memoryOverview);
        case 'update_memory_profile':
          if (failMemoryUpdate) return Promise.reject(new Error('profile write failed'));
          memoryOverview = Object.assign({}, memoryOverview, {
            profile: Object.assign({}, memoryOverview.profile, {
              identity: Object.assign({}, memoryOverview.profile.identity, args.patch || {}),
            }),
          });
          return Promise.resolve({
            profile: memoryOverview.profile,
            runtime: null,
            warnings: [{ code: 'runtime_refresh_failed', source: 'runtime', detail: 'runtime cache locked' }],
          });
        case 'delete_memory_preference':
          memoryOverview = Object.assign({}, memoryOverview, {
            preferences: (memoryOverview.preferences || []).filter(function (item) { return item.id !== args.id; }),
          });
          return Promise.resolve(null);
        case 'delete_work_context_memory':
          memoryOverview = Object.assign({}, memoryOverview, {
            work_context: (memoryOverview.work_context || []).filter(function (item) { return item.id !== args.id; }),
          });
          return Promise.resolve(null);
        default: return Promise.resolve(null);
      }
    }
    window.__SETTINGS_TEST__ = {
      calls: calls,
      setProbedLocalKind: function (kind) { probedLocalKind = kind; },
      emit: emit,
      models: function () { return models.slice(); },
      settings: function () { return settings; },
      activeModelId: function () { return activeModelId; },
      setUpdateResponse: function (next) { updateResponse = Object.assign({}, updateResponse, next || {}); },
      setUpdateCheckFailure: function (message) { updateCheckFailure = message || null; },
      setModelTestResponse: function (next) { modelTestResponse = Object.assign({}, next || {}); },
      setProbeEndpointListing: function (next) { probeEndpointListing = JSON.parse(JSON.stringify(next || { status: 'listed', models: [] })); },
      setImageTestResponse: function (next) { imageTestResponse = Object.assign({}, next || {}); },
      setImageTestDelay: function (ms) { imageTestDelay = Number(ms) || 0; },
      setSaveModelError: function (message) { saveModelError = message || null; },
      setSaveModelDelay: function (ms) { saveModelDelay = Number(ms) || 0; },
      armGetSettingsHold: function () { holdGetSettings = true; },
      heldGetSettingsCount: function () { return heldGetSettings.length; },
      releaseGetSettingsHold: function () {
        holdGetSettings = false;
        var queued = heldGetSettings.slice();
        heldGetSettings = [];
        queued.forEach(function (resolve) { resolve(settings); });
      },
      armListModelsFailure: function () { listModelsFailure = true; },
      armListModelsHold: function (count) { listModelsHoldCount = Number(count) || 0; },
      heldListModelsCount: function () { return heldListModels.length; },
      releaseHeldListModels: function (index) {
        // 单个放行:缺省 FIFO(先解最老,⑦.9h 只放 N 让 N+1 继续 pending);
        // 传 1-based index 时按位选择(⑦.9i 需要先放 N+1 再放 N/M)。
        // payload 用放行时刻的当前世界。
        var resolve = index ? heldListModels.splice(index - 1, 1)[0] : heldListModels.shift();
        if (resolve) resolve({ models: models.slice(), active_model_id: activeModelId });
      },
      setModelImageCapability: function (id, override) {
        models = models.map(function (m) { return m.id === id ? Object.assign({}, m, { image_capability_override: override }) : m; });
      },
      setDependencyCheckResponse: function (next) { dependencyCheckResponse = (next || []).slice(); },
      setFailMemoryOverview: function (value) { failMemoryOverview = !!value; },
      setFailMemoryUpdate: function (value) { failMemoryUpdate = !!value; },
      resolveDownload: function () {
        if (pendingDownloadResolve) {
          var resolve = pendingDownloadResolve;
          pendingDownloadResolve = null;
          resolve({ package_path: 'C:\\\\tmp\\\\pinvou.zip', installer_path: 'C:\\\\tmp\\\\pinvou.msi', latest_version: updateResponse.latest_version });
        }
      },
      rejectDownload: function (rawError) {
        if (pendingDownloadResolve) {
          var reject = pendingDownloadReject;
          pendingDownloadResolve = null;
          reject(new Error(rawError));
        }
      },
    };
    window.__TAURI__ = {
      core: { invoke: invoke },
      event: {
        listen: function (name, handler) {
          (handlers[name] || (handlers[name] = [])).push(handler);
          return Promise.resolve(function () {
            var index = handlers[name].indexOf(handler);
            if (index >= 0) handlers[name].splice(index, 1);
          });
        },
        emit: emit,
      },
      window: {
        getCurrentWindow: function () {
          return {
            minimize: function () {},
            maximize: function () {},
            close: function () {},
            toggleMaximize: function () {},
            isMaximized: function () { return Promise.resolve(false); },
            onResized: function () { return Promise.resolve(function () {}); },
            startDragging: function () {},
          };
        },
      },
      dialog: { open: function () { return Promise.resolve(null); } },
    };
  })();`;
}

const sleep = ms => new Promise(resolve => { setTimeout(resolve, ms); });
const callCount = (page, cmd) => page.evaluate(command =>
  window.__SETTINGS_TEST__.calls.filter(call => call.cmd === command).length, cmd);

async function clickExact(page, text) {
  const ok = await page.evaluate(t => {
    const elements = [...document.querySelectorAll('button,span,div,a,h1,h2')].filter(element => (element.textContent || '').trim() === t);
    const element = elements.find(el => el.tagName === 'BUTTON') || elements[elements.length - 1];
    if (!element) return false;
    const target = element.closest('button') || element;
    target.scrollIntoView({ block: 'center' });
    target.click();
    return true;
  }, text);
  if (!ok) throw new Error('找不到可点击文本: ' + text);
}

async function clickModalExact(page, text) {
  const ok = await page.evaluate(t => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    if (!root) return false;
    const elements = [...root.querySelectorAll('button,span,div,a,h1,h2')].filter(element => (element.textContent || '').trim() === t);
    const element = elements.find(el => el.tagName === 'BUTTON') || elements[elements.length - 1];
    if (!element) return false;
    const target = element.closest('button') || element;
    target.scrollIntoView({ block: 'center' });
    target.click();
    return true;
  }, text);
  if (!ok) {
    const dialogText = await page.evaluate(() => (document.querySelector('[data-testid="model-form-dialog"]')?.innerText || '').slice(0, 600));
    throw new Error('找不到弹窗内可点击文本: ' + text + '\n弹窗文本: ' + dialogText);
  }
}

async function clickSettingsSection(page, label) {
  const ok = await page.evaluate(text => {
    const buttons = [...document.querySelectorAll('aside button')].filter(button => (button.textContent || '').trim() === text);
    const button = buttons[buttons.length - 1];
    if (!button) return false;
    button.click();
    return true;
  }, label);
  if (!ok) throw new Error('找不到设置分区: ' + label);
  await sleep(250);
}

async function clickRowAction(page, rowText, actionText) {
  const ok = await page.evaluate((rowText, actionText) => {
    const row = [...document.querySelectorAll('div')].find(node =>
      typeof node.className === 'string'
      && node.className.includes('grid-cols-')
      && (node.textContent || '').includes(rowText));
    const button = row && [...row.querySelectorAll('button')].find(candidate => (candidate.textContent || '').trim() === actionText);
    if (!button) return false;
    button.scrollIntoView({ block: 'center' });
    button.click();
    return true;
  }, rowText, actionText);
  if (!ok) throw new Error('找不到行操作: ' + rowText + ' / ' + actionText);
}

async function clickRowRadio(page, rowText) {
  const ok = await page.evaluate(rowText => {
    const row = [...document.querySelectorAll('div')].find(node =>
      typeof node.className === 'string'
      && node.className.includes('grid-cols-')
      && (node.textContent || '').includes(rowText));
    const button = row && row.querySelector('button[title]');
    if (!button) return false;
    button.scrollIntoView({ block: 'center' });
    button.click();
    return true;
  }, rowText);
  if (!ok) throw new Error('找不到行单选: ' + rowText);
}

async function modalWidth(page, headingText) {
  return page.evaluate(text => {
    const heading = [...document.querySelectorAll('h2,h3')].find(node => (node.textContent || '').trim() === text);
    if (!heading) return null;
    let node = heading.parentElement;
    while (node && node.getBoundingClientRect().width < 200) node = node.parentElement;
    return node ? Math.round(node.getBoundingClientRect().width) : null;
  }, headingText);
}

(async () => {
  const { url } = await startUiTestServer();
  const browser = await puppeteer.launch({
    executablePath: CHROME,
    headless: 'new',
    args: ['--no-sandbox', '--disable-gpu', '--no-first-run', '--no-default-browser-check'],
    userDataDir: PROFILE,
  });
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push('console:' + message.text()); });
  await page.evaluateOnNewDocument(injectSource());
  await page.setViewport({ width: 1440, height: 1000 });
  await page.goto(url, { waitUntil: 'networkidle0' });
  await page.waitForFunction(() => window.TauriBridge && document.querySelector('[data-testid="app-root"]'), { timeout: 20000 });
  await sleep(1000);

  const results = [];
  const rec = (name, pass, detail = '') => {
    results.push({ name, pass });
    console.log(`${pass ? '✅' : '❌'} ${name}${detail ? '  ' + detail : ''}`);
  };

  await page.evaluate(() => {
    const button = [...document.querySelectorAll('button[title="设置"]')].pop();
    if (button) button.click();
  });
  await page.waitForFunction(() => document.querySelector('[data-testid="app-root"]')?.getAttribute('data-current-view') === 'settings', { timeout: 8000 });
  await sleep(400);
  rec('① 设置页可打开且无错误边界', await page.evaluate(() => document.body.innerText.includes('通用') && !document.body.innerText.includes('设置页加载失败')));

  // Exercise the app shell -> SettingsView callback and actual bridge payload:
  // an explicit light preference must not follow a darkened OS, "system"
  // saves the authoritative color_scheme plus the legacy theme mirror, and an
  // explicit dark pick overrides a light OS.
  await page.emulateMediaFeatures([{ name: 'prefers-color-scheme', value: 'dark' }]);
  // The .dark class is toggled from a passive useEffect on the media query,
  // so an absence assertion needs the media change to be observed and one
  // render committed first — otherwise the check can read the pre-effect DOM
  // and vacuously pass exactly when explicit light wrongly follows the OS.
  await page.waitForFunction(() => window.matchMedia('(prefers-color-scheme: dark)').matches);
  await page.evaluate(() => new Promise(resolve => {
    requestAnimationFrame(() => { requestAnimationFrame(resolve); });
  }));
  await sleep(200);
  rec('① 旧显式浅色偏好不随系统变深', await page.evaluate(() => !document.documentElement.classList.contains('dark')));
  await clickExact(page, '跟随系统');
  await page.waitForFunction(() => document.documentElement.classList.contains('dark')
    && window.__SETTINGS_TEST__.settings().color_scheme === 'system');
  rec('① 跟随系统保存权威偏好并兼容旧主题字段', await page.evaluate(() =>
    window.__SETTINGS_TEST__.settings().theme === 'genesis'));
  await page.emulateMediaFeatures([{ name: 'prefers-color-scheme', value: 'light' }]);
  await page.waitForFunction(() => !document.documentElement.classList.contains('dark'));
  await clickExact(page, '深色');
  await page.waitForFunction(() => document.documentElement.classList.contains('dark')
    && window.__SETTINGS_TEST__.settings().color_scheme === 'dark');
  rec('① 显式深色覆盖系统浅色', await page.evaluate(() =>
    window.__SETTINGS_TEST__.settings().theme === 'genesis'));
  await clickExact(page, '浅色');
  await page.waitForFunction(() => !document.documentElement.classList.contains('dark')
    && window.__SETTINGS_TEST__.settings().color_scheme === 'light');
  rec('① 显式浅色恢复初始偏好与旧主题镜像', await page.evaluate(() =>
    window.__SETTINGS_TEST__.settings().color_scheme === 'light'
      && window.__SETTINGS_TEST__.settings().theme === 'liquid-light'));

  const voiceShortcutRow = await page.evaluate(() => {
    const row = [...document.querySelectorAll('div')].filter(node => (node.textContent || '').includes('启用 Alt 唤醒语音') && node.querySelector('[role="switch"]')).pop();
    const button = row && row.querySelector('[role="switch"]');
    return {
      exists: !!row,
      enabled: !!button && !button.disabled,
      nativeDesc: !!row && (row.textContent || '').includes('Alt 可语音输入；输入框已有文本时'),
      unsupportedDescHidden: !!row && !(row.textContent || '').includes('仅 Windows 支持'),
    };
  });
  rec('①e voice shortcut row renders enabled (not greyed out) on a supported platform', Object.values(voiceShortcutRow).every(Boolean), JSON.stringify(voiceShortcutRow));

  // The info entry reuses the first-use intro card: clicking it must open the
  // intro modal (without triggering ASR checks, downloads, or recording).
  await page.click('[data-testid="voice-shortcut-info"]');
  await page.waitForFunction(() => document.querySelector('#voice-shortcut-intro-title'), { timeout: 4000 });
  rec('①f shortcut info entry opens the intro card', await page.evaluate(() =>
    (document.querySelector('#voice-shortcut-intro-title')?.textContent || '').length > 0));
  await page.keyboard.press('Escape');
  await page.waitForFunction(() => !document.querySelector('#voice-shortcut-intro-title'), { timeout: 4000 });
  rec('①g Esc closes the intro card', await page.evaluate(() =>
    !document.querySelector('#voice-shortcut-intro-title')));

  await page.click('[data-testid="settings-section-memory"]');
  await page.waitForFunction(() => (document.querySelector('[data-testid="memory-profile-call-name"]')?.textContent || '').includes('升级前称呼'));
  rec('①a 升级后的记忆资料从权威 profile 正常回显', await page.evaluate(() =>
    (document.querySelector('[data-testid="memory-profile-call-name"]')?.textContent || '').includes('升级前称呼')));
  await page.evaluate(() => window.__SETTINGS_TEST__.setFailMemoryOverview(true));
  await page.click('[data-testid="memory-profile-call-name"] button');
  await page.click('[data-testid="memory-editor-input"]', { clickCount: 3 });
  await page.type('[data-testid="memory-editor-input"]', '升级后称呼');
  await page.click('[data-testid="memory-editor-save"]');
  await page.waitForFunction(() => !document.querySelector('[data-testid="memory-editor-input"]'));
  rec('①b 派生概览刷新失败不吞掉已保存的称呼', await page.evaluate(() =>
    (document.querySelector('[data-testid="memory-profile-call-name"]')?.textContent || '').includes('升级后称呼')));
  rec('①c 记忆加载失败有明确提示且不伪装成未设置', await page.evaluate(() =>
    document.querySelector('[data-testid="memory-settings-error"]')?.getAttribute('role') === 'alert'
      && document.querySelector('[data-testid="memory-settings-error"]')?.getAttribute('aria-live') === 'polite'));
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.setFailMemoryOverview(false);
    window.__SETTINGS_TEST__.setFailMemoryUpdate(true);
  });
  await page.click('[data-testid="memory-profile-call-name"] button');
  await page.click('[data-testid="memory-editor-input"]', { clickCount: 3 });
  await page.type('[data-testid="memory-editor-input"]', '不会保存');
  await page.click('[data-testid="memory-editor-save"]');
  await page.waitForFunction(() => !!document.querySelector('[data-testid="memory-editor-error"]'));
  rec('①d 源资料保存失败时保留编辑器并展示错误', await page.evaluate(() =>
    !!document.querySelector('[data-testid="memory-editor-input"]')
      && document.querySelector('[data-testid="memory-editor-error"]')?.getAttribute('role') === 'alert'
      && document.querySelector('[data-testid="memory-editor-error"]')?.getAttribute('aria-live') === 'assertive'));
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.setFailMemoryUpdate(false);
    const input = document.querySelector('[data-testid="memory-editor-input"]');
    const dialog = input && input.closest('.fixed');
    const buttons = dialog ? [...dialog.querySelectorAll('button')] : [];
    if (buttons.length) buttons[0].click();
  });

  // ①e memory delete: row delete button → in-app confirm dialog (no native
  // confirm in WebView2) → confirming really invokes the delete command and
  // removes the row, leaving the sibling item untouched.
  await page.waitForFunction(() => !!document.querySelector('[data-testid="memory-item-delete"]'));
  await page.click('[data-testid="memory-item-delete"]');
  await page.waitForFunction(() => !!document.querySelector('[data-testid="memory-delete-confirm"]'));
  await page.click('[data-testid="memory-delete-confirm-ok"]');
  await page.waitForFunction(() => !document.querySelector('[data-testid="memory-delete-confirm"]'));
  await page.waitForFunction(() => !document.body.innerText.includes('待删除的偏好记忆'), { timeout: 5000 });
  rec('①e memory delete removes the item after in-app confirmation', await page.evaluate(() =>
    !document.body.innerText.includes('待删除的偏好记忆')
      && document.body.innerText.includes('保留的偏好记忆')
      && window.__SETTINGS_TEST__.calls.some(function (item) { return item.cmd === 'delete_memory_preference'; })));

  // ①f the work-context row delete goes through the same confirm path (the
  // bridge maps the kind to delete_work_context_memory). ①e already removed the
  // first preference row, so the last remaining delete button in the long-term
  // memory list belongs to the work-context row.
  await page.evaluate(() => {
    const buttons = [...document.querySelectorAll('[data-testid="memory-item-delete"]')];
    if (buttons.length) buttons[buttons.length - 1].click();
  });
  await page.waitForFunction(() => !!document.querySelector('[data-testid="memory-delete-confirm"]'));
  await page.click('[data-testid="memory-delete-confirm-ok"]');
  await page.waitForFunction(() => !document.body.innerText.includes('待删除的工作上下文'), { timeout: 5000 });
  rec('①f work-context delete goes through the same confirm path', await page.evaluate(() =>
    !document.body.innerText.includes('待删除的工作上下文')
      && document.body.innerText.includes('保留的偏好记忆')
      && window.__SETTINGS_TEST__.calls.some(function (item) { return item.cmd === 'delete_work_context_memory'; })));

  await clickSettingsSection(page, '更新');
  // A failed manual check (offline, blocked request, ...) shows only the short localized hint, never the raw backend error.
  await page.evaluate(async () => {
    window.__SETTINGS_TEST__.setUpdateCheckFailure('update request failed: error sending request for url');
    await window.TauriBridge.updater.checkForUpdate();
  });
  await sleep(250);
  const updateCheckFailureText = await page.evaluate(() => document.querySelector('#settings-version-update')?.innerText || '');
  rec('update: a failed manual check shows only the short localized hint',
    updateCheckFailureText.includes('检查失败')
    && !updateCheckFailureText.includes('update request failed')
    && !updateCheckFailureText.includes('error sending request'),
    updateCheckFailureText);
  await page.evaluate(async () => {
    window.__SETTINGS_TEST__.setUpdateCheckFailure(null);
    window.__SETTINGS_TEST__.setUpdateResponse({
      available: true,
      current_version: '0.6.1',
      latest_version: '0.6.2',
      notes: '设置页更新按钮测试',
      platform: 'windows',
    });
    await window.TauriBridge.updater.checkForUpdate();
  });
  await sleep(500);
  const beforeDownloadCalls = await callCount(page, 'download_update');
  await page.click('#settings-version-update [data-settings-update-action="true"]');
  await sleep(500);
  const updateDownloadState = await page.evaluate(() => {
    const root = document.querySelector('#settings-version-update');
    const button = root && root.querySelector('button');
    return {
      disabled: !!(button && button.disabled),
      text: button ? button.textContent.trim() : '',
      desc: root ? root.innerText : '',
    };
  });
  const afterDownloadCalls = await callCount(page, 'download_update');
  // The update:progress listener is back as the dormant frontend contract
  // (nothing emits the event today), so the downloading state still only
  // asserts the button entering "取消下载" and the description entering
  // "正在下载更新" copy, without asserting a specific percentage.
  rec('①b 设置页下载按钮进入下载态后可取消',
    beforeDownloadCalls === 0
    && afterDownloadCalls === 1
    && !updateDownloadState.disabled
    && updateDownloadState.text.includes('取消下载')
    && updateDownloadState.desc.includes('正在下载更新'),
    JSON.stringify(updateDownloadState));
  await page.click('[data-testid="settings-section-update"]');
  await page.waitForFunction(() => !!document.querySelector('#settings-version-update'));
  await page.click('#settings-version-update [data-settings-update-action="true"]');
  await sleep(150);
  rec('①c 设置页可取消正在进行的更新下载', await callCount(page, 'cancel_download') === 1);
  await page.evaluate(() => window.__SETTINGS_TEST__.resolveDownload());
  await sleep(250);

  // ①u A failed download/install shows only the localized short hint: the raw
  // backend error string (it may carry request details and its language is
  // undefined) never enters the card — it stays on bridge state and the
  // updater's log as diagnostics. Do not await the returned promise: the
  // download hangs until the reject hook fires, and awaiting would stall CDP.
  await page.evaluate(() => { window.TauriBridge.updater.downloadAndInstallUpdate(); });
  await sleep(250);
  await page.evaluate(() => window.__SETTINGS_TEST__.rejectDownload('RAW update diagnostics: apt stderr http://proxy.internal'));
  await sleep(250);
  const updateFailureText = await page.evaluate(() => {
    const root = document.querySelector('#settings-version-update');
    return root ? root.innerText : '';
  });
  rec('①u 更新失败仅显示本地化提示,原始错误串不进卡片',
    updateFailureText.includes('更新失败')
    && !updateFailureText.includes('RAW update diagnostics')
    && !updateFailureText.includes('proxy.internal'),
    JSON.stringify(updateFailureText));
  // Note: bs.updateError stays set on the bridge after this rec (the bridge only
  // clears it at the start of the next download). Harmless today because no
  // later test reads the update card, but any future update-section test must
  // run a fresh downloadAndInstallUpdate (or reset state) before asserting.

  await clickSettingsSection(page, '模型');
  const modelList = await page.evaluate(() => {
    const text = document.body.innerText;
    const rows = [...document.querySelectorAll('div')].filter(node => typeof node.className === 'string' && node.className.includes('grid-cols-'));
    const deepseekRow = rows.find(node => (node.textContent || '').includes('deepseek-v4-flash'));
    const localRow = rows.find(node => (node.textContent || '').includes('qwen36_35b_256k') && (node.textContent || '').includes('本地模型'));
    return {
      localTag: text.includes('本地模型'),
      activeTag: text.includes('默认'),
      noStatusNoise: !text.includes('已配置') && !text.includes('未配置'),
      localHasDelete: !!localRow && (localRow.textContent || '').includes('删除'),
      qwenLocalUsesBrandIcon: !!localRow && !!localRow.querySelector('img'),
      cloudHasDelete: !!deepseekRow,
      oldSaveSearchButtonHidden: !text.includes('保存搜索配置'),
    };
  });
  rec('② 模型列表符合 iOS 列表交互标识且用户本地模型可删除', Object.values(modelList).every(Boolean), JSON.stringify(modelList));

  await clickRowRadio(page, 'deepseek-v4-flash');
  await sleep(350);
  const activeSet = await page.evaluate(() => window.__SETTINGS_TEST__.activeModelId());
  rec('③ 圆圈单选可设默认模型', activeSet === 'cloud-deepseek', activeSet);

  await clickRowAction(page, 'deepseek-v4-flash', '编辑');
  await sleep(250);
  const maskedSavedKey = await page.evaluate(() => {
    const keyInput = document.querySelector('[data-testid="model-form-dialog"] [data-testid="model-api-key-input"]');
    return {
      maskedPlaceholder: [...document.querySelectorAll('input')].some(node => node.placeholder === '••••••••'),
      noConfiguredText: !document.body.innerText.includes('已配置'),
      // 掩码不变量:输入框是 type=text,遮挡完全来自 -webkit-text-security。
      // 没有这条断言,删掉那行 style 就会让 Key 明文渲染而所有门禁照样全绿。
      maskedByTextSecurity: !!keyInput && window.getComputedStyle(keyInput).webkitTextSecurity === 'disc',
    };
  });
  await clickExact(page, '显示');
  await sleep(350);
  const editModelBehavior = await page.evaluate(() => {
    const text = document.body.innerText;
    const input = [...document.querySelectorAll('input')].find(node => node.value === 'sk-saved-deepseek');
    const keyInput = document.querySelector('[data-testid="model-form-dialog"] [data-testid="model-api-key-input"]');
    return {
      revealCall: window.__SETTINGS_TEST__.calls.some(call => call.cmd === 'reveal_model_api_key' && call.args.id === 'cloud-deepseek'),
      keyRevealed: !!input,
      // 「显示」必须真正撤掉掩码,而不仅仅是把值取回来。
      unmaskedAfterReveal: !!keyInput && window.getComputedStyle(keyInput).webkitTextSecurity === 'none',
      sameProviderOnlyClosed: !text.includes('kimi-k3') && !text.includes('glm-5.2'),
      // 带 provider_kind 的官方模型必须仍能找到目录组,配置区与测试连接不被隐藏。
      testConnectionVisible: text.includes('测试连接'),
    };
  });
  await clickExact(page, '更换');
  await sleep(250);
  const sameProviderPicker = await page.evaluate(() => {
    const text = document.body.innerText;
    // The DeepSeek group now lists only deepseek-flash (the new mainline) +
    // deepseek-v4-pro; the deleted row
    // deepseek-v4-flash survives only as a legacyAliases compatibility entry
    // and no longer appears in the clickable catalog.
    return text.includes('deepseek-flash') && text.includes('deepseek-v4-pro') && !text.includes('kimi-k3') && !text.includes('glm-5.2');
  });
  rec('④ 编辑模型默认掩码显示已保存 Key，显示后回显且只允许同厂商更换', Object.values(maskedSavedKey).every(Boolean) && Object.values(editModelBehavior).every(Boolean) && sameProviderPicker, JSON.stringify({ ...maskedSavedKey, ...editModelBehavior, sameProviderPicker }));
  await clickExact(page, '取消');
  await sleep(200);

  await clickExact(page, '添加模型');
  await sleep(250);
  await clickExact(page, '深度求索 / DeepSeek');
  await sleep(250);
  await clickModalExact(page, '模型');
  await sleep(250);
  await clickModalExact(page, '自定义 DeepSeek 模型');
  await sleep(250);
  const modelIdInput = await page.$('input[placeholder="输入模型 ID"]');
  await modelIdInput.click();
  await modelIdInput.type('custom-model-id');
  const typedModelId = await page.evaluate(() => document.querySelector('input[placeholder="输入模型 ID"]')?.value || '');
  rec('④ 模型 ID 输入框可连续输入', typedModelId === 'custom-model-id', typedModelId);
  await clickExact(page, '取消');
  await sleep(200);

  const deleteBefore = await callCount(page, 'delete_model');
  await clickRowAction(page, 'deepseek-v4-flash', '删除');
  await sleep(250);
  const deleteDialog = await page.evaluate(() => document.body.innerText.includes('删除模型？') && document.body.innerText.includes('将移除该模型配置'));
  const deleteWidth = await modalWidth(page, '删除模型？');
  const deleteStillBeforeConfirm = await callCount(page, 'delete_model');
  rec('④ 删除模型先出 iOS 二次确认且未立即删除', deleteDialog && deleteWidth >= 260 && deleteWidth <= 285 && deleteStillBeforeConfirm === deleteBefore, `width=${deleteWidth}`);
  await clickExact(page, '取消');
  await sleep(200);
  await clickRowAction(page, 'deepseek-v4-flash', '删除');
  await sleep(200);
  await clickExact(page, '删除模型');
  await sleep(500);
  const deleted = await page.evaluate(() => ({
    calls: window.__SETTINGS_TEST__.calls.filter(call => call.cmd === 'delete_model').map(call => call.args.id),
    remaining: window.__SETTINGS_TEST__.models().map(model => model.id),
  }));
  rec('⑤ 确认后才调用删除模型并刷新列表', deleted.calls.includes('cloud-deepseek') && !deleted.remaining.includes('cloud-deepseek'), JSON.stringify(deleted));

  // 思考深度档位迁移：编辑 Kimi Global 自定义模型（已存 off），手输改 ID 为 kimi-k3 后
  // 档位表由 off/high 变为 low/high/max，off 不在其中，必须按新 route 归一为 low——
  // 否则界面无高亮、保存仍写旧值（底座 K3 实际按 low 执行）。
  await clickRowAction(page, 'custom-kimi-id', '编辑');
  await sleep(300);
  await page.evaluate(() => {
    const input = document.querySelector('[data-testid="model-form-dialog"] input[placeholder="输入模型 ID"]');
    if (!input) return;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(input, 'kimi-k3');
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await sleep(300);
  const kimiEffortMigration = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    if (!dialog) return { found: false, labels: [], selected: [] };
    const label = [...dialog.querySelectorAll('span')].find(node => (node.textContent || '').trim() === '思考深度');
    const row = label && label.parentElement;
    const buttons = row ? [...row.querySelectorAll('button')] : [];
    const selected = buttons.filter(node => (node.className || '').includes('bg-[#007AFF]')).map(node => (node.textContent || '').trim());
    return { found: !!row, labels: buttons.map(node => (node.textContent || '').trim()), selected };
  });
  rec('⑥ 自定义模型 ID 手输 kimi-k3 后思考深度归一为 low（关闭残留 off）',
    kimiEffortMigration.found
      && kimiEffortMigration.labels.includes('低')
      && !kimiEffortMigration.labels.includes('关闭')
      && kimiEffortMigration.selected.length === 1
      && kimiEffortMigration.selected[0] === '低',
    JSON.stringify(kimiEffortMigration));
  await clickExact(page, '取消');
  await sleep(200);

  await clickExact(page, '添加模型');
  await sleep(300);
  const freshCatalog = await page.evaluate(() => {
    const text = document.body.innerText;
    const stale = ['deepseek-chat', 'kimi-k2.5', 'glm-4-plus', 'minimax-m1', 'abab6.5s-chat', 'mimo-v2-flash', 'qwen-max', 'qwen-plus', 'qwen-turbo', 'doubao-pro-256k', 'gpt-4o', 'qwen3.8-max-preview'];
    return {
      hasSections: text.includes('Coding Plan') && text.includes('官方 API') && text.includes('自定义兼容接口'),
      hasProviders: text.includes('智谱 Coding Plan / GLM Coding Plan') && text.includes('Kimi Coding Plan') && text.includes('深度求索 / DeepSeek') && text.includes('MiniMax 中国版 / MiniMax China'),
      hasOverseas: text.includes('OpenAI') && text.includes('Anthropic Claude') && text.includes('Google Gemini') && text.includes('xAI Grok'),
      hasIntlNodes: text.includes('Kimi 国际版 / Kimi Global') && text.includes('智谱国际版 / GLM API (z.ai)')
        && text.includes('MiniMax 国际版 / MiniMax Global') && text.includes('通义千问国际版 / Qwen International'),
      hasTokenPlan: text.includes('通义千问 Token Plan'),
      hasAggregators: text.includes('聚合平台') && text.includes('OpenRouter') && text.includes('硅基流动 SiliconFlow'),
      providerFirst: !text.includes('deepseek-v4-pro') && !text.includes('kimi-k3'),
      noStale: stale.every(name => !text.includes(name)),
    };
  });
  rec('⑥ 添加模型首屏按厂商分组且不展示已下线推荐项', Object.values(freshCatalog).every(Boolean), JSON.stringify(freshCatalog));
  const addPickerDefault = await page.evaluate(() => {
    const text = document.body.innerText;
    return {
      hasCloudTab: [...document.querySelectorAll('button')].some(button => (button.textContent || '').trim() === '云端模型'),
      hasLocalTab: [...document.querySelectorAll('button')].some(button => (button.textContent || '').trim() === '本地模型'),
      cloudCatalogVisible: text.includes('智谱 Coding Plan / GLM Coding Plan') && text.includes('深度求索 / DeepSeek'),
      localPickerHidden: !text.includes('自动检测本地模型') && !text.includes('手动添加本地模型'),
    };
  });
  rec('⑥.1 添加模型默认展示云端 tab 且保留本地 tab 入口', Object.values(addPickerDefault).every(Boolean), JSON.stringify(addPickerDefault));

  // ⑥.2 长列表可达性守卫：云端目录远超弹窗 max-h，滚动必须发生在内层
  // model-form-scroll 容器上，且能一路滚到底部（模型列表后半部分必须可选）。
  const pickerScroll = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const scroller = document.querySelector('[data-testid="model-form-scroll"]');
    const rect = dialog ? dialog.getBoundingClientRect() : null;
    return {
      found: !!dialog && !!scroller,
      overflowY: scroller ? getComputedStyle(scroller).overflowY : null,
      capped: dialog ? (parseFloat(getComputedStyle(dialog).maxHeight) || 0) > 0 : false,
      overflowing: !!scroller && scroller.scrollHeight > scroller.clientHeight,
      center: rect ? { x: Math.round(rect.x + rect.width / 2), y: Math.round(rect.y + Math.min(rect.height / 2, 200)) } : null,
    };
  });
  let pickerScrollAfter = { top: -1, reachedEnd: false };
  if (pickerScroll.found && pickerScroll.overflowing && pickerScroll.center) {
    await page.mouse.move(pickerScroll.center.x, pickerScroll.center.y);
    await page.mouse.wheel({ deltaY: 600 });
    await sleep(250);
    for (let i = 0; i < 8; i += 1) {
      await page.mouse.wheel({ deltaY: 1200 });
      await sleep(100);
    }
    pickerScrollAfter = await page.evaluate(() => {
      const scroller = document.querySelector('[data-testid="model-form-scroll"]');
      return {
        top: scroller.scrollTop,
        reachedEnd: Math.abs(scroller.scrollTop + scroller.clientHeight - scroller.scrollHeight) <= 2,
      };
    });
  }
  rec('⑥.2 添加模型弹窗长列表可在内层滚动容器一路滚到底（列表尾部可达）',
    pickerScroll.found
      && pickerScroll.overflowY === 'auto'
      && pickerScroll.capped
      && pickerScroll.overflowing
      && pickerScrollAfter.top > 0
      && pickerScrollAfter.reachedEnd === true,
    JSON.stringify({ pickerScroll, pickerScrollAfter }));

  await clickExact(page, '通义千问 Token Plan');
  await sleep(300);
  await clickModalExact(page, '模型');
  await sleep(200);
  const tokenPlanModels = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const lines = (root ? root.innerText : '').split('\n').map(line => line.trim());
    return {
      hasGa: lines.includes('qwen3.8-max'),
      hasFlash38: lines.includes('qwen3.8-flash'),
      hasFlash: lines.includes('qwen3.6-flash'),
      noPreview: !lines.includes('qwen3.8-max-preview'),
    };
  });
  rec('⑥.1d Token Plan 条目列正式旗舰与白名单 Flash，且不展示已下线预览版', Object.values(tokenPlanModels).every(Boolean), JSON.stringify(tokenPlanModels));
  await clickExact(page, '取消');
  await sleep(300);

  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '豆包');
  await sleep(300);
  await clickModalExact(page, '模型');
  await sleep(200);
  const doubaoModels = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const text = root ? root.innerText : '';
    return {
      hasCanonicalId: text.includes('doubao-seed-2-1-pro-260628'),
      noLegacyId: !text.includes('doubao-seed-2.1-pro'),
    };
  });
  rec('⑥.1e 豆包条目使用火山方舟规范模型 ID', Object.values(doubaoModels).every(Boolean), JSON.stringify(doubaoModels));
  await clickExact(page, '取消');
  await sleep(300);

  await clickExact(page, '添加模型');
  await sleep(300);

  await clickExact(page, '智谱 Coding Plan / GLM Coding Plan');
  await sleep(300);
  const codingPlanBeforePick = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const text = root ? root.innerText : '';
    return {
      title: text.includes('添加 智谱 Coding Plan'),
      // The 2026-09 catalog default first item is the flagship GLM-5.3;
      // GLM-5-Turbo stays as a "legacy model" entry.
      defaultModel: text.includes('GLM-5.3'),
      noDisplayNameField: !text.includes('显示名'),
      noServiceUrlField: !text.includes('服务地址') && !(root && [...root.querySelectorAll('input')].some(input => input.value === 'https://open.bigmodel.cn/api/coding/paas/v4')),
      noNativeSelect: document.querySelectorAll('[data-testid="model-form-dialog"] select').length === 0,
    };
  });
  await clickModalExact(page, '模型');
  await sleep(200);
  await clickModalExact(page, 'GLM-5-Turbo');
  await sleep(200);
  const codingApiInput = await page.$('input[placeholder="输入 API Key"]');
  await codingApiInput.type('sk-coding-plan');
  await sleep(150);
  await page.evaluate(() => window.__SETTINGS_TEST__.setModelTestResponse({
    ok: false,
    code: 'auth_invalid',
    message: 'API Key 无效，请检查后重新填写',
    detail: 'HTTP 401',
    http_status: 401,
  }));
  await clickExact(page, '测试连接');
  await sleep(250);
  const codingPlanTestResult = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const text = root ? root.innerText : '';
    return {
      friendlyMessage: text.includes('API Key 无效，请检查后重新填写'),
      noTechnicalDetail: !text.includes('技术详情') && !text.includes('HTTP 401'),
      noAddressHint: !text.includes('改地址') && !text.includes('修改服务地址'),
      called: window.__SETTINGS_TEST__.calls.some(call => call.cmd === 'test_model_connection' && call.args.baseUrl === 'https://open.bigmodel.cn/api/coding/paas/v4'),
    };
  });
  rec('⑥.1a Coding Plan 测试连接只显示友好错误且不提示改地址', Object.values(codingPlanTestResult).every(Boolean), JSON.stringify(codingPlanTestResult));
  await clickExact(page, '保存');
  await sleep(500);
  const savedCodingPlan = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return call && call.args && call.args.model;
  });
  rec('⑥.1b Coding Plan 隐藏服务地址并保存专用地址',
    Object.values(codingPlanBeforePick).every(Boolean)
      && savedCodingPlan
      && savedCodingPlan.provider_kind === 'coding_plan'
      && savedCodingPlan.vendor === 'glm'
      && savedCodingPlan.model === 'glm-5-turbo'
      && savedCodingPlan.name === 'glm-5-turbo'
      && savedCodingPlan.base_url === 'https://open.bigmodel.cn/api/coding/paas/v4'
      && savedCodingPlan.api_key === 'sk-coding-plan'
      && savedCodingPlan.credential_action === 'replace',
    JSON.stringify({ ...codingPlanBeforePick, savedCodingPlan }));

  await clickExact(page, '添加模型');
  await sleep(250);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-backdrop"]')?.click());
  await sleep(200);
  const addPickerOutsideClick = await page.evaluate(() => {
    const stayedOpen = !!document.querySelector('[data-testid="model-form-dialog"]');
    document.querySelector('[data-testid="model-form-cancel"]')?.click();
    return { stayedOpen };
  });
  await sleep(200);
  const addPickerClosedByCancel = await page.evaluate(() => !document.querySelector('[data-testid="model-form-dialog"]'));
  rec('⑥.1c 添加模型选择弹窗点击外部区域不关闭且取消可关闭',
    addPickerOutsideClick.stayedOpen && addPickerClosedByCancel,
    JSON.stringify({ ...addPickerOutsideClick, closedByCancel: addPickerClosedByCancel }));

  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '本地模型');
  await sleep(250);
  const localPickerInitial = await page.evaluate(() => {
    const text = document.body.innerText;
    const localRows = ['自动检测本地模型', '手动添加本地模型'].map(label => {
      const title = [...document.querySelectorAll('span')].find(node => (node.textContent || '').trim() === label);
      let row = title;
      while (row && !String(row.className || '').includes('items-center')) row = row.parentElement;
      return row;
    }).filter(Boolean);
    return {
      hasAutoDetect: text.includes('自动检测本地模型') && text.includes('检测 vLLM、Ollama、LM Studio'),
      hasManual: text.includes('手动添加本地模型') && text.includes('填写 API 地址和模型 ID'),
      cloudCatalogHidden: !text.includes('深度求索 / DeepSeek'),
      localIconIsGeneric: localRows.length === 2 && localRows.every(row => !row.querySelector('img') && row.querySelector('svg')),
    };
  });
  rec('⑥.2 本地模型 tab 只保留检测和手动添加两条主路径', Object.values(localPickerInitial).every(Boolean), JSON.stringify(localPickerInitial));
  await clickExact(page, '检测');
  await sleep(500);
  const localDetectUi = await page.evaluate(() => {
    const text = document.body.innerText;
    return {
      called: window.__SETTINGS_TEST__.calls.some(call => call.cmd === 'discover_local_vllm'),
      ollamaModel: text.includes('qwen2.5-coder:32b') && text.includes('deepseek-r1:14b'),
      vllmModel: text.includes('qwen36_35b_256k'),
      providerLine: text.includes('Ollama · http://127.0.0.1:11434/v1') && text.includes('vLLM · http://127.0.0.1:8000/v1'),
      notLoadedTag: text.includes('未加载') && text.includes('尚未载入内存，首次使用时会自动加载'),
      loadedSortedFirst: text.includes('qwen2.5-coder:32b') && text.includes('deepseek-r1:14b')
        && text.indexOf('qwen2.5-coder:32b') < text.indexOf('deepseek-r1:14b'),
    };
  });
  rec('⑥.3 本地模型自动检测展示多个服务与多个模型 ID', Object.values(localDetectUi).every(Boolean), JSON.stringify(localDetectUi));
  // Custom port: fill 8080 and re-detect; the probe request must pass through
  // customPort=8080 (the main path for picking a local model by port).
  // The value is written through the native value setter: clearing a controlled
  // input via select+backspace is unreliable.
  const setPortDraft = value => page.evaluate(text => {
    const input = document.querySelector('[data-testid="local-detect-port"]');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, text);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  }, value);
  await setPortDraft('8080');
  await clickExact(page, '重新检测');
  await sleep(500);
  const localDetectWithPort = await page.evaluate(() => {
    const calls = window.__SETTINGS_TEST__.calls.filter(call => call.cmd === 'discover_local_vllm');
    const last = calls[calls.length - 1];
    return { request: last && last.args && last.args.request };
  });
  rec('⑥.3b custom port is passed through to the discovery command',
    localDetectWithPort.request
      && localDetectWithPort.request.customPort === 8080
      && localDetectWithPort.request.currentBaseUrl === null
      && localDetectWithPort.request.savedBaseUrl === null,
    JSON.stringify(localDetectWithPort));
  // Invalid port (out of range): inline error, no probe request, and the
  // previously detected candidates stay visible.
  const discoverCallsBeforeInvalidPort = await page.evaluate(() =>
    window.__SETTINGS_TEST__.calls.filter(call => call.cmd === 'discover_local_vllm').length);
  await setPortDraft('99999');
  await clickExact(page, '重新检测');
  await sleep(300);
  const localDetectInvalidPort = await page.evaluate(before => ({
    errorShown: document.body.innerText.includes('端口无效：请输入 1-65535 之间的数字'),
    noNewCall: window.__SETTINGS_TEST__.calls.filter(call => call.cmd === 'discover_local_vllm').length === before,
    candidatesKept: document.body.innerText.includes('qwen2.5-coder:32b'),
  }), discoverCallsBeforeInvalidPort);
  rec('⑥.3c invalid port errors inline without probing and keeps previous candidates', Object.values(localDetectInvalidPort).every(Boolean), JSON.stringify(localDetectInvalidPort));
  // Negative and decimal inputs: the raw value stays as typed, an inline error
  // is shown, and no probe request is issued (the input must not silently
  // rewrite -1 into 1 or 1.5 into 15 before validation).
  const discoverCallsBeforeNegativePort = await page.evaluate(() =>
    window.__SETTINGS_TEST__.calls.filter(call => call.cmd === 'discover_local_vllm').length);
  await setPortDraft('-1');
  await clickExact(page, '重新检测');
  await sleep(300);
  const localDetectNegativePort = await page.evaluate(before => ({
    rawValueKept: document.querySelector('[data-testid="local-detect-port"]').value === '-1',
    errorShown: document.body.innerText.includes('端口无效：请输入 1-65535 之间的数字'),
    noNewCall: window.__SETTINGS_TEST__.calls.filter(call => call.cmd === 'discover_local_vllm').length === before,
  }), discoverCallsBeforeNegativePort);
  rec('⑥.3e negative port keeps the raw input, errors inline, issues no probe', Object.values(localDetectNegativePort).every(Boolean), JSON.stringify(localDetectNegativePort));
  const discoverCallsBeforeDecimalPort = await page.evaluate(() =>
    window.__SETTINGS_TEST__.calls.filter(call => call.cmd === 'discover_local_vllm').length);
  await setPortDraft('1.5');
  await clickExact(page, '重新检测');
  await sleep(300);
  const localDetectDecimalPort = await page.evaluate(before => ({
    rawValueKept: document.querySelector('[data-testid="local-detect-port"]').value === '1.5',
    errorShown: document.body.innerText.includes('端口无效：请输入 1-65535 之间的数字'),
    noNewCall: window.__SETTINGS_TEST__.calls.filter(call => call.cmd === 'discover_local_vllm').length === before,
  }), discoverCallsBeforeDecimalPort);
  rec('⑥.3f decimal port keeps the raw input, errors inline, issues no probe', Object.values(localDetectDecimalPort).every(Boolean), JSON.stringify(localDetectDecimalPort));
  // Clear the port and run a valid detection; this also locks the request shape
  // for "empty port = default ports only" (customPort null).
  await setPortDraft('');
  await clickExact(page, '重新检测');
  await sleep(500);
  const localDetectPortCleared = await page.evaluate(() => {
    const calls = window.__SETTINGS_TEST__.calls.filter(call => call.cmd === 'discover_local_vllm');
    const last = calls[calls.length - 1];
    return { request: last && last.args && last.args.request };
  });
  rec('⑥.3d cleared port sends a discovery request without a custom port',
    localDetectPortCleared.request
      && localDetectPortCleared.request.customPort === null
      && localDetectPortCleared.request.currentBaseUrl === null
      && localDetectPortCleared.request.savedBaseUrl === null,
    JSON.stringify(localDetectPortCleared));
  await sleep(100);
  const localAddClicked = await page.evaluate(() => {
    const title = [...document.querySelectorAll('span')].find(node => (node.textContent || '').trim() === 'qwen2.5-coder:32b');
    let row = title;
    while (row && ![...row.querySelectorAll('button')].some(button => (button.textContent || '').trim() === '添加')) {
      row = row.parentElement;
    }
    const button = row && [...row.querySelectorAll('button')].find(candidate => (candidate.textContent || '').trim() === '添加');
    if (!button) return false;
    button.click();
    return true;
  });
  await sleep(500);
  const savedLocalModel = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return call && call.args && call.args.model;
  });
  rec('⑥.4 检测候选可一键添加为本地模型且不强制 API Key',
    localAddClicked
      && savedLocalModel
      && savedLocalModel.preset === 'local_vllm'
      && savedLocalModel.model === 'qwen2.5-coder:32b'
      && savedLocalModel.base_url === 'http://127.0.0.1:11434/v1'
      // Ollama 的 max_model_len 是运行时生效值（/api/ps），不持久化为用户
      // 声明——声明会永久 min-clamp 后续探测且表单对该预设隐藏本字段；窗口
      // 由引擎每次 spawn 原生探测实时采纳（vLLM 候选仍持久化其部署配置）。
      && savedLocalModel.context_window_tokens === null
      && savedLocalModel.api_key === ''
      && savedLocalModel.credential_action === 'keep_existing',
    JSON.stringify(savedLocalModel));

  // ⑥.5 The other two arms of the persistence ternary: LM Studio persists
  // null too (its loaded_context_length is runtime-effective like Ollama's
  // /api/ps), and a vLLM row keeps persisting its deployment max_model_len —
  // an always-null mutation of buildLocalModelPayload must go red here.
  await setPortDraft('1234');
  await clickExact(page, '重新检测');
  await sleep(500);
  const lmstudioAddClicked = await page.evaluate(() => {
    const title = [...document.querySelectorAll('span')].find(node => (node.textContent || '').trim() === 'gemma-3-12b');
    let row = title;
    while (row && ![...row.querySelectorAll('button')].some(button => (button.textContent || '').trim() === '添加')) {
      row = row.parentElement;
    }
    const button = row && [...row.querySelectorAll('button')].find(candidate => (candidate.textContent || '').trim() === '添加');
    if (!button) return false;
    button.click();
    return true;
  });
  await sleep(500);
  const savedLmstudioModel = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return call && call.args && call.args.model;
  });
  rec('⑥.5a LM Studio candidate persists a null window (runtime-effective served value)',
    lmstudioAddClicked
      && savedLmstudioModel
      && savedLmstudioModel.preset === 'local_vllm'
      && savedLmstudioModel.base_url === 'http://127.0.0.1:1234/v1'
      && savedLmstudioModel.model === 'gemma-3-12b'
      && savedLmstudioModel.context_window_tokens === null,
    JSON.stringify(savedLmstudioModel));
  await setPortDraft('8000');
  await clickExact(page, '重新检测');
  await sleep(500);
  // ⑥.5b 本地候选行加入同一 (model id + base_url) 身份契约:8000 行与种子
  // local-qwen(qwen36_35b_256k @ http://127.0.0.1:8000/v1)同身份——旧行为
  // 直接落库第二个新 id(正是复审抓到的同弹窗重复路径)。守卫后该行禁用并
  // 显示「已保存」,点击零落库;未保存的 Ollama 行保持可添加(不误伤)。
  const localRowGuard = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    if (!dialog) return { dialogFound: false };
    const rows = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].map(button => ({
      rowText: (button.parentElement.textContent || '').slice(0, 80),
      disabled: button.disabled,
      label: (button.textContent || '').trim(),
    }));
    const savesBefore = window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model').length;
    const vllmButton = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('qwen36_35b_256k'));
    if (vllmButton) vllmButton.click();
    return {
      dialogFound: true,
      rows,
      vllmButtonFound: !!vllmButton,
      savesBefore,
      savesAfter: window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model').length,
    };
  });
  rec('⑥.5b 本地探测行同守身份契约:已保存身份(种子 local-qwen)禁用+「已保存」+点击零落库,未保存行不误伤(⑥.4/⑥.5b2 的未保存行照常添加)',
    localRowGuard.dialogFound && localRowGuard.vllmButtonFound
      && localRowGuard.rows.every(row => row.rowText.includes('qwen36_35b_256k') === (row.label === '已保存' && row.disabled))
      && localRowGuard.savesAfter === localRowGuard.savesBefore,
    JSON.stringify(localRowGuard));
  // ⑥.5b2 vLLM 部署 max_model_len 持久化臂移到未保存身份(8100):守卫不得
  // 误伤正常的候选行添加,持久化语义与旧行为一致。
  await setPortDraft('8100');
  await clickExact(page, '重新检测');
  await sleep(500);
  const unsavedRowAdd = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = dialog && [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('qwen3-next'));
    if (!button || button.disabled) return { clicked: false };
    button.click();
    return { clicked: true };
  });
  await sleep(500);
  const savedUnsavedRow = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return call && call.args && call.args.model;
  });
  rec('⑥.5b2 未保存身份的候选行仍可添加且 vLLM 持久化部署 max_model_len',
    unsavedRowAdd.clicked
      && savedUnsavedRow
      && savedUnsavedRow.preset === 'local_vllm'
      && savedUnsavedRow.base_url === 'http://127.0.0.1:8100/v1'
      && savedUnsavedRow.model === 'qwen3-next'
      && savedUnsavedRow.context_window_tokens === 262144,
    JSON.stringify(savedUnsavedRow));
  await setPortDraft('');
  await clickExact(page, '重新检测');
  await sleep(500);
  // ⑥.5b4 同步在飞闩锁挡连击(复审 r2 P1 实证:仅 busy-key state 守卫时,
  // 两次原生点击间隔 80ms 各自通过守卫、各落一库——state 提交跑不赢连点,
  // 正是 pickInFlightRef(round-3 M3)要挡的形状)。注入失败保持行未落库;
  // save_model 在包装器入口即计数,注入的 reject 不影响调用计数,故双击后
  // 计数差恰等于 handleLocalModelAdd 真正放行的次数(闩锁生效=1)。
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelError('⑥.5b4 连击守卫注入'));
  const dblClickSavesBefore = await page.evaluate(() =>
    window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model').length);
  const dblClickProbe = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = dialog && [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('deepseek-r1:14b'));
    if (!button || button.disabled) return { clicked: false };
    button.click();
    button.click();
    return { clicked: true };
  });
  await sleep(800);
  const dblClickSavesAfter = await page.evaluate(() =>
    window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model').length);
  rec('⑥.5b4 同任务双击仅放行一次落库(同步闩锁;state-only 守卫时计数差为 2 即红)',
    dblClickProbe.clicked && dblClickSavesAfter - dblClickSavesBefore === 1,
    JSON.stringify({ probe: dblClickProbe, saveModelCallsDelta: dblClickSavesAfter - dblClickSavesBefore }));
  // ⑥.5b3 行添加失败必须就地可见(复审 r2 P2:saveError 的唯一渲染点在表单
  // 分支,picker 早退分支永远到不了——失败在 picker 屏上不可见,与 PR 声明的
  // 「行内提示」相悖,他审已用浏览器探针复现)。延续失败注入:错误提示出现
  // 在 picker 内(role=alert)、行按钮恢复可点(在飞闩锁已清),清除注入后
  // 同一行可重试成功且错误消失。r1 复审 P2 的「stuck busy key」形状也由
  // 此活体钉住(失败后仍可再次添加)。
  const localAddFail = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = dialog && [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('deepseek-r1:14b'));
    if (!button || button.disabled) return { clicked: false, wasDisabled: !!button && button.disabled };
    button.click();
    return { clicked: true };
  });
  await sleep(600);
  const localAddFailState = await page.evaluate(() => ({
    pickerStillOpen: !!document.querySelector('[data-testid="local-model-row-add"]'),
    errorVisible: !!document.querySelector('[data-testid="local-model-add-error"]')
      && document.body.innerText.includes('⑥.5b4 连击守卫注入'),
    errorRoleAlert: document.querySelector('[data-testid="local-model-add-error"]')?.getAttribute('role') === 'alert',
  }));
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelError(null));
  const localAddRetry = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = dialog && [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('deepseek-r1:14b'));
    if (!button || button.disabled) return { clicked: false };
    button.click();
    return { clicked: true };
  });
  await sleep(600);
  const localAddRetryState = await page.evaluate(() => ({
    saved: window.__SETTINGS_TEST__.models().some(item => item.model === 'deepseek-r1:14b'
      && item.base_url === 'http://127.0.0.1:11434/v1'),
    errorGone: !document.querySelector('[data-testid="local-model-add-error"]'),
  }));
  rec('⑥.5b3 行添加失败在 picker 屏就地可见(alert),失败后行仍可重试,成功后错误消失且落库',
    localAddFail.clicked && localAddFailState.pickerStillOpen
      && localAddFailState.errorVisible && localAddFailState.errorRoleAlert
      && localAddRetry.clicked && localAddRetryState.saved && localAddRetryState.errorGone,
    JSON.stringify({ fail: localAddFail, state: localAddFailState, retry: localAddRetry, retryState: localAddRetryState }));

  // ⑥.5b5/b6 跨入口在途身份互斥(复审 P2×2,J+zhuowp 双审):本地行保存
  // 待定(saveModelDelay 拉长)时,同一身份的创建从任何入口都必须拒发——
  // 端点单行(切 tab 走 J 的复现链)与弹窗取消重开(zhuowp 的 ref 清零
  // 场景;ref 随实例死亡,模块级注册表跨重开存活)。
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(4000));
  await setPortDraft('8200');
  await clickExact(page, '重新检测');
  await sleep(500);
  const b5Arm = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = dialog && [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(node =>
      (node.parentElement.textContent || '').includes('qwen3-next-2'));
    if (!button || button.disabled) return { clicked: false };
    button.click();
    return { clicked: true };
  });
  await sleep(300);
  const inFlightSaves = await page.evaluate(() => window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model'
    && item.args && item.args.model && item.args.model.model === 'qwen3-next-2').length);
  // (b5/b6) 跨入口在途互斥的行为钉:在飞时取消弹窗(实例销毁,ref 全零)
  // → 重开 → 重新检测出同一行 → 行必须仍禁用(模块级注册表跨重开存活;
  // ref-only 的旧实现此处行可点、第二笔 save_model 照发)→ 点击零落库;
  // 延迟过后原保存唯一落库。doSave/端点单行/批量的查册分支由上方源码
  // 守卫族钉住(行为层的导航脆点不作为断言面)。
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]')?.click());
  await sleep(500);
  const savesBeforeReopen = await page.evaluate(() => window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model'
    && item.args && item.args.model && item.args.model.model === 'qwen3-next-2').length);
  await clickExact(page, '添加模型');
  await sleep(500);
  await clickExact(page, '本地模型');
  await sleep(600);
  // 重开后探测结果是实例 state、已随取消清空:重新检测让行出现,再断言
  // 注册表仍在(行禁用)。
  await setPortDraft('8200');
  await sleep(200);
  // 重开后 localDetectResult 为 null,按钮文案是「检测」而非「重新检测」。
  await clickExact(page, '检测');
  await sleep(700);
  const reopenedRow = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = dialog && [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(node =>
      (node.parentElement.textContent || '').includes('qwen3-next-2'));
    if (!button) return { found: false };
    if (!button.disabled) button.click();
    return { found: true, disabled: button.disabled, clicked: !button.disabled };
  });
  await sleep(300);
  const afterReopenSaves = await page.evaluate(() => window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model'
    && item.args && item.args.model && item.args.model.model === 'qwen3-next-2').length);
  // ⑥.5b7(r5 复审 MINOR 判别):窗口未关时(注册表仍记着在途身份)把重开的
  // 表单切到手动录入同一身份——保存按钮必须已禁用(doSave 守卫会静默吞掉
  // 点击,按钮却亮着=死点击窗,旧实现此处可点)。延迟落定注册表出册后,
  // 按钮恢复可用。手动表单的 API 地址按预设默认值播种(startManualLocalModel
  // setBaseUrl(defs.baseUrl)),必须显式改回 8200,身份键才能与注册表对上。
  await setPortDraft('');
  await clickExact(page, '手动添加本地模型');
  await sleep(250);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, 'http://127.0.0.1:8200/v1');
    const modelInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === '本地模型 ID').nextElementSibling;
    typeInto(modelInput, 'qwen3-next-2');
  });
  await sleep(250);
  const b7MidFlight = await page.evaluate(() => {
    const save = document.querySelector('[data-testid="model-form-save"]');
    return { found: !!save, disabled: !!save && save.disabled };
  });
  await sleep(2600);
  // 落定后原身份已是已保存重复项——按钮因 duplicateSaved 保持禁用是正确
  // 行为;换一个未保存身份必须恢复可用(证明在飞折叠按身份释放)。
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const modelInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === '本地模型 ID').nextElementSibling;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(modelInput, 'qwen3-next-2-relieved');
    modelInput.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await sleep(250);
  const b7AfterSettle = await page.evaluate(() => {
    const save = document.querySelector('[data-testid="model-form-save"]');
    return { found: !!save, enabled: !!save && !save.disabled };
  });
  await sleep(1600);
  const settled = await page.evaluate(() => ({
    saved: window.__SETTINGS_TEST__.models().filter(item => item.model === 'qwen3-next-2'
      && item.base_url === 'http://127.0.0.1:8200/v1').length,
    totalSaves: window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model'
      && item.args && item.args.model && item.args.model.model === 'qwen3-next-2').length,
  }));
  rec('⑥.5b5/b6 跨入口在途互斥:本地待定时取消重开(实例销毁),探测行仍禁用且点击零第二落库,延迟后唯一落库',
    b5Arm.clicked && inFlightSaves === 1
      && reopenedRow.found && reopenedRow.disabled
      && savesBeforeReopen === 1 && afterReopenSaves === 1
      && settled.saved === 1 && settled.totalSaves === 1,
    JSON.stringify({ b5Arm, inFlightSaves, savesBeforeReopen, reopenedRow, afterReopenSaves, settled }));
  rec('⑥.5b7 窗口内重开表单手录同一身份:保存按钮在飞禁用,落定后换未保存身份恢复可用(死点击窗收口)',
    b7MidFlight.found && b7MidFlight.disabled && b7AfterSettle.found && b7AfterSettle.enabled,
    JSON.stringify({ b7MidFlight, b7AfterSettle }));
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(0));
  // 收尾取消:下一用例要全新表单(空模型 ID 的禁用断言)且手动视图没有
  // 「手动添加本地模型」入口行,挂着它会断掉后续导航。
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]')?.click());
  await sleep(250);

  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '本地模型');
  await sleep(200);
  await clickExact(page, '手动添加本地模型');
  await sleep(250);
  const manualLocalForm = await page.evaluate(() => {
    const text = document.body.innerText;
    const save = [...document.querySelectorAll('button')].reverse().find(button => (button.textContent || '').trim() === '保存');
    return {
      noDisplayNameOnCreate: !text.includes('显示名'),
      hasModelId: text.includes('本地模型 ID'),
      hasBaseUrl: text.includes('API 地址'),
      hasKeySwitch: text.includes('需要 API Key'),
      hasConnectionTest: text.includes('测试连接'),
      saveDisabledBeforeModelId: !!save && save.disabled,
      keyInputHiddenByDefault: document.querySelectorAll('input[placeholder="输入 API Key"]').length === 0,
    };
  });
  rec('⑥.5 手动添加本地模型表单保持 iOS 分组且默认无需 Key，不强制显示名', Object.values(manualLocalForm).every(Boolean), JSON.stringify(manualLocalForm));
  // Thinking-effort leftover: a fresh draft initializes to DeepSeek's high;
  // switching to "manual local model add" must reset the thinking effort to
  // the vLLM default lowest thinking tier low — real-machine testing shows
  // local models like the Qwen3.8 family cannot reliably turn thinking off,
  // so off is no longer the default; otherwise saving would write an explicit
  // high, bypassing the bridge's vllm→low default constraint. Asserts the
  // real UI selects 低 (low).
  const manualLocalEffort = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    if (!dialog) return { found: false, labels: [], selected: [] };
    const label = [...dialog.querySelectorAll('span')].find(node => (node.textContent || '').trim() === '思考深度');
    const row = label && label.parentElement;
    const buttons = row ? [...row.querySelectorAll('button')] : [];
    const selected = buttons.filter(node => (node.className || '').includes('bg-[#007AFF]')).map(node => (node.textContent || '').trim());
    return { found: !!row, labels: buttons.map(node => (node.textContent || '').trim()), selected };
  });
  rec('⑥.5b 手动添加本地模型思考深度重置为 vLLM 默认「低」（不残留 high）',
    manualLocalEffort.found
      && manualLocalEffort.labels.includes('低')
      && manualLocalEffort.labels.includes('关闭')
      && manualLocalEffort.selected.length === 1
      && manualLocalEffort.selected[0] === '低',
    JSON.stringify(manualLocalEffort));
  // A local model's window comes from the probed max_model_len (authoritative), so no manual input is offered.
  const localNoContextWindowField = await page.evaluate(() =>
    !document.querySelector('[data-testid="model-form-context-window"]'));
  rec('⑥.5c local model form hides the context window input (probed value is authoritative)', localNoContextWindowField);
  // A legacy high (also selectable while the local_vllm probe is pending)
  // must highlight its effective medium tier once Tachyon is detected.
  await page.evaluate(() => window.__SETTINGS_TEST__.setProbedLocalKind('tachyon'));
  const tachyonUrlInput = await page.evaluateHandle(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const label = [...dialog.querySelectorAll('label,span')].find(node => (node.textContent || '').trim() === 'API 地址');
    return label.closest('div').querySelector('input');
  });
  await tachyonUrlInput.click({ clickCount: 3 });
  await tachyonUrlInput.type('http://127.0.0.1:8001/v1');
  await clickModalExact(page, '高');
  await sleep(700);
  const tachyonEffort = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const label = [...dialog.querySelectorAll('span')].find(node => (node.textContent || '').trim() === '思考深度');
    const buttons = [...label.parentElement.querySelectorAll('button')];
    return {
      count: buttons.length,
      hasHigh: buttons.some(node => node.textContent.trim() === '高'),
      selected: buttons.filter(node => node.className.includes('bg-[#007AFF]')).map(node => node.textContent.trim()),
    };
  });
  rec('Tachyon probe keeps legacy high highlighted as effective medium',
    tachyonEffort.count === 4 && !tachyonEffort.hasHigh
      && tachyonEffort.selected.length === 1 && tachyonEffort.selected[0] === '中',
    JSON.stringify(tachyonEffort));
  await page.evaluate(() => window.__SETTINGS_TEST__.setProbedLocalKind(null));
  await clickExact(page, '取消');
  await sleep(200);

  // Probe window and credential pass-through (PR #218 round 6): openai_compatible
  // + a local/private address → thinking depth enters pending "from the moment
  // the probe is scheduled" (the default four tiers from
  // localProbeTiersForKind(null) are not exposed, preventing the user from
  // picking/saving a misleading tier before the endpoint type is known); the
  // probe_local_server_kind issued after the debounce must carry the form's
  // apiKey/modelId (authenticated vLLM 401s on /v1/models without credentials,
  // misjudging generic).
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(250);
  const baseUrlInput = await page.evaluateHandle(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const label = dialog && [...dialog.querySelectorAll('label,span')].find(node => (node.textContent || '').trim() === 'API 地址');
    const row = label && label.closest('div');
    return row && row.querySelector('input');
  });
  await baseUrlInput.type('http://127.0.0.1:8000/v1', { delay: 20 });
  // Within the < 400ms debounce window: the pending copy must show and no
  // tier buttons may be offered (pre-fix, probePending was only set in the
  // timer callback, so the window fell back to the default four tiers).
  const probeWindow = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const text = dialog ? dialog.innerText : '';
    const effortRow = [...(dialog ? dialog.querySelectorAll('span') : [])].find(node => (node.textContent || '').trim() === '思考深度');
    const buttons = effortRow && effortRow.parentElement ? [...effortRow.parentElement.querySelectorAll('button')].map(node => (node.textContent || '').trim()) : [];
    return {
      pendingShown: text.includes('正在探测服务类型'),
      noTiersDuringWindow: buttons.length === 0,
    };
  });
  rec('⑥.5d local-compatible probe window enters pending immediately, default four tiers not exposed',
    probeWindow.pendingShown && probeWindow.noTiersDuringWindow,
    JSON.stringify(probeWindow));
  // After the debounce (400ms) settles: the probe command must be issued and
  // carry the credential args (empty form key → apiKey:null + edit-mode model
  // id; create mode modelId:null, with Rust reading the stored credential as
  // the fallback).
  await sleep(700);
  const probeCall = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'probe_local_server_kind');
    return call && call.args;
  });
  rec('⑥.5e probe command carries credential args (apiKey/modelId); authenticated endpoints no longer misjudged as generic',
    !!probeCall
      && probeCall.baseUrl === 'http://127.0.0.1:8000/v1'
      && 'apiKey' in probeCall
      && 'modelId' in probeCall,
    JSON.stringify(probeCall));
  await clickExact(page, '取消');
  await sleep(200);
  // Custom-model context-window entry: a newly released model ID not yet in the
  // catalog (e.g. a 1M-class model) previously had no way to declare its window,
  // so saves were always null and the runtime fell back to the conservative 128K.
  // Regression lock: field renders empty, manual value persists via save_model,
  // edit echoes it back, and clearing saves null.
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(300);
  const customContextEntry = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const field = root && root.querySelector('[data-testid="model-form-context-window"]');
    return {
      visible: !!field,
      empty: !!field && field.value === '',
      hintShown: !!root && root.innerText.includes('1M 填 1048576'),
    };
  });
  rec('⑥.5f custom compatible model form offers an empty context-window entry with hint',
    customContextEntry.visible && customContextEntry.empty && customContextEntry.hintShown,
    JSON.stringify(customContextEntry));
  const customModelIdInput = await page.$('input[placeholder="输入模型 ID"]');
  await customModelIdInput.type('glm-5.3-flash');
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, 'https://example.com/v1');
    typeInto(root.querySelector('[data-testid="model-form-context-window"]'), '1048576');
  });
  const customKeyInput = await page.$('input[placeholder="输入 API Key"]');
  await customKeyInput.type('sk-ctx-test');
  await sleep(150);
  await clickExact(page, '保存');
  await sleep(500);
  const savedCustomContext = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return call && call.args && call.args.model;
  });
  rec('⑥.5g manual context window persists via save_model',
    savedCustomContext
      && savedCustomContext.model === 'glm-5.3-flash'
      && savedCustomContext.preset === 'openai_compatible'
      && savedCustomContext.provider_kind === 'custom'
      && savedCustomContext.base_url === 'https://example.com/v1'
      && savedCustomContext.context_window_tokens === 1048576,
    JSON.stringify(savedCustomContext));
  await clickRowAction(page, 'glm-5.3-flash', '编辑');
  await sleep(300);
  const editContextEcho = await page.evaluate(() => {
    const field = document.querySelector('[data-testid="model-form-context-window"]');
    return { visible: !!field, value: field ? field.value : null };
  });
  rec('⑥.5h edit model echoes the saved context window', editContextEcho.visible && editContextEcho.value === '1048576', JSON.stringify(editContextEcho));
  await page.evaluate(() => {
    const field = document.querySelector('[data-testid="model-form-context-window"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(field, '');
    field.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await sleep(150);
  await clickExact(page, '保存');
  await sleep(500);
  const clearedContext = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return call && call.args && call.args.model;
  });
  rec('⑥.5i clearing the context window saves null (system default kept, no 0/garbage value)',
    clearedContext && clearedContext.model === 'glm-5.3-flash' && clearedContext.context_window_tokens === null,
    JSON.stringify(clearedContext));
  // Input bound: context_window_tokens is persisted as u32, so oversized input
  // must be truncated to 9 digits; otherwise save_model fails with a raw serde
  // deserialization error (or Infinity silently saves null).
  await clickRowAction(page, 'glm-5.3-flash', '编辑');
  await sleep(300);
  await page.evaluate(() => {
    const field = document.querySelector('[data-testid="model-form-context-window"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(field, '9999999999999');
    field.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await sleep(150);
  const overflowTruncated = await page.evaluate(() =>
    document.querySelector('[data-testid="model-form-context-window"]').value);
  rec('⑥.5j oversized numeric input is truncated to 9 digits (within the u32 persist bound)', overflowTruncated === '999999999', JSON.stringify({ overflowTruncated }));
  await clickExact(page, '取消');
  await sleep(200);

  // round-5 审 m1 机制钉(装在 ⑥.5k 开始前,确保组件后续渲染捕获的都是
  // 包裹后的引用):在桥 facade 层包一层 saveModel 记录 options——deferRefresh
  // 不上 wire(invoke 只带 model),单看 reloadBursts===1 在 1 行批次下分不掉
  // 「逐行 reload + 删循环后刷新」的回退实现;机制回退(丢 options)时 k6
  // 的增量断言变红。
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.saveModelOptionLog = [];
    const bridgeModels = window.TauriBridge && window.TauriBridge.models;
    if (bridgeModels && typeof bridgeModels.saveModel === 'function' && !bridgeModels.__optionSpy) {
      const orig = bridgeModels.saveModel.bind(bridgeModels);
      bridgeModels.__optionSpy = true;
      bridgeModels.saveModel = (model, options) => {
        window.__SETTINGS_TEST__.saveModelOptionLog.push(options || null);
        return orig(model, options);
      };
    }
  });
  // ⑥.5k 端点模型自动发现(规格 §2.6):显式点击「获取模型列表」→ probe 命令 →
  // 候选面板(「已保存」行禁用)→ 逐行添加落库 → 应用内确认弹窗确认后批量添加(重复行跳过)。
  // 表单 base URL 刻意与 ⑥.5g 已保存的 glm-5.3-flash @ https://example.com/v1
  // 对齐,验证 (model id + base_url) 去重。
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(300);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, 'https://example.com/v1');
  });
  await sleep(150);
  // fill-only-when-empty(规格决策 4):测试连接返回端点事实时只填空字段。
  // mock 的 test_model_connection 带上可选的 context/max_output 事实字段。
  await page.evaluate(() => window.__SETTINGS_TEST__.setModelTestResponse({
    ok: true, code: 'ok', message: '连接成功，服务可用', detail: 'HTTP 200', http_status: 200,
    context_window_tokens: 131072, max_output_tokens: 8192,
  }));
  await clickModalExact(page, '测试连接');
  await sleep(500);
  const probedFill = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    return {
      context: root.querySelector('[data-testid="model-form-context-window"]').value,
      maxOutput: root.querySelector('[data-testid="model-form-max-output"]').value,
      hintShown: !!root.querySelector('[data-testid="model-form-filled-from-endpoint"]'),
    };
  });
  rec('⑥.5k1 test connection fills EMPTY context/max-output fields from endpoint facts and shows the one-shot hint',
    probedFill.context === '131072' && probedFill.maxOutput === '8192' && probedFill.hintShown,
    JSON.stringify(probedFill));
  // 绝不覆盖非空值:手动填入后再次测试,返回不同事实,已填值必须原样保留。
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    typeInto(root.querySelector('[data-testid="model-form-context-window"]'), '999');
    typeInto(root.querySelector('[data-testid="model-form-max-output"]'), '777');
  });
  await page.evaluate(() => window.__SETTINGS_TEST__.setModelTestResponse({
    ok: true, code: 'ok', message: '连接成功，服务可用', detail: 'HTTP 200', http_status: 200,
    context_window_tokens: 262144, max_output_tokens: 16384,
  }));
  await clickModalExact(page, '测试连接');
  await sleep(500);
  const keptManual = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    return {
      context: root.querySelector('[data-testid="model-form-context-window"]').value,
      maxOutput: root.querySelector('[data-testid="model-form-max-output"]').value,
      hintShown: !!root.querySelector('[data-testid="model-form-filled-from-endpoint"]'),
    };
  });
  rec('⑥.5k2 second test with different facts must NOT overwrite manually filled values (and the hint clears)',
    keptManual.context === '999' && keptManual.maxOutput === '777' && !keptManual.hintShown,
    JSON.stringify(keptManual));
  // round-4 m1-8:fact_model 比较臂的运行时钉。⑥.5k1/k2 的 mock 响应不带
  // fact_model,比较臂此前从未在运行时跑过——删掉渲染镜像
  // modelRef.current = model(比较退化成挂载值 ≠ fact_model 的恒假分支)
  // 全套用例仍然全绿。事实名引用 ⑥.5g 已保存的 glm-5.3-flash(后端按
  // 已保存记录的 model 探得并经 fact_model 回带);先清空 ⑥.5k2 手填的
  // 字段,fill-only-when-empty 才有可观察的落点。
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const clear = input => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, '');
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    clear(root.querySelector('[data-testid="model-form-context-window"]'));
    clear(root.querySelector('[data-testid="model-form-max-output"]'));
  });
  await sleep(150);
  await page.evaluate(() => window.__SETTINGS_TEST__.setModelTestResponse({
    ok: true, code: 'ok', message: '连接成功，服务可用', detail: 'HTTP 200', http_status: 200,
    context_window_tokens: 555000, max_output_tokens: 444,
    fact_model: 'glm-5.3-flash',
  }));
  const mismatchModelInput = await page.$('input[placeholder="输入模型 ID"]');
  await mismatchModelInput.type('glm-9-other-model', { delay: 10 });
  await clickModalExact(page, '测试连接');
  await sleep(500);
  const staleFactsDropped = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    return {
      context: root.querySelector('[data-testid="model-form-context-window"]').value,
      maxOutput: root.querySelector('[data-testid="model-form-max-output"]').value,
      hintShown: !!root.querySelector('[data-testid="model-form-filled-from-endpoint"]'),
    };
  });
  rec('⑥.5k2b facts whose fact_model differs from the live wire model must NOT fill (the per-saved-model comparison arm runs at runtime)',
    staleFactsDropped.context === '' && staleFactsDropped.maxOutput === '' && !staleFactsDropped.hintShown,
    JSON.stringify(staleFactsDropped));
  // 匹配臂:表单模型改回 fact_model 同名 → 照常填入。删掉渲染镜像或破坏
  // 比较(拿表单模型跟自己比/恒真放行)在此臂变红。
  await page.evaluate(() => {
    const input = document.querySelector('input[placeholder="输入模型 ID"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(input, 'glm-5.3-flash');
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await sleep(150);
  await clickModalExact(page, '测试连接');
  await sleep(500);
  const matchingFactsFilled = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    return {
      context: root.querySelector('[data-testid="model-form-context-window"]').value,
      maxOutput: root.querySelector('[data-testid="model-form-max-output"]').value,
      hintShown: !!root.querySelector('[data-testid="model-form-filled-from-endpoint"]'),
    };
  });
  rec('⑥.5k2c facts whose fact_model equals the live wire model fill the empty fields (render-mirror comparison stays load-bearing)',
    matchingFactsFilled.context === '555000' && matchingFactsFilled.maxOutput === '444' && matchingFactsFilled.hintShown,
    JSON.stringify(matchingFactsFilled));
  // 获取模型列表:显式点击 → probe_endpoint_models(带表单 base_url + preset)。
  await clickModalExact(page, '获取模型列表');
  await sleep(600);
  const endpointProbeCall = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'probe_endpoint_models');
    return call && call.args;
  });
  rec('⑥.5k3 fetch button probes probe_endpoint_models with the form base URL and preset',
    !!endpointProbeCall && endpointProbeCall.baseUrl === 'https://example.com/v1' && endpointProbeCall.preset === 'openai_compatible' && endpointProbeCall.apiKey === null,
    JSON.stringify(endpointProbeCall));
  // round-5 审 M1 运行时钉:免鉴权声明生效时,已输入的残留 key 绝不随
  // 「获取模型列表」发出。输入 key → 开免鉴权(输入行隐藏但 state 残留)
  // → 再点获取:probe args 的 apiKey 必须仍是 null——探测与保存后的鉴权
  // 形态一致(#851;handleTest/图片能力测试早有同款臂,fetch-models 是
  // round-5 补上的第三条路径)。旧实现漏臂时残留 key 原样发出,本步变红。
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    const keyInput = root.querySelector('input[placeholder="输入 API Key"]');
    setter.call(keyInput, 'sk-residual-key');
    keyInput.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await page.evaluate(() => document.querySelector('[data-testid="model-form-no-auth-toggle"]').click());
  await sleep(200);
  await clickModalExact(page, '获取模型列表');
  await sleep(600);
  const keylessProbeCall = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'probe_endpoint_models');
    return call && call.args;
  });
  rec('⑥.5k3b fetch-models under an active keyless declaration sends NO residual typed key (all three probe sites share the #851 shape-parity arm)',
    !!keylessProbeCall && keylessProbeCall.apiKey === null && keylessProbeCall.baseUrl === 'https://example.com/v1',
    JSON.stringify(keylessProbeCall));
  // 还原表单:关免鉴权、清残留 key——⑥.5k4 起的落库断言(credential ladder /
  // auth_mode)依赖默认形态,残留状态不得外溢。
  await page.evaluate(() => document.querySelector('[data-testid="model-form-no-auth-toggle"]').click());
  await sleep(150);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    const keyInput = root.querySelector('input[placeholder="输入 API Key"]');
    setter.call(keyInput, '');
    keyInput.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await sleep(100);
  const pickerRows = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    if (!dialog) return { open: false };
    const rows = [...dialog.querySelectorAll('[data-testid="endpoint-model-row"]')];
    const glmRow = rows.find(node => (node.textContent || '').includes('glm-5-flash'));
    return {
      open: true,
      count: rows.length,
      savedTags: dialog.querySelectorAll('[data-testid="endpoint-model-saved-tag"]').length,
      disabledAdds: rows.filter(row => row.querySelector('[data-testid="endpoint-model-row-add"]').disabled).length,
      glmBadge: glmRow ? ((glmRow.textContent || '').includes('1M') && (glmRow.textContent || '').includes('33K')) : false,
    };
  });
  rec('⑥.5k4 picker lists candidates with K/M badges and disables the already-saved row',
    pickerRows.open && pickerRows.count === 3 && pickerRows.savedTags === 1 && pickerRows.disabledAdds === 1 && pickerRows.glmBadge,
    JSON.stringify(pickerRows));
  // 逐行添加:显式点击候选行的「添加」才写库。
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    const row = [...dialog.querySelectorAll('[data-testid="endpoint-model-row"]')].find(node => (node.textContent || '').includes('glm-5-flash'));
    row.querySelector('[data-testid="endpoint-model-row-add"]').click();
  });
  await sleep(700);
  const pickedSave = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return call && call.args && call.args.model;
  });
  rec('⑥.5k5 explicit row add saves the discovered draft with endpoint facts (payload parity with doSave: ladder + auth_mode + alias)',
    !!pickedSave && pickedSave.model === 'glm-5-flash' && pickedSave.base_url === 'https://example.com/v1'
      && pickedSave.context_window_tokens === 1048576 && pickedSave.max_output_tokens === 32768
      && pickedSave.preset === 'openai_compatible'
      && pickedSave.credential_action === 'replace' && pickedSave.api_key === ''
      && pickedSave.auth_mode === null && pickedSave.alias === null,
    JSON.stringify(pickedSave));
  // round-4 M4 运行时钉(⑥.5k6 前奏):单行保存在飞窗口内「全部添加」必须
  // 禁用且点击是 no-op(确认弹窗绝不出现)。harness 用 saveModelDelay 拉长
  // 在飞窗口、saveModelError 让该行保存失败收场——kimi-k3 仍未落库,不改
  // 变 ⑥.5k6 的去重计划;顺带钉住失败横幅(add-notice)的运行时渲染。
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.setSaveModelDelay(1500);
    window.__SETTINGS_TEST__.setSaveModelError('save exploded');
  });
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    const row = [...dialog.querySelectorAll('[data-testid="endpoint-model-row"]')].find(node => (node.textContent || '').includes('kimi-k3'));
    row.querySelector('[data-testid="endpoint-model-row-add"]').click();
  });
  await sleep(300);
  const addAllWhilePick = await page.evaluate(() => {
    const button = document.querySelector('[data-testid="endpoint-model-add-all"]');
    return {
      buttonDisabled: !!button && button.disabled,
      pickerOpen: !!document.querySelector('[data-testid="endpoint-model-picker-dialog"]'),
    };
  });
  // 在飞窗口内点「全部添加」:守卫直接 return,确认弹窗不得出现。
  await page.evaluate(() => document.querySelector('[data-testid="endpoint-model-add-all"]').click());
  await sleep(250);
  const addAllNoOp = await page.evaluate(() => ({
    noConfirm: !document.querySelector('[data-testid="add-all-models-confirm"]'),
    pickerStillOpen: !!document.querySelector('[data-testid="endpoint-model-picker-dialog"]'),
  }));
  await sleep(1500);
  const pickFailure = await page.evaluate(() => {
    const banner = document.querySelector('[data-testid="endpoint-model-add-notice"]');
    return {
      failureBanner: !!banner,
      // round-5 审 m1(此前仅源码钉):失败横幅必须是红色系(ok 类是绿色
      // text-[#137333];失败臂 text-[#FF3B30])——ternary 两臂互换在此变红。
      bannerRed: !!banner && (banner.className || '').includes('text-[#FF3B30]'),
      bannerNotGreen: !!banner && !(banner.className || '').includes('text-[#137333]'),
      pickerKeptOpen: !!document.querySelector('[data-testid="endpoint-model-picker-dialog"]'),
    };
  });
  rec('⑥.5k5b Add All disables and stays a no-op while a row save is in flight; the failed row keeps the picker open with the red notice banner',
    addAllWhilePick.buttonDisabled && addAllWhilePick.pickerOpen && addAllNoOp.noConfirm
    && addAllNoOp.pickerStillOpen && pickFailure.failureBanner && pickFailure.bannerRed && pickFailure.bannerNotGreen && pickFailure.pickerKeptOpen,
    JSON.stringify({ addAllWhilePick, addAllNoOp, pickFailure }));
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.setSaveModelError(null);
    window.__SETTINGS_TEST__.setSaveModelDelay(0);
  });
  await sleep(150);
  // 「全部添加」:应用内确认对话框(原生 confirm 在 WebView2 不渲染)。点按钮
  // 后先断言对话框出现且尚未落库(保存调用数不增),点确认按钮才逐行保存;
  // 重复行(glm-5.3-flash 已在 ⑥.5g 保存、glm-5-flash 刚保存)跳过,只落 kimi-k3。
  const savesBeforeAddAll = await page.evaluate(() => window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model').length);
  const reloadsBeforeAddAll = await page.evaluate(() => window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'list_models').length);
  const optionLogBefore = await page.evaluate(() => (window.__SETTINGS_TEST__.saveModelOptionLog || []).length);
  await page.evaluate(() => document.querySelector('[data-testid="endpoint-model-add-all"]').click());
  await sleep(300);
  const addAllGate = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="add-all-models-confirm"]');
    return {
      shown: !!dialog,
      title: dialog && dialog.querySelector('h3') ? dialog.querySelector('h3').textContent || '' : '',
      savesBeforeConfirm: window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model').length,
    };
  });
  await page.evaluate(() => document.querySelector('[data-testid="add-all-models-confirm-ok"]').click());
  await sleep(900);
  const batchState = await page.evaluate((reloadsBefore, optionLogBefore) => {
    const calls = window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model');
    const saved = new Set(calls.map(call => `${call.args.model.model}@${call.args.model.base_url}`));
    return {
      pickerClosed: !document.querySelector('[data-testid="endpoint-model-picker-dialog"]'),
      dialogClosed: !document.querySelector('[data-testid="add-all-models-confirm"]'),
      hasKimi: saved.has('kimi-k3@https://example.com/v1'),
      hasEndpointGlm: saved.has('glm-5-flash@https://example.com/v1'),
      // glm-5.3-flash 只有 ⑥.5g/⑥.5i 的两次保存,批量添加不得再落一次。
      dupSaveCalls: calls.filter(call => call.args.model.model === 'glm-5.3-flash' && call.args.model.base_url === 'https://example.com/v1').length,
      // 复审 M9:批量逐行 deferRefresh,整个批次只允许一次 list_models 重载
      // (旧实现逐行 reload,1 行也会打满 3 个 loader;这里钉「恰好一次」)。
      reloadBursts: window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'list_models').length - reloadsBefore,
      deferRefreshed: (() => {
        const delta = (window.__SETTINGS_TEST__.saveModelOptionLog || []).slice(optionLogBefore);
        return delta.length > 0 && delta.every(option => option && option.deferRefresh === true);
      })(),
    };
  }, reloadsBeforeAddAll, optionLogBefore);
  rec('⑥.5k6 confirmed add-all saves only non-duplicate rows, reloads once, defers per-row refresh, and closes the picker',
    addAllGate.shown && addAllGate.savesBeforeConfirm === savesBeforeAddAll && addAllGate.title.includes('1')
      && batchState.pickerClosed && batchState.dialogClosed && batchState.hasKimi && batchState.hasEndpointGlm && batchState.dupSaveCalls === 2
      && batchState.reloadBursts === 1 && batchState.deferRefreshed,
    JSON.stringify({ addAllGate, batchState }));
  // ⑥.5k6b 表单保存加入同一 (model id + base_url) 身份契约。此时表单
  // model=glm-5.3-flash(⑥.5k2c 设置)且 ⑥.5g 已把它落库在同一端点——
  // 「全部添加」确认后 picker 关闭、用户再点「保存」正是用户实测的复现
  // 路径(旧实现以新 id 双落库 deepseek-flash)。填 key 让 canSave 通过:
  // 保存必须禁用 + 行内提示 + 点击零落库;换成未保存的 model id 后保存
  // 恢复可用(禁用由身份冲突驱动,不只是 canSave)。
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    const keyInput = root.querySelector('input[placeholder="输入 API Key"]');
    setter.call(keyInput, 'sk-dup-guard');
    keyInput.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await sleep(200);
  const dupGuardState = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const save = root.querySelector('[data-testid="model-form-save"]');
    return {
      saveDisabled: !!save && save.disabled,
      hintShown: !!root.querySelector('[data-testid="model-form-duplicate-hint"]'),
      // 复审 r2 P3:添加态文案此前只有 boolean 断言,空串/垃圾文案也能绿——
      // 钉住添加态专用措辞,且不得串到编辑态文案。
      hintText: (root.querySelector('[data-testid="model-form-duplicate-hint"]')?.textContent || '').trim(),
      saves: window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model').length,
    };
  });
  await page.evaluate(() => document.querySelector('[data-testid="model-form-save"]').click());
  await sleep(300);
  const dupGuardNoSave = await page.evaluate(saves => ({
    savesAfter: window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model').length,
    unchanged: window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model').length === saves,
  }), dupGuardState.saves);
  await page.evaluate(() => {
    const input = document.querySelector('[data-testid="model-form-dialog"] input[placeholder="输入模型 ID"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(input, 'glm-9-unsaved');
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await sleep(200);
  const reenabled = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    return {
      saveDisabled: root.querySelector('[data-testid="model-form-save"]').disabled,
      hintGone: !root.querySelector('[data-testid="model-form-duplicate-hint"]'),
    };
  });
  await page.evaluate(() => {
    const input = document.querySelector('[data-testid="model-form-dialog"] input[placeholder="输入模型 ID"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(input, 'glm-5.3-flash');
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await sleep(200);
  const reDisabled = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    return {
      saveDisabled: root.querySelector('[data-testid="model-form-save"]').disabled,
      hintShown: !!root.querySelector('[data-testid="model-form-duplicate-hint"]'),
    };
  });
  rec('⑥.5k6b the form save joins the (model id + base_url) identity contract — after add-all, save is disabled with an inline hint and stays a no-op click; the guard tracks the live model id (unsaved id re-enables)',
    dupGuardState.saveDisabled && dupGuardState.hintShown && dupGuardNoSave.unchanged
    && dupGuardState.hintText.includes('无需重复保存') && !dupGuardState.hintText.includes('删除重复项')
    && !reenabled.saveDisabled && reenabled.hintGone
    && reDisabled.saveDisabled && reDisabled.hintShown,
    JSON.stringify({ dupGuardState, dupGuardNoSave, reenabled, reDisabled }));
  // 还原:清 key(回到 k6 之前的表单形态),后续 k7+ 的取列表断言不受影响。
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    const keyInput = root.querySelector('input[placeholder="输入 API Key"]');
    setter.call(keyInput, '');
    keyInput.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await sleep(150);
  // 恢复默认 test_model_connection 响应,不污染后续流程。
  await page.evaluate(() => window.__SETTINGS_TEST__.setModelTestResponse({ ok: true, code: 'ok', message: '连接成功，服务可用', detail: 'HTTP 200', http_status: 200 }));
  // round-4 m1-12:truncated 通知与渲染上限尾注的运行时钉(setProbeEndpointListing
  // 此前是死钩子)。注入 truncated + 210 行(205 个 bulk- 前缀 + 5 个独立 id):
  // 空筛选 → 平文尾注「还有 10 个未显示」;输入 bulk → 可见 205 行、隐藏 5 行,
  // 尾注必须切到带「缩小筛选范围」建议的变体——三目两臂与 truncated 条件的
  // 突变在此变红。
  await page.evaluate(() => window.__SETTINGS_TEST__.setProbeEndpointListing({
    status: 'listed', http_status: 200, transport: null, server_kind: null, truncated: true,
    models: [
      ...Array.from({ length: 205 }, (_, i) => ({ id: 'bulk-model-' + i, context_window_tokens: null, max_output_tokens: null, context_from: null, output_from: null, loaded: null, title: null })),
      { id: 'solo-a', context_window_tokens: null, max_output_tokens: null, context_from: null, output_from: null, loaded: null, title: null },
      { id: 'solo-b', context_window_tokens: null, max_output_tokens: null, context_from: null, output_from: null, loaded: null, title: null },
      { id: 'solo-c', context_window_tokens: null, max_output_tokens: null, context_from: null, output_from: null, loaded: null, title: null },
      { id: 'solo-d', context_window_tokens: null, max_output_tokens: null, context_from: null, output_from: null, loaded: null, title: null },
      { id: 'solo-e', context_window_tokens: null, max_output_tokens: null, context_from: null, output_from: null, loaded: null, title: null },
    ],
  }));
  await clickModalExact(page, '获取模型列表');
  await sleep(600);
  const truncatedState = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    const footer = dialog && dialog.querySelector('[data-testid="endpoint-model-picker-more"]');
    return {
      open: !!dialog,
      truncatedNotice: !!dialog && !!dialog.querySelector('[data-testid="endpoint-model-picker-truncated"]'),
      footerShown: !!footer,
      footerText: footer ? (footer.textContent || '').trim() : '',
    };
  });
  rec('⑥.5k7 a truncated listing surfaces the notice and the plain-count "N more" footer while the filter is untouched',
    truncatedState.open && truncatedState.truncatedNotice && truncatedState.footerShown
      && truncatedState.footerText.includes('10') && !truncatedState.footerText.includes('缩小筛选范围'),
    JSON.stringify(truncatedState));
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    const input = dialog.querySelector('[data-testid="endpoint-model-filter"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(input, 'bulk');
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await sleep(300);
  const filteredFooter = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    const footer = dialog && dialog.querySelector('[data-testid="endpoint-model-picker-more"]');
    return { footerText: footer ? (footer.textContent || '').trim() : '' };
  });
  rec('⑥.5k7b typing a filter swaps the footer to the refine-the-filter variant with the filtered hidden count',
    filteredFooter.footerText.includes('5') && filteredFooter.footerText.includes('缩小筛选范围'),
    JSON.stringify(filteredFooter));
  // round-4 m1-2 运行时钉:listing 带 server_kind:'ollama' 时,行的
  // context_window_tokens 是运行时生效值——徽章照常显示 33K,落库必须存
  // null(server_kind 闸;非 ollama/lm_studio 的落库臂已由 ⑥.5k5 钉住)。
  // 同轮断言 truncated:false 下截断通知不得渲染(条件的另一侧)。
  await page.evaluate(() => window.__SETTINGS_TEST__.setProbeEndpointListing({
    status: 'listed', http_status: 200, transport: null, server_kind: 'ollama', truncated: false,
    models: [
      { id: 'qwen2.5-coder:32b', context_window_tokens: 32768, context_from: 'endpoint', max_output_tokens: null, output_from: null, loaded: true, title: null },
    ],
  }));
  await clickModalExact(page, '获取模型列表');
  await sleep(600);
  const ollamaBadge = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    const row = dialog && [...dialog.querySelectorAll('[data-testid="endpoint-model-row"]')].find(node => (node.textContent || '').includes('qwen2.5-coder'));
    return {
      truncatedNoticeAbsent: !!dialog && !dialog.querySelector('[data-testid="endpoint-model-picker-truncated"]'),
      rowShown: !!row,
      badgeShown: !!row && (row.textContent || '').includes('33K'),
    };
  });
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    const row = [...dialog.querySelectorAll('[data-testid="endpoint-model-row"]')].find(node => (node.textContent || '').includes('qwen2.5-coder'));
    row.querySelector('[data-testid="endpoint-model-row-add"]').click();
  });
  await sleep(700);
  const ollamaRowSave = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return call && call.args && call.args.model;
  });
  rec('⑥.5k8 ollama lane shows the runtime context badge but persists a null declaration (server_kind persistence gate)',
    ollamaBadge.truncatedNoticeAbsent && ollamaBadge.rowShown && ollamaBadge.badgeShown
      && !!ollamaRowSave && ollamaRowSave.model === 'qwen2.5-coder:32b' && ollamaRowSave.context_window_tokens === null,
    JSON.stringify({ ollamaBadge, ollamaRowSave }));
  // round-5 审 m1(键盘可达的拆批)运行时钉:批量保存循环进行中改 URL,
  // 已确认的批次必须整体落在确认时刻的快照端点上——确认弹窗没有焦点
  // 陷阱,键盘用户可以在循环进行中改 URL(URL 变更清扫只摘弹窗与 picker,
  // 已开跑的循环照常推进)。saveModelDelay 拉长循环,第一行在飞时改 URL:
  // 旧实现逐行读 live state,第二行 payload 会落到新 URL(拆批),本步变红;
  // 快照实现下两行都落在确认时刻的 URL。
  await page.evaluate(() => window.__SETTINGS_TEST__.setProbeEndpointListing({
    status: 'listed', http_status: 200, transport: null, server_kind: null, truncated: false,
    models: [
      { id: 'snap-a', context_window_tokens: null, context_from: null, max_output_tokens: null, output_from: null, loaded: null, title: null },
      { id: 'snap-b', context_window_tokens: null, context_from: null, max_output_tokens: null, output_from: null, loaded: null, title: null },
    ],
  }));
  await clickModalExact(page, '获取模型列表');
  await sleep(500);
  const snapSavesBefore = await page.evaluate(() => window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model').length);
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(700));
  await page.evaluate(() => document.querySelector('[data-testid="endpoint-model-add-all"]').click());
  await sleep(250);
  await page.evaluate(() => document.querySelector('[data-testid="add-all-models-confirm-ok"]').click());
  await sleep(400);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    setter.call(baseUrlInput, 'https://moved.example.com/v1');
    baseUrlInput.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await sleep(1800);
  const snapshotBatch = await page.evaluate(savesBefore => {
    const calls = window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model').slice(savesBefore);
    return {
      savedModels: calls.map(call => `${call.args.model.model}@${call.args.model.base_url}`),
      bothSnapshotted: calls.length === 2 && calls.every(call => call.args.model.base_url === 'https://example.com/v1'),
    };
  }, snapSavesBefore);
  rec('⑥.5k9 a mid-batch base-URL edit cannot split the confirmed batch (both rows land on the snapshot endpoint, not the live field)',
    snapshotBatch.bothSnapshotted && snapshotBatch.savedModels.includes('snap-a@https://example.com/v1') && snapshotBatch.savedModels.includes('snap-b@https://example.com/v1'),
    JSON.stringify(snapshotBatch));
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(0));
  // 还原 URL 与默认 probe 列表,不污染后续流程。
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    setter.call(baseUrlInput, 'https://example.com/v1');
    baseUrlInput.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await sleep(150);
  // 恢复默认 probe 列表,不污染后续流程。
  await page.evaluate(() => window.__SETTINGS_TEST__.setProbeEndpointListing({
    status: 'listed', http_status: 200, transport: null, server_kind: null, truncated: false,
    models: [
      { id: 'glm-5.3-flash', context_window_tokens: 131072, context_from: 'catalog', max_output_tokens: null, output_from: null, loaded: null, title: null },
      { id: 'glm-5-flash', context_window_tokens: 1048576, context_from: 'endpoint', max_output_tokens: 32768, output_from: 'endpoint', loaded: null, title: 'GLM-5 Flash' },
      { id: 'kimi-k3', context_window_tokens: null, context_from: null, max_output_tokens: null, output_from: null, loaded: null, title: null },
    ],
  }));
  // Escape 分层关(round-5 审 m1):聊天线 picker 此前只有 X 按钮能关(ACP
  // 线已有窗口级 Escape 分层);重新拉起 picker 后按 Escape,断言摘除。
  await clickModalExact(page, '获取模型列表');
  await sleep(500);
  const pickerBeforeEscape = await page.evaluate(() => !!document.querySelector('[data-testid="endpoint-model-picker-dialog"]'));
  await page.keyboard.press('Escape');
  await sleep(250);
  const escapeClosed = await page.evaluate(() => !document.querySelector('[data-testid="endpoint-model-picker-dialog"]'));
  rec('⑥.5k10 Escape closes the chat-line endpoint picker (ACP-line parity; X was previously the only dismissal path)',
    pickerBeforeEscape && escapeClosed,
    JSON.stringify({ pickerBeforeEscape, escapeClosed }));
  await clickModalExact(page, '取消');
  await sleep(200);

  // A noControl model (always-thinking and not controllable, e.g. deepseek-r1)
  // takes precedence over the probed effort tier: after the probe completes, the
  // effort tier area shows the "thinking always on" hint instead of tier buttons
  // or the "endpoint not supported" hint.
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(250);
  const r1ModelInput = await page.evaluateHandle(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const label = dialog && [...dialog.querySelectorAll('label,span')].find(node => (node.textContent || '').trim() === '模型 ID');
    const row = label && label.closest('div');
    return row && row.querySelector('input');
  });
  await r1ModelInput.type('deepseek-r1:14b', { delay: 10 });
  const r1BaseUrlInput = await page.evaluateHandle(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const label = dialog && [...dialog.querySelectorAll('label,span')].find(node => (node.textContent || '').trim() === 'API 地址');
    const row = label && label.closest('div');
    return row && row.querySelector('input');
  });
  await r1BaseUrlInput.type('http://127.0.0.1:8000/v1', { delay: 20 });
  // Wait past the 400ms debounce; the probe returns null (mock default), so the
  // knowledge table noControl entry still wins
  await sleep(700);
  const r1Effort = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const text = dialog ? dialog.innerText : '';
    const effortRow = [...(dialog ? dialog.querySelectorAll('span') : [])].find(node => (node.textContent || '').trim() === '思考深度');
    const buttons = effortRow && effortRow.parentElement ? [...effortRow.parentElement.querySelectorAll('button')].map(node => (node.textContent || '').trim()) : [];
    return {
      alwaysOnShown: text.includes('该模型思考始终开启，无法关闭'),
      unsupportedHidden: !text.includes('该端点不支持思考档位调节'),
      noTierButtons: buttons.length === 0,
    };
  });
  rec('⑥.5e noControl model (deepseek-r1) local route shows "thinking always on" hint with no effort tier buttons',
    r1Effort.alwaysOnShown && r1Effort.unsupportedHidden && r1Effort.noTierButtons,
    JSON.stringify(r1Effort));
  await clickExact(page, '取消');
  await sleep(200);

  await clickExact(page, '添加模型');
  await sleep(300);
  const cloudPickerWidth = await modalWidth(page, '添加模型');
  await clickExact(page, '深度求索 / DeepSeek');
  await sleep(300);
  const addModelBeforeKey = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const text = root ? root.innerText : '';
    const save = [...document.querySelectorAll('button')].reverse().find(button => (button.textContent || '').trim() === '保存');
    return {
      noAdvancedCollapse: !text.includes('高级设置'),
      noServiceUrlField: !text.includes('服务地址'),
      // The 2026-09 catalog DeepSeek group's default first item is the new
      // mainline deepseek-flash
      hasModelPicker: text.includes('模型') && text.includes('deepseek-flash'),
      hasOptionalAlias: text.includes('别名') && !!document.querySelector('[data-testid="model-form-alias"]'),
      saveDisabled: !!save && save.disabled,
      hasSingleKeyInput: document.querySelectorAll('input[placeholder="输入 API Key"]').length === 1,
    };
  });
  const addModelBeforeKeyPass = cloudPickerWidth >= 430 && cloudPickerWidth <= 455
    && addModelBeforeKey.noAdvancedCollapse
    && addModelBeforeKey.noServiceUrlField
    && addModelBeforeKey.hasModelPicker
    && addModelBeforeKey.hasOptionalAlias
    && addModelBeforeKey.saveDisabled
    && addModelBeforeKey.hasSingleKeyInput;
  rec('⑥.6 添加预置云模型表单精简且 API Key 前禁用保存', addModelBeforeKeyPass, JSON.stringify({ cloudPickerWidth, ...addModelBeforeKey }));
  // Explicitly pick the deepseek-v4-pro entry before saving; the later
  // ⑦/⑦.img assertions keep using that model.
  await clickModalExact(page, '模型');
  await sleep(200);
  await clickModalExact(page, 'deepseek-v4-pro');
  await sleep(200);
  const apiInput = await page.$('input[placeholder="输入 API Key"]');
  const aliasInput = await page.$('[data-testid="model-form-alias"]');
  await aliasInput.type('Daily assistant');
  await apiInput.type('sk-model-test');
  await sleep(150);
  await clickExact(page, '保存');
  await sleep(500);
  const savedModel = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return call && call.args && call.args.model;
  });
  rec('⑦ 输入 API Key 后可保存新增模型',
    savedModel
      && savedModel.model === 'deepseek-v4-pro'
      && savedModel.name === 'deepseek-v4-pro'
      && savedModel.alias === 'Daily assistant'
      && savedModel.api_key === 'sk-model-test',
    JSON.stringify(savedModel));
  const savedAliasInventory = await page.evaluate(() => {
    const title = [...document.querySelectorAll('span')]
      .find(node => (node.textContent || '').trim() === 'Daily assistant');
    const row = title && title.closest('.grid');
    const text = row ? row.innerText : '';
    return {
      aliasVisible: !!title,
      providerVisible: text.includes('DeepSeek'),
      wireModelVisible: text.includes('deepseek-v4-pro'),
    };
  });
  rec('⑦.1 保存别名后模型列表立即以别名为标题并保留服务商与模型 ID',
    Object.values(savedAliasInventory).every(Boolean),
    JSON.stringify(savedAliasInventory));

  // ⑦.2 编辑态重复条目(旧版本落库的同身份双记录,1.0.2 用户实测):编辑任意
  // 一条时保存禁用并给「先去重」的编辑态文案;经列表删除其一后,同一条记录
  // 恢复可保存且以原 id 落库——duplicateSaved 只由其余条目驱动(编辑态自身
  // 排除的活体证据,守住删除过滤的正向用例)。
  await page.evaluate(async () => {
    await window.__TAURI__.core.invoke('save_model', { model: {
      id: 'dup-twin-local-qwen', name: '重复副本', preset: 'local_vllm',
      model: 'qwen36_35b_256k', base_url: 'http://127.0.0.1:8000/v1',
      context_window_tokens: null, max_output_tokens: null,
      api_key: '', credential_action: 'keep_existing',
    } });
  });
  await page.evaluate(() => window.TauriBridge.models.setActiveModel(window.__SETTINGS_TEST__.activeModelId()));
  await sleep(400);
  // 行标题 = alias || model || name,本地记录别名被强制为空 → 标题即模型 ID,
  // 副本名不渲染:两条同身份记录在列表里视觉完全同形(正是用户删错权威行的
  // 根源)。首个匹配行是种子 local-qwen(副本按 applySaveModel 追加在末位)。
  await clickRowAction(page, 'qwen36_35b_256k', '编辑');
  await sleep(300);
  const twinEditState = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const save = root.querySelector('[data-testid="model-form-save"]');
    const hint = root.querySelector('[data-testid="model-form-duplicate-hint"]');
    return {
      saveDisabled: !!save && save.disabled,
      hintShown: !!hint,
      hintText: hint ? (hint.textContent || '').trim() : '',
    };
  });
  rec('⑦.2 编辑态命中盘上重复条目:保存禁用且提示先去重(编辑态专用文案,非新建态措辞)',
    twinEditState.saveDisabled && twinEditState.hintShown
      && twinEditState.hintText.includes('删除重复项或修改本条目')
      && !twinEditState.hintText.includes('无需重复保存'),
    JSON.stringify(twinEditState));
  await clickExact(page, '取消');
  await sleep(200);
  // 删除副本行:同形行取 DOM 末位(applySaveModel 追加序),不能按名找(名不渲染)。
  const twinDeleteClicked = await page.evaluate(() => {
    const rows = [...document.querySelectorAll('button')]
      .filter(button => (button.textContent || '').trim() === '删除')
      .map(button => button.closest('[class*="grid-cols-["]'))
      .filter(row => row && (row.textContent || '').includes('qwen36_35b_256k'));
    const twinRow = rows[rows.length - 1];
    const button = twinRow && [...twinRow.querySelectorAll('button')].find(candidate => (candidate.textContent || '').trim() === '删除');
    if (!button) return false;
    button.scrollIntoView({ block: 'center' });
    button.click();
    return true;
  });
  if (!twinDeleteClicked) throw new Error('找不到重复副本行');
  await sleep(200);
  await clickExact(page, '删除模型');
  await sleep(500);
  await clickRowAction(page, 'qwen36_35b_256k', '编辑');
  await sleep(300);
  const twinGoneSave = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const save = root.querySelector('[data-testid="model-form-save"]');
    if (save.disabled) return { clicked: false, id: null };
    save.click();
    return { clicked: true, id: null };
  });
  await sleep(500);
  const twinGoneState = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return {
      root: !!document.querySelector('[data-testid="model-form-dialog"]'),
      hintGone: !document.querySelector('[data-testid="model-form-duplicate-hint"]'),
      savedId: call && call.args && call.args.model && call.args.model.id,
    };
  });
  rec('⑦.2b 去重后同一条记录恢复可保存且以原 id 落库(自身排除的活体证据)',
    twinGoneSave.clicked && !twinGoneState.root && twinGoneState.hintGone
      && twinGoneState.savedId === 'local-qwen',
    JSON.stringify(twinGoneState));

  // ===== r3 复审补全活体场景(在作者注册表之上的三个剩余缺口):
  // ⑦.4 刷新失败分支身份不复活(查册落定即出册,loadModels 吞 list_models
  // 失败清空列表——lane G 实证空列表反而邀请全量重加)、⑦.5a/⑦.5b 在飞禁关
  // 层(Escape/X;查册已补齐去重,残余危害是在飞写入失去可见性)、⑦.6 Save
  // 同任务双击仅一笔(注册表按身份键控的活体证据)、⑦.7 保存中提示闸(M9
  // 判别器:上一套件该突变体存活)。全部走真实 UI 点击 + saveModelDelay。=====
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '本地模型');
  await sleep(200);
  await setPortDraft('8225');
  await clickExact(page, '检测');
  await sleep(600);
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.armListModelsFailure();
    window.__SETTINGS_TEST__.setSaveModelDelay(500);
  });
  const refreshSavesBefore = await callCount(page, 'save_model');
  const refreshRowClick = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('refresh-twin-7c'));
    if (!button || button.disabled) return { clicked: false };
    button.click();
    return { clicked: true };
  });
  await sleep(1300);
  const refreshLanded = await page.evaluate(() =>
    window.__SETTINGS_TEST__.models().some(item => item.model === 'refresh-twin-7c' && item.base_url === 'http://127.0.0.1:8225/v1'));
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]').click());
  await sleep(200);
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '本地模型');
  await sleep(200);
  await setPortDraft('8225');
  await clickExact(page, '检测');
  await sleep(600);
  const refreshReopenState = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = dialog && [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('refresh-twin-7c'));
    return {
      found: !!button,
      disabled: !!button && button.disabled,
      savedLabel: !!button && (button.textContent || '').includes('已保存'),
    };
  });
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('refresh-twin-7c'));
    button.click();
  });
  await sleep(300);
  const refreshSavesAfter = await callCount(page, 'save_model');
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(0));
  rec('⑦.4 刷新失败后身份不复活:落库成功但列表清空时,重开弹窗该行仍禁用+已保存且点击零落库',
    refreshRowClick.clicked && refreshLanded && refreshReopenState.found
      && refreshReopenState.disabled && refreshReopenState.savedLabel
      && refreshSavesAfter === refreshSavesBefore + 1,
    JSON.stringify({ click: refreshRowClick, landed: refreshLanded, reopen: refreshReopenState, savesBefore: refreshSavesBefore, savesAfter: refreshSavesAfter }));
  // ⑦.4b 同一分支的表单侧:duplicateSaved 的 just-committed 折叠活体——刷新
  // 失败后手填同一身份,保存禁用+提示可见(否则空列表表单同样邀请重加)。
  await clickExact(page, '手动添加本地模型');
  await sleep(300);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, 'http://127.0.0.1:8225/v1');
  });
  await sleep(150);
  // 本地手填表单的模型输入 placeholder 为空(⑥.5):按「本地模型 ID」label
  // 的相邻 input 定位(renderInlineField 的 label+input sibling 结构)。
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const modelInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === '本地模型 ID').nextElementSibling;
    typeInto(modelInput, 'refresh-twin-7c');
  });
  await sleep(250);
  const refreshFormState = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const save = root && root.querySelector('[data-testid="model-form-save"]');
    const hint = root && root.querySelector('[data-testid="model-form-duplicate-hint"]');
    return {
      saveDisabled: !!save && save.disabled,
      hintShown: !!hint,
    };
  });
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]').click());
  await sleep(200);
  rec('⑦.4b 刷新失败分支的表单侧:手填已提交未确认身份 → 保存禁用+提示可见(duplicateSaved 折叠)',
    refreshFormState.saveDisabled && refreshFormState.hintShown,
    JSON.stringify(refreshFormState));
  // 恢复已存列表(knob 已一次性自清;setActiveModel 触发桥内重载,⑦.2 同款)。
  // ⑦.4b 收尾已关弹窗,可选关。
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]')?.click());
  await sleep(200);
  await page.evaluate(() => window.TauriBridge.models.setActiveModel(window.__SETTINGS_TEST__.activeModelId()));
  await sleep(500);
  const listRecovered = await page.evaluate(() => {
    const text = document.body.innerText;
    return text.includes('deepseek-v4-pro') && text.includes('qwen36_35b_256k');
  });
  if (!listRecovered) throw new Error('⑦.4 后已存列表未恢复,后续场景会误红');
  // ⑦.5a 在飞禁关层:单行保存在飞时 Escape 不再收走端点面板、X 按钮禁用。
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(300);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, 'https://example.com/v1');
  });
  await sleep(150);
  await page.evaluate(() => window.__SETTINGS_TEST__.setProbeEndpointListing({
    status: 'listed',
    models: [{ id: 'probe-gate-5f', context_window_tokens: 4096, max_output_tokens: 512 }],
  }));
  await clickModalExact(page, '获取模型列表');
  await sleep(700);
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(2000));
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    const row = [...dialog.querySelectorAll('[data-testid="endpoint-model-row"]')].find(node => (node.textContent || '').includes('probe-gate-5f'));
    row.querySelector('[data-testid="endpoint-model-row-add"]').click();
  });
  await sleep(300);
  await page.keyboard.press('Escape');
  await sleep(250);
  const midPickGate = await page.evaluate(() => ({
    pickerStillOpen: !!document.querySelector('[data-testid="endpoint-model-picker-dialog"]'),
    closeDisabled: document.querySelector('[data-testid="endpoint-model-picker-close"]')?.disabled === true,
  }));
  await sleep(2100);
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(0));
  rec('⑦.5a 单行保存在飞:Escape 不关层且 X 禁用(写入在飞禁关层)',
    midPickGate.pickerStillOpen && midPickGate.closeDisabled,
    JSON.stringify(midPickGate));
  // ⑦.5b 批量执行中 Escape 不关层:重设 listing 只含未保存行 → 全部添加 →
  // 确认 → 批量循环进行中按 Escape → 面板与确认弹窗都仍在(旧实现把面板收走,
  // 批量继续在后台隐身写库)。
  await page.evaluate(() => window.__SETTINGS_TEST__.setProbeEndpointListing({
    status: 'listed',
    models: [{ id: 'batch-gate-5g', context_window_tokens: 2048, max_output_tokens: 256 }],
  }));
  await clickModalExact(page, '获取模型列表');
  await sleep(700);
  await page.evaluate(() => document.querySelector('[data-testid="endpoint-model-add-all"]').click());
  await sleep(300);
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(1200));
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="add-all-models-confirm"]');
    const confirm = [...dialog.querySelectorAll('button')].find(button => (button.textContent || '').trim() === '全部添加');
    confirm.click();
  });
  await sleep(300);
  await page.keyboard.press('Escape');
  await sleep(250);
  const midBatchGate = await page.evaluate(() => ({
    pickerStillOpen: !!document.querySelector('[data-testid="endpoint-model-picker-dialog"]'),
    confirmStillShown: !!document.querySelector('[data-testid="add-all-models-confirm"]'),
  }));
  await sleep(1400);
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(0));
  rec('⑦.5b 批量执行中:Escape 不关面板、确认弹窗保持可见(批量不在后台隐身写库)',
    midBatchGate.pickerStillOpen && midBatchGate.confirmStillShown,
    JSON.stringify(midBatchGate));
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    if (!dialog) return;
    const close = dialog.querySelector('[data-testid="endpoint-model-picker-close"]');
    if (close && !close.disabled) close.click();
  });
  await sleep(300);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]').click());
  await sleep(250);
  // ⑦.6 Save 同任务双击仅一笔(注册表按身份键控的活体证据:第二次点击同
  // 一身份查册即拒;state-only 旧实现下差为 2 即红)。
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(300);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, 'http://127.0.0.1:8321/v1');
  });
  await sleep(150);
  // 云端表单在提供凭据前禁用保存(⑥.6):走免鉴权声明解开(⑥.5k3 同款)。
  await page.evaluate(() => document.querySelector('[data-testid="model-form-no-auth-toggle"]').click());
  await sleep(150);
  await page.type('input[placeholder="输入模型 ID"]', 'dblsave-twin-7d', { delay: 5 });
  await sleep(150);
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(2200));
  const dblSaveSavesBefore = await callCount(page, 'save_model');
  await page.evaluate(() => {
    const save = document.querySelector('[data-testid="model-form-save"]');
    save.click();
    save.click();
  });
  await sleep(300);
  const dblSaveMidFlight = await callCount(page, 'save_model');
  await sleep(2400);
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(0));
  const dblSaveSettled = await page.evaluate(() =>
    window.__SETTINGS_TEST__.models().filter(item => item.model === 'dblsave-twin-7d').length);
  rec('⑦.6 Save 同任务双击只放行一次落库(注册表身份键控;state-only 守卫时差为 2 即红)',
    dblSaveMidFlight === dblSaveSavesBefore + 1 && dblSaveSettled === 1,
    JSON.stringify({ savesBefore: dblSaveSavesBefore, savesMidFlight: dblSaveMidFlight, settled: dblSaveSettled }));
  // 复审 r4 P2(J 的 A,B,B 交错复现的行为钉):批量 [A,B] 卡在 A 时取消
  // 重开弹窗(实例 ref 清零),新弹窗把 B 正常保存落库(J 复现里这一步是
  // 放行的——当时 B 确实未保存)并经 onSaveCommitted 记入父级 justSaved;
  // 原批次随后推进到 B:in-flight 注册表已随 B settle 出册,此时唯一挡住
  // 第二个 ID 的是 pendingSavedKeys(committed-but-unlisted)。三源缺一
  // 即红:去掉 pending 查,本用例的原批次会对 B 再落一笔。
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.setProbeEndpointListing({
      status: 'listed',
      models: [
        { id: 'batch-ab-a', context_window_tokens: 2048, max_output_tokens: 256 },
        { id: 'batch-ab-b', context_window_tokens: 2048, max_output_tokens: 256 },
      ],
    });
  });
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(300);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, 'https://batch-race.example.com/v1');
  });
  await sleep(150);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-no-auth-toggle"]').click());
  await sleep(150);
  await clickModalExact(page, '获取模型列表');
  await sleep(700);
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(1200));
  await page.evaluate(() => document.querySelector('[data-testid="endpoint-model-add-all"]').click());
  await sleep(300);
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="add-all-models-confirm"]');
    const confirm = [...dialog.querySelectorAll('button')].find(button => (button.textContent || '').trim() === '全部添加');
    confirm.click();
  });
  await sleep(150);
  // A 在飞:换段短延迟,让新弹窗的 B 先于原批次抵达 B 行前落库出册。
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(350));
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]').click());
  await sleep(250);
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(300);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, 'https://batch-race.example.com/v1');
  });
  await sleep(150);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-no-auth-toggle"]').click());
  await sleep(150);
  await clickModalExact(page, '获取模型列表');
  await sleep(700);
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="endpoint-model-row-add"]')].find(node =>
      (node.parentElement.textContent || '').includes('batch-ab-b'));
    button.click();
  });
  await sleep(700);
  const bSettledOnce = await page.evaluate(() =>
    window.__SETTINGS_TEST__.models().filter(item => item.model === 'batch-ab-b').length);
  await sleep(1400);
  const raceSettled = await page.evaluate(() => ({
    aSaved: window.__SETTINGS_TEST__.models().filter(item => item.model === 'batch-ab-a').length,
    bSaved: window.__SETTINGS_TEST__.models().filter(item => item.model === 'batch-ab-b').length,
    bSaves: window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model'
      && item.args && item.args.model && item.args.model.model === 'batch-ab-b').length,
    aSaves: window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model'
      && item.args && item.args.model && item.args.model.model === 'batch-ab-a').length,
  }));
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(0));
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    if (!dialog) return;
    const close = dialog.querySelector('[data-testid="endpoint-model-picker-close"]');
    if (close && !close.disabled) close.click();
  });
  await sleep(300);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]').click());
  await sleep(250);
  rec('⑦.6b 批量 A 卡在飞时重开弹窗保存 B:原批次推进到 B 必须命中 committed-unlisted 跳过(A,B 各恰一笔)',
    bSettledOnce === 1
      && raceSettled.aSaved === 1 && raceSettled.aSaves === 1
      && raceSettled.bSaved === 1 && raceSettled.bSaves === 1,
    JSON.stringify({ bSettledOnce, raceSettled }));

  // ⑦.6c/⑦.6d(r5 复审判别钉):⑦.6b 的 1200ms 时序里原批次先到 B(良性序,
  // 模新行被刷新后的 saved-list 禁行挡住)——committed/listed 两个活源都没参与。
  // 这两个用例把时序倒过来(A 拖 3500ms,重开弹窗的 B 先落库):循环推进到 B
  // 时唯一活源是模块级镜像/簿,缺一即红。⑦.6c 走成功路径(B 的 reload 落表 →
  // committed 出册 → 唯一活源是 listed);⑦.6d 在 B 的 reload 前武装一次性
  // list_models 失败(loadModels 吞错清列表 → listed 变空 → 唯一活源是
  // committed——这正是 committed 簿存在的分支)。
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.setProbeEndpointListing({
      status: 'listed',
      models: [
        { id: 'race6c-a', context_window_tokens: 2048, max_output_tokens: 256 },
        { id: 'race6c-b', context_window_tokens: 2048, max_output_tokens: 256 },
      ],
    });
  });
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(300);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, 'https://race6c.example.com/v1');
  });
  await sleep(150);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-no-auth-toggle"]').click());
  await sleep(150);
  await clickModalExact(page, '获取模型列表');
  await sleep(700);
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(3500));
  await page.evaluate(() => document.querySelector('[data-testid="endpoint-model-add-all"]').click());
  await sleep(300);
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="add-all-models-confirm"]');
    const confirm = [...dialog.querySelectorAll('button')].find(button => (button.textContent || '').trim() === '全部添加');
    confirm.click();
  });
  await sleep(150);
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(300));
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]').click());
  await sleep(250);
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(300);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, 'https://race6c.example.com/v1');
  });
  await sleep(150);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-no-auth-toggle"]').click());
  await sleep(150);
  await clickModalExact(page, '获取模型列表');
  await sleep(700);
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="endpoint-model-row-add"]')].find(node =>
      (node.parentElement.textContent || '').includes('race6c-b'));
    button.click();
  });
  await sleep(700);
  await sleep(2400);
  const race6c = await page.evaluate(() => ({
    aSaved: window.__SETTINGS_TEST__.models().filter(item => item.model === 'race6c-a').length,
    bSaved: window.__SETTINGS_TEST__.models().filter(item => item.model === 'race6c-b').length,
    bSaves: window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model'
      && item.args && item.args.model && item.args.model.model === 'race6c-b').length,
    aSaves: window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model'
      && item.args && item.args.model && item.args.model.model === 'race6c-a').length,
  }));
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(0));
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    if (!dialog) return;
    const close = dialog.querySelector('[data-testid="endpoint-model-picker-close"]');
    if (close && !close.disabled) close.click();
  });
  await sleep(300);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]').click());
  await sleep(250);
  rec('⑦.6c 竞态序(成功路径):重开弹窗的 B 先落库并列表确认,原批次推进到 B 必须命中实时 listed 镜像(A,B 各恰一笔)',
    race6c.aSaved === 1 && race6c.aSaves === 1 && race6c.bSaved === 1 && race6c.bSaves === 1,
    JSON.stringify(race6c));

  await page.evaluate(() => {
    window.__SETTINGS_TEST__.setProbeEndpointListing({
      status: 'listed',
      models: [
        { id: 'race6d-a', context_window_tokens: 2048, max_output_tokens: 256 },
        { id: 'race6d-b', context_window_tokens: 2048, max_output_tokens: 256 },
      ],
    });
  });
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(300);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, 'https://race6d.example.com/v1');
  });
  await sleep(150);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-no-auth-toggle"]').click());
  await sleep(150);
  await clickModalExact(page, '获取模型列表');
  await sleep(700);
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(3500));
  await page.evaluate(() => document.querySelector('[data-testid="endpoint-model-add-all"]').click());
  await sleep(300);
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="add-all-models-confirm"]');
    const confirm = [...dialog.querySelectorAll('button')].find(button => (button.textContent || '').trim() === '全部添加');
    confirm.click();
  });
  await sleep(150);
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(300));
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]').click());
  await sleep(250);
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(300);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, 'https://race6d.example.com/v1');
  });
  await sleep(150);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-no-auth-toggle"]').click());
  await sleep(150);
  await clickModalExact(page, '获取模型列表');
  await sleep(700);
  // B 落库后、其内联刷新前武装一次性失败:loadModels 吞错清列表 → listed
  // 镜像变空,循环推进到 B 时唯一活源是 committed 簿。
  await page.evaluate(() => window.__SETTINGS_TEST__.armListModelsFailure());
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="endpoint-model-row-add"]')].find(node =>
      (node.parentElement.textContent || '').includes('race6d-b'));
    button.click();
  });
  await sleep(700);
  await sleep(2400);
  const race6d = await page.evaluate(() => ({
    aSaved: window.__SETTINGS_TEST__.models().filter(item => item.model === 'race6d-a').length,
    bSaved: window.__SETTINGS_TEST__.models().filter(item => item.model === 'race6d-b').length,
    bSaves: window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model'
      && item.args && item.args.model && item.args.model.model === 'race6d-b').length,
    aSaves: window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model'
      && item.args && item.args.model && item.args.model.model === 'race6d-a').length,
  }));
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(0));
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    if (!dialog) return;
    const close = dialog.querySelector('[data-testid="endpoint-model-picker-close"]');
    if (close && !close.disabled) close.click();
  });
  await sleep(300);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]').click());
  await sleep(250);
  rec('⑦.6d 竞态序(刷新失败分支):B 的 reload 被吞、listed 变空,原批次推进到 B 必须命中 committed 活簿(A,B 各恰一笔)',
    race6d.aSaved === 1 && race6d.aSaves === 1 && race6d.bSaved === 1 && race6d.bSaves === 1,
    JSON.stringify(race6d));

  // ⑦.8(r5 复审 P2-2 判别):经表单入口保存 → 列表确认 → 经列表真实删除流
  // 删除 → 同身份重开表单再保存。契约:删除后恢复可保存(正文自述);旧实现
  // pendingSavedKeys 从只增不减的 state 派生,列表丢失身份后旧键复活为
  // pending → 保存禁用 + 提示谎称"已存在"(活体复现)。模块簿确认出册后,
  // 删除后无从复活。
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(300);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, 'https://retire8.example.com/v1');
  });
  await sleep(150);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-no-auth-toggle"]').click());
  await sleep(150);
  await page.type('input[placeholder="输入模型 ID"]', 'retire8-9a', { delay: 5 });
  await sleep(150);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-save"]').click());
  await sleep(1200);
  const retire8Saved = await page.evaluate(() =>
    window.__SETTINGS_TEST__.models().filter(item => item.model === 'retire8-9a').length);
  await page.evaluate(() => {
    const rows = [...document.querySelectorAll('button')]
      .filter(button => (button.textContent || '').trim() === '删除')
      .map(button => button.closest('[class*="grid-cols-["]'))
      .filter(row => row && (row.textContent || '').includes('retire8-9a'));
    const row = rows[rows.length - 1];
    const button = row && [...row.querySelectorAll('button')].find(candidate => (candidate.textContent || '').trim() === '删除');
    if (button) { button.scrollIntoView({ block: 'center' }); button.click(); }
  });
  await sleep(200);
  await clickExact(page, '删除模型');
  await sleep(600);
  const retire8Deleted = await page.evaluate(() =>
    window.__SETTINGS_TEST__.models().filter(item => item.model === 'retire8-9a').length);
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(300);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, 'https://retire8.example.com/v1');
  });
  await sleep(150);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-no-auth-toggle"]').click());
  await sleep(150);
  await page.type('input[placeholder="输入模型 ID"]', 'retire8-9a', { delay: 5 });
  await sleep(400);
  const retire8ReAdd = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const save = root.querySelector('[data-testid="model-form-save"]');
    const hint = root.querySelector('[data-testid="model-form-duplicate-hint"]');
    return {
      saveDisabled: !!save && save.disabled,
      hintShown: !!hint,
    };
  });
  if (!retire8ReAdd.saveDisabled && !retire8ReAdd.hintShown) {
    await page.evaluate(() => document.querySelector('[data-testid="model-form-save"]').click());
    await sleep(800);
  }
  const retire8Final = await page.evaluate(() =>
    window.__SETTINGS_TEST__.models().filter(item => item.model === 'retire8-9a').length);
  rec('⑦.8 经列表删除已保存身份后,同身份必须恢复可保存(保存按钮可用、无提示、能再落一笔)',
    retire8Saved === 1 && retire8Deleted === 0
      && !retire8ReAdd.saveDisabled && !retire8ReAdd.hintShown
      && retire8Final === 1,
    JSON.stringify({ retire8Saved, retire8Deleted, ...retire8ReAdd, retire8Final }));
  // 再保存成功时 doSave 已自行关弹窗(onCancel),cancel 按钮可能已不在 DOM。
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]')?.click());
  await sleep(200);

  // ⑦.7 保存中提示闸(M9 判别器):保存在飞时把表单模型改成本已保存的身份,
  // duplicateSaved 即刻翻真——提示必须仍隐藏(savingModel 闸)。特异性对照在
  // 全新的空闲表单上做:同一重复身份在非保存态必须显示提示。
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(300);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, 'http://127.0.0.1:8000/v1');
  });
  await sleep(150);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-no-auth-toggle"]').click());
  await sleep(150);
  await page.type('input[placeholder="输入模型 ID"]', 'hint-gate-7e', { delay: 5 });
  await sleep(150);
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(2500));
  await page.evaluate(() => document.querySelector('[data-testid="model-form-save"]').click());
  await sleep(400);
  await page.evaluate(() => {
    const input = document.querySelector('input[placeholder="输入模型 ID"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(input, 'qwen36_35b_256k');
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await sleep(250);
  const midSaveHintGate = await page.evaluate(() => ({
    hintHiddenWhileSaving: !document.querySelector('[data-testid="model-form-duplicate-hint"]'),
    saveDisabledWhileSaving: document.querySelector('[data-testid="model-form-save"]')?.disabled === true,
  }));
  await sleep(2600);
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(0));
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(300);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, 'http://127.0.0.1:8000/v1');
  });
  await sleep(150);
  await page.type('input[placeholder="输入模型 ID"]', 'qwen36_35b_256k', { delay: 5 });
  await sleep(250);
  const idleHintControl = await page.evaluate(() => ({
    hintVisibleIdle: !!document.querySelector('[data-testid="model-form-duplicate-hint"]'),
  }));
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]').click());
  await sleep(200);
  rec('⑦.7 保存中输入重复身份:提示保持隐藏(savingModel 闸,M9 判别器);空闲同身份显示提示(特异性对照)',
    midSaveHintGate.hintHiddenWhileSaving && midSaveHintGate.saveDisabledWhileSaving
      && idleHintControl.hintVisibleIdle,
    JSON.stringify({ mid: midSaveHintGate, idle: idleHintControl }));

  // ===== ⑦.9 review P2 (read-failure freeze; Edge reproduced one identity
  // under two record ids): a confirmed identity must not revive inside the
  // failed-read window. Discriminator vs ⑦.4: there the identity had just
  // landed but was NOT yet confirmed (the committed book intercepts); here
  // A was confirmed by the list (retired from committed), so after B's
  // refresh fails the frozen mirror is the only live source — without it,
  // two ids. The real-delete re-add behavior (zhuowp's same-comment demand)
  // is covered by the closing leg. Isolated roster 8240.=====
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '本地模型');
  await sleep(200);
  await setPortDraft('8240');
  await clickExact(page, '检测');
  await sleep(600);
  // A: add -> lands -> refresh succeeds -> confirmed by the list (from
  // now on A is held by the listed mirror alone).
  const freezeAddA = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('freeze-twin-a'));
    if (!button || button.disabled) return { clicked: false };
    button.click();
    return { clicked: true };
  });
  await sleep(900);
  // B: arm the next list_models failure, then add — the save lands, but
  // the refresh chain swallows the failure, empties the list and sets the
  // marker; the mirror freezes and the empty list must not retract A.
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.armListModelsFailure();
    window.__SETTINGS_TEST__.setSaveModelDelay(400);
  });
  const freezeAddB = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('freeze-twin-b'));
    if (!button || button.disabled) return { clicked: false };
    button.click();
    return { clicked: true };
  });
  await sleep(1300);
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelDelay(0));
  const freezeLanded = await page.evaluate(() => {
    const saved = window.__SETTINGS_TEST__.models();
    return {
      aSaved: saved.some(item => item.model === 'freeze-twin-a' && item.base_url === 'http://127.0.0.1:8240/v1'),
      bSaved: saved.some(item => item.model === 'freeze-twin-b' && item.base_url === 'http://127.0.0.1:8240/v1'),
    };
  });
  // Freeze assertions: reopen detect — on the emptied list A's row stays
  // disabled with the Saved label (frozen mirror) and clicking lands
  // nothing.
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]').click());
  await sleep(200);
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '本地模型');
  await sleep(200);
  await setPortDraft('8240');
  await clickExact(page, '检测');
  await sleep(600);
  const freezeRowsFrozen = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const rowButton = needle => {
      const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
        (button.parentElement.textContent || '').includes(needle));
      return button
        ? { found: true, disabled: button.disabled, savedLabel: (button.textContent || '').includes('已保存') }
        : { found: false };
    };
    return { a: rowButton('freeze-twin-a'), b: rowButton('freeze-twin-b') };
  });
  const freezeSavesBefore = await callCount(page, 'save_model');
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('freeze-twin-a'));
    if (button) button.click();
  });
  await sleep(300);
  const freezeSavesAfter = await callCount(page, 'save_model');
  // Form side, same contract: typing A's identity inside the failed-read
  // window keeps Save disabled with the hint (duplicateSaved folds the
  // frozen mirror).
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]').click());
  await sleep(200);
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '本地模型');
  await sleep(200);
  await clickExact(page, '手动添加本地模型');
  await sleep(300);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, 'http://127.0.0.1:8240/v1');
    const modelInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === '本地模型 ID').nextElementSibling;
    typeInto(modelInput, 'freeze-twin-a');
  });
  await sleep(250);
  const freezeFormGate = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const save = root && root.querySelector('[data-testid="model-form-save"]');
    const hint = root && root.querySelector('[data-testid="model-form-duplicate-hint"]');
    return { saveDisabled: !!save && save.disabled, hintShown: !!hint };
  });
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]').click());
  await sleep(200);
  rec('⑦.9 读取失败窗口内已确认身份不复活:空列表下 A 行禁用+已保存、点击零落库、表单侧保存禁用+提示(B 的刷新失败不撤回 A)',
    freezeAddA.clicked && freezeAddB.clicked && freezeLanded.aSaved && freezeLanded.bSaved
      && freezeRowsFrozen.a.found && freezeRowsFrozen.a.disabled && freezeRowsFrozen.a.savedLabel
      && freezeSavesAfter === freezeSavesBefore
      && freezeFormGate.saveDisabled && freezeFormGate.hintShown,
    JSON.stringify({ addA: freezeAddA, addB: freezeAddB, landed: freezeLanded, rows: freezeRowsFrozen, savesBefore: freezeSavesBefore, savesAfter: freezeSavesAfter, form: freezeFormGate }));
  // ⑦.9c (review P2, both reviewers, Edge-reproduced): the ENDPOINT
  // picker's single-row path joins the same freeze. Probing the same
  // endpoint while the marker is still on must show A's row with the Saved
  // tag and a disabled add button (the row view folds the frozen mirror)
  // and clicking it must land nothing (the pick entry mirrors the same
  // set). B's row is tagged through the committed book.
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]')?.click());
  await sleep(200);
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(300);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    const input = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    setter.call(input, 'http://127.0.0.1:8240/v1');
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await page.evaluate(() => window.__SETTINGS_TEST__.setProbeEndpointListing({
    status: 'listed',
    models: [{ id: 'freeze-twin-a' }, { id: 'freeze-twin-b' }],
  }));
  await clickModalExact(page, '获取模型列表');
  await sleep(700);
  const endpointFreezeSavesBefore = await callCount(page, 'save_model');
  const endpointFreezeRows = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    const rowOf = needle => {
      const row = [...dialog.querySelectorAll('[data-testid="endpoint-model-row"]')].find(node => (node.textContent || '').includes(needle));
      if (!row) return { found: false };
      const button = row.querySelector('[data-testid="endpoint-model-row-add"]');
      return {
        found: true,
        savedTag: !!row.querySelector('[data-testid="endpoint-model-saved-tag"]'),
        disabled: !!button && button.disabled,
      };
    };
    return { a: rowOf('freeze-twin-a'), b: rowOf('freeze-twin-b') };
  });
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    const row = [...dialog.querySelectorAll('[data-testid="endpoint-model-row"]')].find(node => (node.textContent || '').includes('freeze-twin-a'));
    const button = row && row.querySelector('[data-testid="endpoint-model-row-add"]');
    if (button) button.click();
  });
  await sleep(300);
  const endpointFreezeSavesAfter = await callCount(page, 'save_model');
  rec('⑦.9c 读取失败窗口内端点单行同口径:A 行已保存标签+按钮禁用、点击零落库(B 行经 committed 簿同样禁用)',
    endpointFreezeRows.a.found && endpointFreezeRows.a.savedTag && endpointFreezeRows.a.disabled
      && endpointFreezeRows.b.found && endpointFreezeRows.b.savedTag && endpointFreezeRows.b.disabled
      && endpointFreezeSavesAfter === endpointFreezeSavesBefore,
    JSON.stringify({ rows: endpointFreezeRows, savesBefore: endpointFreezeSavesBefore, savesAfter: endpointFreezeSavesAfter }));
  await page.evaluate(() => window.__SETTINGS_TEST__.setProbeEndpointListing({ status: 'listed', models: [] }));
  await page.evaluate(() => document.querySelector('[data-testid="endpoint-model-picker-close"]')?.click());
  await sleep(200);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]')?.click());
  await sleep(200);
  // Closing leg (the other side of the same demand): the next successful
  // read thaws the mirror — after really deleting A the same identity must
  // become addable again. setActiveModel chains the bridge reload (⑦.4's
  // knob), clearing the marker and refreshing the mirror.
  await page.evaluate(() => window.TauriBridge.models
    .setActiveModel(window.__SETTINGS_TEST__.activeModelId()));
  await sleep(500);
  const thawedListRecovered = await page.evaluate(() => {
    const text = document.body.innerText;
    return text.includes('freeze-twin-a') && text.includes('freeze-twin-b');
  });
  await clickRowAction(page, 'freeze-twin-a', '删除');
  await sleep(300);
  await clickExact(page, '删除模型');
  await sleep(500);
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '本地模型');
  await sleep(200);
  await setPortDraft('8240');
  await clickExact(page, '检测');
  await sleep(600);
  const thawedReaddable = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('freeze-twin-a'));
    return button
      ? { found: true, disabled: button.disabled, savedLabel: (button.textContent || '').includes('已保存') }
      : { found: false };
  });
  const thawedSave = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('freeze-twin-a'));
    if (!button || button.disabled) return { clicked: false };
    button.click();
    return { clicked: true };
  });
  await sleep(900);
  const thawedRelanded = await page.evaluate(() =>
    window.__SETTINGS_TEST__.models().some(item => item.model === 'freeze-twin-a' && item.base_url === 'http://127.0.0.1:8240/v1'));
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]')?.click());
  await sleep(200);
  rec('⑦.9b 成功读取解冻+真实删除可重加:列表恢复后删除 A,同身份行恢复可点并能再落一笔',
    thawedListRecovered && thawedReaddable.found && !thawedReaddable.disabled && !thawedReaddable.savedLabel
      && thawedSave.clicked && thawedRelanded,
    JSON.stringify({ recovered: thawedListRecovered, readdable: thawedReaddable, save: thawedSave, relanded: thawedRelanded }));

  // ⑦.9d (zhuowp's demanded coverage, endpoint side): a real deletion thaws
  // the mirror through its own successful refresh — after deleting A once
  // more, the endpoint row is addable again and the pick lands exactly one
  // record.
  await clickRowAction(page, 'freeze-twin-a', '删除');
  await sleep(300);
  await clickExact(page, '删除模型');
  await sleep(500);
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(300);
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    const input = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    setter.call(input, 'http://127.0.0.1:8240/v1');
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await page.evaluate(() => window.__SETTINGS_TEST__.setProbeEndpointListing({
    status: 'listed',
    models: [{ id: 'freeze-twin-a' }],
  }));
  await clickModalExact(page, '获取模型列表');
  await sleep(700);
  const endpointThawSavesBefore = await callCount(page, 'save_model');
  const endpointThawRow = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    const row = [...dialog.querySelectorAll('[data-testid="endpoint-model-row"]')].find(node => (node.textContent || '').includes('freeze-twin-a'));
    if (!row) return { found: false };
    const button = row.querySelector('[data-testid="endpoint-model-row-add"]');
    return {
      found: true,
      savedTag: !!row.querySelector('[data-testid="endpoint-model-saved-tag"]'),
      disabled: !!button && button.disabled,
    };
  });
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="endpoint-model-picker-dialog"]');
    const row = [...dialog.querySelectorAll('[data-testid="endpoint-model-row"]')].find(node => (node.textContent || '').includes('freeze-twin-a'));
    const button = row && row.querySelector('[data-testid="endpoint-model-row-add"]');
    if (button && !button.disabled) button.click();
  });
  await sleep(900);
  const endpointThawSavesAfter = await callCount(page, 'save_model');
  const endpointThawLanded = await page.evaluate(() =>
    window.__SETTINGS_TEST__.models().some(item => item.model === 'freeze-twin-a' && item.base_url === 'http://127.0.0.1:8240/v1'));
  rec('⑦.9d 成功读取解冻后端点单行恢复可添加:删除 A 后行可点且恰好落一笔',
    endpointThawRow.found && !endpointThawRow.savedTag && !endpointThawRow.disabled
      && endpointThawSavesAfter === endpointThawSavesBefore + 1 && endpointThawLanded,
    JSON.stringify({ row: endpointThawRow, savesBefore: endpointThawSavesBefore, savesAfter: endpointThawSavesAfter, landed: endpointThawLanded }));
  await page.evaluate(() => window.__SETTINGS_TEST__.setProbeEndpointListing({ status: 'listed', models: [] }));
  await page.evaluate(() => document.querySelector('[data-testid="endpoint-model-picker-close"]')?.click());
  await sleep(200);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]')?.click());
  await sleep(200);

  // ===== ⑦.9e review P2 (pre-mount boundary; two independent reviewer
  // reproductions): the failed refresh must land BEFORE Settings has ever
  // mounted. The main page's mirror has been populated since scenario ①,
  // so this runs on a dedicated fresh page where the first settings open
  // is genuinely first: boot reads the roster successfully, an armed
  // list_models failure lands pre-mount through the production
  // setActiveModel refresh chain, and the first-open guards must seed from
  // the shared lastConfirmedModels snapshot — a component-owned freeze
  // alone starts from a never-populated (empty) mirror and presents the
  // confirmed roster identity as addable (a second record id). The closing
  // leg re-adds through a real deletion's successful refresh, proving the
  // snapshot replacement keeps genuinely deleted models re-addable
  // (zhuowp's demand, snapshot edition). Isolated roster: the boot fixture
  // local-qwen (qwen36_35b_256k @ port 8000) itself. =====
  const page2 = await browser.newPage();
  page2.on('pageerror', error => errors.push('⑦.9e:' + error.message));
  page2.on('console', message => { if (message.type() === 'error') errors.push('⑦.9e console:' + message.text()); });
  await page2.evaluateOnNewDocument(injectSource());
  await page2.setViewport({ width: 1440, height: 1000 });
  await page2.goto(url, { waitUntil: 'networkidle0' });
  await page2.waitForFunction(() => window.TauriBridge && document.querySelector('[data-testid="app-root"]'), { timeout: 20000 });
  await sleep(1000);
  // Boot read succeeded (roster in the shared list); arm the one-shot
  // failure and land it pre-mount through the production refresh chain —
  // Settings has never mounted on this page.
  await page2.evaluate(() => {
    window.__SETTINGS_TEST__.armListModelsFailure();
    return window.TauriBridge.models.setActiveModel('cloud-deepseek');
  });
  await sleep(500);
  // FIRST settings open on this page: the mirror effect must seed the
  // saved-identity guards from the shared snapshot despite the emptied
  // list.
  await page2.evaluate(() => {
    const button = [...document.querySelectorAll('button[title="设置"]')].pop();
    if (button) button.click();
  });
  await page2.waitForFunction(() => document.querySelector('[data-testid="app-root"]')?.getAttribute('data-current-view') === 'settings', { timeout: 8000 });
  await sleep(400);
  await clickSettingsSection(page2, '模型');
  await sleep(300);
  await clickExact(page2, '添加模型');
  await sleep(300);
  await clickExact(page2, '本地模型');
  await sleep(200);
  await page2.evaluate(text => {
    const input = document.querySelector('[data-testid="local-detect-port"]');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, text);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  }, '8000');
  await clickExact(page2, '检测');
  await sleep(600);
  const premountGuard = await page2.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    if (!dialog) return { dialogFound: false };
    const savesBefore = window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model').length;
    const vllmButton = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('qwen36_35b_256k'));
    if (vllmButton) vllmButton.click();
    return {
      dialogFound: true,
      vllmButtonFound: !!vllmButton,
      disabled: vllmButton ? vllmButton.disabled : null,
      savedLabel: vllmButton ? (vllmButton.textContent || '').includes('已保存') : null,
      savesBefore,
      savesAfter: window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model').length,
    };
  });
  rec('⑦.9e 首开前失败仍守身份契约:pre-mount 刷新失败后首次挂载,共享快照播种的镜像让已确认身份禁用+「已保存」+点击零落库',
    premountGuard.dialogFound && premountGuard.vllmButtonFound && premountGuard.disabled
      && premountGuard.savedLabel && premountGuard.savesAfter === premountGuard.savesBefore,
    JSON.stringify(premountGuard));
  // Closing leg: a genuinely successful deletion refresh replaces the
  // shared snapshot — delete local-qwen (the confirmed identity above),
  // let the delete chain's refresh succeed (the armed failure is spent),
  // and the same identity must be addable again for exactly one record.
  await page2.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]')?.click());
  await sleep(200);
  // Recover the displayed list first (a successful read clears the failure
  // marker and re-renders the roster — the armed failure is spent); the
  // deletion's OWN refresh is what must replace the shared snapshot.
  await page2.evaluate(() => window.TauriBridge.models.setActiveModel('cloud-deepseek'));
  await sleep(500);
  await clickRowAction(page2, 'qwen36_35b_256k', '删除');
  await sleep(300);
  await clickExact(page2, '删除模型');
  await sleep(600);
  await clickExact(page2, '添加模型');
  await sleep(300);
  await clickExact(page2, '本地模型');
  await sleep(200);
  await page2.evaluate(text => {
    const input = document.querySelector('[data-testid="local-detect-port"]');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, text);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  }, '8000');
  await clickExact(page2, '检测');
  await sleep(600);
  const premountThawRow = await page2.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('qwen36_35b_256k'));
    return button
      ? { found: true, disabled: button.disabled, savedLabel: (button.textContent || '').includes('已保存') }
      : { found: false };
  });
  const premountReadd = await page2.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('qwen36_35b_256k'));
    if (!button || button.disabled) return { clicked: false };
    button.click();
    return { clicked: true };
  });
  await sleep(900);
  const premountRelanded = await page2.evaluate(() =>
    window.__SETTINGS_TEST__.models().some(item => item.model === 'qwen36_35b_256k' && item.base_url === 'http://127.0.0.1:8000/v1'));
  await page2.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]')?.click());
  await sleep(200);
  rec('⑦.9e 真实删除的成功刷新替换共享快照:同身份行恢复可点且恰好再落一笔',
    premountThawRow.found && !premountThawRow.disabled && !premountThawRow.savedLabel
      && premountReadd.clicked && premountRelanded,
    JSON.stringify({ row: premountThawRow, readd: premountReadd, relanded: premountRelanded }));
  await page2.close();

  // ===== ⑦.9f review P2×2 (late compound-save completion, zhuowp's exact
  // schedule): a local row add whose reload tail is still pending can race a
  // deletion of the freshly visible record — the user deletes A while the
  // save chain sits between loadModels (A already published) and the tail
  // end. The chain-end verdict must keep the deleted identity OUT of the
  // committed book: booked after the deletion, no later successful list could
  // ever contain it and the ordinary same-identity re-add would stay blocked
  // until reload. Isolated roster 8260; the get_settings hold parks the save
  // chain mid-tail so the deletion deterministically lands inside the window.
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '本地模型');
  await sleep(200);
  await setPortDraft('8260');
  await clickExact(page, '检测');
  await sleep(600);
  await page.evaluate(() => window.__SETTINGS_TEST__.armGetSettingsHold());
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('late-twin-a'));
    if (button) button.click();
  });
  await sleep(600); // save_model ✓ → loadModels ✓(A 已发布)→ get_settings 挂起(刷新尾中段)
  // 刷新尾窗内的删除:走生产 deleteModel 链(行按钮 onDeleteModel 的同一
  // 处理器)。不 await:删除链自己也会挂进保持的 get_settings 队列,evaluate
  // 等它落定会吊死 CDP;下面的 sleep 让链推进到 loadModels 更新列表。
  await page.evaluate(() => {
    const record = window.__SETTINGS_TEST__.models().find(item => item.model === 'late-twin-a');
    window.TauriBridge.models.deleteModel(record.id);
  });
  await sleep(600); // 删除链:delete_model ✓ → loadModels(无 A,镜像已更新)→ get_settings 亦挂起
  await page.evaluate(() => window.__SETTINGS_TEST__.releaseGetSettingsHold());
  await sleep(600); // 两链落定:保存链尾裁定读到的列表已无 A → 不入簿
  const lateReaddable = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('late-twin-a'));
    return button
      ? { found: true, disabled: button.disabled, savedLabel: (button.textContent || '').includes('已保存') }
      : { found: false };
  });
  const lateReadd = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('late-twin-a'));
    if (!button || button.disabled) return { clicked: false };
    button.click();
    return { clicked: true };
  });
  await sleep(900);
  const lateRelanded = await page.evaluate(() => ({
    saves: window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model'
      && item.args && item.args.model && item.args.model.model === 'late-twin-a').length,
    landed: window.__SETTINGS_TEST__.models().some(item => item.model === 'late-twin-a'),
  }));
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]')?.click());
  await sleep(200);
  rec('⑦.9f 刷新尾窗内删除:链尾裁定缺席不入簿,同身份行恢复可点且恰好再落一笔(晚到完成不残留永久预订)',
    lateReaddable.found && !lateReaddable.disabled && !lateReaddable.savedLabel
      && lateReadd.clicked && lateRelanded.saves === 2 && lateRelanded.landed,
    JSON.stringify({ readdable: lateReaddable, readd: lateReadd, relanded: lateRelanded }));

  // ===== ⑦.9g review P2 (full-URL → canonical list → delete → re-add):
  // Rust normalizes the persisted URL (normalize_provider_metadata strips the
  // /chat/completions suffix) before the record is ever listed; the frontend
  // dedup key must fold the same suffix or a booking made from the raw form
  // URL never matches its own canonical record — the false already-exists
  // guard would survive the record's deletion for the whole session. The mock
  // persistence (applySaveModel) mirrors the Rust normalization, so the
  // canonical lifecycle runs against listed-canonical records.
  const fillManualLocal = rawUrl => page.evaluate(url => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, url);
    const modelInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === '本地模型 ID').nextElementSibling;
    typeInto(modelInput, 'canonical-twin');
  }, rawUrl);
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '本地模型');
  await sleep(200);
  await clickExact(page, '手动添加本地模型');
  await sleep(250);
  await fillManualLocal('http://127.0.0.1:8290/v1/chat/completions/');
  await sleep(250);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-save"]').click());
  await sleep(900);
  const canonicalLanded = await page.evaluate(() =>
    window.__SETTINGS_TEST__.models().some(item => item.model === 'canonical-twin'
      && item.base_url === 'http://127.0.0.1:8290/v1'));
  // 在录时重开表单手填同 raw URL:键折叠后必须命中 canonical 记录——
  // 保存禁用+重复提示(真实重复,P2 修复前这单查不出来)。
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '本地模型');
  await sleep(200);
  await clickExact(page, '手动添加本地模型');
  await sleep(250);
  await fillManualLocal('http://127.0.0.1:8290/v1/chat/completions');
  await sleep(400);
  const canonicalDup = await page.evaluate(() => {
    const save = document.querySelector('[data-testid="model-form-save"]');
    const hint = document.querySelector('[data-testid="model-form-duplicate-hint"]');
    return {
      saveDisabled: !!save && save.disabled,
      hintShown: !!hint && (hint.textContent || '').trim().length > 0,
    };
  });
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]')?.click());
  await sleep(200);
  // 删除 → 同 raw URL 重加:预订随真实删除的成功刷新退休(缺席地板),
  // 保存恢复可用且恰好再落一笔 canonical 记录。
  await clickRowAction(page, 'canonical-twin', '删除');
  await sleep(300);
  await clickExact(page, '删除模型');
  await sleep(500);
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '本地模型');
  await sleep(200);
  await clickExact(page, '手动添加本地模型');
  await sleep(250);
  await fillManualLocal('http://127.0.0.1:8290/v1/chat/completions');
  await sleep(400);
  const canonicalReadd = await page.evaluate(() => {
    const save = document.querySelector('[data-testid="model-form-save"]');
    return { found: !!save, enabled: !!save && !save.disabled };
  });
  await page.evaluate(() => document.querySelector('[data-testid="model-form-save"]').click());
  await sleep(900);
  const canonicalRelanded = await page.evaluate(() => ({
    landed: window.__SETTINGS_TEST__.models().some(item => item.model === 'canonical-twin'
      && item.base_url === 'http://127.0.0.1:8290/v1'),
    count: window.__SETTINGS_TEST__.models().filter(item => item.model === 'canonical-twin').length,
  }));
  rec('⑦.9g 全URL→canonical→删除→重加:raw 键与 canonical 记录同键(在录时重复守卫命中),删除后预订随缺席退休,重加恢复可保存且恰好一笔',
    canonicalLanded && canonicalDup.saveDisabled && canonicalDup.hintShown
      && canonicalReadd.found && canonicalReadd.enabled
      && canonicalRelanded.landed && canonicalRelanded.count === 1,
    JSON.stringify({ landed: canonicalLanded, dup: canonicalDup, readd: canonicalReadd, relanded: canonicalRelanded }));

  // ===== ⑦.9h review round-10 P2 (superseded save-owned read): the save
  // chain's own list request N can be discarded by a newer concurrent
  // request N+1 that is still pending — last-write-wins publishes nothing,
  // so the chain ends with the PRE-upsert accepted world and the verdict
  // must degrade to 'superseded' (book present-only, never treat absence as
  // deletion). The old shape read the mutable chain-end state and produced
  // {ok, models:[B]} for a save nobody deleted, skipped the booking, and let
  // a same-identity re-add land a duplicate while N+1 was pending.
  // Schedule: hold N, race N+1 via a production setActiveModel reload chain,
  // resolve N with the post-upsert list (discarded), assert the identity is
  // still blocked, then let N+1 land and prove no duplicate was created.
  // Isolated roster 8270.
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '本地模型');
  await sleep(200);
  await setPortDraft('8270');
  await clickExact(page, '检测');
  await sleep(600);
  // 计数按身份过滤(与 blocked/afterAck 的口径一致,勿混总量)。
  const staleSavesBefore = await page.evaluate(() =>
    window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model'
      && item.args && item.args.model && item.args.model.model === 'stale-twin-a').length);
  await page.evaluate(() => window.__SETTINGS_TEST__.armListModelsHold(2));
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('stale-twin-a'));
    if (button) button.click();
  });
  await sleep(600); // save_model ✓ → loadModels N 挂起(序号已领取)
  // 并发更新请求 N+1:走生产 setActiveModel 重载链(行内其它场景同款旋钮)。
  // 不 await——链会停在第二格挂起上,evaluate 等它落定会吊死 CDP。
  await page.evaluate(() => {
    window.TauriBridge.models.setActiveModel(window.__SETTINGS_TEST__.activeModelId());
  });
  await sleep(600); // N+1 链推进到它自己的 list_models(第二格挂起)
  // N+1 到站轮询(确定性调度):它的链差一跳才到 list_models,等 held 涨到 2
  // (N、N+1 双双在飞)——超时说明链根本没起,场景自身要红而不是静默走样。
  let staleHeldSeen = false;
  for (let i = 0; i < 30; i += 1) {
    const held = await page.evaluate(() => window.__SETTINGS_TEST__.heldListModelsCount());
    if (held === 2) { staleHeldSeen = true; break; }
    await sleep(100);
  }
  if (!staleHeldSeen) throw new Error('⑦.9h N+1 未到站:setActiveModel 链未抵达挂起的 list_models');
  // N 携带含 A 的列表后解析——被 N+1 作废,不发布:裁定必须退化 superseded,
  // 入簿 present-only(旧形态在此读链尾 state 得到 {ok,[B]} 跳过入簿)。
  await page.evaluate(() => window.__SETTINGS_TEST__.releaseHeldListModels());
  await sleep(600);
  const staleBlocked = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('stale-twin-a'));
    const saves = window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model'
      && item.args && item.args.model && item.args.model.model === 'stale-twin-a').length;
    return button
      ? { found: true, disabled: button.disabled, savedLabel: (button.textContent || '').includes('已保存'), saves }
      : { found: false };
  });
  // N+1 落地(放行第二格)→ 受理观察 [B,A] → present 退休,守卫自然移交列表。
  await page.evaluate(() => window.__SETTINGS_TEST__.releaseHeldListModels());
  await sleep(600);
  const staleAfterAck = await page.evaluate(() => ({
    saves: window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model'
      && item.args && item.args.model && item.args.model.model === 'stale-twin-a').length,
    landed: window.__SETTINGS_TEST__.models().filter(item => item.model === 'stale-twin-a').length,
  }));
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]')?.click());
  await sleep(200);
  rec('⑦.9h 被作废的保存链读取:裁定退化 superseded,窗口内重加仍被簿拦截(零重复落库),后继受理后守卫移交列表且恰好一笔',
    staleSavesBefore >= 0
      && staleBlocked.found && staleBlocked.disabled && staleBlocked.savedLabel && staleBlocked.saves === staleSavesBefore + 1
      && staleAfterAck.saves === staleSavesBefore + 1 && staleAfterAck.landed === 1,
    JSON.stringify({ before: staleSavesBefore, blocked: staleBlocked, afterAck: staleAfterAck }));

  // ===== ⑦.9i review round-11 P2 (presence → deletion → late completion):
  // the superseded arm must carry accepted-publication generation evidence.
  // Schedule: the save chain's read N is parked; a newer read N+1 is raced
  // and RELEASED first, publishing [B,A] (accepted, after our seq — a
  // confirmed post-upsert observation); the user then deletes A, whose
  // newer refresh M publishes [B]; only after both publications does the
  // original tail finish. A superseded verdict without generation evidence
  // would book A present-only here and strand the reservation for a record
  // the accepted list already proves deleted (the legitimate re-add would
  // stay blocked until reload). The generation gate upgrades the verdict to
  // ok-absent and the booking is skipped. Isolated roster 8280.
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '本地模型');
  await sleep(200);
  await setPortDraft('8280');
  await clickExact(page, '检测');
  await sleep(600);
  // 计数按身份过滤。
  const genSavesBefore = await page.evaluate(() =>
    window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model'
      && item.args && item.args.model && item.args.model.model === 'gen-twin-a').length);
  await page.evaluate(() => window.__SETTINGS_TEST__.armGetSettingsHold());
  await page.evaluate(() => window.__SETTINGS_TEST__.armListModelsHold(3));
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('gen-twin-a'));
    if (button) button.click();
  });
  await sleep(600); // save_model ✓ → N 挂 slot1;链停在 get_settings 保持上
  await page.evaluate(() => {
    window.TauriBridge.models.setActiveModel(window.__SETTINGS_TEST__.activeModelId());
  });
  let genHeld = 0;
  for (let i = 0; i < 30; i += 1) {
    genHeld = await page.evaluate(() => window.__SETTINGS_TEST__.heldListModelsCount());
    if (genHeld === 2) break;
    await sleep(100);
  }
  if (genHeld !== 2) throw new Error('⑦.9i N+1 未到站');
  // ① N+1 先发布 [B,A](受理,序号 > N)——镜像随之更新为含 A 的世界。
  await page.evaluate(() => window.__SETTINGS_TEST__.releaseHeldListModels(2));
  await sleep(600);
  // ② 用户删除 A(生产链):M 挂 slot3 并发布 [B]。
  await page.evaluate(() => {
    const record = window.__SETTINGS_TEST__.models().find(item => item.model === 'gen-twin-a');
    window.TauriBridge.models.deleteModel(record.id);
  });
  let genHeldAfterDelete = 0;
  for (let i = 0; i < 30; i += 1) {
    genHeldAfterDelete = await page.evaluate(() => window.__SETTINGS_TEST__.heldListModelsCount());
    if (genHeldAfterDelete === 2) break;
    await sleep(100);
  }
  if (genHeldAfterDelete !== 2) throw new Error('⑦.9i M 未到站');
  // ③ 放行 N(作废)→ 放行 M(受理 [B])→ 最后放保存尾(get_settings 释放):
  // 裁定按已发布代际升级为 ok,缺席=已删 → 不入簿,合法重加即刻可用。
  await page.evaluate(() => window.__SETTINGS_TEST__.releaseHeldListModels(1));
  await sleep(300);
  await page.evaluate(() => window.__SETTINGS_TEST__.releaseHeldListModels());
  await sleep(300);
  await page.evaluate(() => window.__SETTINGS_TEST__.releaseGetSettingsHold());
  await sleep(700);
  const genAfter = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('gen-twin-a'));
    const saves = window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model'
      && item.args && item.args.model && item.args.model.model === 'gen-twin-a').length;
    return {
      found: !!button,
      disabled: !!button && button.disabled,
      savedLabel: !!button && (button.textContent || '').includes('已保存'),
      saves,
      landed: window.__SETTINGS_TEST__.models().filter(item => item.model === 'gen-twin-a').length,
    };
  });
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]')?.click());
  await sleep(200);
  rec('⑦.9i 在场→删除→迟到完成:superseded 臂携带受理代际证据,已发布观察确认删除后不入簿,合法重加不被搁浅(恰好一笔且行恢复可点)',
    genAfter.found && !genAfter.disabled && !genAfter.savedLabel
      && genAfter.saves === genSavesBefore + 1 && genAfter.landed === 0,
    JSON.stringify({ before: genSavesBefore, after: genAfter }));

  // ===== ⑦.9j review round-12 P2 (publications inside the remaining
  // refresh tail): the superseded classification must read LIVE accepted
  // evidence at chain completion, not the snapshot from the discarded
  // read's return. Schedule: the save chain's read N parks; N+1 races in
  // (seq taken, N now stale); N is RELEASED FIRST and returns discarded —
  // its captured evidence still describes the pre-save world; the chain
  // then parks INSIDE the get_settings hold (the tail window); N+1 and the
  // user's deletion of A publish while that hold is pending. A snapshot-
  // based classification (round 11) sees publishedSeq ≤ requestedSeq,
  // answers superseded and books A present-only — stranded. The live read
  // sees the deletion's publication and upgrades to ok-absent. Isolated
  // roster 8290.
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '本地模型');
  await sleep(200);
  await setPortDraft('8290');
  await clickExact(page, '检测');
  await sleep(600);
  // 计数按身份过滤。
  const gen2SavesBefore = await page.evaluate(() =>
    window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model'
      && item.args && item.args.model && item.args.model.model === 'gen2-twin-a').length);
  await page.evaluate(() => window.__SETTINGS_TEST__.armGetSettingsHold());
  await page.evaluate(() => window.__SETTINGS_TEST__.armListModelsHold(3));
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('gen2-twin-a'));
    if (button) button.click();
  });
  await sleep(600); // save_model ✓ → N 挂 slot1(此刻链停在 list_models 在飞)
  let gen2Held = 0;
  for (let i = 0; i < 30; i += 1) {
    gen2Held = await page.evaluate(() => window.__SETTINGS_TEST__.heldListModelsCount());
    if (gen2Held === 1) break;
    await sleep(100);
  }
  if (gen2Held !== 1) throw new Error('⑦.9j N 未到站');
  await page.evaluate(() => {
    window.TauriBridge.models.setActiveModel(window.__SETTINGS_TEST__.activeModelId());
  });
  for (let i = 0; i < 30; i += 1) {
    gen2Held = await page.evaluate(() => window.__SETTINGS_TEST__.heldListModelsCount());
    if (gen2Held === 2) break;
    await sleep(100);
  }
  if (gen2Held !== 2) throw new Error('⑦.9j N+1 未到站');
  // ① 先放 N:作废返回,快照证据停留在 upsert 之前的世界;保存链随即
  // 钻进 get_settings 保持(尾窗开启)——轮询到站后才继续,保证后续发布
  // 真正落在「剩余刷新 pending」窗口内。
  await page.evaluate(() => window.__SETTINGS_TEST__.releaseHeldListModels(1));
  let gen2Tail = 0;
  for (let i = 0; i < 30; i += 1) {
    gen2Tail = await page.evaluate(() => window.__SETTINGS_TEST__.heldGetSettingsCount());
    if (gen2Tail === 1) break;
    await sleep(100);
  }
  if (gen2Tail !== 1) throw new Error('⑦.9j 保存链未钻进尾窗');
  // ② N+1 发布 [B,A](受理,publishedSeq>N)——落在尾窗内,对 N 的快照
  // 不可见。
  await page.evaluate(() => window.__SETTINGS_TEST__.releaseHeldListModels());
  await sleep(400);
  // ③ 用户删除 A(生产链):M 挂 slot3 → 放行发布 [B](确认删除)。
  await page.evaluate(() => {
    const record = window.__SETTINGS_TEST__.models().find(item => item.model === 'gen2-twin-a');
    window.TauriBridge.models.deleteModel(record.id);
  });
  let gen2HeldAfterDelete = 0;
  for (let i = 0; i < 30; i += 1) {
    gen2HeldAfterDelete = await page.evaluate(() => window.__SETTINGS_TEST__.heldListModelsCount());
    if (gen2HeldAfterDelete === 1) break;
    await sleep(100);
  }
  if (gen2HeldAfterDelete !== 1) throw new Error('⑦.9j M 未到站');
  await page.evaluate(() => window.__SETTINGS_TEST__.releaseHeldListModels());
  await sleep(300);
  // ④ 放保存尾:链完成时点读取活体证据(已发布 [B],缺席 A)→ ok-缺席
  // → 不入簿;快照式分类在此日程下给 superseded,present-only 搁浅。
  await page.evaluate(() => window.__SETTINGS_TEST__.releaseGetSettingsHold());
  await sleep(700);
  const gen2After = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const button = [...dialog.querySelectorAll('[data-testid="local-model-row-add"]')].find(button =>
      (button.parentElement.textContent || '').includes('gen2-twin-a'));
    const saves = window.__SETTINGS_TEST__.calls.filter(item => item.cmd === 'save_model'
      && item.args && item.args.model && item.args.model.model === 'gen2-twin-a').length;
    return {
      found: !!button,
      disabled: !!button && button.disabled,
      savedLabel: !!button && (button.textContent || '').includes('已保存'),
      saves,
      landed: window.__SETTINGS_TEST__.models().filter(item => item.model === 'gen2-twin-a').length,
    };
  });
  await page.evaluate(() => document.querySelector('[data-testid="model-form-cancel"]')?.click());
  await sleep(200);
  rec('⑦.9j 尾窗内发布(N 先作废返回,N+1 与删除落在剩余刷新 pending 窗口):链完成时点读活体受理证据升级 ok-缺席,不入簿不搁浅,合法重加即刻可用(恰好一笔且行恢复可点)',
    gen2After.found && !gen2After.disabled && !gen2After.savedLabel
      && gen2After.saves === gen2SavesBefore + 1 && gen2After.landed === 0,
    JSON.stringify({ before: gen2SavesBefore, after: gen2After }));

  // ⑦.img 图片输入能力/视觉模型控件:渲染默认值、排除自身、保存往返。
  await clickRowAction(page, 'deepseek-v4-pro', '编辑');
  await sleep(300);
  const imageSectionDefault = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const text = root ? root.innerText : '';
    const capabilityToggle = root && root.querySelector('[data-testid="image-capability-toggle"]');
    const visionToggle = root && root.querySelector('[data-testid="vision-model-toggle"]');
    return {
      // deepseek-v4-pro is explicitly annotated text-only (false) in the
      // catalog; the edit form echoes that annotation ("image input not supported").
      hasCapabilityRow: !!capabilityToggle && (capabilityToggle.textContent || '').includes('不支持图片'),
      hasVisionRow: !!visionToggle && (visionToggle.textContent || '').includes('无'),
      hasHelpText: text.includes('当前模型不能看图时，用该模型分析图片'),
      // §11.8/§11.9 静态隐私说明:云端外发/本地不离机。
      hasPrivacyText: text.includes('使用云端模型时，图片会发送给你选择的模型服务商') && text.includes('本地模型图片不离开本机'),
    };
  });
  rec('⑦.img.1 edit form shows image-input capability/vision-model controls, catalog-annotated echo of the "not supported"/"none" values, and the static privacy notice', Object.values(imageSectionDefault).every(Boolean), JSON.stringify(imageSectionDefault));
  // 图片能力三档:自动处理/支持图片/不支持图片(「保存时检测」档已下线)。
  await page.click('[data-testid="image-capability-toggle"]');
  await sleep(200);
  const imageCapabilityOptions3 = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    return root ? [...root.querySelectorAll('[data-testid^="image-capability-option-"]')].map(node => node.getAttribute('data-testid')) : [];
  });
  await page.click('[data-testid="image-capability-toggle"]');
  await sleep(200);
  rec('⑦.img.1b 图片能力三档齐全(pinvou/enabled/disabled),不再提供保存时检测',
    imageCapabilityOptions3.includes('image-capability-option-pinvou')
      && imageCapabilityOptions3.includes('image-capability-option-enabled')
      && imageCapabilityOptions3.includes('image-capability-option-disabled')
      && !imageCapabilityOptions3.includes('image-capability-option-auto'),
    JSON.stringify(imageCapabilityOptions3));
  await page.click('[data-testid="image-capability-toggle"]');
  await sleep(200);
  await page.click('[data-testid="image-capability-option-enabled"]');
  await sleep(200);
  await page.click('[data-testid="vision-model-toggle"]');
  await sleep(200);
  const visionOptions = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const options = root ? [...root.querySelectorAll('[data-testid^="vision-model-option-"]')].map(node => node.getAttribute('data-testid')) : [];
    const editing = window.__SETTINGS_TEST__.models().find(model => model.model === 'deepseek-v4-pro');
    return {
      hasNone: options.includes('vision-model-option-none'),
      hasLocalQwen: options.includes('vision-model-option-local-qwen'),
      excludesSelf: !!editing && !options.includes(`vision-model-option-${editing.id}`),
    };
  });
  rec('⑦.img.2 视觉模型下拉含「无」与其他模型且排除当前模型自身', Object.values(visionOptions).every(Boolean), JSON.stringify(visionOptions));
  // 视觉模型候选不做 disabled 过滤:disabled 可能是历史探测误判残留
  // (如 kimi-for-coding 曾因探测链路 400 被回填),应由选择时的识图探测
  // 验证(supported 才可选),而不是提前隐藏。
  // models.loadModels left the facade with the dead-surface cleanup; after the
  // mock change, re-selecting the current active model chains the same
  // loadModels refresh so React re-renders the dialog's vision candidates from
  // the new savedModels.
  const toggleVision = async () => { await page.click('[data-testid="vision-model-toggle"]'); await sleep(150); };
  const refreshModels = () => page.evaluate(() => window.TauriBridge.models
    .setActiveModel(window.__SETTINGS_TEST__.activeModelId()));
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.setModelImageCapability('local-qwen', 'disabled');
  });
  await refreshModels();
  await toggleVision(); // 关闭再打开,按新候选渲染
  await toggleVision();
  const visionWithDisabled = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    return root ? [...root.querySelectorAll('[data-testid^="vision-model-option-"]')].map(node => node.getAttribute('data-testid')) : [];
  });
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.setModelImageCapability('local-qwen', 'auto');
  });
  await refreshModels();
  await toggleVision(); await toggleVision();
  const visionAfterRestore = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    return root ? [...root.querySelectorAll('[data-testid^="vision-model-option-"]')].map(node => node.getAttribute('data-testid')) : [];
  });
  rec('⑦.img.2b 视觉模型候选不因 disabled 标记隐藏(由选择探测验证)',
    visionWithDisabled.includes('vision-model-option-local-qwen')
      && visionAfterRestore.includes('vision-model-option-local-qwen'),
    JSON.stringify({ visionWithDisabled, visionAfterRestore }));
  // ⑦.img.2c/2d 视觉模型选择探测:点击不收起列表,该行右侧显示忙转圈
  // 「正在检测识图能力」;探测未通过拒绝选择并提示排查,通过后按结果收起列表。
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.setImageTestResponse({ status: 'unsupported', verified: false, summary: 'this model does not support image input', http_status: 400 });
    window.__SETTINGS_TEST__.setImageTestDelay(400);
  });
  await page.click('[data-testid="vision-model-option-local-qwen"]');
  await sleep(150);
  const visionProbing = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const probing = root && root.querySelector('[data-testid="vision-model-probing"]');
    return {
      probingShown: !!(probing && (probing.textContent || '').includes('正在测试图片能力')),
      spinning: !!(probing && probing.querySelector('.animate-spin')),
      listStillOpen: !!root.querySelector('[data-testid="vision-model-option-local-qwen"]'),
    };
  });
  rec('⑦.img.2c 视觉模型探测中:列表不收起,该行右侧显示忙转圈', Object.values(visionProbing).every(Boolean), JSON.stringify(visionProbing));
  await sleep(500);
  const visionRejected = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const toggle = root && root.querySelector('[data-testid="vision-model-toggle"]');
    const error = root && root.querySelector('[data-testid="vision-model-probe-error"]');
    return {
      stillNone: !!(toggle && (toggle.textContent || '').includes('无')),
      errorShown: !!(error && (error.textContent || '').includes('无法作为视觉模型')),
      probingCleared: !root.querySelector('[data-testid="vision-model-probing"]'),
      listStillOpen: !!root.querySelector('[data-testid="vision-model-option-local-qwen"]'),
    };
  });
  rec('⑦.img.2d 视觉模型探测未通过:拒绝选择、提示排查且列表保持展开', Object.values(visionRejected).every(Boolean), JSON.stringify(visionRejected));
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.setImageTestResponse({ status: 'supported', verified: true, summary: '红色', http_status: 200 });
    window.__SETTINGS_TEST__.setImageTestDelay(0);
  });
  await page.click('[data-testid="vision-model-option-local-qwen"]');
  await sleep(400);
  const visionAccepted = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const toggle = root && root.querySelector('[data-testid="vision-model-toggle"]');
    return {
      selected: !!(toggle && (toggle.textContent || '').includes('本地 vLLM')),
      listClosed: !root.querySelector('[data-testid="vision-model-option-local-qwen"]'),
    };
  });
  rec('⑦.img.2e 视觉模型探测通过:按结果收起列表并选中', Object.values(visionAccepted).every(Boolean), JSON.stringify(visionAccepted));
  await clickExact(page, '保存');
  await sleep(500);
  const savedImageConfig = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return call && call.args && call.args.model;
  });
  rec('⑦.img.3 保存写入图片能力 override 与视觉模型引用',
    savedImageConfig
      && savedImageConfig.image_capability_override === 'enabled'
      && savedImageConfig.vision_model_id === 'local-qwen',
    JSON.stringify(savedImageConfig && { override: savedImageConfig.image_capability_override, vision: savedImageConfig.vision_model_id }));
  await clickRowAction(page, 'deepseek-v4-pro', '编辑');
  await sleep(300);
  const imageSectionRoundTrip = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const capabilityToggle = root && root.querySelector('[data-testid="image-capability-toggle"]');
    const visionToggle = root && root.querySelector('[data-testid="vision-model-toggle"]');
    return {
      capabilityEcho: !!capabilityToggle && (capabilityToggle.textContent || '').includes('支持图片'),
      visionEcho: !!visionToggle && (visionToggle.textContent || '').includes('本地 vLLM'),
    };
  });
  rec('⑦.img.4 重新打开编辑表单回显已保存的图片能力与视觉模型', Object.values(imageSectionRoundTrip).every(Boolean), JSON.stringify(imageSectionRoundTrip));
  await page.click('[data-testid="vision-model-toggle"]');
  await sleep(200);
  await page.click('[data-testid="vision-model-option-none"]');
  await sleep(200);
  await clickExact(page, '保存');
  await sleep(500);
  const clearedVision = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return call && call.args && call.args.model;
  });
  rec('⑦.img.5 视觉模型选回「无」保存为 null 且能力 override 保留',
    clearedVision
      && clearedVision.vision_model_id === null
      && clearedVision.image_capability_override === 'enabled',
    JSON.stringify(clearedVision && { override: clearedVision.image_capability_override, vision: clearedVision.vision_model_id }));

  // ⑦.img.6-11 测试图片能力(设计 §7.3):按钮渲染、supported/unsupported/error 分态、表单变更清除结果。
  await clickRowAction(page, 'deepseek-v4-pro', '编辑');
  await sleep(300);
  const imageTestInitial = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const button = root && root.querySelector('[data-testid="image-capability-test"]');
    const result = root && root.querySelector('[data-testid="image-capability-test-result"]');
    return {
      hasButton: !!button && (button.textContent || '').includes('测试图片能力'),
      buttonEnabled: !!button && !button.disabled,
      hintShown: !!result && (result.textContent || '').includes('纯色测试图'),
    };
  });
  rec('⑦.img.6 编辑模型展示「测试图片能力」按钮且默认显示提示文案', Object.values(imageTestInitial).every(Boolean), JSON.stringify(imageTestInitial));
  await page.click('[data-testid="image-capability-test"]');
  await sleep(300);
  const imageTestSupported = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const result = root && root.querySelector('[data-testid="image-capability-test-result"]');
    const text = result ? (result.textContent || '') : '';
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'test_image_input_capability');
    return {
      text,
      args: call && call.args,
      showsSupported: text.includes('支持图片'),
      showsReply: text.includes('模型回复：红色'),
      // 当前档位已是「支持图片」(⑦.img.3 保存),不应再提示设置。
      noEnableHint: !text.includes('可在上方将图片输入能力设为'),
    };
  });
  rec('⑦.img.7 supported 结果展示模型回复摘要且按当前表单值发起测试',
    imageTestSupported.showsSupported
      && imageTestSupported.showsReply
      && imageTestSupported.noEnableHint
      && imageTestSupported.args
      && imageTestSupported.args.model === 'deepseek-v4-pro'
      && imageTestSupported.args.baseUrl === 'https://api.deepseek.com'
      && imageTestSupported.args.apiKey === '',
    JSON.stringify(imageTestSupported));
  // 档位切回「自动处理」,supported 结果应提示可设为「支持图片」。
  await page.click('[data-testid="image-capability-toggle"]');
  await sleep(200);
  await page.click('[data-testid="image-capability-option-pinvou"]');
  await sleep(200);
  await page.click('[data-testid="image-capability-test"]');
  await sleep(300);
  const imageTestAutoHint = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const result = root && root.querySelector('[data-testid="image-capability-test-result"]');
    const text = result ? (result.textContent || '') : '';
    return { text, showsEnableHint: text.includes('可在上方将图片输入能力设为') };
  });
  rec('⑦.img.8 档位为自动处理时 supported 结果提示可设为「支持图片」', imageTestAutoHint.showsEnableHint, imageTestAutoHint.text);
  await page.evaluate(() => window.__SETTINGS_TEST__.setImageTestResponse({ status: 'unsupported', verified: false, summary: 'this model does not support image input', http_status: 400 }));
  await page.click('[data-testid="image-capability-test"]');
  await sleep(300);
  const imageTestUnsupported = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const result = root && root.querySelector('[data-testid="image-capability-test-result"]');
    const text = result ? (result.textContent || '') : '';
    return {
      text,
      showsUnsupported: text.includes('不支持图像识别'),
      showsProvider: text.includes('does not support image input'),
    };
  });
  rec('⑦.img.9 unsupported 结果展示 provider 错误摘要', imageTestUnsupported.showsUnsupported && imageTestUnsupported.showsProvider, imageTestUnsupported.text);
  // 审阅缺口 #104:未识别出测试色(2xx 无关回复 / 400 非图片拒绝)统一显示
  // 「未能正确识别图像，原因未知」,不宣称支持也不宣称不支持。
  await page.evaluate(() => window.__SETTINGS_TEST__.setImageTestResponse({ status: 'unverified', verified: false, summary: '未能正确识别图像，原因未知（模型回复：一张正方形图片）', http_status: 200 }));
  await page.click('[data-testid="image-capability-test"]');
  await sleep(300);
  const imageTestUnverified = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const result = root && root.querySelector('[data-testid="image-capability-test-result"]');
    const text = result ? (result.textContent || '') : '';
    return {
      text,
      showsUnverified: text.includes('未能正确识别图像，原因未知'),
      showsProvider: text.includes('正方形图片'),
      noEnableHint: !text.includes('可在上方将图片输入能力设为'),
      notClaimingUnsupported: !text.includes('不支持图像识别'),
    };
  });
  rec('⑦.img.9b 未识别态:原因未知,不宣称支持或不支持,展示摘要且不提示设档', Object.values(imageTestUnverified).every(Boolean), imageTestUnverified.text);
  await page.evaluate(() => window.__SETTINGS_TEST__.setImageTestResponse({ status: 'error', verified: false, summary: '连接超时', http_status: null }));
  await page.click('[data-testid="image-capability-test"]');
  await sleep(300);
  const imageTestError = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const result = root && root.querySelector('[data-testid="image-capability-test-result"]');
    const text = result ? (result.textContent || '') : '';
    return { text, showsError: text.includes('测试失败') && !text.includes('不支持图像识别') && !text.includes('支持图片') };
  });
  rec('⑦.img.10 error 结果与「不支持」严格区分', imageTestError.showsError, imageTestError.text);
  // 表单值变化后上一次测试结果应清除(恢复提示文案)。按 testid 选择:API Key 输入框
  // 统一为 type=text + WebkitTextSecurity 掩码(消除 WebView2 自带的第二个眼睛图标),
  // 已存 Key 时占位符也是掩码,按 type 或 placeholder 选择都会落空。
  const imageTestKeyInput = await page.$('[data-testid="model-form-dialog"] [data-testid="model-api-key-input"]');
  await imageTestKeyInput.type('k');
  await sleep(200);
  const imageTestCleared = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const result = root && root.querySelector('[data-testid="image-capability-test-result"]');
    const text = result ? (result.textContent || '') : '';
    return { text, backToHint: text.includes('纯色测试图') };
  });
  rec('⑦.img.11 表单值变化后清除上一次测试结果', imageTestCleared.backToHint, imageTestCleared.text);
  await clickExact(page, '取消');
  await sleep(200);

  // ⑦.img.12 保存不再自动检测:任意档位保存都不带探测参数、直接落盘关闭。
  const echoOverride = async () => {
    await clickRowAction(page, 'deepseek-v4-pro', '编辑');
    await sleep(300);
    const state = await page.evaluate(() => {
      const root = document.querySelector('[data-testid="model-form-dialog"]');
      const toggle = root && root.querySelector('[data-testid="image-capability-toggle"]');
      return toggle ? (toggle.textContent || '') : '';
    });
    await clickExact(page, '取消');
    await sleep(200);
    return state;
  };
  await clickRowAction(page, 'deepseek-v4-pro', '编辑');
  await sleep(300);
  await page.click('[data-testid="image-capability-toggle"]');
  await sleep(200);
  await page.click('[data-testid="image-capability-option-pinvou"]');
  await sleep(200);
  await page.click('[data-testid="model-form-save"]');
  await sleep(400);
  const savedPinvou = await page.evaluate(() => {
    const saveCall = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return {
      dialogClosed: !document.querySelector('[data-testid="model-form-dialog"]'),
      noProbeArg: !(saveCall && saveCall.args && saveCall.args.probeImageCapability),
      savedWithPinvou: !!(saveCall && saveCall.args && saveCall.args.model.image_capability_override === 'pinvou'),
    };
  });
  rec('⑦.img.12 「自动处理」档保存:不探测、直接落盘关闭',
    savedPinvou.dialogClosed && savedPinvou.noProbeArg && savedPinvou.savedWithPinvou,
    JSON.stringify(savedPinvou));
  const echoPinvou = await echoOverride();
  // Reopen after saving with pinvou (not pinned): deepseek-v4-pro is
  // explicitly annotated false in the catalog, so the form echoes the
  // annotation ("image input not supported"); only unannotated models fall
  // back to the "auto" (auto-handling) tier.
  rec('⑦.img.12b reopened form echoes the catalog "image input not supported" annotation', echoPinvou.includes('不支持图片'), echoPinvou);

  // Legacy detect-on-save (auto) tier leftover: the reopened form
  // must not render the retired detect-on-save tier,
  // and unpinned tiers echo the catalog annotation (this model is annotated
  // false → "image input not supported"). In the production path "auto"
  // only reaches the frontend after Rust migrates it to pinvou via serde;
  // feeding "auto" directly here exercises the frontend's defensive layer
  // (the serde migration has its own settings unit tests). After the mock
  // changes the tier, re-selecting the active model chains the loadModels
  // refresh (same as ⑦.img.2b).
  await page.evaluate(() => window.__SETTINGS_TEST__.setModelImageCapability(
    window.__SETTINGS_TEST__.models().find(model => model.model === 'deepseek-v4-pro').id, 'auto'));
  await page.evaluate(() => window.TauriBridge.models
    .setActiveModel(window.__SETTINGS_TEST__.activeModelId()));
  await sleep(200);
  const echoLegacyAuto = await echoOverride();
  rec('⑦.img.12c legacy auto tier leftover does not render the retired detect-on-save tier and echoes the catalog annotation',
    echoLegacyAuto.includes('不支持图片') && !echoLegacyAuto.includes('保存时检测'),
    echoLegacyAuto);

  // 保存失败(连接/写盘错误):弹窗保持 + 行内错误提示,表单输入不丢弃;
  // 修正后重试成功正常关闭(不再依赖不存在的"调用链处理")。
  await clickRowAction(page, 'deepseek-v4-pro', '编辑');
  await sleep(300);
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelError('磁盘写入失败'));
  await page.click('[data-testid="model-form-save"]');
  await sleep(400);
  const saveFailed = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const error = root && root.querySelector('[data-testid="model-form-save-error"]');
    return {
      dialogOpen: !!root,
      errorShown: !!(error && (error.textContent || '').includes('磁盘写入失败')),
    };
  });
  rec('⑦.img.13 保存失败:弹窗保持并显示行内错误',
    saveFailed.dialogOpen && saveFailed.errorShown,
    JSON.stringify(saveFailed));
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelError(null));
  await page.click('[data-testid="model-form-save"]');
  await sleep(400);
  const retrySaved = await page.evaluate(() => !document.querySelector('[data-testid="model-form-dialog"]'));
  rec('⑦.img.13b 修正后重试保存成功关闭弹窗', retrySaved, String(retrySaved));

  // ⑦.img.14 catalog vision-capability annotations auto-fill the image-input
  // (image input capability) control: the Kimi family and DeepSeek group
  // first items (k3 / deepseek-flash) are both annotated, so opening prefills
  // the "image input supported" value; after manually changing the tier,
  // switching entries no longer follows the catalog annotation (switching to
  // the false-annotated deepseek-v4-pro keeps the manual value).
  const capabilityToggleText = () => page.evaluate(() => {
    const toggle = document.querySelector('[data-testid="model-form-dialog"] [data-testid="image-capability-toggle"]');
    return toggle ? (toggle.textContent || '') : '';
  });
  await clickExact(page, '添加模型');
  await sleep(250);
  await clickExact(page, 'Kimi Coding Plan');
  await sleep(300);
  const capGroupDefault = await capabilityToggleText();
  await clickExact(page, '取消');
  await sleep(200);
  await clickExact(page, '添加模型');
  await sleep(250);
  await clickExact(page, '深度求索 / DeepSeek');
  await sleep(300);
  const capDeepseekDefault = await capabilityToggleText();
  await page.click('[data-testid="image-capability-toggle"]');
  await sleep(200);
  await page.click('[data-testid="image-capability-option-pinvou"]');
  await sleep(200);
  await clickModalExact(page, '模型');
  await sleep(250);
  await clickModalExact(page, 'deepseek-v4-pro');
  await sleep(250);
  const capAfterTouched = await capabilityToggleText();
  rec('⑦.img.14 目录视觉能力标注自动填写,手动改档后不再跟随',
    capGroupDefault.includes('支持图片')
      && capDeepseekDefault.includes('支持图片')
      && capAfterTouched.includes('自动处理'),
    JSON.stringify({ capGroupDefault, capDeepseekDefault, capAfterTouched }));
  await clickExact(page, '取消');
  await sleep(200);

  await clickSettingsSection(page, '搜索');
  const searchList = await page.evaluate(() => {
    const text = document.body.innerText;
    return {
      hasBing: text.includes('Bing'),
      hasMetaso: text.includes('秘塔'),
      noStatusNoise: !text.includes('已配置') && !text.includes('未配置'),
      noStandaloneSave: !text.includes('保存搜索配置'),
      addInGroup: text.includes('添加搜索源'),
    };
  });
  rec('⑧ 搜索列表按模型页交互精简', Object.values(searchList).every(Boolean), JSON.stringify(searchList));

  await clickRowAction(page, '秘塔', '编辑');
  await sleep(250);
  const existingSearchNoChange = await page.evaluate(() => {
    const text = document.body.innerText;
    const save = [...document.querySelectorAll('button')].reverse().find(button => (button.textContent || '').trim() === '保存');
    return {
      dialog: text.includes('编辑搜索源') && text.includes('秘塔'),
      masked: !!document.querySelector('input[placeholder="••••••••"]'),
      saveDisabled: !!save && save.disabled,
    };
  });
  rec('⑧.1 已有搜索源未输入新 Key 时保存置灰', Object.values(existingSearchNoChange).every(Boolean), JSON.stringify(existingSearchNoChange));
  await clickExact(page, '取消');
  await sleep(150);

  const modelsBeforeSearchSave = await page.evaluate(() => window.__SETTINGS_TEST__.models().map(model => model.id).sort()); // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic order of string arrays matches assertion expectations
  const savesBeforePick = await callCount(page, 'save_search_settings_and_restart');
  await clickExact(page, '添加搜索源');
  await sleep(250);
  const searchPickerWidth = await modalWidth(page, '添加搜索源');
  await clickExact(page, '博查');
  await sleep(300);
  const pickWithoutSave = await page.evaluate(() => ({
    restartDialog: document.body.innerText.includes('重启以应用搜索配置？'),
    saveButtonDisabled: [...document.querySelectorAll('button')].some(button => (button.textContent || '').trim() === '保存' && button.disabled),
    bochaInSavedSettings: (window.__SETTINGS_TEST__.settings().search.enabled_providers || []).includes('bocha'),
  }));
  const savesAfterPick = await callCount(page, 'save_search_settings_and_restart');
  rec('⑨ 选择搜索源但未点保存不会持久化', searchPickerWidth >= 430 && searchPickerWidth <= 455 && !pickWithoutSave.restartDialog && pickWithoutSave.saveButtonDisabled && !pickWithoutSave.bochaInSavedSettings && savesAfterPick === savesBeforePick, JSON.stringify({ searchPickerWidth, ...pickWithoutSave, savesBeforePick, savesAfterPick }));

  const searchKeyInput = await page.$('input[placeholder="输入 API Key"]');
  await searchKeyInput.click();
  await searchKeyInput.type('bocha-key');
  const typedValue = await page.evaluate(() => document.querySelector('input[placeholder="输入 API Key"]')?.value || '');
  rec('⑩ 搜索源 API Key 输入框可连续输入', typedValue === 'bocha-key', typedValue);
  await clickExact(page, '保存');
  await sleep(300);
  const restartPrompt = await page.evaluate(() => document.body.innerText.includes('重启以应用搜索配置？'));
  const savesBeforeRestart = await callCount(page, 'save_search_settings_and_restart');
  rec('⑪ 搜索源保存后先提示重启，未确认前不写盘重启', restartPrompt && savesBeforeRestart === savesBeforePick, String(savesBeforeRestart));
  await clickExact(page, '现在重启');
  await sleep(300);
  const searchSaved = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_search_settings_and_restart');
    const search = call && call.args && call.args.search;
    return search && {
      provider: search.provider,
      enabled: search.enabled_providers,
      bochaAction: search.credentials && search.credentials.bocha && search.credentials.bocha.credential_action,
      bochaKey: search.credentials && search.credentials.bocha && search.credentials.bocha.api_key,
    };
  });
  rec('⑫ 确认重启后写入搜索源和凭据草稿', searchSaved && searchSaved.provider === 'bocha' && searchSaved.enabled.includes('bocha') && searchSaved.bochaAction === 'replace' && searchSaved.bochaKey === 'bocha-key', JSON.stringify(searchSaved));

  await clickSettingsSection(page, '搜索');
  const modelsAfterSearchSave = await page.evaluate(() => window.__SETTINGS_TEST__.models().map(model => model.id).sort()); // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic order of string arrays matches assertion expectations
  rec('新增模型后保存搜索配置不会清空模型', JSON.stringify(modelsAfterSearchSave) === JSON.stringify(modelsBeforeSearchSave), JSON.stringify({ modelsBeforeSearchSave, modelsAfterSearchSave }));
  const savesBeforeDeleteLater = await callCount(page, 'update_search_settings');
  const restartsBeforeDeleteLater = await callCount(page, 'save_search_settings_and_restart');
  await clickRowAction(page, '秘塔', '删除');
  await sleep(250);
  const searchDeleteWidth = await modalWidth(page, '删除搜索源？');
  const searchDeleteDialog = await page.evaluate(() => document.body.innerText.includes('删除搜索源？') && document.body.innerText.includes('将移除 秘塔'));
  await clickExact(page, '删除搜索源');
  await sleep(250);
  await clickExact(page, '稍后');
  await sleep(300);
  const deleteLaterSaved = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'update_search_settings');
    const search = call && call.args && call.args.search;
    return search && {
      enabled: search.enabled_providers,
      metasoAction: search.credentials && search.credentials.metaso && search.credentials.metaso.credential_action,
    };
  });
  const savesAfterDeleteLater = await callCount(page, 'update_search_settings');
  const restartsAfterDeleteLater = await callCount(page, 'save_search_settings_and_restart');
  rec('⑬ 删除搜索源使用窄 iOS 确认框，选择稍后会写盘但不重启',
    searchDeleteDialog
      && searchDeleteWidth >= 260
      && searchDeleteWidth <= 285
      && savesAfterDeleteLater === savesBeforeDeleteLater + 1
      && restartsAfterDeleteLater === restartsBeforeDeleteLater
      && deleteLaterSaved
      && !deleteLaterSaved.enabled.includes('metaso')
      && deleteLaterSaved.metasoAction === 'delete',
    JSON.stringify({ width: searchDeleteWidth, savesBeforeDeleteLater, savesAfterDeleteLater, restartsBeforeDeleteLater, restartsAfterDeleteLater, deleteLaterSaved }));

  await page.evaluate(() => window.__SETTINGS_TEST__.setDependencyCheckResponse([
    { key: 'voice_asr_model', installed: false, apt: '', install_action: 'voice_asr_model' },
    { key: 'knowledge_embedding_model', installed: false, apt: '', install_action: 'knowledge_embedding_model' },
  ]));
  await clickSettingsSection(page, '权限与环境');
  await page.evaluate(() => document.querySelector('#settings-dependencies button')?.click());
  await sleep(300);
  const onDemandModels = await page.evaluate(() => {
    const section = document.querySelector('#settings-dependencies');
    const text = section?.textContent || '';
    const buttons = [...(section?.querySelectorAll('button') || [])];
    buttons.at(-1)?.click();
    return {
      voiceModelVisible: text.includes('SenseVoice q8'),
      knowledgeModelVisible: text.includes('bge-m3'),
      runtimeItemHidden: !text.includes('本地语音识别'),
      buttonCount: buttons.length,
    };
  });
  await sleep(300);
  const modelInstallCall = await page.evaluate(() => {
    const call = window.__SETTINGS_TEST__.calls.find(item => item.cmd === 'install_dependencies');
    return call && call.args;
  });
  rec('⑭.1 Windows 仅展示可修复模型并调用对应下载动作',
    onDemandModels.voiceModelVisible
      && onDemandModels.knowledgeModelVisible
      && onDemandModels.runtimeItemHidden
      && onDemandModels.buttonCount === 2
      && Array.isArray(modelInstallCall?.packages)
      && modelInstallCall.packages.length === 0
      && JSON.stringify(modelInstallCall.actions) === JSON.stringify(['voice_asr_model', 'knowledge_embedding_model']),
    JSON.stringify({ onDemandModels, modelInstallCall }));
  await page.evaluate(() => {
    const row = [...document.querySelectorAll('div')].find(node => (node.textContent || '').includes('高级执行权限') && node.querySelector('[role="switch"]'));
    const button = row && row.querySelector('[role="switch"]');
    if (button) button.click();
  });
  await sleep(600);
  const permToast = await page.evaluate(() => ({
    showSuperPermissionSettings: !!(window.TauriBridge.state.get('platform').platformCapabilities || {}).showSuperPermissionSettings,
    setCall: window.__SETTINGS_TEST__.calls.some(call => call.cmd === 'set_super_permission'),
    toast: document.body.innerText.includes('pkexec unavailable') || document.body.innerText.includes('无法开启高级执行权限'),
    checked: document.querySelector('[role="switch"]')?.getAttribute('aria-checked'),
    advancedPermissionVisible: document.body.innerText.includes('高级执行权限'),
    dependencyCheckVisible: document.body.innerText.includes('依赖体检'),
  }));
  const permissionPass = permToast.showSuperPermissionSettings
    ? permToast.setCall && permToast.toast && permToast.checked === 'false'
    : !permToast.advancedPermissionVisible && permToast.dependencyCheckVisible && !permToast.setCall;
  rec('⑭ 权限与环境按平台展示并保持失败回滚', permissionPass, JSON.stringify(permToast));

  await clickSettingsSection(page, '用户交流');
  await sleep(250);
  await page.evaluate(() => {
    window.__COMMUNITY_CLIPBOARD_TEXT__ = null;
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: {
        writeText: async (value) => { window.__COMMUNITY_CLIPBOARD_TEXT__ = value; },
      },
    });
  });
  await page.click('[data-testid="community-copy-group"]');
  await sleep(50);
  const communityPanel = await page.evaluate(() => {
    const panel = document.querySelector('[data-community-panel="true"]');
    const navIds = [...document.querySelectorAll('[data-testid="settings-nav"] [data-testid^="settings-section-"]')]
      .map(node => node.getAttribute('data-testid'));
    const rect = panel && panel.getBoundingClientRect();
    const qr = panel && panel.querySelector('[data-testid="community-qr-image"]');
    const groupName = panel && panel.querySelector('[data-testid="community-group-name"]');
    const groupNumber = panel && panel.querySelector('[data-testid="community-group-number"]');
    const copyButton = panel && panel.querySelector('[data-testid="community-copy-group"]');
    const text = panel ? panel.innerText : '';
    return {
      exists: !!panel,
      separateTab: !!document.querySelector('[data-testid="settings-section-community"]')
        && (document.querySelector('[data-testid="settings-content"] h1')?.textContent || '').trim() === '用户交流',
      immediatelyAfterHelp: navIds.indexOf('settings-section-community') === navIds.indexOf('settings-section-help') + 1,
      directTopLevel: !document.querySelector('[data-community-dialog="true"]') && !document.querySelector('[data-testid="community-entry-open"]'),
      width: rect ? Math.round(rect.width) : 0,
      insideViewport: !!rect && rect.left >= -1 && rect.right <= window.innerWidth + 1 && rect.top >= -1 && rect.bottom <= window.innerHeight + 1,
      hasQrImage: !!qr && qr.complete && qr.naturalWidth === 824 && qr.naturalHeight === 824 && qr.getBoundingClientRect().width >= 220,
      correctGroupName: (groupName?.textContent || '').trim() === 'pinvou-agent官方交流群',
      correctGroupNumber: (groupNumber?.textContent || '').trim() === '1108909346',
      copyEnabled: !!copyButton && !copyButton.disabled,
      copyWorks: window.__COMMUNITY_CLIPBOARD_TEXT__ === '1108909346' && text.includes('群号已复制'),
      hasDiscussionsFallback: text.includes('GitHub Discussions'),
      hasSupportBoundary: text.includes('不等同于官方支持或工单') && text.includes('安全漏洞'),
      noHorizontalOverflow: document.documentElement.scrollWidth <= window.innerWidth + 1 && (!panel || panel.scrollWidth <= panel.clientWidth + 1),
    };
  });
  rec('⑮ Community follows Help & Feedback and directly exposes the official group', communityPanel.exists && communityPanel.separateTab && communityPanel.immediatelyAfterHelp && communityPanel.directTopLevel && communityPanel.width >= 500 && communityPanel.width <= 700 && communityPanel.insideViewport && communityPanel.hasQrImage && communityPanel.correctGroupName && communityPanel.correctGroupNumber && communityPanel.copyEnabled && communityPanel.copyWorks && communityPanel.hasDiscussionsFallback && communityPanel.hasSupportBoundary && communityPanel.noHorizontalOverflow, JSON.stringify(communityPanel));
  await clickSettingsSection(page, '帮助反馈');
  await clickExact(page, '提交反馈');
  await sleep(250);
  const feedbackModal = await page.evaluate(() => {
    const title = [...document.querySelectorAll('h2')].find(node => (node.textContent || '').trim() === '我要反馈');
    const modal = title && title.closest('[data-feedback-dialog="true"]');
    const rect = modal && modal.getBoundingClientRect();
    const buttons = [...document.querySelectorAll('button')].map(button => (button.textContent || '').trim());
    const textarea = document.querySelector('textarea[placeholder*="请描述"]');
    const titleInput = document.querySelector('input[placeholder*="一句话概括"]');
    const groupCount = modal ? modal.querySelectorAll('.rounded-\\[16px\\]').length : 0;
    const nativeAlertCalls = window.__SETTINGS_TEST__.calls.filter(call => call.cmd === 'window_alert').length;
    const scrollWidth = modal ? modal.scrollWidth : 0;
    const clientWidth = modal ? modal.clientWidth : 0;
    return {
      exists: !!modal,
      width: rect ? Math.round(rect.width) : 0,
      insideViewport: !!rect && rect.left >= -1 && rect.right <= window.innerWidth + 1 && rect.top >= -1 && rect.bottom <= window.innerHeight + 1,
      compactTitle: !!title && Number.parseFloat(window.getComputedStyle(title).fontSize) <= 20,
      hasBottomActions: buttons.includes('取消') && buttons.includes('提交反馈'),
      hasGroupedInputs: !!textarea && !!titleInput,
      singleLayerGroups: groupCount === 3,
      noNativeAlertBeforeSubmit: nativeAlertCalls === 0,
      noHorizontalOverflow: document.documentElement.scrollWidth <= window.innerWidth + 1,
      modalNoHorizontalOverflow: scrollWidth <= clientWidth + 1,
    };
  });
  rec('⑯ 反馈弹窗符合模型/搜索一致的 iOS 规格', feedbackModal.exists && feedbackModal.width >= 420 && feedbackModal.width <= 455 && feedbackModal.insideViewport && feedbackModal.compactTitle && feedbackModal.hasBottomActions && feedbackModal.hasGroupedInputs && feedbackModal.singleLayerGroups && feedbackModal.noNativeAlertBeforeSubmit && feedbackModal.noHorizontalOverflow && feedbackModal.modalNoHorizontalOverflow, JSON.stringify(feedbackModal));
  await page.evaluate(() => {
    const textarea = document.querySelector('[data-feedback-dialog="true"] textarea[placeholder*="请描述"]');
    if (!textarea) return;
    const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
    textarea.focus();
    setter.call(textarea, '反馈弹窗测试');
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
  });
  const feedbackTyped = await page.evaluate(() => document.querySelector('[data-feedback-dialog="true"] textarea[placeholder*="请描述"]')?.value || '');
  await page.evaluate(() => {
    const modal = document.querySelector('[data-feedback-dialog="true"]');
    const button = modal && [...modal.querySelectorAll('button')].find(node => (node.textContent || '').trim() === '提交反馈');
    if (button) button.click();
  });
  await sleep(400);
  const feedbackSubmit = await page.evaluate(() => ({
    nativeAlertCalls: window.__SETTINGS_TEST__.calls.filter(call => call.cmd === 'window_alert').length,
    submitCalls: window.__SETTINGS_TEST__.calls.filter(call => call.cmd === 'submit_feedback').length,
    failureBanner: document.body.innerText.includes('反馈提交失败，请稍后重试。'),
    dialogStillOpen: !!document.querySelector('[data-feedback-dialog="true"]'),
  }));
  rec('⑰ failed feedback submit shows the in-app red banner instead of a system alert (community edition behavior)', feedbackTyped === '反馈弹窗测试' && feedbackSubmit.nativeAlertCalls === 0 && feedbackSubmit.submitCalls === 1 && feedbackSubmit.failureBanner && feedbackSubmit.dialogStillOpen, JSON.stringify({ feedbackTyped, ...feedbackSubmit }));
  await sleep(200);
  // The failure path must not auto-close the dialog: first close the
  // draft-carrying dialog through the in-app confirm layer (without
  // submitting), then reopen it in ⑰.5.
  await page.evaluate(() => {
    const modal = document.querySelector('[data-feedback-dialog="true"]');
    const button = modal && [...modal.querySelectorAll('button')].find(node => (node.textContent || '').trim() === '取消');
    if (button) button.click();
  });
  await page.waitForFunction(() => !!document.querySelector('[data-testid="feedback-close-confirm"]'));
  await page.click('[data-testid="feedback-close-confirm-ok"]');
  await page.waitForFunction(() => !document.querySelector('[data-testid="feedback-close-confirm"]') && !document.querySelector('[data-feedback-dialog="true"]'));

  // ⑰.5 dirty-draft close: goes through the in-app confirm layer (no native
  // confirm in Tauri WebView2); cancel keeps the draft and the panel, confirm
  // truly closes, and nothing is submitted along the way.
  await clickExact(page, '提交反馈');
  await sleep(250);
  await page.evaluate(() => {
    const textarea = document.querySelector('[data-feedback-dialog="true"] textarea[placeholder*="请描述"]');
    if (!textarea) return;
    const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
    textarea.focus();
    setter.call(textarea, '关闭确认测试');
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await page.evaluate(() => {
    const modal = document.querySelector('[data-feedback-dialog="true"]');
    const button = modal && [...modal.querySelectorAll('button')].find(node => (node.textContent || '').trim() === '取消');
    if (button) button.click();
  });
  await page.waitForFunction(() => !!document.querySelector('[data-testid="feedback-close-confirm"]'));
  await page.evaluate(() => {
    const layer = document.querySelector('[data-testid="feedback-close-confirm"]');
    const button = layer && [...layer.querySelectorAll('button')].find(node => (node.textContent || '').trim() === '取消');
    if (button) button.click();
  });
  await page.waitForFunction(() => !document.querySelector('[data-testid="feedback-close-confirm"]'));
  const feedbackCloseGuard = await page.evaluate(() => ({
    panelStillOpen: !!document.querySelector('[data-feedback-dialog="true"]'),
    draftKept: (document.querySelector('[data-feedback-dialog="true"] textarea[placeholder*="请描述"]')?.value || '') === '关闭确认测试',
  }));
  await page.evaluate(() => {
    const modal = document.querySelector('[data-feedback-dialog="true"]');
    const button = modal && [...modal.querySelectorAll('button')].find(node => (node.textContent || '').trim() === '取消');
    if (button) button.click();
  });
  await page.waitForFunction(() => !!document.querySelector('[data-testid="feedback-close-confirm"]'));
  await page.click('[data-testid="feedback-close-confirm-ok"]');
  await page.waitForFunction(() => !document.querySelector('[data-testid="feedback-close-confirm"]') && !document.querySelector('[data-feedback-dialog="true"]'));
  const feedbackCloseSubmitCalls = await page.evaluate(() =>
    window.__SETTINGS_TEST__.calls.filter(call => call.cmd === 'submit_feedback').length);
  rec('⑰.5 dirty-draft close uses the in-app confirm layer: cancel keeps the draft, confirm truly closes without submitting', feedbackCloseGuard.panelStillOpen && feedbackCloseGuard.draftKept && feedbackCloseSubmitCalls === 1, JSON.stringify({ ...feedbackCloseGuard, submitCalls: feedbackCloseSubmitCalls }));
  await sleep(200);

  await page.setViewport({ width: 760, height: 620 });
  await sleep(350);
  await clickSettingsSection(page, '通用');
  const responsiveSettings = await page.evaluate(() => {
    const aside = document.querySelector('aside');
    const panel = aside && aside.parentElement;
    const label = [...document.querySelectorAll('div')].find(node => (node.textContent || '').trim() === '界面语言');
    const panelRect = panel && panel.getBoundingClientRect();
    const labelRect = label && label.getBoundingClientRect();
    return {
      panelInsideViewport: !!panelRect && panelRect.left >= -1 && panelRect.top >= -1 && panelRect.right <= window.innerWidth + 1 && panelRect.bottom <= window.innerHeight + 1,
      labelNotVertical: !!labelRect && labelRect.width >= 54 && labelRect.height <= 28,
      noHorizontalOverflow: document.documentElement.scrollWidth <= window.innerWidth + 1,
    };
  });
  rec('⑱ 小窗口设置弹窗随窗口收缩且标签不竖排', Object.values(responsiveSettings).every(Boolean), JSON.stringify(responsiveSettings));

  // R16: the computer-use settings section finally EXECUTES (the row
  // previously had only source-regex pins). The capability mock carries no
  // `virtualDisplayNote`, so the unknown-platform honesty note must render
  // next to the absolute isolation hint (the M9 shape: the hint never
  // stands on the page unqualified), and the switch must bind the
  // persisted preference from the status payload (ON and clickable — the
  // fail-neutral "unknown" default would render it disabled).
  // The computer-use block renders inside the permissions/environment
  // section (it is not a sidebar section of its own; renderPermissions
  // hosts it next to the dependency-installer block).
  await clickSettingsSection(page, '权限与环境');
  await sleep(250);
  const computerUseSection = await page.evaluate(() => {
    const text = document.body.textContent || '';
    // The virtual-display toggle row: the smallest div that mentions the
    // row label AND contains a switch. Its aria-checked state must mirror
    // the status payload's persisted preference (prefer_virtual_display:
    // true in the mock), not the fail-neutral "unknown" default (which
    // would render it off and disabled).
    const rows = [...document.querySelectorAll('div')]
      .filter(node => (node.textContent || '').includes('优先使用隔离的虚拟显示器'))
      .filter(node => node.querySelector('[role="switch"]'))
      .sort((a, b) => a.textContent.length - b.textContent.length);
    const row = rows[0];
    const sw = row && row.querySelector('[role="switch"]');
    return {
      toggleRow: !!row,
      switchBoundOn: sw ? sw.getAttribute('aria-checked') === 'true' : false,
      switchEnabled: sw ? sw.getAttribute('disabled') === null : false,
      absoluteHint: text.includes('画面不会出现在你的屏幕上'),
      readyState: text.includes('隔离显示器可用'),
      unknownNote: text.includes('无法验证此平台的隔离限制'),
      statusCalled: (window.__SETTINGS_TEST__.calls || []).some(c => c.cmd === 'computer_use_get_status'),
    };
  });
  rec('⑱.5 电脑使用设置区真实渲染且无能力时显示诚实说明', Object.values(computerUseSection).every(Boolean), JSON.stringify(computerUseSection));

  await page.mouse.click(8, 8);
  await page.waitForFunction(() => document.querySelector('[data-testid="app-root"]')?.getAttribute('data-current-view') === 'chat', { timeout: 8000 });
  await page.evaluate(() => window.TauriBridge && window.TauriBridge.sessions.createNewSession && window.TauriBridge.sessions.createNewSession());
  await sleep(600);
  const responsiveGreeting = await page.evaluate(() => {
    const greeting = [...document.querySelectorAll('h1')].find(node => {
      const text = node.textContent || '';
      const fontSize = Number.parseFloat(window.getComputedStyle(node).fontSize);
      return text.includes('今天想聊点什么') || text.includes("what's good") || text.includes('今日は') || fontSize >= 30;
    });
    const rect = greeting && greeting.getBoundingClientRect();
    const fontSize = greeting ? Number.parseFloat(window.getComputedStyle(greeting).fontSize) : 0;
    return {
      exists: !!greeting,
      insideViewport: !!rect && rect.left >= -1 && rect.right <= window.innerWidth + 1,
      smallWindowFont: fontSize > 0 && fontSize <= 34,
      noHorizontalOverflow: document.documentElement.scrollWidth <= window.innerWidth + 1,
    };
  });
  rec('⑲ 小窗口欢迎文案不溢出', Object.values(responsiveGreeting).every(Boolean), JSON.stringify(responsiveGreeting));

  rec('⑳ 全程无运行时报错', errors.length === 0, errors.slice(0, 3).join(' | '));
  await browser.close();

  const failed = results.filter(result => !result.pass).length;
  console.log(failed ? `\n❌ ${failed}/${results.length} FAILED` : `\n✅ ALL ${results.length} PASS`);
  process.exit(failed ? 1 : 0);
// eslint-disable-next-line unicorn/prefer-top-level-await -- smoke script keeps its existing async main() structure
})().catch(error => {
  console.error('FATAL', error.stack || error.message);
  process.exit(1);
});
