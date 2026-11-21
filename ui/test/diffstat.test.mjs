// npm run test:ui — line diffs of the file tools and the "N files edited"
// summary of a code turn (ui/js/diffstat.js).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { editedFiles, lineDiff } from '../js/diffstat.js';

test('lineDiff trims the common lines and keeps two lines of context', () => {
  assert.deepEqual(lineDiff('a\nb\nc\nd\ne\nf', 'a\nb\nc\nX\ne\nf'), [
    ['ctx', 'b'], ['ctx', 'c'], ['del', 'd'], ['add', 'X'], ['ctx', 'e'], ['ctx', 'f'],
  ]);
  assert.deepEqual(lineDiff('x\ny', 'x\nz\nw'), [['ctx', 'x'], ['del', 'y'], ['add', 'z'], ['add', 'w']]);
  assert.deepEqual(lineDiff(null, 'one'), [['del', ''], ['add', 'one']]);
});

const tool = (name, input, extra = {}) => ({ kind: 'tool', name, input, ...extra });

test('editedFiles counts the lines each file tool changed, per file, relative to the folder', () => {
  const parts = [
    { kind: 'text', text: 'Editing.' },
    tool('Write', { file_path: 'C:\\w\\src\\new.rs', content: 'a\nb\n' }, { result: { text: 'File created successfully at C:\\w\\src\\new.rs' } }),
    tool('Edit', { file_path: 'c:/w/src/lib.rs', old_string: 'x\ny', new_string: 'x\nz\nw' }),
    tool('MultiEdit', { file_path: 'C:\\w\\src\\lib.rs', edits: [{ old_string: 'p', new_string: 'q' }] }),
    tool('Bash', { command: 'ls' }),
    tool('Edit', { file_path: 'C:\\elsewhere\\x.txt', old_string: 'a', new_string: 'b' }),
  ];
  assert.deepEqual(editedFiles(parts, 'C:\\w'), [
    { path: 'src/new.rs', added: 2, removed: 0, created: true },
    { path: 'src/lib.rs', added: 3, removed: 2, created: false },
    { path: 'C:\\elsewhere\\x.txt', added: 1, removed: 1, created: false },
  ]);
});

test('editedFiles leaves out calls that changed nothing', () => {
  const parts = [
    tool('Write', { file_path: '/w/a', content: 'x' }, { status: 'denied' }),
    tool('Write', { file_path: '/w/b', content: 'x' }, { status: 'error' }),
    tool('Edit', { file_path: '/w/c', old_string: 'a', new_string: 'b' }, { status: 'cancelled' }),
    tool('Edit', { file_path: '/w/d', old_string: 'a', new_string: 'b' }, { result: { is_error: true, text: 'no match' } }),
    tool('Edit', { old_string: 'a', new_string: 'b' }),
  ];
  assert.deepEqual(editedFiles(parts, '/w'), []);
  assert.deepEqual(editedFiles(undefined, '/w'), []);
});
