// npm run test:ui — the sidebar's room: it folds by itself when a narrow
// window or an artifact leaves it short, opens again when the room is back,
// and a fold or opening of yours wins.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { tight, roomStep } from '../js/sidebar_room.js';

test('the sidebar is short of room in a narrow window, or beside an artifact under 1500 px', () => {
  assert.ok(tight(700, false));
  assert.ok(tight(760, false));
  assert.ok(!tight(761, false));
  assert.ok(tight(1400, true));
  assert.ok(!tight(1500, true));
  assert.ok(!tight(1400, false));
});

test('the sidebar folds by itself and opens again when the room is back', () => {
  // an artifact opens in a 1200 px window: the open sidebar folds
  assert.equal(roomStep(false, true, false, false), 'fold');
  // the panel closes (or the window widens): it was folded by itself, so it opens
  assert.equal(roomStep(true, false, true, true), 'open');
  // still tight, or room unchanged: nothing
  assert.equal(roomStep(true, true, true, true), null);
  assert.equal(roomStep(false, false, false, false), null);
});

test('a fold or opening of yours wins over the room', () => {
  // you folded it yourself before the panel opened: it stays folded after
  assert.equal(roomStep(false, true, true, false), null);
  assert.equal(roomStep(true, false, true, false), null);
  // you opened it while the panel was open: closing the panel leaves it open
  assert.equal(roomStep(true, false, false, false), null);
});
