// npm run test:ui — code shows the characters typed: no programming ligatures ("<=" drawn as "≤",
// "=>" as "⇒") from fonts like Cascadia Code, in code, pre, the file viewer and the artifact code view.
import test from 'node:test';
import assert from 'node:assert/strict';
import { source } from './source.mjs';

const CSS = { 'app.css': source('../css/app.css'), 'attachment_preview.css': source('../css/attachment_preview.css') };

test('contextual alternates (where those ligatures live) are off for the whole page', () => {
  const body = CSS['app.css'].match(/\nbody \{([^}]*)\}/)[1];
  assert.match(body, /font-variant-ligatures: no-contextual;/);
  // the controls take the page's font as it is
  assert.match(CSS['app.css'], /\nbutton, input, textarea, select \{ font: inherit;/);
});

test('a rule that sets the mono font with the `font` shorthand (which resets it) says it again', () => {
  for (const [file, css] of Object.entries(CSS)) {
    for (const m of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
      if (!/(^|;)\s*font:[^;]*var\(--font-mono\)/.test(m[2])) continue;
      assert.match(m[2], /font-variant-ligatures: no-contextual/, `${file}: ${m[1].trim()}`);
    }
  }
  // and nothing turns them back on
  for (const [file, css] of Object.entries(CSS)) assert.doesNotMatch(css, /font-variant-ligatures: (normal|contextual)|"calt" 1/, file);
});
