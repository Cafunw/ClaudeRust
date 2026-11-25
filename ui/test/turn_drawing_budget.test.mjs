// npm run test:ui — opening or scrolling up a long conversation draws its turns
// (and parked turns coming back) a frame's worth at a time, so no frame runs long.
import test from 'node:test';
import assert from 'node:assert/strict';
import { drawWithin, FRAME_BUDGET, OPEN_BUDGET } from '../js/turnwindow.js';
import { source } from './source.mjs';

/** A clock that `draw` moves on by `costs[i]` ms. */
const clocked = (costs) => {
  let t = 0;
  const drawn = [];
  return { drawn, now: () => t, draw: (i) => { drawn.push(i); t += costs[i]; } };
};

test('draws while the budget lasts, at least one', () => {
  const a = clocked([2, 2, 2, 2, 2]);
  assert.equal(drawWithin(5, a.draw, 6, a.now), 3);
  assert.deepEqual(a.drawn, [0, 1, 2]);
  const b = clocked([40, 1]);
  assert.equal(drawWithin(2, b.draw, 6, b.now), 1, 'one huge turn still gets drawn');
  const c = clocked([0.1, 0.1, 0.1]);
  assert.equal(drawWithin(3, c.draw, 6, c.now), 3, 'small turns all in one frame');
  assert.equal(drawWithin(0, () => assert.fail('nothing to draw'), 6, () => 0), 0);
  assert.ok(FRAME_BUDGET > 0 && FRAME_BUDGET < 16);
});

test('older turns, the opened page and parked turns are drawn through the budget', () => {
  const chat = source('../js/chat.js');
  const fn = (name) => chat.slice(chat.indexOf(`function ${name}(`), chat.indexOf('\n}\n', chat.indexOf(`function ${name}(`)));
  // each turn laid out as it comes, so the budget counts its layout
  assert.match(fn('drawBefore'), /drawWithin\(c\.shown - start, \(\) => \{[\s\S]*upsertTurn\([\s\S]*void c\.colEl\.offsetHeight;\s*\}, budget\);/);
  // a chunk in a frame of its own, after the frame's other work: never in the scroll event that asked for it
  assert.match(fn('drawBack'), /while \(cur === c && c\.shown > start\) \{\s*await new Promise\(\(r\) => requestAnimationFrame\(r\)\);[\s\S]*drawBefore\(c, start, FRAME_BUDGET\);/);
  assert.match(fn('drawEarlier'), /c\.stick = false;\s*await drawBack\(c, start\);/);
  // opening: a frame's worth from the end now, the rest after, which a scroll up waits for
  assert.match(chat, /cur\.shown = data\.turns\.length;\s*const from = pageStart\(data\.turns, cur\.shown\);\s*drawBefore\(cur, from, OPEN_BUDGET\);\s*if \(cur\.shown > from\) \{\s*const c = cur;\s*c\.earlierP = drawBack\(c, from\)/);
  assert.ok(OPEN_BUDGET > FRAME_BUDGET && OPEN_BUDGET <= 16);
  assert.match(chat, /if \(back\.length && unpark\(c, back, FRAME_BUDGET\) < back\.length\) parkLater\(\);/);
  // the find bar still gets every turn at once
  assert.match(chat, /if \(list\.length\) unpark\(c, list\);/);
});
