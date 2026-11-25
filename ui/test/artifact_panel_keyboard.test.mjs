// npm run test:ui — Tab never strands the keyboard after a frame (an artifact's preview, the
// Code preview pane): the frame's own keys never reach the page, so a stop right after it passes
// the focus on, round the panel, dialog or sheet it is in, or round the app (a11y.js frameEnd).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { source } from './source.mjs';
import { frameExit } from '../js/a11y.js';

// stand-ins placed in document order by `at`
const node = (at, name) => ({ at, name, compareDocumentPosition: (o) => (o.at > at ? 4 : 2) });

test('Tab out of the last frame of the page comes round to the first control, not to nowhere', () => {
  const [first, mid, frame, end] = [node(1, 'sidebar'), node(5, 'close'), node(8, 'frame'), node(9, 'end')];
  assert.equal(frameExit(end, frame, null, [first, mid, frame], null), first);
});

test('Tab out of a frame with controls after it goes on to the next one', () => {
  const [first, frame, end, next] = [node(1, 'a'), node(5, 'frame'), node(6, 'end'), node(7, 'b')];
  assert.equal(frameExit(end, frame, null, [first, frame, next], null), next);
});

test('Shift+Tab from a control after the frame goes back into the frame', () => {
  const [frame, end, next] = [node(5, 'frame'), node(6, 'end'), node(7, 'b')];
  assert.equal(frameExit(end, frame, next, [frame, next], null), frame);
  // (focus coming out of the frame itself is a Tab forward)
  assert.equal(frameExit(end, frame, frame, [node(1, 'a'), frame, next], null), next);
});

test('in full screen, a narrow window\'s sheet or a dialog, Tab out of the frame comes round to its first control', () => {
  const [back, close, frame, end] = [node(3, 'back'), node(4, 'close'), node(5, 'frame'), node(6, 'end')];
  assert.equal(frameExit(end, frame, null, [back, close, frame], { box: true }), back);
  assert.equal(frameExit(end, frame, null, [], { box: true }), null);
});

test('the stop is never a stop of its own: wraps and traps skip it', () => {
  const a11y = source('../js/a11y.js');
  assert.match(a11y, /\[tabindex\]:not\(\[tabindex="-1"\]\):not\(\.tab-end\), iframe';/);
  assert.match(a11y, /end\.className = 'sr-only tab-end';\n\s*end\.tabIndex = 0;/);
  assert.match(source('../js/artifacts.js'), /const sheetControls = \(\) => \[\.\.\.aside\(\)\.querySelectorAll\('[^']*\[tabindex\]:not\(\[tabindex="-1"\]\):not\(\.tab-end\)'\)\]/);
});

test('every artifact preview frame and the Code preview pane have the stop after them', () => {
  // one place draws the panel's frame for every kind (HTML, React, SVG, Mermaid, typed); full screen keeps Tab in the panel
  assert.match(source('../js/artifacts.js'), /body\.append\(frame, frameEnd\(frame, \(\) => \(full \? aside\(\) : null\)\)\);/);
  const pv = source('../js/panes/preview.js');
  assert.match(pv, /frame\.after\(frameEnd\(frame\)\);/);
  assert.match(pv, /<iframe class="pv-frame" title="\$\{esc\(t\('ws_preview'\)\)\}"/, 'the pane\'s frame has a name');
});

test('a sheet\'s grid lets Tab go on out of it at a row\'s ends (Tab there used to stay on the cell for good)', () => {
  const sheet = source('../js/artifact_types/sheet.js');
  assert.match(sheet, /else if \(k === 'Tab'\) \{ const c = sel\.c \+ \(e\.shiftKey \? -1 : 1\); if \(c < 0 \|\| c >= width\(\)\) return; e\.preventDefault\(\); move\(0, e\.shiftKey \? -1 : 1\); \}/);
  // one cell at a time is a stop of Tab (a roving tabindex), so the default Tab leaves the grid
  assert.match(sheet, /tabindex="\$\{sel\.r === r && sel\.c === c \? 0 : -1\}"/);
});
