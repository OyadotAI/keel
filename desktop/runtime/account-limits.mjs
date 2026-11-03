// Normalize provider-reported account windows without treating token or spend totals as limits.
const THRESHOLD = 70;
export const MAX_READING_AGE_MS = 5 * 60 * 1000;

function percentage(value) {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= 100 ? value : undefined;
}

function window(id, label, value, reset, scope, observedAt) {
  const percent = percentage(value);
  if (percent === undefined) return undefined;
  return { id, label, percent, reset: reset === null ? null : Number.isFinite(reset) ? reset : undefined, scope, observedAt };
}

/** Use full Codex snapshots only: sparse notifications invalidate a reading instead of refreshing it. */
export function codexWindows(response, observedAt) {
  const buckets = response?.rateLimitsByLimitId;
  const snapshots = buckets && typeof buckets === 'object' && Object.keys(buckets).length
    ? Object.entries(buckets) : [['codex', response?.rateLimits]];
  return snapshots.flatMap(([key, snapshot]) => {
    if (!snapshot || typeof snapshot !== 'object') return [];
    const slug = snapshot.normalModelSlug;
    // Unknown aliases remain visible but cannot be assumed to apply to this model.
    const scope = slug ? { model: slug } : key === 'codex' ? { general: true } : { unknown: true };
    return ['primary', 'secondary'].flatMap((part) => {
      const source = snapshot[part];
      const result = source && window(`${key}:${part}`, snapshot.limitName || `${key} ${part}`,
        source.usedPercent, source.resetsAt == null ? null : source.resetsAt * 1000, scope, observedAt);
      return result ? [result] : [];
    });
  });
}

/** Claude's structured usage response uses percentages, unlike its unqualified rate-limit events. */
export function claudeWindows(report, observedAt) {
  const limits = report?.rate_limits;
  if (!report?.rate_limits_available || !limits || typeof limits !== 'object') return [];
  const entries = [
    ['five_hour', limits.five_hour, { general: true }],
    ['seven_day', limits.seven_day, { general: true }],
    ['seven_day_opus', limits.seven_day_opus, { unknown: true }],
    ['seven_day_sonnet', limits.seven_day_sonnet, { unknown: true }],
    ...(Array.isArray(limits.model_scoped) ? limits.model_scoped.map((row) =>
      [`model:${row?.display_name}`, row, { model: row?.display_name }]) : []),
  ];
  return entries.flatMap(([id, row, scope]) => {
    if (!row || typeof row !== 'object') return [];
    const parsed = row.resets_at ? Date.parse(row.resets_at) : null;
    const result = window(id, id, row.utilization, parsed, scope, observedAt);
    return result ? [result] : [];
  });
}

/** A suggestion needs a fresh applicable reading and a known account identity. */
export function overThreshold(reading, { account, model, now }) {
  if (!account || !Number.isFinite(now) || !Number.isFinite(reading.observedAt)
      || now < reading.observedAt || now - reading.observedAt > MAX_READING_AGE_MS
      || reading.reset === undefined || (reading.reset !== null && reading.reset <= now)) return false;
  const applicable = reading.scope.general || (model && reading.scope.model === model);
  return Boolean(applicable && reading.percent > THRESHOLD);
}
