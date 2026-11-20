// Issue #821: the CRON product subset shared by the schedule editor —
// five whitespace-separated fields, a fixed minute value 0-59, standard
// cron vocabulary in the other fields. Structural mirror of validate_cron_expr
// in src-tauri's features/scheduled/creation_requests.rs and server.py's
// validate_cron_expr (B9: every product surface must accept and reject the
// same shapes), on top of the engine's deterministic semantics (ParsedCronExpr
// in CodeWhale's automation_manager.rs): field bounds, range order, step and
// name validity, and the impossible day/month check. The detail editor
// persists every patch, so an expression the engine would reject must be
// caught here and held as a local draft — a bounced save wedges the panel's
// pending-patch flush.

// Exactly the Unicode White_Space set, what the Rust watcher's
// split_whitespace splits; JS \s additionally matches U+FEFF, which neither
// backend treats as whitespace.
const CRON_WS_CLASS = '[\\t\\n\\v\\f\\r \u0085\u00A0\u1680\u2000-\u200A\u2028\u2029\u202F\u205F\u3000]';
const CRON_WS_TRIM = new RegExp(`^${CRON_WS_CLASS}+|${CRON_WS_CLASS}+$`, 'g');
const CRON_WS_SPLIT = new RegExp(`${CRON_WS_CLASS}+`);

// Engine cron vocabulary: per-field [min, max], names only in month
// (1-indexed) and day-of-week (0-indexed, 7 folds to 0). Leap-year lengths
// for the impossible-date check, which the engine runs against 2024.
const CRON_MONTH_NAMES = ['JAN', 'FEB', 'MAR', 'APR', 'MAY', 'JUN', 'JUL', 'AUG', 'SEP', 'OCT', 'NOV', 'DEC'];
const CRON_DOW_NAMES = ['SUN', 'MON', 'TUE', 'WED', 'THU', 'FRI', 'SAT'];
const CRON_FIELD_BOUNDS = [[0, 59], [0, 23], [1, 31], [1, 12, CRON_MONTH_NAMES, 1], [0, 7, CRON_DOW_NAMES, 0]];
const CRON_LEAP_DAYS = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const CRON_U32_MAX = 4294967295;

/**
 * Splits a CRON EXPR into its whitespace-separated fields ([] when blank).
 */
export function parseCronExprFields(expr) {
  return String(expr || '').replaceAll(CRON_WS_TRIM, '').split(CRON_WS_SPLIT).filter(Boolean);
}

function cronAtomValue(token, index) {
  const [min, max, names, nameOffset] = CRON_FIELD_BOUNDS[index];
  if (/^\d+$/.test(token)) {
    const value = Number(token);
    return value >= min && value <= max ? value : null;
  }
  if (!names) return null;
  const nameIndex = names.indexOf(token.toUpperCase());
  return nameIndex >= 0 ? nameIndex + nameOffset : null;
}

// Parses one comma item into [start, end, step] (null when invalid):
// `*`, a-b with start <= end, a bare p/s extends to the field max.
function cronItemRange(raw, index) {
  const [min, max] = CRON_FIELD_BOUNDS[index];
  const slash = raw.indexOf('/');
  const step = slash >= 0 ? raw.slice(slash + 1) : '1';
  if (!/^\d+$/.test(step) || Number(step) < 1 || Number(step) > CRON_U32_MAX) return null;
  const base = slash >= 0 ? raw.slice(0, slash) : raw;
  let start;
  let end;
  if (base === '*') {
    [start, end] = [min, max];
  } else {
    const dash = base.indexOf('-');
    if (dash >= 0) {
      start = cronAtomValue(base.slice(0, dash), index);
      end = cronAtomValue(base.slice(dash + 1), index);
      if (start === null || end === null || start > end) return null;
    } else {
      start = cronAtomValue(base, index);
      if (start === null) return null;
      end = slash >= 0 ? max : start;
    }
  }
  return [start, end, Number(step)];
}

/**
 * Expands one cron field into its values (null when invalid) — the
 * CronField::parse grammar: `*`, lists, ranges, p/s steps.
 */
function cronFieldValues(field, index) {
  const values = [];
  for (const raw of field.split(',')) {
    if (!raw) return null;
    const range = cronItemRange(raw, index);
    if (!range) return null;
    const [start, end, step] = range;
    for (let value = start; value <= end; value += step) {
      if (!values.includes(value)) values.push(value);
    }
  }
  return values;
}

/**
 * True when the expression is outside the product subset or would be
 * rejected by the engine on save: wrong field count, a non-fixed or
 * out-of-range minute, a field outside the cron charset, an empty list
 * item, an out-of-bounds value or malformed range/step, an unknown name,
 * or a day-of-month no listed month can host.
 */
export function cronExprInvalid(expr) {
  const fields = parseCronExprFields(expr);
  if (fields.length !== 5) return true;
  if (!/^\d+$/.test(fields[0]) || Number(fields[0]) > 59) return true;
  const expanded = [];
  for (let index = 1; index < 5; index += 1) {
    if (!/^[0-9A-Za-z*,\-/]+$/.test(fields[index])) return true;
    const values = cronFieldValues(fields[index], index);
    if (!values) return true;
    expanded.push(values);
  }
  // validate_date_space: a restricted day-of-month must fit at least one
  // listed month; a wildcard month already expanded to all twelve.
  const domMin = Math.min(...expanded[1]);
  const monthMaxDays = Math.max(...expanded[2].map(month => CRON_LEAP_DAYS[month - 1]));
  return fields[2] !== '*' && domMin > monthMaxDays;
}

/**
 * The { hour, minute } pair for expressions with fixed minute and hour
 * fields (the canonical monthly shape), null for everything else — the
 * editor only wires the familiar time wheel to expressions it can rewrite
 * without destroying.
 */
export function fixedCronTime(expr) {
  const fields = parseCronExprFields(expr);
  if (fields.length !== 5
    || !/^\d+$/.test(fields[0]) || Number(fields[0]) > 59
    || !/^\d+$/.test(fields[1]) || Number(fields[1]) > 23) return null;
  return {
    minute: String(Number(fields[0])).padStart(2, '0'),
    hour: String(Number(fields[1])).padStart(2, '0'),
  };
}

/**
 * Rewrites the minute/hour fields of a fixed-time expression from a
 * "HH:MM" wheel value; returns the expression unchanged when it is not a
 * fixed-time expression (the editor hides the wheel in that case).
 */
export function cronExprWithTime(expr, time) {
  if (!fixedCronTime(String(expr || ''))) return String(expr || '');
  const cronFields = parseCronExprFields(String(expr || ''));
  const [newHour, newMinute] = String(time || '').split(':');
  cronFields[1] = String(Number(newHour || 0));
  cronFields[0] = String(Number(newMinute || 0));
  return cronFields.join(' ');
}
