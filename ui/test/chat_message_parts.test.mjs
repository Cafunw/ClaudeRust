// npm run test:ui — parts of an answer: "Thought for 12s" on a thinking
// block, and help links in error notices.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { setLanguage } from '../js/i18n.js';
import { thoughtFor, linkHelp } from '../js/parts.js';

test('a thinking block says how long it thought, in either language', () => {
  setLanguage('en');
  assert.equal(thoughtFor(12000), 'Thought for 12s');
  assert.equal(thoughtFor(65000), 'Thought for 1m 5s');
  assert.equal(thoughtFor(120400), 'Thought for 2m');
  assert.equal(thoughtFor(300), 'Thought for 1s', 'under a second still reads as a second');
  for (const none of [undefined, null, '', 'x', -5, NaN]) assert.equal(thoughtFor(none), '', `untimed: ${none}`);
  setLanguage('tr');
  assert.equal(thoughtFor(12000), '12 sn düşündü');
  assert.equal(thoughtFor(65000), '1 dk 5 sn düşündü');
  setLanguage('en');
});

test('an error notice links the help sites it names, and only to them', () => {
  assert.equal(linkHelp('Install it from claude.com/claude-code, or set its path.'),
    'Install it from <a href="https://claude.com/claude-code">claude.com/claude-code</a>, or set its path.');
  assert.equal(linkHelp('Get it at https://nodejs.org.'), 'Get it at <a href="https://nodejs.org">https://nodejs.org</a>.');
  assert.equal(linkHelp('see notclaude.com/claude-code'), 'see notclaude.com/claude-code', 'a look-alike host stays text');
  assert.equal(linkHelp('see https://evil.example/claude.com'), 'see https://evil.example/claude.com');
});
