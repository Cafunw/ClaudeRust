// npm run test:ui — the keyboard path through a menu's controls (its switches
// and effort levels too).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { menuStep } from '../js/util.js';

test('arrows cycle through a menu\'s controls, Home and End jump to its ends', () => {
  assert.equal(menuStep(4, -1, 'ArrowDown'), 0, 'from outside: the first');
  assert.equal(menuStep(4, -1, 'ArrowUp'), 3, 'from outside: the last');
  assert.equal(menuStep(4, 1, 'ArrowDown'), 2);
  assert.equal(menuStep(4, 3, 'ArrowDown'), 0, 'wraps');
  assert.equal(menuStep(4, 0, 'ArrowUp'), 3, 'wraps');
  assert.equal(menuStep(4, 2, 'Home'), 0);
  assert.equal(menuStep(4, 0, 'End'), 3);
  assert.equal(menuStep(4, 1, 'Tab'), -1, 'other keys are left alone');
  assert.equal(menuStep(0, -1, 'ArrowDown'), -1, 'an empty menu');
});
