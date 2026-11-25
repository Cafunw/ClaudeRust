// npm run test:ui — window.fs.readFile of artifacts (src-tauri/src/artifact_fs.js
// in the frame, readFileFor in ui/js/artifact_bridge.js in the app).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { readFileFor, bytesOf } from '../js/artifact_bridge.js';

const SHIM = readFileSync(new URL('../../src-tauri/src/artifact_fs.js', import.meta.url), 'utf8');

function frame() {
  const listeners = [];
  const posted = [];
  const parent = { postMessage: (m) => posted.push(structuredClone(m)) };
  const win = { parent, addEventListener: (type, fn) => listeners.push({ type, fn }), Promise, Error, String };
  win.window = win;
  vm.createContext(win);
  vm.runInContext(SHIM, win);
  const deliver = (data, source = parent) => listeners.filter((l) => l.type === 'message').forEach((l) => l.fn({ data, source }));
  return { win, posted, deliver, parent };
}

test('the frame asks the app for a file by name and gets its answer back', async () => {
  const f = frame();
  const text = f.win.fs.readFile('data.csv', { encoding: 'utf8' });
  const bytes = f.win.fs.readFile('pic.png');
  const short = f.win.fs.readFile('notes.md', 'utf8');
  assert.deepEqual(f.posted, [
    { __artifact: 'fs', id: 1, name: 'data.csv', encoding: 'utf8' },
    { __artifact: 'fs', id: 2, name: 'pic.png', encoding: null },
    { __artifact: 'fs', id: 3, name: 'notes.md', encoding: 'utf8' },
  ]);
  // only the parent's replies count, and only to a call that waits
  f.deliver({ __artifact: 'fs-reply', id: 1, result: 'forged' }, {});
  f.deliver({ __artifact: 'fs-reply', id: 1, result: 'a,b' });
  f.deliver({ __artifact: 'fs-reply', id: 1, result: 'again' });
  f.deliver({ __artifact: 'fs-reply', id: 2, result: new Uint8Array([137, 80]) });
  f.deliver({ __artifact: 'fs-reply', id: 3, error: 'No file named "notes.md" is attached to this chat.' });
  assert.equal(await text, 'a,b');
  assert.deepEqual([...(await bytes)], [137, 80]);
  await assert.rejects(short, /No file named "notes.md"/);
});

test('the app reads from the frame\'s own conversation only, by name', async () => {
  const asked = [];
  const read = async (args) => { asked.push(args); return args.encoding ? { text: 'x,y' } : { base64: 'iVBO' }; };
  assert.equal(await readFileFor('conv-a', { name: 'data.csv', encoding: 'utf8', conversationId: 'conv-b' }, read), 'x,y');
  const bytes = await readFileFor('conv-a', { name: 'pic.png', encoding: null }, read);
  assert.ok(bytes instanceof Uint8Array);
  assert.deepEqual([...bytes], [0x89, 0x50, 0x4e]);
  assert.deepEqual(asked, [
    { conversationId: 'conv-a', name: 'data.csv', encoding: 'utf8' },
    { conversationId: 'conv-a', name: 'pic.png', encoding: null },
  ], 'what the page posts never picks the conversation');
  // a frame outside a chat, a missing, odd or huge name: refused before the backend is asked
  await assert.rejects(readFileFor(undefined, { name: 'a' }, read), /isn't shown in a chat/);
  for (const name of ['', 7, null, 'x'.repeat(2000)]) await assert.rejects(readFileFor('c', { name }, read), /takes the name/);
  assert.equal(asked.length, 2);
  // the backend's refusal is the page's rejection
  await assert.rejects(readFileFor('c', { name: 'secret.txt' }, async () => { throw new Error('No file named "secret.txt" is attached to this chat.'); }), /secret\.txt/);
  assert.deepEqual([...bytesOf('')], []);
});
