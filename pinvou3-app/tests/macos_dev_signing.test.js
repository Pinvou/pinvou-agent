const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const { randomUUID } = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { test } = require('node:test');
const { developmentArgs, sign, readState, appleIdentities, selectIdentity } = require('../scripts/tauri/macos-dev-signing.js');

const state = { version: 2, identity: 'A'.repeat(40) };
const identityOutput = `1) ${state.identity} "Apple Development: Local Tester (TESTTEAM)"`;
const options = { platform: 'darwin', environment: {}, state, node: '/node path/node', runner: '/repo path/macos-dev-runner.sh' };

test('dev signing leaves release, non-macOS and explicit opt-out untouched', () => {
  for (const [args, settings] of [
    [['build'], options],
    [['bundle'], options],
    [['dev'], { ...options, platform: 'linux' }],
    [['dev'], { ...options, platform: 'win32' }],
    [['dev'], { ...options, environment: { PINVOU3_MACOS_DEV_SIGNING: '0' } }],
  ]) assert.equal(developmentArgs(args, settings), args);
});

test('self-signed and ambiguous identities cannot silently replace Apple signing', () => {
  assert.deepEqual(appleIdentities(`1) ${state.identity} "Fresh Assistant Local Development"`), []);
  assert.throws(() => selectIdentity([], null), /No valid Apple/);
  const identities = appleIdentities(identityOutput);
  assert.equal(selectIdentity(identities).identity, state.identity);
  assert.throws(() => selectIdentity([...identities, ...identities]), /Multiple Apple/);
  assert.equal(selectIdentity(identities, state.identity.toLowerCase()).identity, state.identity);
});

test('shell runner forwards arguments and exit status and does not launch after signing fails', { skip: process.platform === 'win32' }, () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'fresh runner test '));
  try {
    const signer = path.join(directory, 'signer');
    const application = path.join(directory, 'application');
    const marker = path.join(directory, 'launched');
    fs.writeFileSync(signer, '#!/bin/sh\nexit "${SIGN_RESULT:-0}"\n', { mode: 0o700 });
    fs.writeFileSync(application, '#!/bin/sh\nprintf "%s" "$1" > "$MARKER"\nexit 17\n', { mode: 0o700 });
    const args = [path.join(__dirname, '../scripts/tauri/macos-dev-runner.sh'), signer, application, 'two words'];
    const environment = { ...process.env, MARKER: marker };
    const success = spawnSync('/bin/sh', args, { env: environment });
    assert.equal(success.status, 17);
    assert.equal(fs.readFileSync(marker, 'utf8'), 'two words');
    fs.unlinkSync(marker);
    const denied = spawnSync('/bin/sh', args, { env: { ...environment, SIGN_RESULT: '9' } });
    assert.equal(denied.status, 9);
    assert.ok(!fs.existsSync(marker));
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test('Cargo runner preserves spaces, Tauri overlays and application arguments', () => {
  const args = ['dev', '--config', '/overlay path/mac.json', '--', '--release', '--', '--example', 'two words'];
  const result = developmentArgs(args, options);
  assert.deepEqual(result.slice(0, 4), args.slice(0, 4));
  assert.deepEqual(result.slice(6), args.slice(4));
  assert.deepEqual(JSON.parse(result[4].split('.runner=')[1]), ['/bin/sh', options.runner, options.node]);
  assert.match(result[4], /^--config=target\.aarch64-apple-darwin\.runner=/);
  assert.match(result[5], /^--config=target\.x86_64-apple-darwin\.runner=/);
  assert.equal(developmentArgs(['dev'], options)[1], '--');
});

test('signing requires the configured key and verifies a Apple-issued identity', () => {
  const calls = [];
  sign('/binary path/app', { state, execute(command, args) {
    calls.push([command, args]);
    return command.endsWith('security') ? identityOutput : '';
  } });
  const signing = calls[1][1];
  assert.equal(signing.at(-1), '/binary path/app');
  assert.ok(signing.includes(state.identity));
  assert.ok(!signing.includes('--requirements'));
  assert.ok(calls[2][1].includes('=anchor apple generic and identifier "com.pinvou.pinvou3"'));
  assert.equal(calls[2][1][0], '--verify');
  assert.throws(() => sign('/app', { state, execute: () => '' }), /identity is unavailable/);
  assert.throws(() => sign('/app', { state: null }), /setup:macos-signing/);
});

test('signing failures stop the runner before launching the application', () => {
  assert.throws(() => sign('/app', { state, execute(command) {
    if (command.endsWith('security')) return identityOutput;
    throw new Error('Signing denied');
  } }), /Signing denied/);
  const source = fs.readFileSync(path.join(__dirname, '../scripts/tauri/macos-dev-runner.sh'), 'utf8');
  assert.match(source, /set -eu/);
  assert.ok(source.indexOf('sign "$1"') < source.indexOf('exec "$@"'));
});

test('signed rebuilds can read their existing Keychain item with interaction disabled', {
  skip: process.platform !== 'darwin' || process.env.PINVOU3_TEST_MACOS_SIGNING !== '1',
}, () => {
  assert.ok(readState(), 'Run npm run setup:macos-signing first');
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'fresh signing test '));
  const binary = path.join(directory, 'probe');
  const source = path.join(directory, 'probe.c');
  const account = randomUUID();
  function execute(command, args) {
    const result = spawnSync(command, args, { encoding: 'utf8' });
    assert.equal(result.status, 0, `${path.basename(command)}: ${result.stderr || result.stdout}`);
    return result.stdout + result.stderr;
  }
  fs.writeFileSync(source, String.raw`
#include <Security/Security.h>
#include <string.h>
#include <stdio.h>
#ifndef BUILD_VARIANT
#define BUILD_VARIANT 1
#endif
int main(int argc, char **argv) {
  if (argc != 3) return 2;
  SecKeychainSetUserInteractionAllowed(false);
  const char *service = "fresh-agent-signing-test";
  const char *account = argv[2];
  OSStatus status;
  if (!strcmp(argv[1], "create")) {
    status = SecKeychainAddGenericPassword(NULL, strlen(service), service,
      strlen(account), account, 4, "test", NULL);
  } else {
    UInt32 length = 0;
    void *data = NULL;
    SecKeychainItemRef item = NULL;
    status = SecKeychainFindGenericPassword(NULL, strlen(service), service,
      strlen(account), account, &length, &data, &item);
    if (status == errSecSuccess) {
      if (length != 4 || memcmp(data, "test", 4)) status = errSecDecode;
      SecKeychainItemFreeContent(NULL, data);
      if (!strcmp(argv[1], "delete")) status = SecKeychainItemDelete(item);
      CFRelease(item);
    }
  }
  if (status) fprintf(stderr, "Keychain status %d, build %d\n", (int)status, BUILD_VARIANT);
  return status ? 1 : 0;
}
`);
  let created = false;
  try {
    execute('/usr/bin/clang', [source, '-framework', 'Security', '-framework', 'CoreFoundation', '-o', binary]);
    sign(binary);
    execute(binary, ['create', account]);
    created = true;
    execute(binary, ['read', account]);
    const first = execute('/usr/bin/codesign', ['-d', '-r-', binary]);
    execute('/usr/bin/clang', [source, '-DBUILD_VARIANT=2', '-framework', 'Security', '-framework', 'CoreFoundation', '-o', binary]);
    sign(binary);
    const second = execute('/usr/bin/codesign', ['-d', '-r-', binary]);
    assert.equal(first.split('designated =>')[1], second.split('designated =>')[1]);
    execute(binary, ['read', account]);
    execute(binary, ['read', account]);
  } finally {
    try {
      if (created) execute('/usr/bin/security', ['delete-generic-password', '-s', 'fresh-agent-signing-test', '-a', account]);
    } finally {
      fs.rmSync(directory, { recursive: true, force: true });
    }
  }
});
