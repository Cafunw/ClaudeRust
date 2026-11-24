// npm run test:ui — round 9 UI items: word-based, Turkish-aware search
// filters, the Learning style, the version arrows' label and visibility, and
// the Turkish wording the evaluator named.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setLanguage, t } from '../js/i18n.js';
import { fold, hasWords } from '../js/util.js';

test('search filters match every word, in any order, Turkish-aware and without accents', () => {
  assert.equal(fold('İSTANBUL Işık ışık ÇİÇEK Şeker Ünlü'), 'istanbul isik isik cicek seker unlu');
  assert.ok(hasWords('Sidebar filler chat number 1 about tests', 'filler 1'));
  assert.ok(hasWords('Sidebar filler chat number 1 about tests', '1 FILLER'));
  assert.ok(!hasWords('Sidebar filler chat number 1 about tests', 'filler 2'));
  assert.ok(hasWords('Işık ayarları', 'isik AYAR'));
  assert.ok(hasWords('çiçek bahçesi', 'CICEK bahce'));
  assert.ok(hasWords('anything', '  '), 'an empty query matches');
  assert.ok(hasWords(undefined, ''));
});

test('the Learning style is offered next to the others, labelled in both languages', async () => {
  const { PRESET_STYLES } = await import('../js/composer.js');
  assert.deepEqual(PRESET_STYLES, ['normal', 'concise', 'explanatory', 'formal', 'learning']);
  setLanguage('en');
  assert.equal(t('style_learning'), 'Learning');
  setLanguage('tr');
  assert.equal(t('style_learning'), 'Öğrenme');
  setLanguage('en');
});

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

test('the Turkish wording the evaluator named reads naturally', () => {
  setLanguage('tr');
  assert.equal(t('from_server', { server: 'evalsrv' }), 'evalsrv sunucusundan');
  // "commit" is the Turkish block's word throughout: the same in the singular and the plural
  assert.equal(t('hist_commit_one'), `1 ${t('hist_commits')}`);
  assert.equal(t('of_capacity', { pct: 40 }), 'Kullanılan proje kapasitesi: %40');
  for (const k of ['welcome_title', 'project_name', 'nickname', 'preferences_help', 'click_to_open', 'mode_plan_d']) {
    assert.ok(!/(geldin|çalışıyorsun|\bsana\b|Tercihlerin|tıkla|planla)$/.test(t(k).replace(/[?.!]$/, '')), `${k}: ${t(k)}`);
  }
  setLanguage('en');
  assert.equal(t('from_server', { server: 'evalsrv' }), 'from evalsrv');
  assert.equal(t('of_capacity', { pct: 40 }), '40% of project capacity used');
});
