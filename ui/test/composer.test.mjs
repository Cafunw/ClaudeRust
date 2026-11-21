// npm run test:ui — the composer's pure helpers (ui/js/composer.js; the
// rest of it needs a DOM).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { thinkingSupported } from '../js/composer.js';

test('the thinking switch is offered only where thinking can be turned on and off', () => {
  // the values of models.rs `Thinking`
  for (const thinking of ['disableable', 'between_tools', 'explicit', 'budget']) assert.ok(thinkingSupported({ thinking }), thinking);
  assert.ok(!thinkingSupported({ thinking: 'always_on' }));
  assert.ok(!thinkingSupported({}));
});
