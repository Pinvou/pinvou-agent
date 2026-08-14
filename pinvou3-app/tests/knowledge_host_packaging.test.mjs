import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
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
const remoteKnowledgeView = readFileSync(
  path.join(appRoot, 'src/features/remote-knowledge/RemoteKnowledgeView.jsx'),
  'utf8',
);

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
});

test('host helper keeps lifecycle operations explicit and persistent', () => {
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

test('owner claim survives until the native client persists it and health uses TLS', () => {
  assert.match(helper, /show_owner_claim\(\)/u);
  assert.match(helper, /install_or_upgrade[\s\S]*show_owner_claim/u);
  assert.match(helper, /restore_host[\s\S]*show_owner_claim/u);
  assert.match(helper, /claim-owner\)[\s\S]*claim_owner/u);
  assert.match(helper, /recover-owner\)[\s\S]*recover_owner/u);
  assert.match(helper, /--recover-host-owner-claim/u);
  assert.match(helper, /--health-check https:\/\/127\.0\.0\.1:3210/u);
});

test('host helper reuses the exact local model directory with systemd hardening', () => {
  assert.match(helper, /\.pinvou3\/knowledge\/models\/bge-m3/u);
  assert.match(helper, /BindPaths=\$model_dir:\$MODEL_MOUNT/u);
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
  assert.match(remoteKnowledgeView, /closeDisabled=\{!\['complete', 'failed'\]\.includes\(hostProgress\.phase\)\}/u);
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
  assert.match(helper, /trap 'rollback_install' EXIT HUP INT TERM/u);
});

test('permanent removal validates the fixed data directory before recursive deletion', () => {
  assert.match(helper, /resolved=\$\(readlink -f "\$DATA_DIR"/u);
  assert.match(helper, /\[ "\$resolved" = "\$DATA_DIR" \]/u);
  assert.match(helper, /rm -rf -- "\$DATA_DIR"/u);
});

test('host helper has valid POSIX shell syntax on Linux', { skip: process.platform !== 'linux' }, () => {
  execFileSync('sh', ['-n', helperPath], { stdio: 'pipe' });
});
