// npm run test:ui — the arrows between versions (branches) of a message.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setLanguage, t } from '../js/i18n.js';

test('the version arrows are labelled "Version 2 of 3" and stay visible without hover', () => {
  setLanguage('en');
  assert.equal(t('a11y_version_of', { n: 2, total: 3 }), 'Version 2 of 3');
  setLanguage('tr');
  assert.equal(t('a11y_version_of', { n: 2, total: 3 }), 'Sürüm 2 / 3');
  setLanguage('en');
  const css = readFileSync(new URL('../css/app.css', import.meta.url), 'utf8');
  const rule = css.split('}').find((r) => r.includes('.turn .turn-actions > .branch-nav'));
  assert.ok(rule && /opacity:\s*1\b/.test(rule), 'the arrows are in the always-visible rule');
  const chat = readFileSync(new URL('../js/chat.js', import.meta.url), 'utf8');
  assert.match(chat, /class="branch-nav" role="group" aria-label="\$\{esc\(t\('a11y_version_of'/);
});
