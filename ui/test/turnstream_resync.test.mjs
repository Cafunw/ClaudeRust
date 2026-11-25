// npm run test:ui — after a missed stream event, the view asks for a full turn.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { receive } from '../js/turnstream.js';

test('a delta that cannot be expanded asks once for a full turn, until one comes', () => {
  const state = new Map();
  const waiting = new Set();
  const asked = [];
  const ask = (c) => asked.push(c);
  const turn = { id: 't', streaming: true, parts: [{ kind: 'text', html: 'a' }], artifacts: [] };
  assert.ok(receive({ conversation_id: 'c', seq: 1, turn }, ask, state, waiting), 'a keyframe');
  const missed = { conversation_id: 'c', seq: 5, base: 4, turn: { ...turn, parts: [null] } };
  assert.equal(receive(missed, ask, state, waiting), null);
  assert.equal(receive({ ...missed, seq: 6, base: 5 }, ask, state, waiting), null);
  assert.deepEqual(asked, ['c'], 'asked once');
  assert.ok(receive({ conversation_id: 'c', seq: 7, turn }, ask, state, waiting), 'the full turn');
  assert.equal(receive({ ...missed, seq: 9, base: 8 }, ask, state, waiting), null);
  assert.deepEqual(asked, ['c', 'c'], 'missed again: asked again');
  assert.equal(receive(null, ask, state, waiting), null);
  assert.deepEqual(asked, ['c', 'c'], 'nothing to ask about');
});
