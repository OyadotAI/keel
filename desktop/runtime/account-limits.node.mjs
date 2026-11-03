// Provider-shaped fixtures cover account usage without opening an account or starting a turn.
import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { claudeWindows, codexWindows, overThreshold, MAX_READING_AGE_MS } from './account-limits.mjs';

test('only fresh applicable values strictly above 70 trigger', () => {
  const now = 1_000_000;
  const rows = claudeWindows({ rate_limits_available: true, rate_limits: {
    five_hour: { utilization: 70, resets_at: null },
    seven_day: { utilization: 70.1, resets_at: null },
    model_scoped: [{ display_name: 'other', utilization: 90, resets_at: null }],
  } }, now);
  const context = { account: 'opaque', model: 'current', now };
  assert.equal(overThreshold(rows[0], context), false);
  assert.equal(overThreshold(rows[1], context), true);
  assert.equal(overThreshold(rows[2], context), false);
  assert.equal(overThreshold(rows[1], { ...context, account: undefined }), false);
  assert.equal(overThreshold(rows[1], { ...context, now: now + MAX_READING_AGE_MS + 1 }), false);
});

test('Codex uses full multi-bucket data and refuses invalid or expired readings', () => {
  const now = 1_000_000;
  const rows = codexWindows({ rateLimits: { primary: { usedPercent: 99 } }, rateLimitsByLimitId: {
    codex: { primary: { usedPercent: 80, resetsAt: 999 }, secondary: { usedPercent: null } },
    alias: { primary: { usedPercent: 90 }, normalModelSlug: 'other' },
  } }, now);
  assert.equal(rows.length, 2);
  assert.equal(overThreshold(rows[0], { account: 'opaque', model: 'current', now }), false);
  assert.equal(overThreshold(rows[1], { account: 'opaque', model: 'current', now }), false);
  assert.deepEqual(claudeWindows({ rate_limits: null }, now), []);
});
