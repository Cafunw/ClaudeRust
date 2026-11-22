// npm run test:ui — screen-reader and keyboard helpers (ui/js/a11y.js).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { trapStep, nextAnnouncement, streamAnnouncer } from '../js/a11y.js';

test('Tab wraps inside a dialog and pulls focus in from outside', () => {
  assert.equal(trapStep(3, 2, false), 0, 'Tab on the last goes to the first');
  assert.equal(trapStep(3, 0, true), 2, 'Shift+Tab on the first goes to the last');
  assert.equal(trapStep(3, 1, false), -1, 'in the middle the browser moves it');
  assert.equal(trapStep(3, 1, true), -1);
  assert.equal(trapStep(3, -1, false), 0, 'from outside: the first');
  assert.equal(trapStep(3, -1, true), 2, 'from outside backwards: the last');
  assert.equal(trapStep(0, -1, false), -1, 'nothing to focus');
  assert.equal(trapStep(1, 0, false), 0);
});

test('a growing answer is announced in whole sentences, the rest at the end', () => {
  assert.deepEqual(nextAnnouncement('Hello there. How a', 0, false), { say: 'Hello there.', said: 12 });
  assert.deepEqual(nextAnnouncement('Hello there. How are you? Fi', 12, false), { say: 'How are you?', said: 25 });
  assert.deepEqual(nextAnnouncement('Hello there. How are you? Fine', 25, false), { say: '', said: 25 }, 'no sentence end yet');
  assert.deepEqual(nextAnnouncement('Hello there. How are you? Fine', 25, true), { say: 'Fine', said: 30 });
  assert.deepEqual(nextAnnouncement('New', 25, false), { say: '', said: 0 }, 'a shorter text starts over');
  assert.deepEqual(nextAnnouncement('3.14 is pi', 0, false), { say: '', said: 0 }, 'a dot inside a number ends nothing');
});

function fakeTimers() {
  let next = 1;
  const due = new Map();
  return {
    setTimeout: (fn) => { const id = next++; due.set(id, fn); return id; },
    clearTimeout: (id) => due.delete(id),
    run() { const fns = [...due.values()]; due.clear(); fns.forEach((f) => f()); },
    get pending() { return due.size; },
  };
}

test('streaming updates are debounced, not said per token', () => {
  const said = [];
  const tm = fakeTimers();
  const a = streamAnnouncer((s) => said.push(s), { wait: 2000, timers: tm });
  a.update('t1', 'One.');
  a.update('t1', 'One. Two');
  a.update('t1', 'One. Two. Thr');
  assert.equal(tm.pending, 1, 'one timer for a burst of updates');
  assert.deepEqual(said, []);
  tm.run();
  assert.deepEqual(said, ['One. Two.']);
  a.update('t1', 'One. Two. Three', true);
  assert.deepEqual(said, ['One. Two.', 'Three'], 'the end at once');
  assert.equal(tm.pending, 0);
  a.update('t1', 'One. Two. Three', true);
  a.finish();
  assert.deepEqual(said, ['One. Two.', 'Three'], 'a finished answer is not said again');
});

test('a new answer starts afresh and finish() says what is left', () => {
  const said = [];
  const tm = fakeTimers();
  const a = streamAnnouncer((s) => said.push(s), { wait: 2000, timers: tm });
  a.update('t1', 'Old answer');
  a.update('t2', 'New one is here');
  a.finish();
  assert.deepEqual(said, ['New one is here']);
  a.finish();
  assert.equal(said.length, 1);
  a.reset();
  a.finish();
  assert.equal(said.length, 1, 'nothing under way after reset');
});

test('what is said is the prepared answer (Markdown left out)', () => {
  const said = [];
  const tm = fakeTimers();
  const a = streamAnnouncer((s) => said.push(s), { wait: 2000, timers: tm, prepare: (md) => md.replace(/\*\*/g, '') });
  a.update('t1', 'It is **bold**. And mo');
  tm.run();
  a.update('t1', 'It is **bold**. And more', true);
  assert.deepEqual(said, ['It is bold.', 'And more']);
});
