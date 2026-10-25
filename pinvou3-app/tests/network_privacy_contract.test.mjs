import assert from 'node:assert/strict';
import test from 'node:test';
import {
  NETWORK_PRIVACY_CONTRACT_ID,
  loadNetworkPrivacyDisclosure,
} from '../src/features/updater/network-privacy-contract.js';
import { dict, ensureLanguage } from '../src/shared/i18n.js';

const SECTION_IDS = [
  'purpose', 'data-form', 'purpose-week-keys', 'retention', 'security-logs', 'separation',
];

test('network disclosure loads all actual language chunks without copy fallback', async () => {
  const copies = [];
  for (const locale of ['zh-CN', 'en', 'ja']) {
    const result = await loadNetworkPrivacyDisclosure(locale);
    assert.ok(result);
    assert.equal(result.protocolVersion, 1);
    assert.equal(result.contractId, NETWORK_PRIVACY_CONTRACT_ID);
    assert.equal(result.contractId, 'pinvou.upgrade-network-privacy.v1');
    assert.equal(result.locale, locale);
    assert.deepEqual(result.sections.map((entry) => entry.id), SECTION_IDS);
    assert.ok(result.title.trim());
    for (const entry of result.sections) {
      assert.ok(entry.title.trim());
      assert.ok(entry.body.trim());
    }
    copies.push(result);
  }
  assert.equal(new Set(copies.map((copy) => copy.title)).size, 3);
  for (const id of SECTION_IDS) {
    assert.equal(new Set(copies.map((copy) => copy.sections.find((entry) => entry.id === id).body)).size, 3);
  }
  assert.deepEqual(await loadNetworkPrivacyDisclosure('zh'), copies[0]);
  assert.deepEqual(await loadNetworkPrivacyDisclosure('zh-Hans'), copies[0]);
});

test('network disclosure rejects unknown and prototype locale names', async () => {
  for (const locale of [null, undefined, 1, {}, [], '', 'fr', 'zh-TW', 'ja-JP',
    'EN', ' en', 'en-US', 'constructor', '__proto__', 'toString']) {
    assert.equal(await loadNetworkPrivacyDisclosure(locale), null);
  }
});

test('network disclosure distinguishes implemented processing, material support and retention enforcement', async () => {
  const result = await loadNetworkPrivacyDisclosure('en');
  assert.ok(result);
  assert.deepEqual(result.implementedCapturePurposes, ['quality-prefix', 'stage-quality', 'release-quality']);
  assert.deepEqual(result.materialSupportedPurposes, ['quality-prefix', 'stage-quality', 'release-quality']);
  assert.deepEqual(result.processing, {
    addressSource: 'direct-connection-peer',
    serviceRawAddressStorage: 'controlled-memory-only',
    servicePlaintextPrefixStorage: 'controlled-memory-only',
    ipv4PrefixLength: 24,
    ipv6PrefixLength: 48,
    sourceNetworkIdentifiers: ['asn', 'prefixHmac'],
  });
  assert.deepEqual(result.keys, {
    separation: 'independent-purpose-and-iso-utc-week', exportable: false, sharedWithSN: false,
  });
  assert.deepEqual(result.retention, {
    sourceDetailMaximumDays: 30,
    sourceDetailStartsAt: 'original-legal-server-acceptance',
    enforcement: 'explicit-owner-prune',
    automaticCleanupScheduleProvided: false,
    retainedAnonymousForm: 'immutable-threshold-suppressed-weekly-counts',
    retainedReplayForm: 'minimal-replay-and-completeness-markers',
  });
  assert.deepEqual(result.securityLogs, {
    separateSourceIPLogProvided: false,
    externalSourceIPLogMaximumDays: 7,
    externalDeploymentAttested: false,
  });
  assert.equal(result.separateSNSettings, true);
  assert.equal(result.deploymentAttested, false);
});

test('network disclosure has a fixed closed output with deeply immutable data', async () => {
  const result = await loadNetworkPrivacyDisclosure('en');
  assert.ok(result);
  assert.deepEqual(Object.keys(result).sort((left, right) => left.localeCompare(right)), [
    'protocolVersion', 'contractId', 'locale', 'implementedCapturePurposes', 'materialSupportedPurposes',
    'processing', 'keys', 'retention', 'securityLogs', 'separateSNSettings',
    'deploymentAttested', 'title', 'sections',
  ].sort((left, right) => left.localeCompare(right)));
  for (const value of [result, result.implementedCapturePurposes, result.materialSupportedPurposes,
    result.processing, result.processing.sourceNetworkIdentifiers, result.keys,
    result.retention, result.securityLogs, result.sections, ...result.sections]) {
    assert.equal(Object.isFrozen(value), true);
  }
  assert.throws(() => { result.retention.sourceDetailMaximumDays = 365; }, TypeError);
  assert.throws(() => { result.implementedCapturePurposes.push('stage-quality'); }, TypeError);
  assert.throws(() => { result.sections[0].body = 'replacement'; }, TypeError);
  assert.deepEqual(await loadNetworkPrivacyDisclosure('en'), result);
  for (const entry of result.sections) assert.deepEqual(
    Object.keys(entry).sort((left, right) => left.localeCompare(right)), ['body', 'id', 'title'],
  );
});

test('network disclosure refuses incomplete, inherited or wrong-language namespaces', { concurrency: false }, async () => {
  await ensureLanguage('ja');
  await ensureLanguage('en');
  const original = dict.ja.uiUpgradeNetworkPrivacy;
  try {
    for (const value of [undefined, null, [], {}, dict.en.uiUpgradeNetworkPrivacy,
      Object.create(original), { ...original, locale: 'en' }, { ...original, title: ' ' }]) {
      dict.ja.uiUpgradeNetworkPrivacy = value;
      assert.equal(await loadNetworkPrivacyDisclosure('ja'), null);
    }
    for (const key of Object.keys(original)) {
      const incomplete = { ...original };
      delete incomplete[key];
      dict.ja.uiUpgradeNetworkPrivacy = incomplete;
      assert.equal(await loadNetworkPrivacyDisclosure('ja'), null, key);
    }
    delete dict.ja.uiUpgradeNetworkPrivacy;
    assert.equal(await loadNetworkPrivacyDisclosure('ja'), null);
  } finally {
    dict.ja.uiUpgradeNetworkPrivacy = original;
  }
  assert.equal((await loadNetworkPrivacyDisclosure('ja')).locale, 'ja');
});

test('network disclosure copies localized text rather than returning mutable dictionary objects', { concurrency: false }, async () => {
  const first = await loadNetworkPrivacyDisclosure('en');
  assert.ok(first);
  const original = dict.en.uiUpgradeNetworkPrivacy;
  try {
    dict.en.uiUpgradeNetworkPrivacy = { ...original, title: 'A changed local title' };
    const second = await loadNetworkPrivacyDisclosure('en');
    assert.equal(second.title, 'A changed local title');
    assert.equal(first.title, original.title);
    assert.notEqual(second, first);
  } finally {
    dict.en.uiUpgradeNetworkPrivacy = original;
  }
});
