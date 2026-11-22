// npm run test:ui — the pages a long conversation is drawn in (ui/js/turnwindow.js).
import test from 'node:test';
import assert from 'node:assert/strict';
import { PAGE_TURNS, pageStart } from '../js/turnwindow.js';

const convo = (n, chars = 100) => Array.from({ length: n }, (_, i) => ({ id: `t${i}`, role: i % 2 ? 'assistant' : 'user', parts: [{ kind: 'text', html: 'x'.repeat(chars) }] }));

test('a page is the last PAGE_TURNS turns, or everything when there are fewer', () => {
  const turns = convo(600);
  assert.equal(pageStart(turns, 600), 600 - PAGE_TURNS);
  assert.equal(pageStart(turns, 600 - PAGE_TURNS), 600 - 2 * PAGE_TURNS, 'the page before it');
  assert.equal(pageStart(convo(7), 7), 0);
  assert.equal(pageStart(turns, 0), 0, 'nothing before the first turn');
});

test('big turns make a shorter page, but never an empty one', () => {
  assert.equal(pageStart(convo(100, 60_000), 100), 96, 'about 250k characters');
  assert.equal(pageStart(convo(3, 1_000_000), 3), 2, 'one huge turn');
  assert.equal(pageStart(convo(2, 1_000_000), 2), 0, 'one huge answer, with its question');
});

test('a page does not start on an answer whose question is just before it', () => {
  const turns = convo(11, 100_000).slice(1); // answer, question, answer, …
  // the budget alone stops after two turns, on an answer: its question comes along
  assert.equal(pageStart(turns, 10), 7);
  assert.equal(turns[7].role, 'user');
  // a conversation that starts with an answer (a fork) can start on it
  assert.equal(pageStart([{ role: 'assistant', parts: [] }, { role: 'user', parts: [] }], 2), 0);
});
