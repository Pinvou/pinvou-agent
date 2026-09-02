import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const require = createRequire(import.meta.url);
const {
  DEVELOPMENT_RESOURCE_SOURCE,
  developmentHelperSource,
  hardenDevelopmentPathChain,
  knowledgeHostDevelopmentConfigSpec,
} = require('../scripts/tauri/knowledge-host.js');
const helperPath = path.join(
  appRoot,
  'src-tauri/resources/platforms/linux/knowledge-host/pinvou-knowledge-host-helper',
);
const helper = readFileSync(helperPath, 'utf8');
const knowledgeHostBuildScript = readFileSync(
  path.join(appRoot, 'scripts/tauri/knowledge-host.js'),
  'utf8',
);
const buildScript = readFileSync(path.join(appRoot, 'scripts/tauri/build.js'), 'utf8');
const platformConfig = readFileSync(
  path.join(appRoot, 'src-tauri/config/platforms/linux/tauri.conf.json'),
  'utf8',
);
const hostCommands = readFileSync(
  path.join(appRoot, 'src-tauri/src/app/commands/shared_knowledge_host.rs'),
  'utf8',
);
const hostPlatformLinux = readFileSync(
  path.join(appRoot, 'src-tauri/src/features/shared_knowledge_host/platform/linux.rs'),
  'utf8',
);
const remoteKnowledgeView = readFileSync(
  path.join(appRoot, 'src/features/remote-knowledge/RemoteKnowledgeView.jsx'),
  'utf8',
);
const runDev = readFileSync(path.join(appRoot, 'run-dev.sh'), 'utf8');

test('Linux package embeds the host helper and standalone server build', () => {
  assert.match(platformConfig, /resources\/platforms\/linux\/knowledge-host\//u);
  assert.match(buildScript, /prepareKnowledgeHost\(\)/u);
  assert.match(buildScript, /hasTauriBuildCommand/u);
  assert.match(knowledgeHostBuildScript, /chmodSync\(helper, 0o755\)/u);
  const repositoryRoot = path.resolve(appRoot, '..');
  const helperRelativePath = path.relative(repositoryRoot, helperPath).replaceAll(path.sep, '/');
  const indexEntry = execFileSync(
    'git',
    ['ls-files', '-s', '--', helperRelativePath],
    { cwd: repositoryRoot, encoding: 'utf8' },
  ).trim();
  assert.match(indexEntry, /^100755\s/u);
  const eolAttribute = execFileSync(
    'git',
    ['check-attr', 'eol', '--', helperRelativePath],
    { cwd: repositoryRoot, encoding: 'utf8' },
  ).trim();
  assert.match(eolAttribute, /: eol: lf$/u);
});

test('Linux dev stages an explicit user-owned host resource without weakening packages', () => {
  assert.match(helper, /^DEVELOPMENT_RESOURCE=0$/mu);
  const developmentHelper = developmentHelperSource(helper);
  assert.match(developmentHelper, /^DEVELOPMENT_RESOURCE=1$/mu);
  assert.doesNotMatch(developmentHelper, /^DEVELOPMENT_RESOURCE=0$/mu);
  assert.deepEqual(JSON.parse(knowledgeHostDevelopmentConfigSpec()), {
    bundle: {
      resources: {
        [DEVELOPMENT_RESOURCE_SOURCE]: 'runtime/knowledge-host',
      },
    },
  });
  assert.match(buildScript, /prepareKnowledgeHost\(\{ development: true \}\)/u);
  assert.match(buildScript, /additionalConfigs\.push\(developmentHost\.configSpec\)/u);
  assert.match(helper, /\[ "\$DEVELOPMENT_RESOURCE" -eq 1 \] \|\| fail "请使用正式安装的 PINVOU/u);
  assert.match(helper, /\[ "\$owner" -eq "\$service_uid" \]/u);
  assert.match(helper, /\[ "\$helper_owner" -eq "\$service_uid" \]/u);
  assert.match(helper, /开发服务资源必须与管理组件同目录/u);
  assert.match(helper, /validate_development_path_chain "\$resource_dir" "\$service_uid"/u);
  assert.match(helper, /\[ "\$directory_owner" -eq 0 \] \|\| \[ "\$directory_owner" -eq "\$service_uid" \]/u);
  assert.match(helper, /开发资源目录链权限不安全/u);
  assert.match(helper, /开发资源目录权限不安全/u);
});

test('Linux dev hardens existing resource ancestors created by a cooperative umask', () => {
  const entries = new Map([
    ['/', { uid: 0, mode: 0o40755 }],
    ['/home', { uid: 0, mode: 0o40755 }],
    ['/home/test', { uid: 1000, mode: 0o40750 }],
    ['/home/test/repo', { uid: 1000, mode: 0o40775 }],
    ['/home/test/repo/target', { uid: 1000, mode: 0o40775 }],
  ]);
  const changed = [];
  hardenDevelopmentPathChain('/home/test/repo/target', {
    currentUid: 1000,
    pathApi: path.posix,
    inspect(directory) {
      const entry = entries.get(directory);
      assert.ok(entry, directory);
      return {
        ...entry,
        isDirectory: () => true,
        isSymbolicLink: () => false,
      };
    },
    chmod: (directory, mode) => changed.push([directory, mode]),
  });
  assert.deepEqual(changed, [
    ['/home/test/repo/target', 0o755],
    ['/home/test/repo', 0o755],
  ]);
  assert.match(knowledgeHostBuildScript, /process\.umask\(0o022\)/u);
  assert.match(knowledgeHostBuildScript, /DEVELOPMENT_RUNTIME_ROOT/u);
});

test('Linux dev rejects an unsafe root-owned ancestor instead of weakening it', () => {
  assert.throws(() => hardenDevelopmentPathChain('/tmp/pinvou', {
    currentUid: 1000,
    pathApi: path.posix,
    inspect(directory) {
      const entry = directory === '/tmp'
        ? { uid: 0, mode: 0o41777 }
        : directory === '/'
          ? { uid: 0, mode: 0o40755 }
          : { uid: 1000, mode: 0o40755 };
      return {
        ...entry,
        isDirectory: () => true,
        isSymbolicLink: () => false,
      };
    },
    chmod: () => assert.fail('must not chmod a root-owned unsafe ancestor'),
  }), /目录链权限不安全/u);
});

test('dev uses the managed knowledge model unless an external directory is explicit', () => {
  assert.match(runDev, /~\/.pinvou3\/knowledge\/models\/bge-m3/u);
  assert.doesNotMatch(runDev, /export PINVOU3_KB_EMBED_MODEL_DIR=/u);
  assert.doesNotMatch(runDev, /\$HOME\/models\/bge-m3/u);
});

test('host helper keeps lifecycle operations explicit and persistent', () => {
  assert.match(hostCommands, /static HOST_LIFECYCLE_LOCK: tokio::sync::Mutex<\(\)>/u);
  assert.match(hostCommands, /HOST_LIFECYCLE_LOCK\.lock\(\)\.await/u);
  assert.match(helper, /^LOCK_DIR=\/run\/pinvou$/mu);
  assert.match(helper, /install -d -m 0700 -o root -g root "\$LOCK_DIR"/u);
  assert.match(helper, /\[ "\$lock_dir_owner" -eq 0 \] && \[ "\$lock_dir_mode" = 700 \]/u);
  assert.match(helper, /\[ ! -L "\$LOCK_FILE" \]/u);
  assert.match(helper, /exec 9>"\$LOCK_FILE"/u);
  assert.match(helper, /chmod 0600 "\$LOCK_FILE"/u);
  assert.doesNotMatch(helper, /\/run\/lock\/pinvou-knowledge-host\.lock/u);
  assert.match(helper, /flock -n 9 \|\| fail/u);
  assert.match(helper, /^DATA_LOCK=\/var\/lib\/\.pinvou-knowledge\.data\.lock$/mu);
  assert.match(helper, /prepare_data_lock "\$service_uid" "\$service_gid"/u);
  assert.match(helper, /ReadWritePaths=\/var\/lib\/pinvou-knowledge \$DATA_LOCK \$MODEL_MOUNT/u);
  assert.match(helper, /install\|upgrade/u);
  assert.match(helper, /set-owner/u);
  assert.match(helper, /recover-owner/u);
  assert.match(helper, /remove\)/u);
  assert.match(helper, /keep-data/u);
  assert.match(helper, /delete-data/u);
  assert.match(helper, /backup\)/u);
  assert.match(helper, /restore\)/u);
  assert.match(helper, /--backup-recipient/u);
  assert.match(helper, /--restore-mode/u);
  assert.match(helper, /same-host\|content-only/u);
  assert.match(helper, /systemctl enable/u);
  assert.match(helper, /WantedBy=multi-user\.target/u);
});

test('host helper compares semantic versions and refuses a service downgrade', () => {
  assert.match(helper, /command -v setpriv[^\n]+无法安全安装共享知识库/u);
  assert.match(helper, /version_output=\$\(setpriv --reuid "\$service_uid" --regid "\$service_gid" --clear-groups --/u);
  assert.doesNotMatch(helper, /version_output=\$\("\$binary" --version/u);
  assert.match(helper, /source_version=\$\(binary_release_version "\$source_bin" "\$service_uid" "\$service_gid"\)/u);
  assert.match(helper, /installed_version=\$\(binary_release_version "\$INSTALL_BIN" "\$service_uid" "\$service_gid"\)/u);
  assert.match(helper, /if \[ -f "\$INSTALL_BIN" \]; then\s+source_version=/u);
  assert.match(helper, /version_result=\$\(semver_compare "\$source_version" "\$installed_version"\)/u);
  assert.match(helper, /\[ "\$version_result" -lt 0 \]/u);
  assert.match(helper, /已拒绝降级/u);

  if (process.platform === 'win32') return;
  const functionsStart = helper.indexOf('validate_semver_identifiers() {');
  const functionsEnd = helper.indexOf('\nwrite_unit() {', functionsStart);
  assert.ok(functionsStart >= 0 && functionsEnd > functionsStart);
  const semanticVersionFunctions = helper.slice(functionsStart, functionsEnd);
  const script = `set -eu
fail() { printf '%s\\n' "$*" >&2; exit 1; }
${semanticVersionFunctions}
actual=$(semver_compare "$1" "$2")
[ "$actual" = "$3" ]`;
  for (const [left, right, expected] of [
    ['0.10.0', '0.9.99', '1'],
    ['0.8.1', '0.8.1', '0'],
    ['0.8.1-rc.2', '0.8.1', '-1'],
    ['0.8.1', '0.8.1-rc.2', '1'],
    ['1.0.0-alpha.1', '1.0.0-alpha.beta', '-1'],
    ['1.0.0-beta.2', '1.0.0-beta.11', '-1'],
    ['0.8.1+desktop', '0.8.1+service', '0'],
  ]) {
    execFileSync('/bin/sh', ['-c', script, 'semver-test', left, right, expected]);
  }
});

test('host helper binds every dropped privilege operation to the account primary group', () => {
  assert.match(helper, /PKEXEC_CALLER_UID=\$PKEXEC_UID/u);
  assert.match(helper, /\[ "\$candidate_uid" = "\$PKEXEC_CALLER_UID" \]/u);
  assert.match(helper, /flock -n 9/u);
  assert.match(helper, /validate_model_path_chain "\$model_dir" "\$user_home" "\$service_uid"/u);
  assert.match(helper, /已拒绝静默变更所有权/u);
  assert.match(helper, /validate_service_account\(\)[\s\S]*getent passwd "\$candidate_uid"[\s\S]*cut -d: -f4[\s\S]*\[ "\$service_primary_gid" = "\$candidate_gid" \]/u);
  for (const operation of ['install_or_upgrade', 'set_owner_device', 'recover_owner', 'validate_service_identity']) {
    const start = helper.indexOf(`${operation}() {`);
    const end = helper.indexOf('\n}', start);
    assert.ok(start >= 0 && end > start, operation);
    assert.match(helper.slice(start, end), /validate_service_account "\$service_uid" "\$service_gid"/u);
  }
  assert.match(helper, /setpriv --reuid "\$service_uid" --regid "\$service_gid" --clear-groups -- \\\s*\n\s*"\$INSTALL_BIN" --health-check/u);
  assert.doesNotMatch(helper, /if "\$INSTALL_BIN" --health-check/u);

  if (process.platform !== 'linux') return;
  const functionsStart = helper.indexOf('validate_semver_identifiers() {');
  const functionsEnd = helper.indexOf('\nwrite_unit() {', functionsStart);
  const accountScript = `set -eu
fail() { printf '%s\\n' "$*" >&2; exit 1; }
${helper.slice(functionsStart, functionsEnd)}
account=$(getent passwd nobody 2>/dev/null || getent passwd | awk -F: '$3 != 0 && $4 != 0 { print; exit }')
uid=$(printf '%s' "$account" | cut -d: -f3)
gid=$(printf '%s' "$account" | cut -d: -f4)
PKEXEC_CALLER_UID=$uid
validate_service_account "$uid" "$gid"
if (validate_service_account "$uid" "$((gid + 1))" >/dev/null 2>&1); then exit 1; fi`;
  execFileSync('/bin/sh', ['-c', accountScript]);
});

test('maintenance operations restart the service after failure, cancellation, or success', () => {
  assert.match(helper, /restart_service_after_maintenance\(\)[\s\S]*systemctl start "\$SERVICE"/u);
  assert.match(helper, /begin_service_maintenance\(\)[\s\S]*trap 'restart_service_after_maintenance' EXIT/u);
  assert.match(helper, /begin_service_maintenance\(\)[\s\S]*systemctl stop "\$SERVICE"/u);
  assert.match(helper, /set_owner_device\(\)[\s\S]*begin_service_maintenance[\s\S]*finish_service_maintenance/u);
  assert.match(helper, /recover_owner\(\)[\s\S]*begin_service_maintenance[\s\S]*finish_service_maintenance/u);
  assert.match(helper, /backup_host\(\)[\s\S]*begin_service_maintenance[\s\S]*finish_service_maintenance/u);
  assert.match(helper, /restore_host\(\)[\s\S]*begin_service_maintenance[\s\S]*finish_service_maintenance/u);
  assert.match(helper, /identity_owner/u);
  assert.match(helper, /identity_mode/u);
});

test('restore validates backup and identity absolute paths independently', () => {
  assert.doesNotMatch(helper, /case "\$input:\$identity_file"/u);
  assert.match(helper, /require_absolute_path "\$input" "备份文件路径无效"/u);
  assert.match(helper, /require_absolute_path "\$identity_file" "恢复密钥路径无效"/u);

  if (process.platform === 'win32') return;
  const functionStart = helper.indexOf('require_absolute_path() {');
  const functionEnd = helper.indexOf('\n\n[ "$(id -u)"', functionStart);
  assert.ok(functionStart >= 0 && functionEnd > functionStart);
  const script = `set -eu
fail() { printf '%s\\n' "$*" >&2; exit 1; }
${helper.slice(functionStart, functionEnd)}
require_absolute_path "/tmp/archive.pinbak" invalid
if (require_absolute_path "relative:/tmp/key" invalid >/dev/null 2>&1); then exit 1; fi
if (require_absolute_path "relative.pinbak" invalid >/dev/null 2>&1); then exit 1; fi`;
  execFileSync('/bin/sh', ['-c', script]);
});

test('native Linux host commands have bounded execution and explicit pkexec cancellation errors', () => {
  assert.doesNotMatch(hostPlatformLinux, /\.output\(\)/u);
  assert.doesNotMatch(hostPlatformLinux, /\.status\(\)/u);
  assert.match(hostPlatformLinux, /output_with_timeout_and_kill_tree\(command, timeout\)/u);
  assert.match(hostPlatformLinux, /BACKUP_HELPER_TIMEOUT: Duration = Duration::from_secs\(60 \* 60\)/u);
  assert.match(hostPlatformLinux, /Some\(126\) => format!\("\{operation\}已取消"\)/u);
  assert.match(hostPlatformLinux, /Some\(127\).*未获系统管理员授权/u);
  assert.match(hostPlatformLinux, /系统操作可能仍在收尾，请刷新状态后再重试/u);
});

test('owner claim survives until the native client persists it and health uses TLS', () => {
  assert.match(helper, /show_owner_claim\(\)/u);
  assert.match(helper, /install_or_upgrade[\s\S]*show_owner_claim/u);
  assert.match(helper, /restore_host[\s\S]*show_owner_claim/u);
  assert.match(helper, /claim-owner\)[\s\S]*claim_owner/u);
  assert.match(helper, /recover-owner\)[\s\S]*recover_owner/u);
  assert.match(helper, /--recover-host-owner-claim/u);
  assert.match(helper, /--health-check https:\/\/127\.0\.0\.1:3210/u);
  assert.match(helper, /read_owner_claim\(\)[\s\S]*\[ ! -L "\$claim" \][\s\S]*\[ "\$claim_owner_uid" -eq "\$service_uid" \]/u);
  assert.match(helper, /setpriv --reuid "\$service_uid" --regid "\$service_gid" --clear-groups -- \\\s*\n\s*cat -- "\$claim"/u);
  const readerStart = helper.indexOf('read_owner_claim() {');
  const readerEnd = helper.indexOf('\n}', readerStart);
  assert.ok(readerStart >= 0 && readerEnd > readerStart);
  assert.equal(helper.match(/cat -- "\$claim"/gu)?.length, 1);
});

test('host helper reuses the exact local model directory with systemd hardening', () => {
  assert.match(helper, /\.pinvou3\/knowledge\/models\/bge-m3/u);
  assert.match(helper, /model_parent=\$\(dirname "\$model_dir"\)/u);
  assert.match(helper, /model_name=\$\(basename "\$model_dir"\)/u);
  assert.match(helper, /BindPaths=\$model_parent:\$MODEL_MOUNT/u);
  assert.match(helper, /--model-dir \$MODEL_MOUNT\/\$model_name/u);
  assert.match(helper, /install -d -m 0700 -o "\$service_uid" -g "\$service_gid" "\$model_parent"/u);
  assert.match(helper, /ProtectSystem=strict/u);
  assert.match(helper, /ProtectHome=tmpfs/u);
  assert.match(helper, /NoNewPrivileges=true/u);
  assert.match(helper, /UMask=0077/u);
  assert.match(helper, /RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_NETLINK/u);
});

test('native host setup reports real milestones to a blocking progress dialog', () => {
  assert.match(hostCommands, /shared-knowledge-host-progress/u);
  for (const phase of ['prepare', 'install', 'connect', 'complete', 'failed']) {
    assert.match(hostCommands, new RegExp(`"${phase}"`, 'u'));
  }
  assert.match(remoteKnowledgeView, /listenTauri\('shared-knowledge-host-progress'/u);
  assert.match(remoteKnowledgeView, /testId="shared-kb-host-progress"/u);
  assert.match(remoteKnowledgeView, /role="progressbar"/u);
  assert.match(remoteKnowledgeView, /shared-kb-host-progress-error/u);
  assert.doesNotMatch(remoteKnowledgeView, /closeDisabled=\{!\['complete', 'failed'\]\.includes\(hostProgress\.phase\)\}/u);
  assert.match(remoteKnowledgeView, /current\?\.operation === 'install'/u);
});

test('existing standalone data and model are adopted with a complete rollback path', () => {
  assert.match(helper, /validate_data_dir/u);
  assert.match(helper, /chown -R "\$service_uid:\$service_gid" "\$DATA_DIR"/u);
  assert.match(helper, /legacy_model=\$DATA_DIR\/models\/bge-m3/u);
  assert.match(helper, /cp -a "\$legacy_model\/\." "\$model_dir\/"/u);
  assert.match(helper, /cp -p "\$UNIT_FILE" "\$UNIT_BACKUP"/u);
  assert.match(helper, /rollback_install\(\)/u);
  assert.match(helper, /mv -f "\$UNIT_BACKUP" "\$UNIT_FILE"/u);
  assert.match(helper, /chown -R "\$\{old_data_uid\}:\$\{old_data_gid\}" "\$DATA_DIR"/u);
  assert.match(helper, /trap 'rollback_install' EXIT\n/u);
  assert.match(helper, /trap 'rollback_install; exit 129' HUP/u);
  assert.match(helper, /trap 'rollback_install; exit 130' INT/u);
  assert.match(helper, /trap 'rollback_install; exit 143' TERM/u);
});

test('permanent removal validates the fixed data directory before recursive deletion', () => {
  assert.match(helper, /resolved=\$\(readlink -f "\$DATA_DIR"/u);
  assert.match(helper, /\[ "\$resolved" = "\$DATA_DIR" \]/u);
  assert.match(helper, /rm -rf -- "\$DATA_DIR"/u);
});

test('host helper has valid POSIX shell syntax on Linux', { skip: process.platform !== 'linux' }, () => {
  execFileSync('sh', ['-n', helperPath], { stdio: 'pipe' });
});
