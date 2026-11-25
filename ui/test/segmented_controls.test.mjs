// npm run test:ui — segmented controls (Settings, Todos and Usage filters):
// the chosen option is visible in both themes and is said to a screen reader.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { contrast } from '../e2e/contrast.mjs';
import { source } from './source.mjs';

const read = source;
const CSS = read('../css/app.css');

/** The custom properties of the first rule whose selector starts with `sel`. */
function tokens(sel) {
  const at = CSS.indexOf(sel + ' {');
  const body = CSS.slice(CSS.indexOf('{', at) + 1, CSS.indexOf('}', at));
  return Object.fromEntries([...body.matchAll(/--([\w-]+):\s*([^;]+);/g)].map((m) => [m[1], m[2].trim()]));
}
const THEMES = { light: tokens(':root, :root[data-theme="light"]'), dark: tokens(':root[data-theme="dark"]') };

/** The declarations of the rule starting a line with `sel`. */
function rule(sel) {
  const at = CSS.indexOf('\n' + sel + ' {');
  assert.ok(at >= 0, `no rule ${sel}`);
  return CSS.slice(CSS.indexOf('{', at) + 1, CSS.indexOf('}', at));
}

const rgb = (v) => (v.startsWith('#') ? `rgb(${parseInt(v.slice(1, 3), 16)}, ${parseInt(v.slice(3, 5), 16)}, ${parseInt(v.slice(5, 7), 16)})` : v);
const nums = (c) => { const [r, g, b, a = 1] = c.match(/[\d.]+/g).map(Number); return [r, g, b, a]; };
/** The colour seen where `layers` (top first, the last opaque) are stacked. */
function flat(...layers) {
  const [r, g, b] = layers.map((c) => nums(rgb(c))).reduceRight((under, top) => [...top.slice(0, 3).map((v, i) => v * top[3] + under[i] * (1 - top[3])), 1]);
  return `rgb(${r}, ${g}, ${b})`;
}

test('the chosen option of a segmented control stands out in both themes (WCAG 1.4.11 and AA text)', () => {
  const base = rule('.segmented button.on');
  const dark = rule(':root[data-theme="dark"] .segmented button.on');
  const track = rule('.segmented').match(/background: var\(--([\w-]+)\)/)[1];
  for (const [theme, own] of [['light', base], ['dark', dark]]) {
    const T = THEMES[theme];
    const val = (v) => { const m = v.match(/^var\(--([\w-]+)\)$/); return m ? T[m[1]] : v; };
    const fill = val((own.match(/background: ([^;]+);/) || base.match(/background: ([^;]+);/))[1]);
    const ring = own.match(/inset 0 0 0 1px (rgba\([^)]+\))/)[1];
    for (const surface of ['bg-0', 'bg-1', 'surface', 'bg-2'].map((k) => T[k])) {
      const trackSeen = flat(T[track], surface);
      const fillSeen = flat(fill, T[track], surface);
      const ringSeen = flat(ring, fill, T[track], surface);
      const where = `${theme} on ${surface}`;
      assert.ok(contrast(ringSeen, [trackSeen]) >= 3, `${where}: ring on the track ${contrast(ringSeen, [trackSeen]).toFixed(2)}`);
      assert.ok(contrast(ringSeen, [fillSeen]) >= 3, `${where}: ring on its fill ${contrast(ringSeen, [fillSeen]).toFixed(2)}`);
      assert.ok(contrast(rgb(T['text-0']), [fillSeen]) >= 4.5, `${where}: chosen label`);
      assert.ok(contrast(rgb(T['text-1']), [trackSeen]) >= 4.5, `${where}: other labels`);
    }
  }
  assert.match(CSS, /@media \(forced-colors: active\) \{ \.segmented button\.on \{ outline:/, 'high-contrast mode, which drops shadows, keeps a mark');
});

test('the Todos and Usage filters say which option is chosen', () => {
  assert.match(read('../js/todos_ui.js'), /class="todos-filters segmented" role="group"><button data-k="" class="\$\{kind \? '' : 'on'\}" aria-pressed="\$\{!kind\}">.*aria-pressed="\$\{kind === k\}"/);
  assert.match(read('../js/usage_ui.js'), /<div class="segmented" role="group">.*aria-pressed="\$\{x === d\}"/);
});
