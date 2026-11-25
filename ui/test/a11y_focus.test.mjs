// npm run test:ui — screen reader and keyboard focus: live regions that empty
// once said and on a language change, controls that say their state, Tab that
// wraps instead of landing on <body>, focus kept on a control drawn again, and
// focus tracking that keeps no closed chat alive.
import './stub_globals.mjs';
import test, { mock } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { announce, clearAnnouncements, SAID_MS, wrapStep, tabStop, controlKey, copyOf } from '../js/a11y.js';

const read = (p) => readFileSync(new URL(p, import.meta.url), 'utf8');

test('a live region empties once said, and at once on a language change', () => {
  const made = [];
  globalThis.document.body = { appendChild(el) { el.isConnected = true; } };
  globalThis.document.createElement = () => { const el = { textContent: '', isConnected: false, setAttribute() {}, className: '' }; made.push(el); return el; };
  mock.timers.enable({ apis: ['setTimeout'] });
  try {
    announce('Claude sunu kullanmak istiyor: echo_upper.', true);
    mock.timers.tick(30);
    const alert = made.find((el) => el.textContent);
    assert.equal(alert.textContent, 'Claude sunu kullanmak istiyor: echo_upper.');
    mock.timers.tick(SAID_MS - 100);
    assert.ok(alert.textContent, 'still there while it is being read');
    mock.timers.tick(100);
    assert.equal(alert.textContent, '', 'gone once said: no answered prompt lingers in the page');
    // a newer announcement restarts the wait; a language change empties it at once
    announce('Polite words');
    mock.timers.tick(30);
    assert.ok(made.some((el) => el.textContent === 'Polite words'));
    clearAnnouncements();
    assert.ok(made.every((el) => el.textContent === ''));
    mock.timers.tick(SAID_MS);
    assert.ok(made.every((el) => el.textContent === ''));
  } finally {
    mock.timers.reset();
  }
  assert.match(read('../js/a11y.js'), /new MutationObserver\(clearAnnouncements\)\.observe\(document\.documentElement, \{ attributes: true, attributeFilter: \['lang'\] \}\)/);
});

test('the sidebar toggle says whether the sidebar is open, kept current', () => {
  const sb = read('../js/sidebar.js');
  assert.match(sb, /data-act="toggle"[^>]*aria-expanded="\$\{!store\.sidebarCollapsed\}"/);
  assert.match(sb, /classList\.toggle\('collapsed', on\);\s*\n\s*\$\('#sidebar \[data-act=toggle\]'\)\?\.setAttribute\('aria-expanded', String\(!on\)\)/);
  // the model button's thinking mark is an icon with a tooltip: its state is in the button's name too
  assert.match(read('../js/composer.js'), /<span class="think-flag" title="\$\{t\('extended_thinking'\)\}">\$\{icon\('bulb', 13\)\}<span class="sr-only">, \$\{esc\(t\('extended_thinking'\)\)\}<\/span><\/span>/);
  // xterm's own input label is English whatever the language: the pane names it
  assert.match(read('../js/panes/terminal.js'), /term\.textarea\.setAttribute\('aria-label', t\('ws_terminal'\)\)/);
});

test('past the last control Tab wraps to the first (Shift+Tab back), never to <body>', () => {
  const [a, b, c] = ['first', 'middle', 'last'];
  assert.equal(wrapStep([a, b, c], c, false), a);
  assert.equal(wrapStep([a, b, c], a, true), c);
  assert.equal(wrapStep([a, b, c], b, false), null, 'inside the page: the browser steps');
  assert.equal(wrapStep([a, b, c], b, true), null);
  assert.equal(wrapStep([a, b, c], a, false), null);
  assert.equal(wrapStep([], a, false), null);
  assert.equal(wrapStep([a], a, false), a, 'a lone control keeps the focus');
  // a list's other rows (and their buttons) are out of the order: a dialog's or sheet's last stop is
  // the last one the browser itself would take, or Tab leaves it for <body>
  const el = (tabIndex, shown = true) => ({ tabIndex, getClientRects: () => (shown ? [{}] : []) });
  assert.ok(tabStop(el(0)));
  assert.ok(!tabStop(el(-1)), 'tabindex=-1: skipped by the browser');
  assert.ok(!tabStop(el(0, false)), 'not shown');
});

test('a control drawn again (its title arrived, it was starred) gets the focus back', () => {
  const el = (tag, cls, attrs = {}) => ({ tagName: tag, classList: cls ? cls.split(' ') : [], attributes: Object.entries(attrs).map(([name, value]) => ({ name, value })) });
  const oldTitle = el('BUTTON', 'title-btn', { 'aria-haspopup': 'menu', title: 'Untitled' });
  const newTitle = el('BUTTON', 'title-btn', { 'aria-haspopup': 'menu', title: 'Add mul function to calc.py' });
  const outline = el('BUTTON', 'icon-btn', { title: 'Your messages' });
  const usage = el('BUTTON', 'icon-btn', { title: 'Usage' });
  // the tooltip changed with the title: found by its kind
  assert.equal(copyOf(oldTitle, [outline, newTitle, usage]), newTitle);
  // two buttons of one kind: the tooltip tells them apart
  assert.equal(copyOf(el('BUTTON', 'icon-btn', { title: 'Usage' }), [outline, usage]), usage);
  // nothing to tell them apart: none is taken rather than the wrong one
  assert.equal(copyOf(el('BUTTON', 'icon-btn', { title: 'Gone' }), [outline, usage]), null);
  assert.equal(copyOf(el('BUTTON', 'chat-link', { 'data-id': 'b' }), [el('BUTTON', 'chat-link two', { 'data-id': 'a' }), el('BUTTON', 'chat-link two active', { 'data-id': 'b' })])?.attributes[0].value, 'b');
  assert.equal(copyOf(oldTitle, []), null);
  // state classes after the first don't change what a control is; its data does
  assert.equal(controlKey(el('BUTTON', 'ws-tab on', { 'data-pane': 'changes' }), false), controlKey(el('BUTTON', 'ws-tab', { 'data-pane': 'changes' }), false));
  assert.notEqual(controlKey(el('BUTTON', 'ws-tab', { 'data-pane': 'changes' }), false), controlKey(el('BUTTON', 'ws-tab', { 'data-pane': 'history' }), false));
  // a menu gives focus back to the control that opened it when it held the keyboard
  assert.match(read('../js/util.js'), /const hadFocus = m\.el\.contains\(document\.activeElement\);\s*\n\s*m\.el\.remove\(\);\s*\n\s*if \(hadFocus && m\.anchor && m\.anchor\.isConnected\) m\.anchor\.focus/);
});

test('on the Code home Tab meets the box\'s "+" before the row under the box', () => {
  const code = read('../js/code.js');
  assert.match(code, /\$\('\[data-act=attach\]', composer\.el\)\.after\(envRow\)/);
  assert.doesNotMatch(code, /composer-bar', composer\.el\)\.prepend\(envRow\)/);
});

test('where focus was is remembered weakly, so a closed chat can be collected', () => {
  const a11y = read('../js/a11y.js');
  assert.match(a11y, /lastOutside = new WeakRef\(e\.target\)/);
  assert.match(a11y, /ancestors\.push\(new WeakRef\(p\)\)/);
  assert.doesNotMatch(a11y, /lastOutside = e\.target|ancestors\.push\(p\)/);
});
