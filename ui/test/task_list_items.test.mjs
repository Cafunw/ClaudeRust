// npm run test:ui — a GFM task item shows its checkbox in place of the list
// bullet (markdown.rs puts the box first in the item or its first paragraph,
// see its test task_items_start_with_their_checkbox), in full colour.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const css = readFileSync(new URL('../css/app.css', import.meta.url), 'utf8');

test('task items have no bullet, and the box stands where it was', () => {
  const sel = '.md li:has(> input[type="checkbox"]:first-child, > p:first-child > input[type="checkbox"]:first-child)';
  assert.ok(css.includes(`${sel} { list-style: none; }`));
  // the list's padding is what the box moves back over
  assert.match(css, /\.md ul, \.md ol \{ margin: 0 0 1em; padding-left: 1\.6em; \}/);
  assert.ok(css.includes('.md li > input[type="checkbox"]:first-child, .md li > p:first-child > input[type="checkbox"]:first-child { font-size: inherit; margin-left: calc(2px - 1.6em); }'));
  assert.ok(css.includes('.md li > input[type="checkbox"]:disabled, .md li > p > input[type="checkbox"]:disabled { opacity: 1; }'));
});
