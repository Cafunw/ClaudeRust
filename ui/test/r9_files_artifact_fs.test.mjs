// npm run test:ui — window.fs.readFile of artifacts (src-tauri/src/artifact_fs.js
// in the frame, readFileFor in ui/js/artifact_bridge.js in the app) and the
// helpers of the attachment preview (ui/js/attachment_preview.js).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { readFileFor, bytesOf } from '../js/artifact_bridge.js';
import { langOf, isDocument, messageSource, hookPreview } from '../js/attachment_preview.js';

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

test('what the preview highlights, and where a sent attachment\'s data is', () => {
  assert.equal(langOf('main.rs'), 'rs');
  assert.equal(langOf('Data.JSON'), 'json');
  assert.equal(langOf('notes.txt'), '');
  assert.equal(langOf('report.docx'), '');
  assert.equal(langOf('README'), '');
  assert.ok(isDocument('book.EPUB') && isDocument('a.rtf') && isDocument('a.odt') && !isDocument('a.md'));
  const list = { children: [] };
  const turn = (id) => ({ closest: (sel) => (sel === '.turn[data-id]' ? { dataset: { id } } : null) });
  const img = { parentElement: list };
  const att = { parentElement: list, closest: turn('msg-1').closest };
  list.children.push(img, att);
  assert.deepEqual(messageSource(att), { messageId: 'msg-1', index: 1 }, 'its place among the images and files of the message');
  assert.equal(messageSource({ parentElement: list, closest: turn('pending-3').closest }), null, 'not stored yet');
  assert.equal(messageSource({ parentElement: list, closest: () => null }), null);
});

test('a staged chip opens its preview from its name, by keyboard too', () => {
  const attrs = {};
  const handlers = {};
  const info = { setAttribute: (k, v) => { attrs[k] = v; }, addEventListener: (type, fn) => { handlers[`info:${type}`] = fn; } };
  const classes = new Set();
  const chip = { querySelector: (sel) => (sel === '.info' ? info : null), classList: { add: (c) => classes.add(c) }, addEventListener: (type, fn) => { handlers[`chip:${type}`] = fn; } };
  hookPreview(chip, { stagedId: 's1' }, 'report.pdf');
  assert.equal(attrs.role, 'button');
  assert.equal(info.tabIndex, 0);
  assert.equal(attrs['aria-label'], 'Preview report.pdf');
  assert.ok(classes.has('previewable') && handlers['chip:click'] && handlers['info:keydown']);
  // the chip's remove button keeps its click, and a key other than Enter or Space does nothing
  // (opening would build the dialog, which needs a DOM this test hasn't)
  handlers['chip:click']({ target: { closest: (sel) => (sel === 'button' ? {} : null) } });
  let prevented = false;
  handlers['info:keydown']({ key: 'a', preventDefault: () => { prevented = true; } });
  assert.equal(prevented, false);
  assert.throws(() => handlers['info:keydown']({ key: 'Enter', preventDefault: () => { prevented = true; } }));
  assert.ok(prevented, 'Enter opens it');
});
