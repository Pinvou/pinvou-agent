/**
 * Pure logic for the endpoint model picker (settings feature).
 *
 * Kept JSX-free on purpose: tests/endpoint_model_picker.test.mjs loads this
 * file in a Node `vm` context (same pattern as tests/model_catalog_grouping.test.js),
 * so it must not import React, icons, i18n, or the bridge. The component
 * (endpoint-model-picker.jsx) only renders; every decision about data shape,
 * filtering, badge text, deduplication, and batch capping lives here.
 *
 * Source of truth for the wire shapes: the frozen discovery spec
 * (.discovery-spec.md §2.1) — `EndpointModelsListing` / `DiscoveredModel`
 * arrive with serde snake_case fields from the Rust command.
 */

/** Rows above this count get the filter input rendered (spec §2.6). */
export const FILTER_VISIBLE_ROW_LIMIT = 15;

/** Hard cap for the "add all" batch (spec §2.6: >100 models truncate to 100). */
export const ADD_ALL_BATCH_LIMIT = 100;

/**
 * Format a token count with K/M units, 1000-based like the monitor view's
 * context chip (MonitorView rounds `ctxNum / 1000` + 'K'). Returns '' for
 * missing/invalid values so callers can skip the badge entirely.
 * @param {number|string|null|undefined} value Raw token count (u32 from Rust).
 * @returns {string} e.g. '999', '4K', '262K', '1M', '1.5M'; '' when absent.
 */
export function formatTokensCount(value) {
  const count = Number(value);
  if (!Number.isFinite(count) || count <= 0) return '';
  if (count >= 1000000) {
    const millions = count / 1000000;
    // >=10M drops the decimals (12.04M reads worse than 12M); below that one
    // decimal is kept with a trailing ".0" stripped (1048576 -> '1' -> '1M').
    const text = millions >= 10
      ? String(Math.round(millions))
      : millions.toFixed(1).replace(/\.0$/, '');
    return `${text}M`;
  }
  if (count >= 1000) return `${Math.round(count / 1000)}K`;
  return String(Math.round(count));
}

/**
 * Map a ModelFactSource discriminant onto its i18n key under uiSettingsDetail.
 * @param {string|null|undefined} source 'endpoint' | 'catalog' | anything else.
 * @returns {string|null} i18n key name, or null when no tag should render.
 */
export function factSourceLabelKey(source) {
  if (source === 'endpoint') return 'factSourceEndpoint';
  if (source === 'catalog') return 'factSourceCatalog';
  return null;
}

const normalizeBaseUrlKey = baseUrl => {
  // while + endsWith instead of a /\/+$/ regex: sonarjs/super-linear-regex
  // flags the trailing-slash strip and the loop is just as clear.
  let text = String(baseUrl || '').trim();
  while (text.endsWith('/')) text = text.slice(0, -1);
  return text;
};

/**
 * Dedup key for one model entry: (model id + base_url), case-insensitive on
 * the id, trailing-slash-insensitive on the URL (spec §2.6 batch dedup rule).
 * '\n' separates the parts so an id can never collide across base URLs.
 * @param {string} modelId Wire model id.
 * @param {string} baseUrl Endpoint base URL the model was found on / saved with.
 */
export function modelDuplicationKey(modelId, baseUrl) {
  return `${String(modelId || '').trim().toLowerCase()}\n${normalizeBaseUrlKey(baseUrl)}`;
}

/**
 * Key set of already-saved models, used to tag/disable picker rows and to
 * dedupe the "add all" batch.
 * @param {Array<{model?: string, id?: string, base_url?: string}>} savedModels SavedModel records.
 * @returns {Set<string>} Keys as produced by modelDuplicationKey.
 */
export function savedModelKeys(savedModels) {
  const keys = new Set();
  for (const model of Array.isArray(savedModels) ? savedModels : []) {
    if (!model) continue;
    const wireId = model.model || model.id;
    if (!wireId) continue;
    keys.add(modelDuplicationKey(wireId, model.base_url));
  }
  return keys;
}

/**
 * Normalize an `EndpointModelsListing` into render rows. Tolerates a missing
 * listing (returns []), legacy string-only model entries, and keeps the
 * optional per-fact source discriminants so rows can tag 端点/目录 origins.
 * Rows keep discovery order; `key` is a stable React key from the id.
 * @param {{status?: string, models?: Array<object|string>}|null} listing - Raw serde listing from the probe command.
 * @returns {Array<{key: string, id: string, title: string, contextWindowTokens: number|null, contextFrom: string, maxOutputTokens: number|null, outputFrom: string, loaded: boolean|null}>} Render-ready picker rows.
 */
export function normalizeDiscoveredRows(listing) {
  const entries = listing && Array.isArray(listing.models) ? listing.models : [];
  const rows = [];
  for (const entry of entries) {
    const model = entry && typeof entry === 'object' ? entry : { id: String(entry || '') };
    const id = String(model.id || '').trim();
    if (!id) continue;
    const contextValue = Number(model.context_window_tokens);
    const outputValue = Number(model.max_output_tokens);
    rows.push({
      key: `${id}:${rows.length}`,
      id,
      title: String(model.title || '').trim(),
      contextWindowTokens: Number.isFinite(contextValue) && contextValue > 0 ? contextValue : null,
      contextFrom: model.context_from === 'endpoint' || model.context_from === 'catalog' ? model.context_from : '',
      maxOutputTokens: Number.isFinite(outputValue) && outputValue > 0 ? outputValue : null,
      outputFrom: model.output_from === 'endpoint' || model.output_from === 'catalog' ? model.output_from : '',
      loaded: model.loaded === true || model.loaded === false ? model.loaded : null,
    });
  }
  return rows;
}

/**
 * Case-insensitive substring filter over the row id and display title.
 * Empty/whitespace query returns every row.
 * @param {Array<{id: string, title?: string}>} rows - Normalized picker rows.
 * @param {string} query - Raw filter input value.
 * @returns {Array<{id: string, title?: string}>} Filtered rows in discovery order.
 */
export function filterModelRows(rows, query) {
  const needle = String(query || '').trim().toLowerCase();
  if (!needle) return Array.isArray(rows) ? [...rows] : [];
  return (Array.isArray(rows) ? rows : []).filter(row =>
    row.id.toLowerCase().includes(needle)
    || String(row.title || '').toLowerCase().includes(needle));
}

/**
 * Split a discovered listing into the "add all" batch:
 * rows already saved (same model id + base_url) are skipped, duplicate ids
 * inside one listing collapse to the first occurrence, and the batch caps at
 * `limit` entries with a `truncated` flag for the confirm copy (spec §2.6).
 * @param {Array<{id: string}>} rows - Normalized picker rows.
 * @param {Set<string>} savedKeys - Keys from savedModelKeys.
 * @param {string} baseUrl - The base URL the batch would be saved with.
 * @param {number} [limit] - Batch cap (default ADD_ALL_BATCH_LIMIT).
 * @returns {{addable: Array, truncated: boolean, duplicates: number}} Batch plan; duplicates counts skipped rows.
 */
export function splitBatchAdd(rows, savedKeys, baseUrl, limit = ADD_ALL_BATCH_LIMIT) {
  const all = Array.isArray(rows) ? rows : [];
  const saved = savedKeys instanceof Set ? savedKeys : new Set();
  const seenInBatch = new Set();
  const unique = [];
  let duplicates = 0;
  for (const row of all) {
    if (!row || !row.id) continue;
    const key = modelDuplicationKey(row.id, baseUrl);
    if (saved.has(key) || seenInBatch.has(key)) {
      duplicates += 1;
      continue;
    }
    seenInBatch.add(key);
    unique.push(row);
  }
  return {
    addable: unique.slice(0, limit),
    truncated: unique.length > limit,
    duplicates,
  };
}
