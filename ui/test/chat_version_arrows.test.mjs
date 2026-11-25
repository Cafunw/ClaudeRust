// npm run test:ui — the arrows between versions (branches) of a message.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { setLanguage, t } from '../js/i18n.js';
import { source } from './source.mjs';

test('the version arrows are labelled "Version 2 of 3" and stay visible without hover', () => {
  setLanguage('en');
  assert.equal(t('a11y_version_of', { n: 2, total: 3 }), 'Version 2 of 3');
  setLanguage('tr');
  assert.equal(t('a11y_version_of', { n: 2, total: 3 }), 'Sürüm 2 / 3');
  setLanguage('en');
  const css = source('../css/app.css');
  const rule = css.split('}').find((r) => r.includes('.turn .turn-actions > .branch-nav'));
  assert.ok(rule && /opacity:\s*1\b/.test(rule), 'the arrows are in the always-visible rule');
  const chat = source('../js/chat.js');
  assert.match(chat, /class="branch-nav" role="group" aria-label="\$\{esc\(t\('a11y_version_of'/);
});
