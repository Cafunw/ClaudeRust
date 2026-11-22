// npm run test:ui — pure parts of Settings → Skills, Memory and the extension forms.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { scriptFiles, sessionFolders } from '../js/skills_page.js';
import { memoryOf } from '../js/memory_page.js';
import { fieldValue, installedExtension } from '../js/extensions_page.js';

test('a server is an installed extension when its config entry says where the extension is', () => {
  const status = { config: { mcpServers: {
    demo: { command: 'node', extension: { path: 'C:/data/extensions/demo', version: '1.2.3', display_name: 'Demo' } },
    hand: { command: 'npx' },
    odd: { command: 'x', extension: 'yes' },
  } } };
  assert.deepEqual(installedExtension(status, 'demo'), { name: 'demo', version: '1.2.3', display_name: 'Demo' });
  for (const n of ['hand', 'odd', 'missing']) assert.equal(installedExtension(status, n), null, n);
  assert.equal(installedExtension({}, 'demo'), null);
});

test('a skill’s scripts and programs are found by extension or a scripts folder', () => {
  const files = ['SKILL.md', 'reference.md', 'scripts/run', 'tools/fill.py', { path: 'bin\\x.EXE', size: 3 }, 'a/b.ps1', 'notes.txt', 'templates/form.docx'];
  assert.deepEqual(scriptFiles(files), ['scripts/run', 'tools/fill.py', 'bin\\x.EXE', 'a/b.ps1']);
  assert.deepEqual(scriptFiles([]), []);
  assert.deepEqual(scriptFiles(undefined), []);
  assert.deepEqual(scriptFiles(['description-scripts.md']), []);
});

test('session folders: the open conversation’s first, each once, chats without a folder left out', () => {
  const convs = [{ id: 'a', cwd: 'D:/one' }, { id: 'b' }, { id: 'c', cwd: 'D:/two' }, { id: 'd', cwd: 'D:/one' }];
  assert.deepEqual(sessionFolders(convs, 'c'), ['D:/two', 'D:/one']);
  assert.deepEqual(sessionFolders(convs, 'b'), ['D:/one', 'D:/two']);
  assert.deepEqual(sessionFolders(undefined, 'x'), []);
});

test('the memory of a scope is found in chat_memory_list’s reply, an empty one when there is none', () => {
  const list = { memories: [{ scope: 'global', project_id: null, text: 'all', updated_at: 5 }, { scope: 'p1', project_id: 'p1', text: 'one', updated_at: 7 }] };
  assert.equal(memoryOf(list, 'global').text, 'all');
  assert.equal(memoryOf(list, 'p1').text, 'one');
  assert.deepEqual(memoryOf(list, 'p2'), { scope: 'p2', text: '', updated_at: 0 });
  assert.deepEqual(memoryOf({}, 'global'), { scope: 'global', text: '', updated_at: 0 });
  assert.equal(memoryOf({ memories: [{ project_id: 'p3', text: 'by project id' }] }, 'p3').text, 'by project id');
});

test('extension settings: types, lists, and a saved secret left empty is kept', () => {
  assert.equal(fieldValue({ type: 'boolean' }, 1), true);
  assert.equal(fieldValue({ type: 'number' }, ' 42 '), 42);
  assert.equal(fieldValue({ type: 'number' }, ''), null);
  assert.deepEqual(fieldValue({ multiple: true }, 'a\n\n b \n'), ['a', 'b']);
  assert.equal(fieldValue({ multiple: true }, '  '), null);
  assert.equal(fieldValue({}, '  x '), 'x');
  assert.equal(fieldValue({}, ''), null);
  assert.equal(fieldValue({ sensitive: true }, '', true), undefined);
  assert.equal(fieldValue({ sensitive: true }, '', false), null);
  assert.equal(fieldValue({ sensitive: true }, 'new', true), 'new');
});
