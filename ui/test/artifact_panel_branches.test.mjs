// npm run test:ui — the artifact panel when you switch branches.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { panelStep, shownArtifact } from '../js/branch_panel.js';

test('the artifact panel folds on a branch without artifacts and comes back only if it folded itself', () => {
  assert.equal(panelStep(false, true, false), 'close');
  assert.equal(panelStep(true, false, true), 'reopen');
  assert.equal(panelStep(true, false, false), null, 'closed by hand: stays closed');
  assert.equal(panelStep(true, true, false), null);
  assert.equal(panelStep(false, false, true), null);
  assert.equal(panelStep(true, true, true), null, 'already open again');
});

test('the artifact panel keeps its artifact on a branch that has it, else shows the branch\'s latest', () => {
  assert.equal(shownArtifact(['a', 'b'], 'a'), 'a');
  assert.equal(shownArtifact(['b', 'c'], 'a'), 'c');
  assert.equal(shownArtifact([], 'a'), null);
});
