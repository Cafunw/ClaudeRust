// npm run test:ui — artifact versions written as edits (ui/js/artifact_update.js).
import test from 'node:test';
import assert from 'node:assert/strict';
import { applyEdits, parseEdits, resolveVersions } from '../js/artifact_update.js';

const edit = (...pairs) => pairs.map(([o, n]) => `<old_str>${o}</old_str>\n<new_str>${n}</new_str>`).join('\n');

test('an old string found once is replaced', () => {
  assert.deepEqual(applyEdits('<h1>Hi</h1><p>x</p>', edit(['<h1>Hi</h1>', '<h1>Hello</h1>'])), { content: '<h1>Hello</h1><p>x</p>' });
  // the replacement is text, not a pattern
  assert.equal(applyEdits('a b', edit(['b', '$& $1 $$'])).content, 'a $& $1 $$');
});

test('an old string not found, or found more than once, fails and leaves the content', () => {
  const r = applyEdits('one two', edit(['three', '3']));
  assert.equal(r.content, 'one two');
  assert.match(r.error, /not found/);
  const twice = applyEdits('x = 1; y = 1;', edit(['= 1', '= 2']));
  assert.equal(twice.content, 'x = 1; y = 1;');
  assert.match(twice.error, /more than once/);
  // overlapping matches are two matches too
  assert.match(applyEdits('aaa', edit(['aa', 'b'])).error, /more than once/);
  assert.match(applyEdits('abc', edit(['', 'b'])).error, /empty/);
  assert.match(applyEdits('abc', 'just text').error, /no <old_str>/);
});

test('edits apply in order, all of them or none', () => {
  // the second sees what the first wrote
  assert.equal(applyEdits('let a = 1;', edit(['a = 1', 'b = 1'], ['b = 1;', 'b = 2;'])).content, 'let b = 2;');
  const r = applyEdits('alpha beta', edit(['alpha', 'ALPHA'], ['gamma', 'GAMMA']));
  assert.equal(r.content, 'alpha beta', 'the first edit is not kept when the second fails');
  assert.match(r.error, /old_str 2 was not found/);
});

test('a newline right inside a tag is layout', () => {
  assert.deepEqual(parseEdits('<old_str>\nline 1\nline 2\n</old_str>\n<new_str>\nline 1\n</new_str>'), [{ old: 'line 1\nline 2', new: 'line 1' }]);
  assert.equal(applyEdits('a\nline 1\nline 2\nb', '<old_str>\nline 1\nline 2\n</old_str><new_str>\nline 1\n</new_str>').content, 'a\nline 1\nb');
});

test('an artifact with CRLF line ends is edited with them', () => {
  const r = applyEdits('a\r\nb\r\nc\r\n', edit(['b\nc', 'x\ny']));
  assert.equal(r.content, 'a\r\nx\r\ny\r\n');
  // LF content is matched as it is
  assert.equal(applyEdits('a\nb', edit(['a\nb', 'c'])).content, 'c');
});

test('an unfinished pair, still streaming, is not an edit yet', () => {
  assert.deepEqual(parseEdits('<old_str>a</old_str><new_str>b</new_str>\n<old_str>c</old_str><new_str>d'), [{ old: 'a', new: 'b' }]);
});

const v1 = { identifier: 'app', version: 1, type: 'text/html', label: 'HTML', previewable: true, ext: 'html', title: 'App', content: '<p>one</p>', complete: true };

test('an update becomes a whole version, with the type and title of the one before', () => {
  const list = [v1, { identifier: 'app', version: 2, type: 'text/markdown', title: '', update: true, complete: true, content: edit(['one', 'two']) }];
  const [, v2] = resolveVersions(list);
  assert.equal(v2.content, '<p>two</p>');
  assert.equal(v2.version, 2);
  assert.equal(v2.type, 'text/html');
  assert.equal(v2.previewable, true);
  assert.equal(v2.title, 'App');
  assert.equal(v2.error, undefined);
  // updates chain, and a full rewrite after them stands as it is
  const v3 = { identifier: 'app', version: 3, title: 'New title', update: true, complete: true, content: edit(['two', 'three']) };
  const v4 = { ...v1, version: 4, content: '<p>four</p>' };
  const r = resolveVersions([...list, v3, v4]);
  assert.deepEqual(r.map((v) => v.content), ['<p>one</p>', '<p>two</p>', '<p>three</p>', '<p>four</p>']);
  assert.equal(r[2].title, 'New title');
  assert.equal(r[3], v4);
});

test('a failed update keeps the content before it and says why, once it is complete', () => {
  const bad = { identifier: 'app', version: 2, update: true, complete: true, content: edit(['missing', 'x']) };
  const [, v2] = resolveVersions([v1, bad]);
  assert.equal(v2.content, '<p>one</p>');
  assert.match(v2.error, /not found/);
  // while it streams: the edits so far, no error
  const streaming = resolveVersions([v1, { ...bad, complete: false, content: '<old_str>one</old_str><new_str>1</new_str><old_str>mis' }])[1];
  assert.equal(streaming.content, '<p>1</p>');
  assert.equal(streaming.error, undefined);
  // nothing to update
  assert.match(resolveVersions([bad])[0].error, /no earlier version/);
});
