const { copyFileSync, mkdirSync, chmodSync } = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { APP_ROOT } = require('./platform-config.js');

function prepareKnowledgeHost({ platform = process.platform, spawn = spawnSync } = {}) {
  if (platform !== 'linux') return null;
  const repositoryRoot = path.resolve(APP_ROOT, '..');
  const resourceRoot = path.join(
    APP_ROOT,
    'src-tauri', 'resources', 'platforms', 'linux', 'knowledge-host',
  );
  const helper = path.join(resourceRoot, 'pinvou-knowledge-host-helper');
  // Git preserves this bit on Linux, but enforce it again while preparing the
  // package so a checkout or archive that flattened modes cannot ship a
  // pkexec target that the operating system refuses to execute.
  chmodSync(helper, 0o755);
  const manifest = path.join(repositoryRoot, 'pinvou-knowledge', 'Cargo.toml');
  const result = spawn(process.env.CARGO || 'cargo', [
    'build', '--locked', '--release', '--manifest-path', manifest, '--bin',
    'pinvou-knowledge-server', '-j', process.env.PINVOU_KNOWLEDGE_BUILD_JOBS || '2',
  ], { cwd: repositoryRoot, stdio: 'inherit', env: process.env });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error('共享知识库服务构建失败');

  const source = path.join(repositoryRoot, 'pinvou-knowledge', 'target', 'release', 'pinvou-knowledge-server');
  const destination = path.join(resourceRoot, 'pinvou-knowledge-server');
  mkdirSync(path.dirname(destination), { recursive: true });
  copyFileSync(source, destination);
  chmodSync(destination, 0o755);
  return destination;
}

module.exports = { prepareKnowledgeHost };
