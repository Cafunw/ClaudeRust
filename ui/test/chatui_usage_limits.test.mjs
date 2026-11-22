// npm run test:ui — plan usage limits (ui/js/usage_limits.js): the payload in any shape, and how it reads.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { setLanguage } from '../js/i18n.js';
import { normalizeLimits, limitLabel, untilText, resetText, headline } from '../js/usage_limits.js';

const NOW = Date.UTC(2026, 10, 22, 12, 0, 0);

test('a list of limits with fractions and reset times in seconds', () => {
  const s = normalizeLimits({ limits: [
    { kind: 'five_hour', utilization: 0.42, resets_at: NOW / 1000 + 3600 },
    { kind: 'seven_day', used_pct: 12, resets_at: new Date(NOW + 86400000).toISOString() },
  ] });
  assert.equal(s.level, 'ok');
  assert.deepEqual(s.items.map((x) => [x.key, x.pct, x.resetsAt]), [['five_hour', 42, NOW + 3600000], ['seven_day', 12, NOW + 86400000]]);
});

test('an object keyed by limit, and the flags the backend sets win', () => {
  const s = normalizeLimits({ five_hour: { utilization: 0.5 }, weekly: { utilization: 0.3 }, near: true, exceeded: false });
  assert.equal(s.level, 'near');
  assert.deepEqual(s.items.map((x) => x.key), ['five_hour', 'weekly']);
  assert.equal(normalizeLimits({ five_hour: { utilization: 0.99 }, exceeded: true }).level, 'exceeded');
});

test('the CLI rate_limit_info as it is: status says how close it is', () => {
  const warn = normalizeLimits({ rate_limit_info: { status: 'allowed_warning', rateLimitType: 'five_hour', resetsAt: NOW / 1000 } });
  assert.equal(warn.level, 'near');
  assert.equal(warn.items[0].pct, null);
  const over = normalizeLimits({ status: 'rejected', rateLimitType: 'seven_day', resetsAt: NOW / 1000 });
  assert.equal(over.level, 'exceeded');
  assert.equal(normalizeLimits([{ type: 'five_hour', utilization: 0.81 }]).level, 'near', '80% and up is near');
  assert.equal(normalizeLimits([{ type: 'five_hour', utilization: 1 }]).level, 'exceeded');
  // a fraction past 1 is past the limit, not 1.2 %
  assert.equal(normalizeLimits([{ type: 'five_hour', utilization: 1.2 }]).items[0].pct, 100);
  assert.equal(normalizeLimits([{ type: 'five_hour', utilization: 'x' }, { type: 'seven_day', utilization: null, used_pct: {} }]).level, 'ok', 'garbage reads as unknown');
});

test('the usage_limits payload of usage.rs', () => {
  const p = { status: 'allowed_warning', limiting: 'seven_day', resets_at: NOW + 3600000, near: true, exceeded: false, limits: [
    { key: 'five_hour', utilization: 0.3, resets_at: NOW + 7200000, status: 'allowed', threshold: null },
    { key: 'seven_day', utilization: 0.91, resets_at: NOW + 3600000, status: 'allowed_warning', threshold: 0.9 },
  ], overage: { status: 'rejected' } };
  const s = normalizeLimits(p);
  assert.equal(s.level, 'near');
  assert.deepEqual(s.items.map((x) => [x.key, Math.round(x.pct), x.status]), [['five_hour', 30, 'allowed'], ['seven_day', 91, 'allowed_warning']]);
  assert.equal(headline(s).key, 'seven_day');
  assert.equal(normalizeLimits({ ...p, near: false, exceeded: true, status: 'rejected' }).level, 'exceeded');
  assert.equal(normalizeLimits(null).items.length, 0, 'before the first answer on the account');
});

test('nothing usable is no limits', () => {
  for (const p of [null, undefined, 'x', {}, { limits: [] }, [null, 3]]) {
    const s = normalizeLimits(p);
    assert.deepEqual([s.items, s.level], [[], 'ok']);
  }
});

test('labels, the headline and the reset line', () => {
  setLanguage('en');
  assert.equal(limitLabel({ key: 'five_hour' }), '5-hour limit');
  assert.equal(limitLabel({ key: 'seven_day' }), 'Weekly · all models');
  assert.equal(limitLabel({ key: 'some_new_one' }), 'some new one');
  assert.equal(limitLabel({ key: 'x', label: 'Own' }), 'Own');
  const s = normalizeLimits([{ type: 'five_hour', utilization: 0.2 }, { type: 'seven_day', utilization: 0.7 }, { type: 'seven_day_opus', status: 'rejected' }]);
  assert.equal(headline(s).key, 'seven_day_opus', 'a reached limit first');
  assert.equal(headline(normalizeLimits([{ type: 'five_hour', utilization: 0.2 }, { type: 'seven_day', utilization: 0.7 }])).key, 'seven_day');
  assert.equal(untilText(NOW + 35 * 60000, NOW), '35 min');
  assert.equal(untilText(NOW + 125 * 60000, NOW), '2 h 5 min');
  assert.equal(untilText(NOW + 120 * 60000, NOW), '2 h');
  assert.equal(untilText(NOW + 76 * 3600000, NOW), '3 d 4 h');
  assert.equal(untilText(NOW + 10, NOW), '1 min');
  assert.match(resetText(NOW + 125 * 60000, NOW, 'en'), /^Resets in 2 h 5 min \(.+\)$/);
  assert.equal(resetText(null, NOW), '');
  assert.equal(resetText(NOW - 1, NOW), 'Resets any moment now');
  setLanguage('tr');
  assert.equal(untilText(NOW + 125 * 60000, NOW), '2 sa 5 dk');
  assert.equal(limitLabel({ key: 'five_hour' }), '5 saatlik sınır');
  setLanguage('en');
});
