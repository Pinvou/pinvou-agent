// Appended to the original fresh-key single-successor fixture generator after
// removing only its final console.log. Both complete frozen operations run.
const { verifyRootChain } = await import(new URL('./signatures.mjs', base));
for (const row of rows) {
  const bytes = Buffer.from(row.bytes, 'base64url');
  const entries = row.activeEnvelopes.map((entry) => ({ ...entry,
    bytes: Buffer.from(entry.bytes, 'base64url') }));
  const activeEnvelopes = Array.from({ length: row.activeRepeat }, () => entries).flat();
  const options = { trustedRoot: row.trustedRoot, product: row.expected.product,
    now: row.now, activeEnvelopes };
  let singleCode = null, chainCode = null;
  try { verifyRootSuccessor(bytes, options); }
  catch (error) {
    if (typeof error.code !== 'string') throw error;
    singleCode = error.code;
  }
  try { verifyRootChain([bytes], options); }
  catch (error) {
    if (typeof error.code !== 'string') throw error;
    chainCode = error.code;
  }
  if (singleCode !== chainCode || (chainCode === null) !== row.accepted) {
    throw new Error(`single/one-chain disagreement: ${row.group}/${row.name}`);
  }
  row.chainReferenceAccepted = chainCode === null;
}
console.log(JSON.stringify(rows));
