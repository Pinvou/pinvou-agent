import { spawn } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';

export function spawnNdjsonChild({
  args,
  env = process.env,
  stderr = 'capture',
  timeoutMs = 10_000,
  collectResponses = false,
  unrefTimeout = false,
}) {
  const child = spawn(process.execPath, args, {
    stdio: ['pipe', 'pipe', stderr === 'inherit' ? 'inherit' : 'pipe'],
    env,
  });
  let stdout = '';
  let stderrOutput = '';
  let nextId = 1;
  let lastId = 0;
  const pending = new Map();
  const responses = [];

  child.stderr?.on('data', (chunk) => { stderrOutput += chunk; });
  child.stdout.on('data', (chunk) => {
    stdout += chunk;
    let newline;
    while ((newline = stdout.indexOf('\n')) >= 0) {
      const line = stdout.slice(0, newline);
      stdout = stdout.slice(newline + 1);
      if (!line.trim()) continue;
      const message = JSON.parse(line);
      if (collectResponses) responses.push(message);
      const waiter = pending.get(message.id);
      if (!waiter) continue;
      pending.delete(message.id);
      clearTimeout(waiter.timeout);
      waiter.resolve(message);
    }
  });

  const send = (message) => child.stdin.write(`${JSON.stringify(message)}\n`);
  const startRequest = (method, params = {}) => {
    const id = nextId++;
    lastId = id;
    const response = new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (!pending.delete(id)) return;
        reject(new Error(`${method} timed out; child stderr: ${stderrOutput}`));
      }, timeoutMs);
      if (unrefTimeout) timeout.unref();
      pending.set(id, { resolve, timeout });
    });
    send({ jsonrpc: '2.0', id, method, params });
    return { id, response };
  };

  return {
    child,
    responses,
    get lastId() { return lastId; },
    stderrText: () => stderrOutput,
    send,
    notify(method, params = {}) {
      send({ jsonrpc: '2.0', method, params });
    },
    startRequest,
    request(method, params = {}) {
      return startRequest(method, params).response;
    },
    dropRequest(id, resolution) {
      const waiter = pending.get(id);
      if (!waiter) return;
      pending.delete(id);
      clearTimeout(waiter.timeout);
      if (arguments.length > 1) waiter.resolve(resolution);
    },
  };
}

export async function stopNdjsonChild(child, graceMs = 500) {
  if (child.exitCode == null) child.stdin.end();
  if (child.exitCode == null) {
    await Promise.race([
      new Promise((resolve) => { child.once('exit', resolve); }),
      sleep(graceMs),
    ]);
  }
  if (child.exitCode == null) child.kill('SIGKILL');
}
