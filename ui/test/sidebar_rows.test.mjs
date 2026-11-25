// npm run test:ui — the sidebar's rows: every nav button is named for the
// folded rail, and row text passes AA in every state and theme.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { contrast } from '../e2e/contrast.mjs';

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
