// npm run test:ui — keyboard dead ends found in the UI sweep: Tab from a <details>' summary inside a
// dialog (a server's error "Details" in Settings → Developer) started the dialog over, so nothing after
// it could be reached; Tab with a menu open left the menu open and wandered on into it from the end
// of the page; the message field had only its placeholder for a name.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { source } from './source.mjs';
import { setLanguage, t } from '../js/i18n.js';

test('a summary (and an editable region) is a stop of the dialog trap and the page wrap', () => {
  const a11y = source('../js/a11y.js');
  const sel = a11y.match(/const FOCUSABLE = '([^']+)';/)[1];
  for (const s of ['summary', '[contenteditable]:not([contenteditable="false"])', 'iframe']) assert.ok(sel.split(', ').includes(s), s);
  // the trap and the wrap both take their stops from it
  assert.match(a11y, /function focusables\(root\) \{\n\s*return \[\.\.\.root\.querySelectorAll\(FOCUSABLE\)\]/);
  assert.match(a11y, /const all = \[\.\.\.document\.querySelectorAll\(FOCUSABLE\)\];/);
});

test('Tab closes an open menu: back to its button from inside it, on from its button', () => {
  const util = source('../js/util.js');
  const key = util.slice(util.indexOf('  const key = (e) => {', util.indexOf('export function showMenu(')), util.indexOf('const entry = { el, anchor'));
  assert.match(key, /if \(e\.key === 'Tab'\) \{ if \(el\.contains\(document\.activeElement\)\) e\.preventDefault\(\); closeMenu\(\); return; \}/);
  // closeMenu gives the focus back to the button when it was in the menu
  assert.match(util, /if \(hadFocus && m\.anchor && m\.anchor\.isConnected\) m\.anchor\.focus\(\{ preventScroll: true \}\);/);
});

test('the message field has a name of its own, in both languages', () => {
  assert.match(source('../js/composer.js'), /<textarea rows="1" spellcheck="true" aria-label="\$\{esc\(t\('composer_label'\)\)\}"/);
  setLanguage('en');
  assert.equal(t('composer_label'), 'Message to Claude');
  setLanguage('tr');
  assert.equal(t('composer_label'), 'Claude’a mesaj');
  setLanguage('en');
});
