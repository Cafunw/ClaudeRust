// npm run test:ui — the artifact panel as a sheet in narrow windows ("Back to
// chat", then the topbar's toggle brings back what was shown).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { reopenId, wrapTab } from '../js/artifact_sheet.js';
import { source } from './source.mjs';

test('the toggle reopens the artifact shown last in this chat, while it is still there', () => {
  const ids = ['a', 'b', 'c'];
  assert.equal(reopenId(ids, 'chat1', { convId: 'chat1', identifier: 'a' }), 'a');
  assert.equal(reopenId(ids, 'chat1', { convId: 'chat2', identifier: 'a' }), 'c', 'another chat\'s: the newest');
  assert.equal(reopenId(ids, 'chat1', { convId: 'chat1', identifier: 'gone' }), 'c', 'no longer on this branch: the newest');
  assert.equal(reopenId(ids, 'chat1', null), 'c');
  assert.equal(reopenId([], 'chat1', null), null);
});

const CSS = source('../css/app.css').replace(/\r\n/g, '\n');
const media = (q) => { const at = CSS.indexOf(`@media (${q}) {\n  .artifact-panel`); assert.ok(at >= 0, q); return CSS.slice(at, CSS.indexOf('\n}', at)); };

test('under 900 px the panel is a sheet over the dimmed chat, with "Back to chat" in place of Close', () => {
  const narrow = media('max-width: 900px');
  assert.match(narrow, /\.artifact-panel \{ position: absolute; right: 0;[^}]*width: min\(640px, 100%\) !important/);
  assert.match(narrow, /\.ap-back \{ display: inline-flex; \}/);
  assert.match(narrow, /\.ap-head \.ap-close \{|\.ap-head \.ap-close/);
  assert.match(narrow, /\.ap-scrim:not\(\.hidden\) \{ display: block;/);
  // and nowhere does the panel cover the whole window with no way back
  assert.doesNotMatch(CSS, /\.artifact-panel \{ position: absolute; inset: 0;/);
});

test('the panel\'s Esc and scrim close it, and the toggle keeps what was shown', () => {
  const src = source('../js/artifacts.js');
  assert.match(src, /scrim\.onclick = close/);
  assert.match(src, /back\.onclick = close/);
  assert.match(src, /e\.key !== 'Escape'/);
  assert.match(src, /lastShown = \{ convId: panel\.convId, identifier: panel\.identifier \}/);
});

test('Tab stays in the sheet: past either end it comes round to the other', () => {
  assert.equal(wrapTab(3, 2, false), 0, 'past the last: the first');
  assert.equal(wrapTab(3, 0, true), 2, 'before the first: the last');
  assert.equal(wrapTab(3, 1, false), -1, 'inside: the browser moves it');
  assert.equal(wrapTab(3, 1, true), -1);
  assert.equal(wrapTab(3, -1, false), 0, 'from no control of the sheet: its first');
  assert.equal(wrapTab(0, -1, false), -1, 'nothing to focus');
});
