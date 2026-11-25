// npm run test:ui — the usage-limit banner: its sentences, and when a
// dismissed one comes back.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { setLanguage } from '../js/i18n.js';
import { bannerText, bannerKey, limitPhrase } from '../js/usage_limits.js';

test('the usage banner reads as a sentence for every limit, in either language', () => {
  setLanguage('en');
  assert.equal(bannerText('near', { key: 'seven_day' }), 'You’re close to your weekly limit for all models.');
  assert.equal(bannerText('near', { key: 'five_hour' }), 'You’re close to your 5-hour limit.');
  assert.match(bannerText('exceeded', { key: 'seven_day_opus' }), /^You’ve reached your weekly Opus limit\. /);
  assert.equal(bannerText('near', { key: 'seven_day_oauth_apps', label: null }), 'You’re close to a usage limit (seven day oauth apps).');
  assert.equal(limitPhrase({ key: 'overage' }), 'your extra usage limit');
  setLanguage('tr');
  assert.equal(bannerText('near', { key: 'seven_day' }), 'Tüm modeller için haftalık sınırınız dolmak üzere.');
  assert.equal(bannerText('near', { key: 'five_hour' }), '5 saatlik sınırınız dolmak üzere.');
  assert.match(bannerText('exceeded', { key: 'weekly' }), /^Tüm modeller için haftalık sınırınız doldu\. /);
  setLanguage('en');
});

test('a dismissed banner comes back at a new level, limit or window, not for the same one restated', () => {
  const x = { key: 'five_hour', resetsAt: Date.UTC(2026, 10, 22, 17, 0, 0) };
  assert.equal(bannerKey('near', x), bannerKey('near', { ...x, resetsAt: x.resetsAt + 4000 }));
  assert.notEqual(bannerKey('near', x), bannerKey('exceeded', x));
  assert.notEqual(bannerKey('near', x), bannerKey('near', { ...x, key: 'seven_day' }));
  assert.notEqual(bannerKey('near', x), bannerKey('near', { ...x, resetsAt: x.resetsAt + 5 * 3600e3 }));
});
