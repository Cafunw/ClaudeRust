// npm run test:ui — search: result snippets without Markdown, and filters
// that match every word, Turkish-aware and without accents.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { snippetHtml, fold, hasWords } from '../js/util.js';

test('search snippets lose table pipes and rules, heading and list marks, and keep the match marks', () => {
  assert.equal(snippetHtml('…| 16 | 12 |\n## \u0001Section\u0002 5'), '… 16 12 <mark>Section</mark> 5');
  assert.equal(snippetHtml('| a | b |\n|---|:--:|\n| c | d |'), 'a b c d');
  assert.equal(snippetHtml('- item\n> quote\n1. first'), 'item quote 1. first');
  assert.equal(snippetHtml('**bold** and `code`, [a link](https://x.y/z) ![pic](a.png) ~~gone~~'), 'bold and code, a link pic gone');
  assert.equal(snippetHtml('```rust\nlet a = b || c;\n```'), 'let a = b || c;', 'pipes inside a line are kept');
  assert.equal(snippetHtml('<b>&'), '&lt;b&gt;&amp;', 'escaped');
  assert.equal(snippetHtml(''), '');
  assert.equal(snippetHtml(null), '');
});

test('search filters match every word, in any order, Turkish-aware and without accents', () => {
  assert.equal(fold('İSTANBUL Işık ışık ÇİÇEK Şeker Ünlü'), 'istanbul isik isik cicek seker unlu');
  assert.ok(hasWords('Sidebar filler chat number 1 about tests', 'filler 1'));
  assert.ok(hasWords('Sidebar filler chat number 1 about tests', '1 FILLER'));
  assert.ok(!hasWords('Sidebar filler chat number 1 about tests', 'filler 2'));
  assert.ok(hasWords('Işık ayarları', 'isik AYAR'));
  assert.ok(hasWords('çiçek bahçesi', 'CICEK bahce'));
  assert.ok(hasWords('anything', '  '), 'an empty query matches');
  assert.ok(hasWords(undefined, ''));
});
