const path = require('node:path');
const { packageBuild } = require('./adapter.cjs');

async function main(args, { platform = process.platform, controller = new AbortController() } = {}) {
  if (platform !== 'win32' || args.length !== 4 || args[0] !== '--config' || args[2] !== '--out'
      || !path.isAbsolute(args[1]) || !path.isAbsolute(args[3])) throw new Error('T04_ARGUMENTS');
  const appVersion = require('../../package.json').version;
  await packageBuild(args[1], args[3], { appVersion, signal: controller.signal });
}
if (require.main === module) {
  const controller = new AbortController();
  const cancel = () => controller.abort();
  process.once('SIGINT', cancel); process.once('SIGTERM', cancel);
  main(process.argv.slice(2), { controller }).catch(() => {
    // No raw signer/provider error, paths, credentials, stdout or stderr.
    process.stderr.write('Offline package build failed; incomplete output is not usable.\n');
    process.exitCode = 1;
  }).finally(() => {
    process.removeListener('SIGINT', cancel); process.removeListener('SIGTERM', cancel);
  });
}
module.exports = { main };
