// npm run test:ui
import test from 'node:test';
import assert from 'node:assert/strict';
import { collectAnswers } from '../js/ask_question.js';

const qs = [
  { question: 'Colour?', options: [{ label: 'Red' }, { label: 'Blue' }], multiSelect: false },
  { question: 'Fruit?', options: [{ label: 'Apple' }, { label: 'Pear' }, { label: 'Plum' }], multiSelect: true },
];

test('answers are the labels picked, several joined with a comma', () => {
  assert.deepEqual(collectAnswers(qs, [{ chosen: [1] }, { chosen: [0, 2] }]), { 'Colour?': 'Blue', 'Fruit?': 'Apple, Plum' });
});

test('what is typed under Other answers a single choice, and adds to a multiple one', () => {
  assert.deepEqual(collectAnswers(qs, [{ chosen: [0], other: ' Green ' }, { chosen: [1], other: 'Fig' }]), { 'Colour?': 'Green', 'Fruit?': 'Pear, Fig' });
});

test('unanswered questions are left out', () => {
  assert.deepEqual(collectAnswers(qs, [{ chosen: [], other: '  ' }]), {});
  assert.deepEqual(collectAnswers([{ question: 'Name?' }], [{ other: 'Ada' }]), { 'Name?': 'Ada' });
});
