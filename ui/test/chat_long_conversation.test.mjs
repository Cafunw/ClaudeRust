// npm run test:ui — long conversations (ui/js/turnwindow.js): far turns are
// parked, and a closed conversation's memory is given back (the trimmer).
import test from 'node:test';
import assert from 'node:assert/strict';
import { TRIM_TURNS, trimmer, parked } from '../js/turnwindow.js';

test('far turns are parked, near ones drawn, and the band between keeps each as it is', () => {
  const view = 800;
  assert.equal(parked(0, view, true), false, 'on screen');
  assert.equal(parked(1000, view, true), false, 'within two screens');
  assert.equal(parked(4000, view, false), true, 'beyond four screens');
  assert.equal(parked(2500, view, false), false, 'between: drawn stays drawn');
  assert.equal(parked(2500, view, true), true, 'between: parked stays parked');
});

function setup(t, busy = () => false) {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let trims = 0;
  const left = trimmer(() => { trims++; }, busy, 1000);
  return { left, trims: () => trims, tick: (ms) => t.mock.timers.tick(ms) };
}

test('leaving a long conversation trims once, a moment later', (t) => {
  const s = setup(t);
  s.left(TRIM_TURNS + 1);
  s.tick(999);
  assert.equal(s.trims(), 0, 'not before the wait');
  s.tick(1);
  assert.equal(s.trims(), 1);
  s.tick(10_000);
  assert.equal(s.trims(), 1, 'and only once');
});

test('leaving short conversations and other views never trims', (t) => {
  const s = setup(t);
  for (let i = 0; i < 5; i++) { s.left(TRIM_TURNS - 1); s.tick(2000); }
  assert.equal(s.trims(), 0);
});

test('ten quick switches through a long chat trim once, after the last', (t) => {
  const s = setup(t);
  for (let i = 0; i < 10; i++) { s.left(i % 2 ? 4 : 40); s.tick(400); }
  assert.equal(s.trims(), 0, 'not while switching');
  s.tick(1000);
  assert.equal(s.trims(), 1, 'the long chat left early in the run still counts');
  s.left(4);
  s.tick(1000);
  assert.equal(s.trims(), 1, 'a later short chat does not trim again');
});

test('a streaming answer puts the trim off until it ends', (t) => {
  let streaming = true;
  const s = setup(t, () => streaming);
  s.left(600);
  s.tick(3000);
  assert.equal(s.trims(), 0);
  streaming = false;
  s.tick(1000);
  assert.equal(s.trims(), 1);
});
