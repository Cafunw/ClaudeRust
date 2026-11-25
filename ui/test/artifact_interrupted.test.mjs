// npm run test:ui — an artifact cut off (a Stop, an error, the app closed;
// render.rs marks it) says so on its card and opens in its normal preview,
// marked incomplete; one still being written shows its source as it comes.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { setLanguage } from '../js/i18n.js';
import { artifactCardSub } from '../js/parts.js';
import { panelView } from '../js/artifact_view.js';
import { resolveVersions } from '../js/artifact_update.js';

test('a card says whether its artifact is being written, was cut off, or opens', () => {
  setLanguage('en');
  assert.equal(artifactCardSub('Document', 'false'), 'Document · Generating…');
  assert.equal(artifactCardSub('Document', 'partial'), 'Document · Interrupted');
  assert.equal(artifactCardSub('Document', 'true'), 'Document · Click to open');
  setLanguage('tr');
  assert.equal(artifactCardSub('Document', 'partial'), 'Belge · Yarıda kesildi');
  setLanguage('en');
});

const md = { type: 'text/markdown', previewable: false };
const html = { type: 'text/html', previewable: true };
const code = { type: 'application/vnd.ant.code', previewable: false, language: 'python' };

test('one cut off opens in its preview, as a finished one does; one being written shows its source', () => {
  assert.deepEqual(panelView({ ...md, complete: false, interrupted: true }), { live: false, previewable: true, view: 'preview', wrap: true });
  assert.deepEqual(panelView({ ...md, complete: false }), { live: true, previewable: true, view: 'code', wrap: true });
  assert.equal(panelView({ ...html, complete: false, interrupted: true }).view, 'preview');
  assert.equal(panelView({ ...html, complete: false }, 'preview').view, 'code', 'no preview while it streams');
  // the person's choice holds for one cut off too
  assert.equal(panelView({ ...html, complete: false, interrupted: true }, 'code').view, 'code');
  assert.equal(panelView({ ...code, complete: false, interrupted: true }).view, 'code', 'code has no preview');
});

test('prose source wraps at the panel edge; code keeps its lines', () => {
  assert.equal(panelView({ type: 'application/vnd.ant.document', complete: true, previewable: true }, 'code').wrap, true);
  assert.equal(panelView({ ...code, complete: true }).wrap, false);
  assert.equal(panelView({ ...html, complete: true }, 'code').wrap, false);
});

test('an update is cut off only when it was, not because the version it edits was', () => {
  const v1 = { identifier: 's', version: 1, type: 'text/markdown', title: 'S', content: 'Once upon', complete: false, interrupted: true };
  const edit = '<old_str>Once upon</old_str>\n<new_str>Once upon a time.</new_str>';
  const [, done] = resolveVersions([v1, { identifier: 's', version: 2, update: true, content: edit, complete: true }]);
  assert.deepEqual([done.content, done.complete, done.interrupted], ['Once upon a time.', true, undefined]);
  const [, streaming] = resolveVersions([v1, { identifier: 's', version: 2, update: true, content: edit.slice(0, 20), complete: false }]);
  assert.equal(panelView(streaming).live, true, 'still being written');
});
