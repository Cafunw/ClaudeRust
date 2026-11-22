// npm run test:ui
// Protocol 3 edits of ui/js/turnstream_delta.js (src-tauri/src/turnstream.rs
// writes them; turnstream_delta.test.mjs decodes a whole encoder fixture).
import test from 'node:test';
import assert from 'node:assert/strict';
import { element, PROTOCOL } from '../js/turnstream_delta.js';

test('the decoder announces protocol 3', () => {
  assert.equal(PROTOCOL, 3);
});

test('a splice replaces units in the middle, in the string as edited so far', () => {
  const prev = { kind: 'text', html: '<div data-open><p>Ça 😀 a</p></div>', raw: 'Ça 😀 a' };
  // " data-open" goes, then the end grows
  const got = element([['html', 4, 10, ''], ['html', 13, 12, ' ab</p></div>'], ['raw', 6, 'ab']], prev);
  assert.deepEqual(got, { kind: 'text', html: '<div><p>Ça 😀 ab</p></div>', raw: 'Ça 😀 ab' });
  assert.equal(prev.html, '<div data-open><p>Ça 😀 a</p></div>', 'the previous element is left as it was');
});

test('lists grow by their new items, other fields are set', () => {
  const prev = { kind: 'tool', status: 'running', result: null, sources: [{ url: 'a' }] };
  const got = element([['sources', 1, [{ url: 'b' }]], ['result', { text: 'ok' }], ['status', 0, 'done']], prev);
  assert.deepEqual(got, { kind: 'tool', status: 'done', result: { text: 'ok' }, sources: [{ url: 'a' }, { url: 'b' }] });
  assert.deepEqual(element([['sources', 0, []]], prev).sources, []);
});

test('an object field is edited in turn', () => {
  const prev = { kind: 'tool', input: { file_path: 'a.rs', content: 'fn a' } };
  const got = element([['input', null, [['content', 4, '() {}']]]], prev);
  assert.deepEqual(got.input, { file_path: 'a.rs', content: 'fn a() {}' });
  assert.equal(prev.input.content, 'fn a');
});

test('edits that do not fit are refused', () => {
  const prev = { s: 'abc', list: [1, 2], o: { x: 'y' }, n: 1 };
  const bad = [
    [['s', 2, 2, 'x']], // removes past the end
    [['s', 4, 0, 'x']],
    [['s', -1, 1, 'x']],
    [['s', 0, 1, 5]],
    [['list', 3, []]],
    [['list', 1, 'x']], // a string appended to a list
    [['s', 1, ['x']]], // a list appended to a string
    [['n', 0, 1, 'x']],
    [['o', null, [['x', 5, 'z']]]],
    [['o', null, 'x']],
    [['n', null, [['x', 0, 'z']]]],
    [['s', 0, 0, 'x', 'y']],
    [[1, 'x']],
    [['s']],
  ];
  for (const ops of bad) assert.equal(element(ops, prev), undefined, JSON.stringify(ops));
});
