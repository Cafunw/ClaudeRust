// npm run test:ui — round 10 Code tab and chat view items (EV8-16..19 and the sweep):
// live regions that empty once said and on a language change, Tab that wraps
// instead of landing on <body>, focus kept on a control drawn again, the
// header's title before the crumbs, the Code home's mark and worktree card.
import './stub_globals.mjs';
import test, { mock } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { announce, clearAnnouncements, SAID_MS, wrapStep, tabStop, controlKey, copyOf } from '../js/a11y.js';

const read = (p) => readFileSync(new URL(p, import.meta.url), 'utf8');
const css = read('../css/app.css');
// the declarations of the rule whose selector list is exactly `sel` (the last one wins, as in CSS)
const rule = (sel) => {
  const all = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].filter((m) => m[1].trim().endsWith(sel) && m[1].trim().split('\n').pop().trim() === sel);
  return all.length ? all[all.length - 1][2] : null;
};

test('EV8-16: a live region empties once said, and at once on a language change', () => {
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

test('EV8-16: the sidebar toggle says whether the sidebar is open, kept current', () => {
  const sb = read('../js/sidebar.js');
  assert.match(sb, /data-act="toggle"[^>]*aria-expanded="\$\{!store\.sidebarCollapsed\}"/);
  assert.match(sb, /classList\.toggle\('collapsed', on\);\s*\n\s*\$\('#sidebar \[data-act=toggle\]'\)\?\.setAttribute\('aria-expanded', String\(!on\)\)/);
  // the model button's thinking mark is an icon with a tooltip: its state is in the button's name too
  assert.match(read('../js/composer.js'), /<span class="think-flag" title="\$\{t\('extended_thinking'\)\}">\$\{icon\('bulb', 13\)\}<span class="sr-only">, \$\{esc\(t\('extended_thinking'\)\)\}<\/span><\/span>/);
  // xterm's own input label is English whatever the language: the pane names it
  assert.match(read('../js/panes/terminal.js'), /term\.textarea\.setAttribute\('aria-label', t\('ws_terminal'\)\)/);
});

test('EV8-19: past the last control Tab wraps to the first (Shift+Tab back), never to <body>', () => {
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

test('EV8-19: a control drawn again (its title arrived, it was starred) gets the focus back', () => {
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

test('EV8-19: on the Code home Tab meets the box\'s "+" before the row under the box', () => {
  const code = read('../js/code.js');
  assert.match(code, /\$\('\[data-act=attach\]', composer\.el\)\.after\(envRow\)/);
  assert.doesNotMatch(code, /composer-bar', composer\.el\)\.prepend\(envRow\)/);
});

test('EV8-17: the session title keeps its room; crumbs and the watch badge give way first', () => {
  const give = rule('.topbar .crumb, .topbar .active-tool.wt');
  assert.ok(give && /flex-shrink:\s*100/.test(give) && /min-width:\s*28px/.test(give), 'crumbs shrink a hundred times faster, down to their icon');
  assert.doesNotMatch(css, /\.topbar \.crumb \{[^}]*min-width:\s*64px/, 'no crumb keeps 64 px ahead of the title');
  assert.match(css, /@container \(max-width: 640px\) \{\s*\n\s*\.topbar \.watch-badge > span:not\(\.wb-ok\):not\(\.wb-err\) \{ display: none; \}/);
  assert.match(css, /@container \(max-width: 400px\) \{[^}]*\.topbar \.ctx-meter \.pct \{ display: none; \}/);
  const title = rule('.topbar .title-btn');
  assert.ok(title && /flex-shrink:\s*1\b/.test(title), 'the title shrinks last');
});

test('EV8-18: the Code home mark heads the stage, the worktree hint sits under its label', () => {
  const code = read('../js/code.js');
  assert.match(code, /<div class="code-stage"><span class="env-mark">\$\{spark\(24\)\}<\/span>\s*\n\s*<div class="env-card"><div class="env-list">/);
  const mark = rule('.env-mark');
  assert.ok(mark && !/position:\s*absolute/.test(mark), 'no longer pinned over the card\'s border');
  const wt = rule('.env-card.env-wt');
  assert.ok(wt && !/flex-direction:\s*row/.test(wt), 'label and hint stacked, not two narrow columns');
  assert.ok(!/white-space:\s*nowrap/.test(rule('.env-wt span') || ''), 'the hint wraps in a narrow window instead of being cut');
});

test('sweep: a narrow Sessions page keeps its head on one row (Routines and Import as icons, still named)', () => {
  const code = read('../js/code.js');
  for (const act of ['routines', 'import']) assert.match(code, new RegExp(`class="btn sess-sec" data-act="${act}" title="[^"]+" aria-label="[^"]+">\\$\\{icon\\([^)]*\\)\\}<span class="lbl">`));
  assert.match(css, /@media \(max-width: 600px\) \{ \.page-head \.sess-sec \.lbl \{ display: none; \} \}/);
});

test('sweep: a narrow chat keeps a gap under the composer\'s note', () => {
  const narrow = css.slice(css.indexOf('@media (max-width: 760px) {'));
  assert.match(narrow.slice(0, narrow.indexOf('}\n}') + 3), /\.chat-bottom \{ padding: 0 12px 6px; \}/);
});
