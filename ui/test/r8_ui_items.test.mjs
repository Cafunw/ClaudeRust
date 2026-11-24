// npm run test:ui — round 8 UI items: "Thought for 12s", help links in error
// notices, the usage banner's sentences and dismiss key, Alt+N pane keys in
// tab order, the translated "Auto" language, sidebar rail names, and the
// contrast of sidebar rows in every state.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setLanguage, LANGUAGES } from '../js/i18n.js';
import { contrast } from '../e2e/contrast.mjs';
import { thoughtFor, linkHelp } from '../js/parts.js';
import { bannerText, bannerKey, limitPhrase } from '../js/usage_limits.js';

window.matchMedia ??= () => ({ matches: false, addEventListener() {}, addListener() {} });
const workspace = await import('../js/workspace.js');

test('a thinking block says how long it thought, in either language', () => {
  setLanguage('en');
  assert.equal(thoughtFor(12000), 'Thought for 12s');
  assert.equal(thoughtFor(65000), 'Thought for 1m 5s');
  assert.equal(thoughtFor(120400), 'Thought for 2m');
  assert.equal(thoughtFor(300), 'Thought for 1s', 'under a second still reads as a second');
  for (const none of [undefined, null, '', 'x', -5, NaN]) assert.equal(thoughtFor(none), '', `untimed: ${none}`);
  setLanguage('tr');
  assert.equal(thoughtFor(12000), '12 sn düşündü');
  assert.equal(thoughtFor(65000), '1 dk 5 sn düşündü');
  setLanguage('en');
});

test('an error notice links the help sites it names, and only to them', () => {
  assert.equal(linkHelp('Install it from claude.com/claude-code, or set its path.'),
    'Install it from <a href="https://claude.com/claude-code">claude.com/claude-code</a>, or set its path.');
  assert.equal(linkHelp('Get it at https://nodejs.org.'), 'Get it at <a href="https://nodejs.org">https://nodejs.org</a>.');
  assert.equal(linkHelp('see notclaude.com/claude-code'), 'see notclaude.com/claude-code', 'a look-alike host stays text');
  assert.equal(linkHelp('see https://evil.example/claude.com'), 'see https://evil.example/claude.com');
});

test('the usage banner reads as a sentence for every limit, in either language', () => {
  setLanguage('en');
  assert.equal(bannerText('near', { key: 'seven_day' }), 'You’re close to your weekly limit for all models.');
  assert.equal(bannerText('near', { key: 'five_hour' }), 'You’re close to your 5-hour limit.');
  assert.match(bannerText('exceeded', { key: 'seven_day_opus' }), /^You’ve reached your weekly Opus limit\. /);
  assert.equal(bannerText('near', { key: 'seven_day_oauth_apps', label: null }), 'You’re close to a usage limit (seven day oauth apps).');
  assert.equal(limitPhrase({ key: 'overage' }), 'your extra usage limit');
  setLanguage('tr');
  assert.equal(bannerText('near', { key: 'seven_day' }), 'Tüm modeller için haftalık sınırınız dolmak üzere.');
  assert.equal(bannerText('near', { key: 'five_hour' }), '5 saatlik sınırınız dolmak üzere.');
  assert.match(bannerText('exceeded', { key: 'weekly' }), /^Tüm modeller için haftalık sınırınız doldu\. /);
  setLanguage('en');
});

test('a dismissed banner comes back at a new level, limit or window, not for the same one restated', () => {
  const x = { key: 'five_hour', resetsAt: Date.UTC(2026, 10, 22, 17, 0, 0) };
  assert.equal(bannerKey('near', x), bannerKey('near', { ...x, resetsAt: x.resetsAt + 4000 }));
  assert.notEqual(bannerKey('near', x), bannerKey('exceeded', x));
  assert.notEqual(bannerKey('near', x), bannerKey('near', { ...x, key: 'seven_day' }));
  assert.notEqual(bannerKey('near', x), bannerKey('near', { ...x, resetsAt: x.resetsAt + 5 * 3600e3 }));
});

test('Alt+1… follow the tabs as they are shown, and the tooltips say them', () => {
  for (const [id, order] of [['files', 65], ['plan', 30], ['changes', 10], ['terminal', 20], ['history', 15]]) {
    workspace.registerPane({ id, order, label: () => id, icon: 'x', mount() {} });
  }
  assert.deepEqual(workspace.tabOrder(), ['changes', 'history', 'terminal', 'plan', 'files']);
  assert.equal(workspace.paneKey(3), 'Alt+3');
  assert.equal(workspace.paneKey(9), '', 'only eight have a key');
  assert.equal(workspace.paneHints(), 'Alt+1 changes · Alt+2 history · Alt+3 terminal · Alt+4 plan · Alt+5 files');
});

test('the "Auto" language is named in the app’s language', () => {
  setLanguage('tr');
  assert.equal(LANGUAGES.find((l) => l.id === 'auto').name, 'Otomatik (sistem)');
  setLanguage('en');
  assert.equal(LANGUAGES.find((l) => l.id === 'auto').name, 'Auto (system)');
});

test('every sidebar nav button has a name and a tooltip, for the folded rail', () => {
  const src = readFileSync(new URL('../js/sidebar.js', import.meta.url), 'utf8');
  const row = src.match(/const row = \(nav, ic, label, kbd = ''\) => `<button class="nav-item"[^>]*>/);
  assert.ok(row, 'the nav row template');
  assert.match(row[0], /aria-label="\$\{esc\(label\)\}"/);
  assert.match(row[0], /title="/);
});

// --------------------------------------------------- sidebar rows' contrast
const CSS = readFileSync(new URL('../css/app.css', import.meta.url), 'utf8');
function tokens(sel) {
  const at = CSS.indexOf(sel + ' {');
  const body = CSS.slice(CSS.indexOf('{', at) + 1, CSS.indexOf('}', at));
  return Object.fromEntries([...body.matchAll(/--([\w-]+):\s*([^;]+);/g)].map((m) => [m[1], m[2].trim()]));
}
const rgb = (v) => (v.startsWith('#') ? `rgb(${parseInt(v.slice(1, 3), 16)}, ${parseInt(v.slice(3, 5), 16)}, ${parseInt(v.slice(5, 7), 16)})` : v);
/** The token a rule gives `color`. */
const colorOf = (sel) => {
  const m = CSS.match(new RegExp(`(^|\\n)${sel.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')} \\{[^}]*?color: var\\(--([\\w-]+)\\)`));
  assert.ok(m, `no colour for ${sel}`);
  return m[2];
};

for (const [theme, sel] of [['light', ':root, :root[data-theme="light"]'], ['dark', ':root[data-theme="dark"]']]) {
  test(`${theme}: sidebar row text passes AA at rest, hovered and active`, () => {
    const T = tokens(sel);
    const on = (fg, ...bg) => contrast(rgb(T[fg]), bg.map((b) => rgb(T[b] || b)));
    const rows = [
      ['.chat-link .sub', ['bg-1'], ['row-hover', 'bg-1']],
      ['.chat-link.active .sub', ['row-active', 'bg-1']],
      ['.kbd', ['row-hover', 'bg-1']],
      ['.nav-item.active .kbd', ['row-active', 'bg-1']],
      ['.user-card .plan', ['bg-1'], ['row-hover', 'bg-1']],
      ['.ul-meter', ['bg-1']],
    ];
    for (const [rule, ...states] of rows) {
      const fg = colorOf(rule);
      for (const bg of states) {
        const r = on(fg, ...bg);
        assert.ok(r >= 4.5, `${rule} (${fg}) on ${bg.join(' over ')}: ${r.toFixed(2)}`);
      }
    }
    for (const fg of ['text-0', 'text-1']) assert.ok(on(fg, 'row-active', 'bg-1') >= 4.5, `${fg} on the active row`);
  });
}
