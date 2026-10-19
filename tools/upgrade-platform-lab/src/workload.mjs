import { open } from 'node:fs/promises';
import { randomBytes } from 'node:crypto';
import { childPath, requireThat } from './owned-files.mjs';
import { writerNames } from './coordination.mjs';

export const profiles = Object.freeze({
  small: { bytes: 1024 ** 3, files: 10_000 },
  medium: { bytes: 10 * 1024 ** 3, files: 100_000 },
  large: { bytes: 50 * 1024 ** 3, files: 500_000 },
});
export function workload(profile, shape) {
  requireThat(Object.hasOwn(profiles, profile) && ['many-small', 'few-large'].includes(shape));
  return { ...profiles[profile], files: shape === 'few-large' ? 12 : profiles[profile].files };
}
export async function generate(root, plan) {
  requireThat(Number.isSafeInteger(plan.bytes) && Number.isSafeInteger(plan.files) && plan.bytes >= plan.files && plan.files > 0 && plan.files <= 500_000 && plan.bytes <= profiles.large.bytes);
  const buffer = randomBytes(1024 ** 2);
  for (let index = 0; index < plan.files; index++) {
    const size = Math.floor(plan.bytes / plan.files) + (index < plan.bytes % plan.files ? 1 : 0);
    const file = await open(childPath(root, `${writerNames[index % writerNames.length]}/payload-${index}.bin`), 'wx', 0o600);
    try {
      let written = 0;
      while (written < size) {
        const length = Math.min(buffer.length, size - written);
        const result = await file.write(buffer, 0, length); requireThat(result.bytesWritten > 0); written += result.bytesWritten;
      }
      await file.sync();
    } finally { await file.close(); }
  }
}
