// npm run test:ui — a GFM task item shows its checkbox in place of the list
// bullet (markdown.rs puts the box first in the item or its first paragraph,
// see its test task_items_start_with_their_checkbox), in full colour.
import test from 'node:test';
import assert from 'node:assert/strict';
import { source } from './source.mjs';

const css = source('../css/app.css');

test('task items have no bullet, and the box stands where it was', () => {
  const sel = '.md li:has(> input[type="checkbox"]:first-child, > p:first-child > input[type="checkbox"]:first-child)';
  assert.ok(css.includes(`${sel} { list-style: none; }`));
  // the list's padding is what the box moves back over
  assert.match(css, /\.md ul, \.md ol \{ margin: 0 0 1em; padding-left: 1\.6em; \}/);
  assert.ok(css.includes('.md li > input[type="checkbox"]:first-child, .md li > p:first-child > input[type="checkbox"]:first-child { font-size: inherit; margin-left: calc(2px - 1.6em); }'));
  assert.ok(css.includes('.md li > input[type="checkbox"]:disabled, .md li > p > input[type="checkbox"]:disabled { opacity: 1; }'));
});

test('a Doc artifact draws its task boxes like the chat, not as faded native ones', () => {
  const doc = source('../../src-tauri/src/artifact_types.css');
  const box = doc.match(/\n\.doc input\[type="checkbox"\] \{([^}]*)\}/)[1];
  assert.match(box, /appearance: none;/);
  assert.match(box, /border: 1\.5px solid var\(--fg2\);/);
  assert.match(box, /opacity: 1;/);
  assert.match(doc, /\n\.doc input\[type="checkbox"\]:checked \{ background: var\(--accent\); border-color: var\(--accent\); \}/);
  assert.match(doc, /\n\.doc input\[type="checkbox"\]:checked::before \{ content: ""; [^}]*clip-path: polygon\(/, 'the tick');
});
