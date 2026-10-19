import { fork } from 'node:child_process';
import { mkdir, readFile } from 'node:fs/promises';
import { DatabaseSync } from 'node:sqlite';
import { randomUUID } from 'node:crypto';
import { childPath, durableFile, requireThat, LabFailure } from './owned-files.mjs';

export const writerNames = ['user-active', 'user-disconnected', 'service'];
export async function startWriters(root, unsavable = false, spawn = fork) {
  const epoch = randomUUID(), children = [];
  try {
    for (const name of writerNames) {
      const directory = childPath(root, name);
      await mkdir(directory, { mode: 0o700 });
      const child = spawn(new URL('./writer.mjs', import.meta.url), [], { stdio: ['ignore', 'pipe', 'pipe', 'ipc'], windowsHide: true });
      // Drain private output without logging content; close includes both pipes.
      child.stdout?.resume(); child.stderr?.resume();
      const lifecycle = { closed: false, error: false, code: null, signal: null };
      child.on('error', () => { lifecycle.error = true; });
      const closed = new Promise(resolve => child.once('close', (code, signal) => {
        Object.assign(lifecycle, { closed: true, code, signal }); resolve();
      }));
      const owner = { child, name, root: directory, epoch, lifecycle, closed };
      children.push(owner);
      const ready = response(child, 'ready', epoch);
      child.send({ type: 'start', root: directory, epoch, unsavable: unsavable && name === 'service' });
      await ready;
    }
    return { epoch, children, phase: 'running' };
  } catch (error) { await joinWriters({ children }); throw error; }
}
function response(child, type, epoch) {
  return new Promise((resolve, reject) => {
    // A test-runner safety allowance, never a product preparation/download cap.
    const timer = setTimeout(() => finish(new LabFailure('lab_writer_unresponsive')), 30_000);
    const onExit = () => finish(new LabFailure('lab_writer_exited'));
    const onError = () => finish(new LabFailure('lab_writer_failed'));
    const onMessage = message => {
      if (message?.type !== type || message.epoch !== epoch) finish(new LabFailure('lab_writer_refused'));
      else finish(null, message);
    };
    function finish(error, message) {
      clearTimeout(timer); child.off('exit', onExit); child.off('error', onError); child.off('message', onMessage);
      if (error) reject(error); else resolve(message);
    }
    child.once('exit', onExit); child.once('error', onError); child.on('message', onMessage);
  });
}
export async function freezeWriters(root, owners) {
  requireThat(owners.phase === 'running', 'lab_owner_phase_invalid');
  owners.phase = 'freezing';
  const start = process.hrtime.bigint();
  const outcomes = await Promise.allSettled(owners.children.map(async owner => {
    const result = response(owner.child, 'frozen', owners.epoch);
    owner.child.send({ type: 'freeze', epoch: owners.epoch });
    const ack = await result;
    requireThat(Number.isSafeInteger(ack.sequence) && ack.sequence > 0);
    const recorded = JSON.parse(await readFile(childPath(owner.root, 'frozen.json'), 'utf8'));
    requireThat(recorded.epoch === owners.epoch && recorded.sequence === ack.sequence);
    const db = new DatabaseSync(childPath(owner.root, 'state.sqlite'), { readOnly: true });
    try {
      requireThat(Number(db.prepare('SELECT count(*) AS n FROM writes').get().n) === ack.sequence);
      requireThat(db.prepare('PRAGMA integrity_check').get().integrity_check === 'ok');
    } finally { db.close(); }
    return { writer: owner.name, epoch: owners.epoch, sequence: ack.sequence };
  }));
  if (outcomes.some(outcome => outcome.status !== 'fulfilled')) {
    owners.phase = 'refused';
    throw new LabFailure('lab_freeze_incomplete');
  }
  const evidence = outcomes.map(outcome => outcome.value);
  await durableFile(childPath(root, 'freeze.json'), JSON.stringify({ epoch: owners.epoch, evidence }));
  owners.phase = 'frozen';
  return { milliseconds: Number(process.hrtime.bigint() - start) / 1e6, evidence };
}
export async function joinWriters(owners) {
  const results = await Promise.allSettled(owners.children.map(async ({ child, lifecycle, closed, epoch }) => {
    if (!lifecycle.closed) {
      try { if (child.connected) child.send({ type: 'stop', epoch }); } catch { lifecycle.error = true; }
      // Only owned synthetic children are terminated if IPC shutdown fails.
      const timer = setTimeout(() => child.kill(), 5_000);
      let deadline;
      try {
        await Promise.race([closed, new Promise((_, reject) => {
          deadline = setTimeout(() => reject(new LabFailure('lab_child_close_unconfirmed')), 10_000);
        })]);
      } finally { clearTimeout(timer); clearTimeout(deadline); }
    }
    requireThat(lifecycle.closed && !lifecycle.error && lifecycle.code === 0 && lifecycle.signal === null, 'lab_child_close_failed');
  }));
  if (results.some(result => result.status === 'rejected')) throw new LabFailure('lab_child_close_failed');
}
