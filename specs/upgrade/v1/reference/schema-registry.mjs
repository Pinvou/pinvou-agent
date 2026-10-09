import { readFile } from 'node:fs/promises';
import Ajv2020 from 'ajv/dist/2020.js';
import { schemas } from './build-schemas.mjs';
import { parseJson } from './canonical-json.mjs';
import { requireCondition } from './errors.mjs';

// Runtime validation reads the committed artifacts, never remote schema URLs.
const ajv = new Ajv2020({ strict: true, allErrors: false, validateFormats: false });
for (const name of schemas.keys()) {
  const bytes = await readFile(new URL(`../schemas/${name}.json`, import.meta.url));
  ajv.addSchema(parseJson(bytes));
}

export function validateSchema(name, value) {
  const validator = ajv.getSchema('urn:pinvou:upgrade:v1:' + name.replaceAll('/', ':'));
  requireCondition(validator !== undefined, 'SCHEMA_UNKNOWN');
  requireCondition(validator(value), 'SCHEMA_INVALID');
  return value;
}

export function validateClaims(name, value) {
  const validator = ajv.getSchema('urn:pinvou:upgrade:v1:' + name.replaceAll('/', ':') + '#/properties/signed');
  requireCondition(validator !== undefined, 'SCHEMA_UNKNOWN');
  requireCondition(validator(value), 'SCHEMA_INVALID');
  return value;
}
