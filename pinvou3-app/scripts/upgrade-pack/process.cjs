const { spawn } = require('node:child_process');
const path = require('node:path');
const { checkSignal, refuse } = require('./files.cjs');

function childEnvironment(directory, parent = process.env) {
  // Tool configuration/credentials are not inherited. A separately controlled
  // signer uses its own external provider integration; never command-line keys.
  const result = {};
  for (const key of ['SystemRoot', 'WINDIR', 'COMSPEC', 'PATHEXT']) {
    if (parent[key] !== undefined) result[key] = parent[key];
  }
  const windows = result.SystemRoot || result.WINDIR;
  if (windows) result.PATH = path.join(windows, 'System32');
  result.TEMP = directory; result.TMP = directory;
  return result;
}
function runChild(executable, args, { cwd, env, signal }) {
  checkSignal(signal);
  return new Promise((resolve, reject) => {
    let child; let failed = false; let cancelled = false;
    try { child = spawn(executable, args, { cwd, env, shell: false, windowsHide: true, stdio: 'ignore' }); }
    catch { reject(new Error('T04_CHILD_START')); return; }
    const abort = () => {
      cancelled = true;
      try { child.kill(); } catch { failed = true; }
      // Await the original direct child's close. This is not an assertion that
      // arbitrary detached descendants are owned/closed; no success on abort.
    };
    child.once('error', () => { failed = true; });
    child.once('close', (code, terminationSignal) => {
      signal?.removeEventListener('abort', abort);
      if (cancelled || signal?.aborted) reject(new Error('T04_CANCELLED'));
      else if (failed || terminationSignal || !Number.isInteger(code)) reject(new Error('T04_CHILD_FAILED'));
      else resolve(code);
    });
    signal?.addEventListener('abort', abort, { once: true });
    if (signal?.aborted) abort();
  });
}
async function requireSuccess(executable, args, options, execute = runChild) {
  if (await execute(executable, args, options) !== 0) refuse('T04_CHILD_FAILED');
  checkSignal(options.signal);
}
module.exports = { childEnvironment, runChild, requireSuccess };
