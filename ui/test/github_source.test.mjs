// npm run test:ui — the GitHub and shared-link sources' pure helpers
// (ui/js/github_source.js; the dialogs need a DOM).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { fileTree, pickSummary, sourceError } from '../js/github_source.js';
import { setLanguage } from '../js/i18n.js';

test('the picked files become folders under the address’s folder', () => {
  const files = [{ path: 'src/a.rs', size: 1 }, { path: 'src/ui/b.js', size: 2 }, { path: 'src/c.md', size: 3 }];
  const top = fileTree(files, 'src');
  assert.deepEqual(top.files.map((f) => [f.name, f.i]), [['a.rs', 0], ['c.md', 2]]);
  assert.deepEqual([...top.dirs.keys()], ['ui']);
  assert.deepEqual(top.dirs.get('ui').files.map((f) => [f.name, f.path, f.size]), [['b.js', 'src/ui/b.js', 2]]);
  const all = fileTree([{ path: 'README.md' }, { path: 'docs/x/y.md' }], '');
  assert.deepEqual(all.files.map((f) => f.name), ['README.md']);
  assert.equal(all.dirs.get('docs').dirs.get('x').files[0].name, 'y.md');
  // a /blob/ address lists the one file
  assert.deepEqual(fileTree([{ path: 'src/main.rs' }], 'src/main.rs').files.map((f) => f.name), ['main.rs']);
});

test('a pick is held against the caps the backend sent', () => {
  const limits = { files: 2, bytes: 10 };
  assert.deepEqual(pickSummary([{ size: 4 }, { size: 6 }], limits), { count: 2, bytes: 10, over: null });
  assert.equal(pickSummary([{ size: 1 }, { size: 1 }, { size: 1 }], limits).over, 'files');
  assert.equal(pickSummary([{ size: 11 }], limits).over, 'bytes');
  assert.deepEqual(pickSummary([], limits), { count: 0, bytes: 0, over: null });
});

test('a source’s refusals are said in the UI’s language', () => {
  setLanguage('tr');
  assert.equal(sourceError({ kind: 'gdrive_private', message: 'This file is not shared by link' }).startsWith('Bu dosya bağlantıyla paylaşılmamış'), true);
  // a file that failed carries its name; a refused attachment says it its own way
  assert.equal(sourceError({ kind: 'github_not_text', message: 'x' }, 'b/logo.txt'), 'b/logo.txt: Bu bir metin dosyası değil.');
  assert.ok(sourceError({ kind: 'unsupported_file', message: 'x', name: 'a.bin' }, 'a.bin').includes('a.bin'));
  // a kind the UI doesn't know keeps the backend's text
  assert.equal(sourceError({ kind: 'something_new', message: 'From the backend' }), 'From the backend');
  setLanguage('en');
  assert.equal(sourceError({ kind: 'github_rate_limited', message: 'x' }), 'GitHub’s limit of 60 requests an hour without a token is used up. Try again later, or add an access token (5,000 an hour).');
});
