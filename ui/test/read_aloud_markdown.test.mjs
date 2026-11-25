// npm run test:ui — read aloud speaks a reply's words, not its Markdown: task
// markers go, and emphasis marks go only where they are emphasis (the same
// rules as the search snippets), so 2*3*4 and snake_case_name are read whole.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { speechText } from '../js/readaloud.js';

test('task items are read without their box', () => {
  assert.equal(speechText('- [ ] buy milk\n- [x] done\n1. [X] numbered'), 'buy milk\ndone\nnumbered');
});

test('emphasis marks go, stars and underscores in words stay', () => {
  assert.equal(speechText('**bold** _it_ *it* ~~gone~~'), 'bold it it gone');
  assert.equal(speechText('use snake_case_name and 2*3*4 and a * b'), 'use snake_case_name and 2*3*4 and a * b');
});
