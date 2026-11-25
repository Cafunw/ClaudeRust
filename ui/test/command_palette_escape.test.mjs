// npm run test:ui — the command palette (Ctrl+K) closes on Escape wherever its focus is (the field,
// a filter, a result) and gives the focus back to where it was.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { source } from './source.mjs';

const SRC = source('../js/search.js');
const open = SRC.slice(SRC.indexOf('export function openSearch()'), SRC.indexOf('/** Match score'));

test('Escape is taken on the whole palette, not on its text field only', () => {
  assert.match(open, /scrim\.addEventListener\('keydown', \(e\) => \{ if \(e\.key === 'Escape' && !e\.defaultPrevented\) \{ e\.preventDefault\(\); close\(\); \} \}\);/);
  // the field keeps only its own keys: the arrows and Enter
  const field = open.slice(open.indexOf("input.addEventListener('keydown'"), open.indexOf("scrim.addEventListener('mousedown'"));
  assert.doesNotMatch(field, /Escape/);
  assert.match(field, /ArrowDown/);
  // a filter is a button inside the scrim, and so is a result: both reach that listener
  assert.match(open, /<div class="ptabs" role="group">\$\{TABS\.map/);
  assert.ok(open.indexOf('const scrim = h(') < open.indexOf("scrim.addEventListener('keydown'"));
});

test('closing gives the focus back to the control that had it', () => {
  assert.ok(open.indexOf('const opener = document.activeElement;') < open.indexOf('document.body.appendChild(scrim);'), 'taken before the palette takes the focus');
  const close = open.slice(open.indexOf('const close = () => {'), open.indexOf('// "@ for files"'));
  assert.match(close, /scrim\.remove\(\);/);
  assert.match(close, /if \(opener && opener\.isConnected && opener !== document\.body\) opener\.focus\(\{ preventScroll: true \}\);/);
});
