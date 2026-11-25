// npm run test:ui — the theme colours of ui/css/app.css keep text readable:
// WCAG AA (4.5:1) for every text colour on the surfaces it is drawn on, in
// the light and the dark theme. (The ratio itself: ui/e2e/contrast.mjs.)
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { contrast } from '../e2e/contrast.mjs';

const CSS = readFileSync(new URL('../css/app.css', import.meta.url), 'utf8');

/** The custom properties of the first rule whose selector starts with `sel`. */
function tokens(sel) {
  const at = CSS.indexOf(sel + ' {');
  assert.ok(at >= 0, `no rule ${sel}`);
  const body = CSS.slice(CSS.indexOf('{', at) + 1, CSS.indexOf('}', at));
  return Object.fromEntries([...body.matchAll(/--([\w-]+):\s*([^;]+);/g)].map((m) => [m[1], m[2].trim()]));
}

/** "#rrggbb" or "rgba(...)" as the CSS colour string contrast() reads. */
const css = (v) => (v.startsWith('#') ? `rgb(${parseInt(v.slice(1, 3), 16)}, ${parseInt(v.slice(3, 5), 16)}, ${parseInt(v.slice(5, 7), 16)})` : v);

const THEMES = { light: tokens(':root, :root[data-theme="light"]'), dark: tokens(':root[data-theme="dark"]') };

/** `fg` on the layers `bg` (a token or a colour, the last one opaque). */
function ratio(theme, fg, ...bg) {
  const T = THEMES[theme];
  return contrast(css(T[fg] || fg), bg.map((b) => css(T[b] || b)));
}

// where each text colour is drawn: the page, the sidebar, menus and the composer, quiet fills
const SURFACES = [['bg-0'], ['bg-1'], ['surface'], ['bg-2']];

for (const theme of Object.keys(THEMES)) {
  test(`${theme}: body, secondary and hint text pass AA on every surface`, () => {
    for (const fg of ['text-0', 'text-1', 'text-2', 'text-3', 'link']) {
      for (const bg of SURFACES) {
        const r = ratio(theme, fg, ...bg);
        assert.ok(r >= 4.5, `${fg} on ${bg.join('/')}: ${r.toFixed(2)}`);
      }
    }
  });

  test(`${theme}: status text passes AA on its own tinted background`, () => {
    for (const [fg, soft] of [['danger', 'danger-soft'], ['ok', 'ok-soft'], ['accent-ink', 'accent-soft']]) {
      for (const under of ['bg-0', 'surface']) {
        const r = ratio(theme, fg, soft, under);
        assert.ok(r >= 4.5, `${fg} on ${soft} over ${under}: ${r.toFixed(2)}`);
      }
    }
    const warn = ratio(theme, 'warn', 'rgba(227, 181, 91, 0.14)', 'bg-0');
    assert.ok(warn >= 4.5, `warn notice: ${warn.toFixed(2)}`);
    // a danger button and an error toast: their text on the danger fill
    const onDanger = ratio(theme, 'on-danger', 'danger');
    assert.ok(onDanger >= 4.5, `on-danger on danger: ${onDanger.toFixed(2)}`);
  });
}

test('the accent is drawn as text only through accent-ink', () => {
  // (the brand accent itself is under 4.5:1 on the light page; icons and borders may use it)
  for (const sel of ['.tool-btn.on', '.active-tool', '.model-btn .think-flag', '.compact-divider.pending .cd-head .lab']) {
    const at = CSS.indexOf(sel);
    const rule = CSS.slice(at, CSS.indexOf('}', at));
    assert.doesNotMatch(rule, /[^-]color: var\(--accent\)/, sel);
  }
});

test('an accent button’s label passes AA on its fill, in both themes', () => {
  const at = CSS.indexOf('.btn.accent {');
  const fill = CSS.slice(at, CSS.indexOf('}', at)).match(/background: (#[0-9a-f]{6})/i)[1];
  for (const theme of Object.keys(THEMES)) {
    const r = ratio(theme, 'accent-text', fill);
    assert.ok(r >= 4.5, `${theme}: accent-text on ${fill}: ${r.toFixed(2)}`);
  }
});
