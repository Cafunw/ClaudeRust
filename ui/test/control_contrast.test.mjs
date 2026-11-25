// npm run test:ui — what tells a control apart (a text field's edge, an off switch's track) has 3:1
// against every surface it is drawn on, in the light and the dark theme (WCAG 1.4.11).
import test from 'node:test';
import assert from 'node:assert/strict';
import { contrast } from '../e2e/contrast.mjs';
import { source } from './source.mjs';

const CSS = source('../css/app.css');

function tokens(sel) {
  const at = CSS.indexOf(sel + ' {');
  const body = CSS.slice(CSS.indexOf('{', at) + 1, CSS.indexOf('}', at));
  return Object.fromEntries([...body.matchAll(/--([\w-]+):\s*([^;]+);/g)].map((m) => [m[1], m[2].trim()]));
}
const rgb = (v) => (v.startsWith('#') ? `rgb(${parseInt(v.slice(1, 3), 16)}, ${parseInt(v.slice(3, 5), 16)}, ${parseInt(v.slice(5, 7), 16)})` : v);
const THEMES = { light: tokens(':root, :root[data-theme="light"]'), dark: tokens(':root[data-theme="dark"]') };

for (const [theme, T] of Object.entries(THEMES)) {
  test(`${theme}: field edges and off switches pass 3:1 on every surface`, () => {
    for (const fg of ['field-border', 'switch-off']) {
      assert.ok(T[fg], `--${fg} is defined`);
      for (const bg of ['bg-0', 'bg-1', 'bg-2', 'bg-3', 'surface']) {
        const r = contrast(rgb(T[fg]), [rgb(T[bg])]);
        assert.ok(r >= 3, `${fg} on ${bg}: ${r.toFixed(2)}`);
      }
    }
    // the knob stays visible on the off track
    const knob = contrast('rgb(255, 255, 255)', [rgb(T['switch-off'])]);
    assert.ok(knob >= 3, `knob on the off track: ${knob.toFixed(2)}`);
  });
}

test('fields and switches draw with those tokens', () => {
  assert.match(CSS, /\.input, \.textarea, \.select \{\n {2}width: 100%; border: 1px solid var\(--field-border\);/);
  assert.match(CSS, /\.switch \.track \{[^}]*background: var\(--switch-off\);/);
  for (const sel of ['.rc-edit textarea', '.mem-main textarea']) {
    const rule = CSS.slice(CSS.indexOf(`\n${sel} {`), CSS.indexOf('}', CSS.indexOf(`\n${sel} {`)));
    assert.match(rule, /border: 1px solid var\(--field-border\)/, sel);
  }
  // focus makes the edge stronger, never fainter than at rest
  assert.match(CSS, /\.input:focus, \.textarea:focus, \.select:focus \{ border-color: var\(--text-2\);/);
});
