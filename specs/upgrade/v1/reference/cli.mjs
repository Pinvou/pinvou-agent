import { createReadStream } from 'node:fs';
import { canonicalize, parseJson, LIMITS } from './canonical-json.mjs';
import { ContractError, requireCondition } from './errors.mjs';
import { validateSchema } from './schema-registry.mjs';
import { verifyEnvelope } from './signatures.mjs';

// Files are local inputs. Errors print stable codes only, never submitted claims.
async function readLimited(path) {
  const chunks = [];
  let size = 0;
  for await (const chunk of createReadStream(path, { highWaterMark: 64 * 1024 })) {
    size += chunk.byteLength;
    requireCondition(size <= LIMITS.metadataBytes, 'JSON_INPUT_LIMIT');
    chunks.push(chunk);
  }
  return Buffer.concat(chunks, size);
}

async function main(args) {
  const [command, ...paths] = args;
  if (command === 'canonicalize' && paths.length === 1) {
    process.stdout.write(canonicalize(parseJson(await readLimited(paths[0]))));
    return;
  }
  if (command === 'schema' && paths.length === 2) {
    validateSchema(paths[0], parseJson(await readLimited(paths[1])));
    process.stdout.write('VALID\n');
    return;
  }
  if (command === 'verify' && paths.length === 3) {
    const [envelopeFile, rootFile, contextFile] = paths;
    const root = parseJson(await readLimited(rootFile));
    const context = parseJson(await readLimited(contextFile));
    verifyEnvelope(await readLimited(envelopeFile), { ...context, trustedRoot: root });
    process.stdout.write('VALID\n');
    return;
  }
  throw new ContractError('USAGE: canonicalize <json> | schema <schema-name> <json> | verify <envelope> <trusted-root-body> <context>');
}

try {
  await main(process.argv.slice(2));
} catch (error) {
  process.stderr.write((error instanceof ContractError ? error.code : 'INPUT_OR_RUNTIME_ERROR') + '\n');
  process.exitCode = 1;
}
