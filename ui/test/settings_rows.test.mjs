// npm run test:ui — Settings rows look the same on every page: the name (and its note) on the
// left, one control at the row's end, a dropdown's value by its caret.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { source } from './source.mjs';

const CSS = source('../css/app.css');

test('a dropdown in a row shows its value at the row’s end, however long its longest option is', () => {
  assert.match(CSS, /\n\.setting-row \.select \{ width: auto; border-color: transparent; background: transparent; padding-right: 8px; text-align-last: right; \}/);
});

test('Memory’s scope picker is a row like the others, not a bordered box of its own', () => {
  const src = source('../js/memory_page.js');
  assert.match(src, /const head = h\(`<div class="setting-row" style="border-bottom:none"><div class="txt"><div class="t">\$\{esc\(t\('mem_scope'\)\)\}<\/div><\/div><\/div>`\);/);
  assert.match(src, /head\.firstChild\.append\(when, excl\);\s*\n\s*head\.append\(sel\);/);
  assert.doesNotMatch(src, /min-width:min\(200px,100%\)/, 'no fixed width that pushes its value away from the caret');
});

test('a row’s note sits inside the row, not pulled up over its border', () => {
  const src = source('../js/settings.js');
  assert.match(src, /engine\.querySelector\('\.txt'\)\.appendChild\(h\(`<div class="d">\$\{esc\(t\('acct_chat_note'\)\)\}<\/div>`\)\);/);
  assert.doesNotMatch(src, /acct_chat_note'\)\)\}<\/p>/);
});

test('in a narrow Settings the control sits under its text from the left, a dropdown’s value too', () => {
  const narrow = CSS.slice(CSS.indexOf('@media (max-width: 560px) {\n  .settings-nav { width: 52px;'));
  const block = narrow.slice(0, narrow.indexOf('\n}\n'));
  assert.match(block, /\.setting-row \.txt \{ flex-basis: 100%; \}/);
  assert.match(block, /\.setting-row \.select \{ text-align-last: left; padding-left: 0; \}/);
});
