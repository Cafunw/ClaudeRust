// npm run test:ui — Settings → Import: a session's title and details, and an extension's
// description, cut short in a narrow window are shown whole on hover.
import test from 'node:test';
import assert from 'node:assert/strict';
import { source } from './source.mjs';

const SRC = source('../js/importer.js');

test('every line the Import lists cut short has its whole text as its title', () => {
  const cut = [...SRC.matchAll(/<span class="[^"]*\bellipsis\b[^"]*"([^>]*)>/g)];
  assert.ok(cut.length >= 3);
  for (const m of cut) assert.match(m[1], /title="\$\{esc\(/, m[0]);
  assert.match(SRC, /<span class="n ellipsis" title="\$\{esc\(s\.title \|\| s\.session_id\)\}">/);
});
