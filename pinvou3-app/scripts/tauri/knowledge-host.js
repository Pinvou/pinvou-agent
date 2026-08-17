const {
  chmodSync,
  copyFileSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
} = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { APP_ROOT } = require('./platform-config.js');

const DEVELOPMENT_MARKER = 'DEVELOPMENT_RESOURCE=0';
const DEVELOPMENT_RESOURCE_SOURCE = 'target/knowledge-host-dev/';
const DEVELOPMENT_RESOURCE_ROOT = path.join(
  APP_ROOT,
  'src-tauri', 'target', 'knowledge-host-dev',
);
const DEVELOPMENT_RUNTIME_ROOT = path.join(
  APP_ROOT,
  'src-tauri', 'target', 'debug', 'runtime', 'knowledge-host',
);

function hardenDevelopmentPathChain(
  directory,
  {
    currentUid = process.getuid?.(),
    inspect = lstatSync,
    chmod = chmodSync,
    pathApi = path,
  } = {},
) {
  if (!Number.isInteger(currentUid)) {
    throw new Error('无法识别 Linux 开发用户，不能安全准备共享知识库资源');
  }
  let current = pathApi.resolve(directory);
  while (true) {
    const info = inspect(current);
    if (!info.isDirectory() || info.isSymbolicLink()) {
      throw new Error(`共享知识库开发资源目录链包含非目录或符号链接: ${current}`);
    }
    if (info.uid !== 0 && info.uid !== currentUid) {
      throw new Error(`共享知识库开发资源目录链不属于当前用户: ${current}`);
    }
    if ((info.mode & 0o022) !== 0) {
      if (info.uid !== currentUid) {
        throw new Error(`共享知识库开发资源目录链权限不安全: ${current}`);
      }
      chmod(current, (info.mode & 0o7777) & ~0o022);
    }
    const parent = pathApi.dirname(current);
    if (parent === current) break;
    current = parent;
  }
}

function prepareDevelopmentResourceDirectories() {
  // helper 将以 root 身份复制这里的服务程序。开发机常见 umask 0002 会
  // 产生 775 目录，因此在构建前收紧已有路径，并让新建的 Tauri 目录
  // 默认不可被同组或其他用户篡改；正式安装包不走这条开发路径。
  process.umask(0o022);
  for (const directory of [DEVELOPMENT_RESOURCE_ROOT, DEVELOPMENT_RUNTIME_ROOT]) {
    mkdirSync(directory, { recursive: true });
    hardenDevelopmentPathChain(directory);
  }
}

function developmentHelperSource(source) {
  const occurrences = source.split(DEVELOPMENT_MARKER).length - 1;
  if (occurrences !== 1) {
    throw new Error('共享知识库 helper 缺少唯一的开发资源标记');
  }
  return source.replace(DEVELOPMENT_MARKER, 'DEVELOPMENT_RESOURCE=1');
}

function knowledgeHostDevelopmentConfigSpec() {
  return JSON.stringify({
    bundle: {
      resources: {
        [DEVELOPMENT_RESOURCE_SOURCE]: 'runtime/knowledge-host',
      },
    },
  });
}

function prepareKnowledgeHost({
  platform = process.platform,
  development = false,
  spawn = spawnSync,
} = {}) {
  if (platform !== 'linux') return null;
  const repositoryRoot = path.resolve(APP_ROOT, '..');
  const packagedResourceRoot = path.join(
    APP_ROOT,
    'src-tauri', 'resources', 'platforms', 'linux', 'knowledge-host',
  );
  const resourceRoot = development ? DEVELOPMENT_RESOURCE_ROOT : packagedResourceRoot;
  const helperSource = path.join(packagedResourceRoot, 'pinvou-knowledge-host-helper');
  const helper = path.join(resourceRoot, 'pinvou-knowledge-host-helper');
  if (development) {
    prepareDevelopmentResourceDirectories();
    writeFileSync(
      helper,
      developmentHelperSource(readFileSync(helperSource, 'utf8')),
      'utf8',
    );
  }
  // Git preserves this bit on Linux, but enforce it again while preparing the
  // package so a checkout or archive that flattened modes cannot ship a
  // pkexec target that the operating system refuses to execute.
  chmodSync(helper, 0o755);
  const manifest = path.join(repositoryRoot, 'pinvou-knowledge', 'Cargo.toml');
  const cargoArgs = ['build', '--locked'];
  if (!development) cargoArgs.push('--release');
  cargoArgs.push(
    '--manifest-path', manifest, '--bin', 'pinvou-knowledge-server',
    '-j', process.env.PINVOU_KNOWLEDGE_BUILD_JOBS || '2',
  );
  const result = spawn(
    process.env.CARGO || 'cargo',
    cargoArgs,
    { cwd: repositoryRoot, stdio: 'inherit', env: process.env },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error('共享知识库服务构建失败');

  const profile = development ? 'debug' : 'release';
  const source = path.join(
    repositoryRoot,
    'pinvou-knowledge', 'target', profile, 'pinvou-knowledge-server',
  );
  const destination = path.join(resourceRoot, 'pinvou-knowledge-server');
  mkdirSync(path.dirname(destination), { recursive: true });
  copyFileSync(source, destination);
  chmodSync(destination, 0o755);
  return {
    configSpec: development ? knowledgeHostDevelopmentConfigSpec() : null,
    serverPath: destination,
  };
}

module.exports = {
  DEVELOPMENT_RESOURCE_SOURCE,
  developmentHelperSource,
  hardenDevelopmentPathChain,
  knowledgeHostDevelopmentConfigSpec,
  prepareKnowledgeHost,
};
