// npm run test:ui — Settings: styles from writing samples (style_gen.js, style_gen.rs) and the strings Settings shows.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { samplesOf, fitSample, MAX_SAMPLES, MAX_SAMPLE_CHARS } from '../js/style_gen.js';
import { t } from '../js/i18n.js';
import { wholeIn, acceleratorLabel } from '../js/settings_values.js';

const read = (rel) => fs.readFileSync(new URL(rel, import.meta.url), 'utf8');

test('the samples sent are trimmed, the empty ones left out', () => {
  assert.deepEqual(samplesOf(['  a ', '', '\n ', 'b']), ['a', 'b']);
  assert.deepEqual(samplesOf([]), []);
});

test('a file is cut to the cap without splitting a character, and one that isn’t text is refused', () => {
  assert.deepEqual(fitSample('hello'), { text: 'hello', cut: false });
  const long = fitSample('é'.repeat(MAX_SAMPLE_CHARS + 10));
  assert.equal(long.cut, true);
  assert.equal([...long.text].length, MAX_SAMPLE_CHARS);
  const emoji = fitSample('😀'.repeat(MAX_SAMPLE_CHARS + 1));
  assert.equal([...emoji.text].length, MAX_SAMPLE_CHARS);
  assert.ok(!/[\ud800-\udbff]$/.test(emoji.text), 'no half of a surrogate pair at the end');
  assert.equal(fitSample('PK\u0003\u0004\u0000\u0000'), null);
});

test('the UI’s caps and the refusals it shows are the backend’s', () => {
  const rs = read('../../src-tauri/src/style_gen.rs');
  const cap = (name) => Number(rs.match(new RegExp(`const ${name}: \\w+ = ([\\d_]+);`))[1].replace(/_/g, ''));
  assert.equal(cap('MAX_SAMPLES'), MAX_SAMPLES);
  assert.equal(cap('MAX_SAMPLE_CHARS'), MAX_SAMPLE_CHARS);
  assert.ok(t('errk_style_too_many').includes(String(MAX_SAMPLES)));
  assert.ok(t('errk_style_too_long').includes(String(MAX_SAMPLE_CHARS)));
  assert.ok(t('errk_style_too_short').includes(String(cap('MIN_CHARS'))));
  assert.ok(t('errk_style_daily_cap').includes(String(cap('DAILY_RUNS'))));
});

test('number fields take whole numbers in their range only', () => {
  assert.equal(wholeIn('4096', 1024, 128000), 4096);
  assert.equal(wholeIn(' 600 ', 5, 600), 600);
  for (const bad of ['', '  ', '5.5', 'abc', '1023', '128001']) assert.equal(wholeIn(bad, 1024, 128000), null, bad);
  assert.equal(wholeIn('-1'), null);
  assert.equal(wholeIn('0'), 0);
});

test('a global shortcut is shown with this system’s key name', () => {
  assert.equal(acceleratorLabel('CommandOrControl+Alt+Space', 'Ctrl'), 'Ctrl+Alt+Space');
  assert.equal(acceleratorLabel('CmdOrCtrl+Shift+K', '⌘'), '⌘+Shift+K');
  assert.equal(acceleratorLabel('Alt+Space', '⌘'), 'Alt+Space');
  assert.equal(acceleratorLabel(undefined, 'Ctrl'), '');
});

test('every string Settings shows has an English and a Turkish text', () => {
  const src = read('../js/settings.js') + read('../js/style_gen.js');
  const named = [...src.matchAll(/\bt\('([a-z0-9_]+)'/g)].map((m) => m[1]).filter((k) => !k.endsWith('_'));
  const rs = read('../../src-tauri/src/style_gen.rs');
  const refusals = [...rs.matchAll(/coded\("([a-z_]+)"/g)].map((m) => `errk_${m[1]}`);
  const keys = new Set([...named, ...refusals, 'oai_key_db', 'oai_key_store', 'key_saved', 'key_removed']);
  const i18n = read('../js/i18n.js');
  const [en, tr] = i18n.slice(i18n.indexOf('const en = {')).split('const tr = {');
  for (const k of keys) {
    const has = new RegExp(`(^|[\\s{,])${k}:`, 'm');
    assert.ok(has.test(en), `English: ${k}`);
    assert.ok(has.test(tr), `Turkish: ${k}`);
  }
  assert.ok(refusals.length >= 9, 'the backend’s refusals were found');
});
