// Experimental encrypted stream. It is not the production backup wire format.
import { createCipheriv, createDecipheriv, createHash, randomBytes } from 'node:crypto';
import { open, readdir } from 'node:fs/promises';
import { childPath, ordinary, requireThat } from './owned-files.mjs';

const magic = Buffer.from('PINVOU-LAB-BACKUP-V0\n');
const maximumBytes = 60 * 1024 ** 3; // Below the per-invocation AES-GCM bound.
const block = 1024 ** 2;
function frame(name, size) {
  const bytes = Buffer.from(name);
  requireThat(bytes.length <= 512 && Number.isSafeInteger(size) && size >= 0 && size <= maximumBytes);
  const result = Buffer.alloc(12 + bytes.length);
  result.writeUInt32BE(bytes.length); result.writeBigUInt64BE(BigInt(size), 4); bytes.copy(result, 12);
  return result;
}
async function writeAll(file, bytes) {
  let offset = 0;
  while (offset < bytes.length) {
    const { bytesWritten } = await file.write(bytes, offset, bytes.length - offset);
    requireThat(bytesWritten > 0, 'lab_write_incomplete'); offset += bytesWritten;
  }
}
export async function inventory(root, names) {
  const entries = [];
  async function visit(relative) {
    const pathname = childPath(root, relative);
    const info = await ordinary(pathname, names.includes(relative));
    if (info.isDirectory()) {
      const children = await readdir(pathname);
      for (const name of children.sort()) {
        const child = `${relative}/${name}`;
        const childInfo = await ordinary(childPath(root, child));
        requireThat(childInfo.isFile());
        entries.push({ name: child, size: Number(childInfo.size), device: String(childInfo.dev), inode: String(childInfo.ino), mtimeNs: String(childInfo.mtimeNs), ctimeNs: String(childInfo.ctimeNs) });
      }
    }
  }
  for (const name of names) await visit(name);
  const freeze = await ordinary(childPath(root, 'freeze.json'));
  entries.push({ name: 'freeze.json', size: Number(freeze.size), device: String(freeze.dev), inode: String(freeze.ino), mtimeNs: String(freeze.mtimeNs), ctimeNs: String(freeze.ctimeNs) });
  requireThat(entries.length <= 510_000 && entries.reduce((sum, entry) => sum + entry.size + frame(entry.name, entry.size).length, 0) < maximumBytes);
  return entries;
}
function sameObject(left, right) {
  return right.isFile() && right.nlink === 1n && left.dev === right.dev && left.ino === right.ino &&
    left.size === right.size && left.mtimeNs === right.mtimeNs && left.ctimeNs === right.ctimeNs;
}
// The optional local OS port enables controlled real-file race experiments.
export async function seal(root, entries, output, key, openFile = open) {
  requireThat(key.length === 32 && entries.length > 0);
  const iv = randomBytes(12), hashes = [], target = await open(output, 'wx', 0o600);
  const cipher = createCipheriv('aes-256-gcm', key, iv);
  cipher.setAAD(magic);
  try {
    await writeAll(target, Buffer.concat([magic, iv]));
    for (const entry of entries) {
      const pathname = childPath(root, entry.name);
      const before = await ordinary(pathname);
      requireThat(Number(before.size) === entry.size && String(before.dev) === entry.device && String(before.ino) === entry.inode &&
        String(before.mtimeNs) === entry.mtimeNs && String(before.ctimeNs) === entry.ctimeNs, 'lab_source_changed');
      const source = await openFile(pathname, 'r');
      try {
        const opened = await source.stat({ bigint: true });
        requireThat(sameObject(before, opened) && Number(opened.size) === entry.size, 'lab_source_changed');
        await writeAll(target, cipher.update(frame(entry.name, entry.size)));
        const hash = createHash('sha256'), buffer = Buffer.alloc(Math.min(block, Math.max(1, entry.size)));
        let total = 0;
        while (total < entry.size) {
          const { bytesRead } = await source.read(buffer, 0, Math.min(buffer.length, entry.size - total), total);
          requireThat(bytesRead > 0, 'lab_source_truncated');
          const bytes = buffer.subarray(0, bytesRead); hash.update(bytes);
          await writeAll(target, cipher.update(bytes)); total += bytesRead;
        }
        const after = await source.stat({ bigint: true });
        requireThat(sameObject(opened, after), 'lab_source_changed');
        hashes.push({ name: entry.name, size: entry.size, sha256: hash.digest('hex') });
      } finally { await source.close(); }
    }
    await writeAll(target, cipher.final()); await writeAll(target, cipher.getAuthTag()); await target.sync();
  } finally { await target.close(); }
  return hashes;
}
async function* plaintext(file, key, size) {
  const header = Buffer.alloc(magic.length + 12), tag = Buffer.alloc(16);
  requireThat((await file.read(header, 0, header.length, 0)).bytesRead === header.length && header.subarray(0, magic.length).equals(magic));
  requireThat((await file.read(tag, 0, 16, size - 16)).bytesRead === 16);
  const decipher = createDecipheriv('aes-256-gcm', key, header.subarray(magic.length));
  decipher.setAAD(magic); decipher.setAuthTag(tag);
  let position = header.length;
  const buffer = Buffer.alloc(block);
  while (position < size - 16) {
    const { bytesRead } = await file.read(buffer, 0, Math.min(block, size - 16 - position), position);
    requireThat(bytesRead > 0, 'lab_archive_truncated'); position += bytesRead;
    yield decipher.update(buffer.subarray(0, bytesRead));
  }
  yield decipher.final(); // No verification result escapes before AEAD final.
}
class Reader {
  constructor(stream) { this.stream = stream[Symbol.asyncIterator](); this.chunk = Buffer.alloc(0); }
  async consume(length, consumer) {
    while (length > 0) {
      if (!this.chunk.length) {
        const next = await this.stream.next(); requireThat(!next.done, 'lab_archive_truncated'); this.chunk = next.value;
        if (!this.chunk.length) continue;
      }
      const amount = Math.min(length, this.chunk.length);
      consumer(this.chunk.subarray(0, amount)); this.chunk = this.chunk.subarray(amount); length -= amount;
    }
  }
  async bytes(length) {
    const chunks = []; await this.consume(length, part => chunks.push(Buffer.from(part))); return Buffer.concat(chunks);
  }
  async finish() {
    requireThat(this.chunk.length === 0, 'lab_archive_trailing');
    for (;;) { const next = await this.stream.next(); if (next.done) break; requireThat(next.value.length === 0, 'lab_archive_trailing'); }
  }
}
export async function verify(output, expected, key, openFile = open) {
  const before = await ordinary(output), file = await openFile(output, 'r');
  try {
    requireThat(before.size >= BigInt(magic.length + 28) && before.size <= BigInt(maximumBytes), 'lab_archive_capacity');
    const opened = await file.stat({ bigint: true });
    requireThat(sameObject(before, opened), 'lab_archive_changed');
    const reader = new Reader(plaintext(file, key, Number(opened.size)));
    for (const entry of expected) {
      const header = frame(entry.name, entry.size);
      requireThat((await reader.bytes(header.length)).equals(header), 'lab_archive_manifest_mismatch');
      const hash = createHash('sha256'); await reader.consume(entry.size, part => hash.update(part));
      requireThat(hash.digest('hex') === entry.sha256, 'lab_archive_hash_mismatch');
    }
    await reader.finish();
    const after = await file.stat({ bigint: true });
    requireThat(sameObject(opened, after), 'lab_archive_changed');
    return { files: expected.length, bytes: expected.reduce((sum, entry) => sum + entry.size, 0) };
  } finally { await file.close(); }
}
