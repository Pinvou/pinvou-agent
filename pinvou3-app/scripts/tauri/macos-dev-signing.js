// Development signing uses an Apple-issued identity so both the designated
// requirement and Keychain partition remain stable across compiled binaries.
// Production signing stays with Tauri and its explicit signing configuration.
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const SIGNING_IDENTIFIER = 'com.pinvou.pinvou3';
const STATE_FILE = path.join(os.homedir(), 'Library', 'Application Support', 'fresh-agent', 'development-signing.json');
const RUNNER = path.join(__dirname, 'macos-dev-runner.sh');
const SETUP_HINT = 'Install an Apple Development or Developer ID Application certificate with its private key, then run npm run setup:macos-signing.';

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: 'utf8', ...options });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${path.basename(command)} failed: ${(result.stderr || result.stdout || '').trim()}`);
  }
  return (result.stdout || '').trim();
}

function readState(file = STATE_FILE) {
  if (!fs.existsSync(file)) return null;
  const state = JSON.parse(fs.readFileSync(file, 'utf8'));
  if (state.version !== 2 || !/^[A-F0-9]{40}$/.test(state.identity)) {
    throw new Error('Invalid local signing configuration. Run npm run setup:macos-signing.');
  }
  return state;
}

function appleIdentities(output) {
  return [...output.matchAll(/\b([A-F0-9]{40}) "((?:Apple Development|Mac Developer|Developer ID Application):[^"\r\n]+)"/g)]
    .map(([, identity, name]) => ({ identity, name }));
}

function availableIdentities(execute = run) {
  return appleIdentities(execute('/usr/bin/security', ['find-identity', '-v', '-p', 'codesigning']));
}

function selectIdentity(identities, requested) {
  if (requested) {
    const match = identities.find(item => item.identity === requested.toUpperCase());
    if (!match) throw new Error(`The configured Apple signing identity is unavailable. ${SETUP_HINT}`);
    return match;
  }
  if (identities.length === 0) throw new Error(`No valid Apple signing identity found. ${SETUP_HINT}`);
  if (identities.length !== 1) {
    throw new Error(`Multiple Apple signing identities found. Set PINVOU3_MACOS_SIGNING_IDENTITY to the selected SHA-1 fingerprint from security find-identity -v -p codesigning.\n${identities.map(item => `${item.identity} ${item.name}`).join('\n')}`);
  }
  return identities[0];
}

function assertIdentity(state, execute = run) {
  selectIdentity(availableIdentities(execute), state.identity);
}

function setup() {
  if (process.platform !== 'darwin') throw new Error('Development signing is supported only on macOS.');
  const identities = availableIdentities();
  const existing = readState();
  const saved = identities.find(item => item.identity === existing?.identity);
  const selected = selectIdentity(identities, process.env.PINVOU3_MACOS_SIGNING_IDENTITY || saved?.identity);
  const state = { version: 2, identity: selected.identity };
  fs.mkdirSync(path.dirname(STATE_FILE), { recursive: true, mode: 0o700 });
  const temporary = `${STATE_FILE}.${process.pid}.tmp`;
  try {
    fs.writeFileSync(temporary, `${JSON.stringify(state, null, 2)}\n`, { mode: 0o600, flag: 'wx' });
    fs.renameSync(temporary, STATE_FILE);
  } finally {
    fs.rmSync(temporary, { force: true });
  }
  console.log('[signing] Apple development identity selected. Keychain credentials and access rules were not changed.');
}

function sign(binary, { state = readState(), execute = run } = {}) {
  if (!state) throw new Error(`Run npm run setup:macos-signing before starting signed development builds. ${SETUP_HINT}`);
  if (!binary) throw new Error('Missing executable to sign.');
  assertIdentity(state, execute);
  // Let codesign generate the Apple/team-bound designated requirement. A
  // self-signed certificate still gets a cdhash partition on modern macOS.
  execute('/usr/bin/codesign', ['--force', '--sign', state.identity, '--identifier', SIGNING_IDENTIFIER, '--timestamp=none', binary]);
  execute('/usr/bin/codesign', ['--verify', '--strict', '-R', `=anchor apple generic and identifier "${SIGNING_IDENTIFIER}"`, binary]);
}

function developmentArgs(args, {
  platform = process.platform,
  environment = process.env,
  state,
  node = process.execPath,
  runner = RUNNER,
} = {}) {
  if (platform !== 'darwin' || !args.includes('dev') || environment.PINVOU3_MACOS_DEV_SIGNING === '0') return args;
  if (state === undefined) state = readState();
  if (!state) {
    console.warn('[signing] Development builds use temporary signatures. Run npm run setup:macos-signing to keep Keychain authorization across rebuilds.');
    return args;
  }
  const prepared = [...args];
  let separator = prepared.indexOf('--');
  if (separator < 0) {
    prepared.push('--');
    separator = prepared.length - 1;
  }
  const overrides = ['aarch64-apple-darwin', 'x86_64-apple-darwin'].map(target =>
    `--config=target.${target}.runner=${JSON.stringify(['/bin/sh', runner, node])}`,
  );
  // These are Cargo flags, not Tauri --config overlays or application arguments.
  prepared.splice(separator + 1, 0, ...overrides);
  return prepared;
}


if (require.main === module) {
  try {
    if (process.argv[2] === 'setup') setup();
    else if (process.argv[2] === 'sign') sign(process.argv[3]);
    else throw new Error('Usage: macos-dev-signing.js setup | sign <executable>');
  } catch (error) {
    console.error(`[signing] ${error.message}`);
    process.exitCode = 1;
  }
}

module.exports = { developmentArgs, readState, sign, assertIdentity, appleIdentities, selectIdentity };
