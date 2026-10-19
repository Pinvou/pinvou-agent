// Experimental synthetic workspace only. No app home, keyring or user discovery.
import { lstat, mkdir, open, realpath, statfs, link } from 'node:fs/promises';
import path from 'node:path';
import { randomUUID } from 'node:crypto';

export class LabFailure extends Error {
  constructor(code) { super(code); this.code = code; }
}
export function requireThat(value, code = 'lab_input_invalid') {
  if (!value) throw new LabFailure(code);
}
export function childPath(root, relative) {
  requireThat(typeof relative === 'string' && /^[a-z0-9][a-z0-9./-]*$/.test(relative));
  requireThat(!relative.split('/').some(part => !part || part === '.' || part === '..'));
  const target = path.resolve(root, relative);
  requireThat(path.relative(root, target) === relative.split('/').join(path.sep));
  return target;
}
export async function ordinary(pathname, directory = false) {
  const info = await lstat(pathname, { bigint: true });
  requireThat(!info.isSymbolicLink() && (directory ? info.isDirectory() : info.isFile()), 'lab_link_or_type_refused');
  if (!directory) requireThat(info.nlink === 1n, 'lab_hardlink_refused');
  return info;
}
export async function durableFile(pathname, body, exclusive = true, openFile = open) {
  const file = await openFile(pathname, exclusive ? 'wx' : 'r+');
  try {
    if (!exclusive) { await ordinary(pathname); await file.truncate(0); }
    await file.writeFile(body);
    await file.sync();
  } finally { await file.close(); }
}
export async function publishReport(root, report, openFile = open) {
  const pending = childPath(root, 'pending-report.json');
  await durableFile(pending, JSON.stringify(report, null, 2), true, openFile);
  // link is an exclusive atomic name publication. Keep the pending name rather
  // than risking a post-publication cleanup failure. No success IO follows it.
  await link(pending, childPath(root, 'report.json'));
}
// A successful file fsync is not an OS-independent directory durability proof.
export async function directoryDurability(root) {
  if (process.platform === 'win32') return 'unsupported-node-directory-handle';
  if (!['linux', 'darwin'].includes(process.platform)) return 'unsupported-platform';
  let file;
  try { file = await open(root, 'r'); await file.sync(); return 'directory-fsync-observed'; }
  catch { return 'directory-fsync-unavailable'; }
  finally { await file?.close(); }
}
export async function createRun(parent, requiredBytes) {
  requireThat(path.isAbsolute(parent) && Number.isSafeInteger(requiredBytes) && requiredBytes >= 0);
  // The caller supplies an existing dedicated scratch parent. Walk every ancestor
  // before creating anything; never follow a junction/symlink into another tree.
  let current = path.resolve(parent);
  while (true) {
    await ordinary(current, true);
    const next = path.dirname(current);
    if (next === current) break;
    current = next;
  }
  requireThat(path.resolve(await realpath(parent)).toLowerCase() === path.resolve(parent).toLowerCase(), 'lab_parent_alias_refused');
  const space = await statfs(parent, { bigint: true });
  requireThat(space.bavail * space.bsize >= BigInt(requiredBytes) + 2n * 1024n ** 3n, 'lab_space_insufficient');
  const runId = randomUUID();
  const root = path.join(parent, `pinvou-lab-${runId}`);
  await mkdir(root, { mode: 0o700 });
  await durableFile(path.join(root, 'owner.json'), JSON.stringify({ experiment: 'platform-v0', runId }));
  return { root, runId };
}
