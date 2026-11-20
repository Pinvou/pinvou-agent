// The RRULE field grammar behind the schedule editor: parse a stored rule
// into its KEY=VALUE map, project it to editor state, and rebuild it after
// an edit. Extracted verbatim from ScheduledTasksView.jsx (#900 review P2:
// the save → detail time-edit chain needs real-path coverage), so the field
// map must mirror what the backend accepts — validate_product_rrule in the
// watcher and server.py's validate_rrule both trim keys and values (and
// match keys case-insensitively), and a stored `FREQ = CRON` must project
// to the CRON editor here instead of the weekly fallback.
import { cronExprWithTime, fixedCronTime } from './schedule-cron-expr.js';

// Bare code array: display labels are rebuilt per current language by the
// component (see weekdayOptions/hourlyIntervalOptions).
export const WEEKDAY_CODES = ['MO', 'TU', 'WE', 'TH', 'FR', 'SA', 'SU'];

export const normalizeScheduleDays = (value) => {
  const requested = new Set(
    (Array.isArray(value) ? value : String(value || '').split(','))
      .map(day => String(day || '').trim().toUpperCase())
      .filter(Boolean)
  );
  return WEEKDAY_CODES.filter(day => requested.has(day));
};

export function parseScheduleFields(rrule) {
  return String(rrule || '').split(';').reduce((result, part) => {
    const split = part.indexOf('=');
    if (split > 0) {
      // The backend trims around the separator (`FREQ = CRON` arrives from
      // MCP/chat as an engine-valid stored CRON rule), so the field map must
      // trim too: an orphaned `FREQ ` key falls into the weekly fallback,
      // whose time edit appends BYHOUR/BYMINUTE to a CRON rule the domain
      // refuses, and the bounced patch then wedges the pending-patch flush
      // on every later save/run/toggle (#900 review P2).
      const key = part.slice(0, split).trim().toUpperCase();
      if (key) result[key] = part.slice(split + 1).trim();
    }
    return result;
  }, {});
}

export function serializeScheduleFields(fields) {
  return Object.keys(fields).filter(key => fields[key] != null && fields[key] !== '')
    .map(key => `${key}=${fields[key]}`).join(';');
}

// The backend stores AT verbatim and also accepts RFC3339, so a stored
// rule may carry a UTC offset. Resolve it to the local wall clock — the
// same moment humanize_rrule shows — so the editor never displays a
// foreign wall time and time edits re-anchor locally instead of
// silently dropping the offset. Returns null when AT is unusable.
export function onceScheduleParts(rawAt) {
  const match = /^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2})(?::\d{2})?(Z|[+-]\d{2}:\d{2})?$/.exec(String(rawAt || ''));
  if (!match) return null;
  if (!match[3]) return { date: match[1], time: match[2] };
  const instant = new Date(match[0]);
  if (Number.isNaN(instant.getTime())) return null;
  const pad = part => String(part).padStart(2, '0');
  return {
    date: `${instant.getFullYear()}-${pad(instant.getMonth() + 1)}-${pad(instant.getDate())}`,
    time: `${pad(instant.getHours())}:${pad(instant.getMinutes())}`,
  };
}

export function scheduleEditorValue(rrule) {
  const fields = parseScheduleFields(rrule);
  if (fields.FREQ === 'ONCE') {
    // One-shot rules have no recurring fields; derive the editor state
    // from AT so the recurring fallback below cannot misread them.
    const once = onceScheduleParts(fields.AT);
    return {
      repeat: 'once',
      days: [],
      day: 'MO',
      interval: 1,
      time: once ? once.time : '',
      date: once ? once.date : '',
      hasTimeAnchor: true,
    };
  }
  if (fields.FREQ === 'CRON') {
    // Issue #821: a CRON rule gets its own editor mode instead of the
    // weekly fallback below, which silently rewrote the schedule to
    // FREQ=WEEKLY on any unrelated save.
    const expr = String(fields.EXPR || '').trim();
    const fixedTime = fixedCronTime(expr);
    return {
      repeat: 'cron',
      days: [],
      day: 'MO',
      interval: 1,
      time: fixedTime ? `${fixedTime.hour}:${fixedTime.minute}` : '',
      hasTimeAnchor: !!fixedTime,
      cronExpr: expr,
    };
  }
  const days = normalizeScheduleDays(fields.BYDAY);
  const hasTimeAnchor = fields.BYHOUR != null || fields.BYMINUTE != null;
  let repeat = 'workdays';
  if (fields.FREQ === 'HOURLY') repeat = 'hourly';
  else if (days.join(',') === 'MO,TU,WE,TH,FR,SA,SU') repeat = 'daily';
  else if (days.join(',') !== 'MO,TU,WE,TH,FR') repeat = 'weekly';
  return {
    repeat,
    days,
    day: days[0] || 'MO',
    interval: Number(fields.INTERVAL || 1),
    time: hasTimeAnchor
      ? `${String(fields.BYHOUR == null ? 0 : fields.BYHOUR).padStart(2, '0')}:${String(fields.BYMINUTE == null ? 0 : fields.BYMINUTE).padStart(2, '0')}`
      : '',
    hasTimeAnchor,
  };
}

// Issue #821: CRON edits stay inside the expression. Only an explicit
// repeat-mode switch converts away (handled by the caller); a time edit
// on a fixed-minute/hour expression rewrites exactly those two fields;
// everything else leaves the rule untouched (the weekly fallback in
// buildRrule used to silently destroy the schedule).
export function buildCronRrule(fields, key, value, currentRrule) {
  if (key === 'expr') {
    fields.EXPR = String(value || '').replaceAll(/[;=]/g, ' ');
    return serializeScheduleFields(fields);
  }
  if (key === 'time') {
    fields.EXPR = cronExprWithTime(String(fields.EXPR || ''), value);
    return serializeScheduleFields(fields);
  }
  return currentRrule;
}

// ONCE rules carry their own AT shape: rewrite AT instead of appending
// BYHOUR/BYMINUTE, which the ONCE parser rejects; bail out unchanged
// when AT cannot be resolved to a local date, e.g. a malformed RFC3339
// offset. Returns null when the edit is an explicit conversion to a
// recurring rule and must fall through to the builders below, which
// reuse the AT time.
export function buildOnceRrule(fields, key, value, currentRrule) {
  if (key === 'time') {
    const once = onceScheduleParts(fields.AT);
    if (!once) return currentRrule;
    const [onceHour, onceMinute] = String(value || '').split(':');
    fields.AT = `${once.date}T${String(Number(onceHour || 0)).padStart(2, '0')}:${String(Number(onceMinute || 0)).padStart(2, '0')}`;
    return serializeScheduleFields(fields);
  }
  if (key === 'repeat' && value === 'once') return currentRrule;
  return null;
}

export function buildRrule(currentRrule, key, value) {
  const fields = parseScheduleFields(currentRrule);
  if (fields.FREQ === 'ONCE') {
    const onceRrule = buildOnceRrule(fields, key, value, currentRrule);
    if (onceRrule !== null) return onceRrule;
    // Any other key is an explicit conversion to a recurring rule and
    // falls through to the builders below, which reuse the AT time.
  }
  if (fields.FREQ === 'CRON' && key !== 'repeat') {
    return buildCronRrule(fields, key, value, currentRrule);
  }
  const previousEditor = scheduleEditorValue(currentRrule);
  const editor = {...previousEditor, [key]: value,};
  const [hour, minute] = String(editor.time || '08:00').split(':');
  if (key === 'repeat' && editor.repeat === 'cron') {
    // Converting into CRON keeps the editor time and lands on a
    // monthly-by-default expression the user can refine in place.
    return `FREQ=CRON;EXPR=${Number(minute || 0)} ${Number(hour || 0)} 1 * *`;
  }
  if (key === 'time') {
    fields.BYHOUR = String(Number(hour || 0));
    fields.BYMINUTE = String(Number(minute || 0));
    return serializeScheduleFields(fields);
  }
  if (key === 'days') {
    const days = normalizeScheduleDays(value);
    if (!days.length) return currentRrule;
    fields.FREQ = 'WEEKLY';
    fields.BYDAY = days.join(',');
    fields.BYHOUR = String(Number(hour || 0));
    fields.BYMINUTE = String(Number(minute || 0));
    return serializeScheduleFields(fields);
  }
  if (editor.repeat === 'hourly') {
    const interval = previousEditor.repeat === 'hourly' ? editor.interval : 1;
    // BYDAY rule (full week or a subset of days) aligns with the Rust humanize "Mon、Tue every N hours";
    // omitted when no day is selected (pure hourly), which reads the same as every day.
    const days = normalizeScheduleDays(editor.days);
    const byday = days.length ? `;BYDAY=${days.join(',')}` : '';
    const anchor = previousEditor.hasTimeAnchor
      ? `;BYHOUR=${Number(hour || 0)};BYMINUTE=${Number(minute || 0)}`
      : '';
    return `FREQ=HOURLY;INTERVAL=${Math.max(1, interval || 1)}${byday}${anchor}`;
  }
  if (key === 'repeat' && editor.repeat === 'weekly') {
    const previousDays = normalizeScheduleDays(previousEditor.days);
    const presetDays = previousDays.join(',');
    const inheritedPreset = presetDays === 'MO,TU,WE,TH,FR' || presetDays === 'MO,TU,WE,TH,FR,SA,SU';
    const weeklyDays = inheritedPreset ? [previousEditor.day || 'MO'] : (previousDays.length ? previousDays : [previousEditor.day || 'MO']);
    return `FREQ=WEEKLY;BYDAY=${weeklyDays.join(',')};BYHOUR=${Number(hour || 0)};BYMINUTE=${Number(minute || 0)}`;
  }
  const days = editor.repeat === 'daily' ? 'MO,TU,WE,TH,FR,SA,SU'
    : (editor.repeat === 'workdays' ? 'MO,TU,WE,TH,FR'
      : (editor.days.join(',') || editor.day || 'MO'));
  return `FREQ=WEEKLY;BYDAY=${days};BYHOUR=${Number(hour || 0)};BYMINUTE=${Number(minute || 0)}`;
}
