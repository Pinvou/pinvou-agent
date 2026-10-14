import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const knowledgeSource = await readFile(new URL('../src/features/knowledge/KnowledgeView.jsx', import.meta.url), 'utf8');
const remoteKnowledgeSource = await readFile(new URL('../src/features/remote-knowledge/RemoteKnowledgeView.jsx', import.meta.url), 'utf8');
const composerControlsSource = await readFile(new URL('../src/features/chat/composer-controls.jsx', import.meta.url), 'utf8');

test('shared knowledge dialogs portal their full-viewport overlay to document.body', () => {
  assert.match(remoteKnowledgeSource, /import \{ createPortal \} from 'react-dom';/);
  assert.match(
    remoteKnowledgeSource,
    /return typeof document === 'undefined' \? dialog : createPortal\(dialog, document\.body\);/,
  );
  assert.match(remoteKnowledgeSource, /className="fixed inset-0 z-80/);
  assert.match(remoteKnowledgeSource, /role="dialog"[\s\S]*?aria-modal="true"/);
  assert.match(remoteKnowledgeSource, /onMouseDown=\{event => event\.stopPropagation\(\)\}/);
});

test('BGE download uses an accessible numeric circular progress indicator', () => {
  const indicator = knowledgeSource.slice(
    knowledgeSource.indexOf('function ModelProgressIndicator'),
    knowledgeSource.indexOf('const KnowledgeView'),
  );
  assert.match(indicator, /role="progressbar"/);
  assert.match(indicator, /aria-valuenow=\{percent\}/);
  assert.match(indicator, /strokeDasharray=\{MODEL_PROGRESS_CIRCUMFERENCE\}/);
  assert.match(indicator, /strokeDashoffset=\{progressOffset\}/);
  assert.match(indicator, /\{percent\}<span[\s\S]*?>%<\/span>/);
  assert.doesNotMatch(indicator, /style=\{\{ width:/);
});

test('BGE model loading keeps a distinct indeterminate status', () => {
  const indicator = knowledgeSource.slice(
    knowledgeSource.indexOf('function ModelProgressIndicator'),
    knowledgeSource.indexOf('const KnowledgeView'),
  );
  assert.match(indicator, /if \(!downloading\)/);
  assert.match(indicator, /role="status" aria-live="polite"/);
  assert.match(indicator, /animate-spin motion-reduce:animate-none/);
  assert.match(knowledgeSource, /<ModelProgressIndicator[\s\S]*?downloading=\{downloading\}[\s\S]*?percent=\{dlPct\}/);
});

test('the remote owner-model download entry is capability-gated, fail closed', () => {
  // loongarch64 的服务端下载漏斗（begin_model_download）必然拒绝；共享知识库宿主面板的
  // 下载按钮必须与 KnowledgeView 的模型卡同向门控：prop 为 true 才渲染，快照未到达或
  // 渲染点漏传时隐藏，而不是渲染一个必然失败的按钮。
  assert.match(remoteKnowledgeSource, /function RemoteKnowledgeView\(\{ t, localEmbedSupported \}\)/);
  assert.match(remoteKnowledgeSource, /const localEmbedReady = localEmbedSupported === true;/);
  assert.match(
    remoteKnowledgeSource,
    /\{isLocalHostOwner && localEmbedReady && !ownerModelStatus\?\.ready && <button/,
  );
  assert.match(
    knowledgeSource,
    /<RemoteKnowledgeView t=\{t\} localEmbedSupported=\{localEmbedSupported\} \/>/,
  );
  // 按钮被能力门隐藏时，宿主（本机）的状态行必须给出平台解释，而不是让
  // 「尚未下载」悬空；非宿主观看者看到的仍是宿主的真实状态（remoteKbModelMissing）。
  // 平台证明不支持时本地化能力说明优先于后端原始诊断（先能力位、后 error），
  // 顺序反过来就会让诊断文本顶掉用户语言里的解释。
  assert.match(
    remoteKnowledgeSource,
    /\{isLocalHostOwner && !localEmbedReady \? t\.kbEmbedUnsupported : \(ownerModelStatus\?\.error \|\|/,
  );
});

test('the web kb gate attributes the blocker to the desktop app, not the platform', () => {
  // web 客户端无法安装本地模型，能力快照永远缺失；但阻塞通常只是「宿主
  // 桌面端尚未安装模型」——宿主平台本身可能是支持的。知识库门与撰写器
  // 挂载引导必须走 isWeb 分支归因到桌面端（kbEmbedWebTitle/Desc），桌面
  // fail-closed 窗口与 loongarch64 平台保持平台文案（kbEmbedUnsupported）。
  for (const [name, source] of [['KnowledgeView', knowledgeSource], ['composer', composerControlsSource]]) {
    assert.match(
      source,
      /isWeb \? t\.kbEmbedWebTitle : t\.kbEmbedUnsupported/u,
      `${name} must route the no-capability copy through the web attribution branch`,
    );
  }
  assert.match(
    knowledgeSource,
    /\(localEmbedSupported \|\| isWeb\)[\s\S]*?t\.kbEmbedWebDesc/u,
    'the web gate must render the desktop-setup explanation where the desktop gate renders its own',
  );
});

test('a proven-unsupported platform opens the full-text knowledge posture', () => {
  // 后端状态携带权威能力位（localEmbedSupported，与命令层下载/装载闸同一来
  // 源）。false = 本设备编译期即无本地嵌入，模型永远无法就绪：知识库必须以
  // 既有全文降级姿态直接可用（可建库/导入/挂载，kb_search 走 FTS），而不是
  // 被永久锁死在下载门后。字段缺失（旧快照/mock/能力未知）按未知 → 维持既
  // 有模型门（fail closed 只对"放行"方向反转：这里要求严格 === false）。
  assert.match(
    knowledgeSource,
    /const platformEmbedUnsupported = !!\(effectiveKbModel && effectiveKbModel\.localEmbedSupported === false\);/,
    'KnowledgeView must derive the posture from the backend status capability bit',
  );
  assert.match(
    knowledgeSource,
    /const modelUsable = \(modelInstalled && \(modelReady \|\| modelDeferred\)\) \|\| platformEmbedUnsupported;/,
    'the proven-unsupported platform must open the library (full-text posture) instead of the download gate',
  );
  assert.match(
    composerControlsSource,
    /const platformEmbedUnsupported = !!\(modelStatus && modelStatus\.localEmbedSupported === false\);/,
    'the composer must read the capability bit from the refreshed model status',
  );
  assert.match(
    composerControlsSource,
    /const modelBlocked = !platformEmbedUnsupported && \(modelMissing \|\| modelNotReady\);/,
    'local collection mounting must stay available on a proven-unsupported platform',
  );
});
