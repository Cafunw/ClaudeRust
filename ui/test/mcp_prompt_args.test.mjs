// npm run test:ui — the arguments of an MCP prompt picked in the composer.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { promptArgValues } from '../js/composer.js';

test('MCP prompt arguments: required ones must be filled, empty optional ones are left out', () => {
  const defs = [{ name: 'topic', required: true }, { name: 'tone' }, { name: 'length', required: true }];
  assert.deepEqual(promptArgValues(defs, ['cats', '', 'short']), { missing: -1, args: { topic: 'cats', length: 'short' } });
  assert.equal(promptArgValues(defs, ['cats', 'dry', '  ']).missing, 2);
  assert.equal(promptArgValues(defs, []).missing, 0);
  assert.deepEqual(promptArgValues([], []), { missing: -1, args: {} });
});
