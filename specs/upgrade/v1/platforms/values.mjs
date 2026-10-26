// Observation DATA only. Nothing in this package constructs an OS capability.
export class PlatformContractError extends Error {
  constructor(code) { super(code); this.code = code; }
}
export function requireFact(condition, code = 'PLATFORM_FACT_INVALID') {
  if (!condition) throw new PlatformContractError(code);
}
export const id = value => typeof value === 'string' && /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/.test(value);
export const hash = value => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);
export const uint = value => Number.isSafeInteger(value) && value >= 0;
export const positive = value => uint(value) && value > 0;
export const boolean = value => typeof value === 'boolean';
export const oneOf = (...values) => value => values.includes(value);
export const nullable = check => value => value === null || check(value);
export const list = check => value => Array.isArray(value) && value.length <= 4096 && value.every(check);
export const ids = value => list(id)(value) && new Set(value).size === value.length;
export function shape(fields) {
  return value => value !== null && typeof value === 'object' && !Array.isArray(value)
    && Object.keys(value).length === Object.keys(fields).length
    && Object.entries(fields).every(([key, check]) => Object.hasOwn(value, key) && check(value[key]));
}
export function same(left, right) { return JSON.stringify(left) === JSON.stringify(right); }
export function sameSet(left, right) { return left.length === right.length && left.every(item => right.includes(item)); }
export function define(request, facts, check, effect = 'none') {
  return Object.freeze({ request: shape(request), facts: shape(facts), check, effect });
}
export const context = shape({
  targetKey: oneOf('windows-x86_64-nsis-perMachine', 'linux-x86_64-deb-perMachine',
    'linux-arm64-deb-perMachine', 'macos-universal-dmg-perMachine'),
  os: oneOf('windows', 'linux', 'macos'), hostArch: oneOf('x86_64', 'arm64'),
  upgradeType: oneOf('normal', 'forced', 'silent'), installationScopeId: id,
});
export function checkContext(value) {
  requireFact(context(value), 'PLATFORM_CONTEXT_INVALID');
  const expected = { 'windows-x86_64-nsis-perMachine': ['windows', 'x86_64'],
    'linux-x86_64-deb-perMachine': ['linux', 'x86_64'], 'linux-arm64-deb-perMachine': ['linux', 'arm64'] }[value.targetKey];
  requireFact(expected ? expected[0] === value.os && expected[1] === value.hostArch : value.os === 'macos',
    'PLATFORM_CONTEXT_INVALID');
}
