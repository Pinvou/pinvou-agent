// Round-31 m6 / round-32 minor 10 (review #455): mask raw backend error text
// on the connector flow cards exactly like the tmeet site did — en/ja
// (showRawErrors=false) get the localized generic instead of untranslated
// diagnostics, zh keeps them. The raw text stays in flow state; masking
// happens at render. Extracted pure for direct node testing.
export const maskedConnectorFlow = (flow, detailCopy) => (
  flow && flow.phase === 'error' && !detailCopy.showRawErrors
    ? { ...flow, err: detailCopy.actions.operationFailed }
    : flow
);
