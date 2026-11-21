// npm run test:ui — expanding the backend's `turn` keyframes and deltas
// (src-tauri/src/turnstream.rs) in ui/js/turnstream.js.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { expand, forget } from '../js/turnstream.js';

const turn = (over = {}) => ({
  id: 't1',
  streaming: true,
  parts: [{ kind: 'text', text: 'Hello' }, { kind: 'tool', id: 'x', status: 'running' }],
  artifacts: [],
  msg_ids: ['m1'],
  siblings: { index: 0, count: 1, ids: ['t1'] },
  meta: { model: 'claude-opus-5-5' },
  ...over,
});

const keyframe = (seq, t = turn()) => ({ conversation_id: 'c', seq, turn: t });

test('a keyframe is passed on and kept while its turn streams', () => {
  const state = new Map();
  const t = turn();
  assert.deepEqual(expand(keyframe(1, t), state), { conversation_id: 'c', turn: t });
  assert.equal(state.get('c').seq, 1);
});

test('a delta gets the unchanged elements and values from the event it was made from', () => {
  const state = new Map();
  const first = turn();
  expand(keyframe(1, first), state);
  const delta = {
    conversation_id: 'c', seq: 2, base: 1, same: ['msg_ids', 'siblings', 'meta'],
    turn: { id: 't1', streaming: true, parts: [null, { kind: 'tool', id: 'x', status: 'done' }, { kind: 'text', text: 'More' }], artifacts: [], msg_ids: null, siblings: null, meta: null },
  };
  const out = expand(delta, state).turn;
  assert.deepEqual(out.parts, [first.parts[0], { kind: 'tool', id: 'x', status: 'done' }, { kind: 'text', text: 'More' }]);
  assert.equal(out.msg_ids, first.msg_ids);
  assert.equal(out.siblings, first.siblings);
  assert.equal(out.meta, first.meta);
  assert.equal(delta.turn.parts[0], null, 'the payload, shared by every listener, is left as it came');
  // the next delta builds on the expanded one
  const next = expand({ conversation_id: 'c', seq: 3, base: 2, same: [], turn: { ...out, parts: [null, null, { kind: 'text', text: 'More!' }], artifacts: [] } }, state).turn;
  assert.deepEqual(next.parts.map((p) => p.text || p.status), ['Hello', 'done', 'More!']);
});

test('a delta made from an event this page missed is dropped until the next keyframe', () => {
  const state = new Map();
  assert.equal(expand({ conversation_id: 'c', seq: 2, base: 1, turn: turn() }, state), null, 'a reloaded page has nothing');
  expand(keyframe(1), state);
  assert.equal(expand({ conversation_id: 'c', seq: 5, base: 4, turn: turn({ parts: [null] }) }, state), null);
  assert.equal(state.get('c').seq, 1, 'what the page has is kept');
  assert.ok(expand(keyframe(6), state));
  assert.ok(expand({ conversation_id: 'c', seq: 7, base: 6, turn: turn({ parts: [null, null] }) }, state));
});

test('a delta that leaves out what the page never had ends the stream', () => {
  const state = new Map();
  expand(keyframe(1, turn({ parts: [{ kind: 'text', text: 'a' }] })), state);
  assert.equal(expand({ conversation_id: 'c', seq: 2, base: 1, turn: turn({ parts: [null, null] }) }, state), null);
  assert.equal(state.has('c'), false);
  expand(keyframe(3), state);
  assert.equal(expand({ conversation_id: 'c', seq: 4, base: 3, same: ['nope'], turn: turn() }, state), null);
  assert.equal(state.has('c'), false);
});

test('a finished turn, a turn without seq or forget() ends the stream', () => {
  const state = new Map();
  expand(keyframe(1), state);
  const done = turn({ streaming: false });
  assert.deepEqual(expand({ conversation_id: 'c', seq: 2, turn: done }, state).turn, done);
  assert.equal(state.has('c'), false);
  assert.equal(expand({ conversation_id: 'c', seq: 3, base: 2, turn: turn() }, state), null);
  expand({ conversation_id: 'c', turn: turn() }, state);
  assert.equal(state.has('c'), false, 'nothing to build on without seq');
  expand(keyframe(4), state);
  forget('c', state);
  assert.equal(state.has('c'), false);
});

test('streams of different conversations are kept apart', () => {
  const state = new Map();
  expand(keyframe(1), state);
  expand({ conversation_id: 'd', seq: 2, turn: turn({ id: 'u1' }) }, state);
  assert.ok(expand({ conversation_id: 'c', seq: 3, base: 1, turn: turn({ parts: [null, null] }) }, state));
  assert.equal(expand({ conversation_id: 'd', seq: 4, base: 1, turn: turn() }, state), null);
});

test('payloads that are not objects are ignored', () => {
  const state = new Map();
  assert.equal(expand(null, state), null);
  assert.equal(expand('turn', state), null);
  expand(keyframe(1), state);
  assert.equal(expand({ conversation_id: 'c', seq: 2, base: 1, turn: null }, state), null);
});
