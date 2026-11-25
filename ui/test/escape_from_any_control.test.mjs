// npm run test:ui — a bar or box with a field and buttons (the find bar, editing a message, a
// review comment) closes on Escape from any of them, not only from its field.
import test from 'node:test';
import assert from 'node:assert/strict';
import { source } from './source.mjs';

const part = (file, from, to) => { const s = source(file); return s.slice(s.indexOf(from), s.indexOf(to, s.indexOf(from))); };

test('the find bar closes on Escape from its field and its buttons', () => {
  const bar = part('../js/findbar.js', "const input = bar.querySelector('input');", "bar.querySelector('[data-a=prev]')");
  assert.match(bar, /bar\.addEventListener\('keydown', \(e\) => \{ if \(e\.key === 'Escape'\) \{ e\.preventDefault\(\); e\.stopPropagation\(\); closeFind\(\); \} \}\);/);
  assert.doesNotMatch(bar.slice(bar.indexOf("input.addEventListener('keydown'"), bar.indexOf('bar.addEventListener')), /Escape/);
});

test('editing a message is cancelled on Escape from its field, Cancel or Save', () => {
  const edit = part('../js/chat.js', 'function startEdit(', '\n}\n');
  assert.match(edit, /box\.addEventListener\('keydown', \(e\) => \{ if \(e\.key === 'Escape'\) \{ e\.preventDefault\(\); cancel\(\); \} \}\);/);
  assert.doesNotMatch(edit.slice(edit.indexOf("ta.addEventListener('keydown'")), /Escape/);
});

test('a review comment is cancelled on Escape from its field or its buttons', () => {
  const box = part('../js/panes/review.js', "const ta = tr.querySelector('textarea');", 'return tr;');
  assert.match(box, /tr\.addEventListener\('keydown', \(e\) => \{ if \(e\.key === 'Escape'\) \{ e\.preventDefault\(\); e\.stopPropagation\(\); onCancel\(\); \} \}\);/);
  assert.doesNotMatch(box.slice(box.indexOf("ta.addEventListener('keydown'"), box.indexOf('tr.addEventListener')), /Escape/);
});
