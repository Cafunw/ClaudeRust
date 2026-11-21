// npm run test:ui
import test from 'node:test';
import assert from 'node:assert/strict';
import { t } from '../js/i18n.js';

test('t fills every occurrence of a placeholder', () => {
  const s = t('hist_explain_p', { sha: 'abc1234', subject: 'Fix it' });
  assert.equal(s, 'Explain commit abc1234 ("Fix it"): run `git show abc1234` and describe what it changes, why, and anything risky about it.');
});

test('t takes values as they are', () => {
  const s = t('hist_explain_p', { sha: 'abc1234', subject: "Escape $& and $' in {sha}" });
  assert.ok(s.startsWith('Explain commit abc1234 ("Escape $& and $\' in {sha}"): run `git show abc1234`'), s);
});

test('t keeps unknown placeholders and works without vars', () => {
  assert.equal(t('hist_explain_p', { subject: 'x' }).includes('{sha}'), true);
  assert.equal(t('no_such_key'), 'no_such_key');
  assert.equal(t('no_such_key {n}', { n: 0 }), 'no_such_key 0');
});
