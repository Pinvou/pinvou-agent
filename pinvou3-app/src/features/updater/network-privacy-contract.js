import { dict, ensureLanguage } from '../../shared/i18n.js';

/**
 * T25's disclosure data for a T40 view. This is neither a deployment attestation
 * nor permission to collect, read, export, or publish network observations.
 * The consumer owns its language-selection lifecycle and renders these strings
 * as text. No event, address, credential, or local preference is an input.
 */
export const NETWORK_PRIVACY_CONTRACT_ID = 'pinvou.upgrade-network-privacy.v1';

const LOCALES = Object.freeze({
  zh: Object.freeze({ language: 'zh', tag: 'zh-CN' }),
  'zh-CN': Object.freeze({ language: 'zh', tag: 'zh-CN' }),
  'zh-Hans': Object.freeze({ language: 'zh', tag: 'zh-CN' }),
  en: Object.freeze({ language: 'en', tag: 'en' }),
  ja: Object.freeze({ language: 'ja', tag: 'ja' }),
});

const FACTS = Object.freeze({
  implementedCapturePurposes: Object.freeze(['quality-prefix', 'stage-quality', 'release-quality']),
  materialSupportedPurposes: Object.freeze([
    'quality-prefix', 'stage-quality', 'release-quality',
  ]),
  processing: Object.freeze({
    addressSource: 'direct-connection-peer',
    serviceRawAddressStorage: 'controlled-memory-only',
    servicePlaintextPrefixStorage: 'controlled-memory-only',
    ipv4PrefixLength: 24,
    ipv6PrefixLength: 48,
    sourceNetworkIdentifiers: Object.freeze(['asn', 'prefixHmac']),
  }),
  keys: Object.freeze({
    separation: 'independent-purpose-and-iso-utc-week',
    exportable: false,
    sharedWithSN: false,
  }),
  retention: Object.freeze({
    sourceDetailMaximumDays: 30,
    sourceDetailStartsAt: 'original-legal-server-acceptance',
    enforcement: 'explicit-owner-prune',
    automaticCleanupScheduleProvided: false,
    retainedAnonymousForm: 'immutable-threshold-suppressed-weekly-counts',
    retainedReplayForm: 'minimal-replay-and-completeness-markers',
  }),
  securityLogs: Object.freeze({
    separateSourceIPLogProvided: false,
    externalSourceIPLogMaximumDays: 7,
    externalDeploymentAttested: false,
  }),
  separateSNSettings: true,
  deploymentAttested: false,
});

/**
 * @typedef {object} DisclosureCopy
 * @property {string} locale
 * @property {string} title
 * @property {string} purposeTitle
 * @property {string} purposeBody
 * @property {string} formTitle
 * @property {string} formBody
 * @property {string} keysTitle
 * @property {string} keysBody
 * @property {string} retentionTitle
 * @property {string} retentionBody
 * @property {string} logsTitle
 * @property {string} logsBody
 * @property {string} separationTitle
 * @property {string} separationBody
 */

const COPY_FIELDS = Object.freeze([
  'locale', 'title', 'purposeTitle', 'purposeBody', 'formTitle', 'formBody',
  'keysTitle', 'keysBody', 'retentionTitle', 'retentionBody', 'logsTitle',
  'logsBody', 'separationTitle', 'separationBody',
]);

/**
 * @param {unknown} value Candidate namespace from the selected dictionary.
 * @param {string} locale Required explicit disclosure locale.
 * @returns {DisclosureCopy | null} Complete copy in the requested language.
 */
function checkedCopy(value, locale) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const copy = /** @type {Record<string, unknown>} */ (value);
  for (const key of COPY_FIELDS) {
    // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 lacks Object.hasOwn
    if (!Object.prototype.hasOwnProperty.call(copy, key)) return null;
    const text = copy[key];
    if (typeof text !== 'string' || !text.trim()) return null;
  }
  // In particular, reject ja's inherited English fallback namespace.
  if (copy.locale !== locale) return null;
  return /** @type {DisclosureCopy} */ (value);
}

/**
 * @param {string} id Stable section identifier.
 * @param {string} title Localized heading.
 * @param {string} body Localized explanation.
 */
function section(id, title, body) {
  return Object.freeze({ id, title, body });
}

/**
 * Explicit supported aliases only; no language or missing-copy fallback.
 * null means the caller cannot present a complete disclosure in this locale.
 * @param {unknown} locale Explicit supported locale or language alias.
 */
export async function loadNetworkPrivacyDisclosure(locale) {
  if (typeof locale !== 'string') return null;
  // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 lacks Object.hasOwn
  if (!Object.prototype.hasOwnProperty.call(LOCALES, locale)) return null;
  const selected = LOCALES[/** @type {keyof typeof LOCALES} */ (locale)];
  if (!(await ensureLanguage(selected.language))) return null;
  const dictionaries = /** @type {Record<string, Record<string, unknown>>} */ (dict);
  const dictionary = dictionaries[selected.language];
  if (!dictionary || typeof dictionary !== 'object') return null;
  // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 lacks Object.hasOwn
  if (!Object.prototype.hasOwnProperty.call(dictionary, 'uiUpgradeNetworkPrivacy')) return null;
  const copy = checkedCopy(dictionary.uiUpgradeNetworkPrivacy, selected.tag);
  if (!copy) return null;
  return Object.freeze({
    protocolVersion: 1,
    contractId: NETWORK_PRIVACY_CONTRACT_ID,
    locale: selected.tag,
    ...FACTS,
    title: copy.title,
    sections: Object.freeze([
      section('purpose', copy.purposeTitle, copy.purposeBody),
      section('data-form', copy.formTitle, copy.formBody),
      section('purpose-week-keys', copy.keysTitle, copy.keysBody),
      section('retention', copy.retentionTitle, copy.retentionBody),
      section('security-logs', copy.logsTitle, copy.logsBody),
      section('separation', copy.separationTitle, copy.separationBody),
    ]),
  });
}
