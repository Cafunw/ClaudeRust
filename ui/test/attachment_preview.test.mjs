// npm run test:ui — the attachment preview (ui/js/attachment_preview.js): what
// it highlights, where a sent attachment's data is, and opening it from a chip.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { langOf, isDocument, messageSource, hookPreview } from '../js/attachment_preview.js';

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
