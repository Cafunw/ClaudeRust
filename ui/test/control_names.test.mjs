// npm run test:ui — controls a screen reader would otherwise name by nothing
// or by a placeholder: a reply's task-list checkboxes and the New project
// dialog's fields.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { taskLabel } from '../js/parts.js';
import { source } from './source.mjs';

const text = (s) => ({ nodeName: '#text', textContent: s });
const el = (name, s) => ({ nodeName: name, textContent: s });

test('a task list checkbox is named by its own item, not by the items nested in it', () => {
  assert.equal(taskLabel({ childNodes: [el('INPUT', ''), text('\nship '), el('STRONG', 'it'), text('\n')] }), 'ship it');
  assert.equal(taskLabel({ childNodes: [el('INPUT', ''), text('parent'), el('UL', 'child one child two'), el('OL', 'x')] }), 'parent');
  assert.equal(taskLabel({ childNodes: [el('INPUT', ''), text('w '.repeat(100))] }).length, 120, 'cut short');
  assert.equal(taskLabel({ childNodes: [el('INPUT', '')] }), '', 'no text: no name (and none is set)');
  const parts = source('../js/parts.js');
  assert.match(parts, /all\('li > input\[type=checkbox\], li > p:first-child > input\[type=checkbox\]'\)\.forEach/, 'tight and loose lists');
  assert.match(parts, /if \(label\) c\.setAttribute\('aria-label', label\);/);
});

test('the New project dialog\'s fields are named by their labels', () => {
  const dlg = source('../js/projects.js');
  assert.match(dlg, /<label class="label" for="np-name">[^<]+<\/label><input class="input" id="np-name"/);
  assert.match(dlg, /<label class="label" for="np-desc">[^<]+<\/label><textarea class="textarea" id="np-desc"/);
});
