// Issue #821: the CRON product subset shared by the schedule editor —
// five whitespace-separated fields, a fixed minute value 0-59, standard
// cron vocabulary in the other fields. Structural mirror of
// validate_cron_expr in src-tauri's features/scheduled/creation_requests.rs
// and server.py's validate_cron_expr (B9: every product surface must accept
// and reject the same shapes); the engine's ParsedCronExpr stays the
// semantic authority when the rule is saved.

/**
 * Splits a CRON EXPR into its whitespace-separated fields ([] when blank).
 */
export function parseCronExprFields(expr) {
  return String(expr || '').trim().split(/\s+/).filter(Boolean);
}

/**
 * True when the expression is outside the product subset: wrong field
 * count, a non-fixed or out-of-range minute, a field outside the cron
 * charset, or an empty list item.
 */
export function cronExprInvalid(expr) {
  const fields = parseCronExprFields(expr);
  if (fields.length !== 5) return true;
  if (!/^\d{1,2}$/.test(fields[0]) || Number(fields[0]) > 59) return true;
  return fields.slice(1).some(field =>
    !/^[0-9A-Za-z*,-/]+$/.test(field) || field.split(',').some(item => !item));
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
    || !/^\d{1,2}$/.test(fields[0]) || Number(fields[0]) > 59
    || !/^\d{1,2}$/.test(fields[1]) || Number(fields[1]) > 23) return null;
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
