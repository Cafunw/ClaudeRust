// npm run test:ui — read aloud (ui/js/readaloud.js): what of an answer is spoken, in which language, in which pieces.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { speechText, speechLang, chunks } from '../js/readaloud.js';

test('code blocks, markup and link targets are not read', () => {
  const md = '# Title\n\nSome **bold** and *italic* text with `code` and a [link](https://x.y/z).\n\n```js\nconst a = 1;\n```\n\n- one\n- two\n\n> quoted\n\n![img](a.png)';
  assert.equal(speechText(md), 'Title\nSome bold and italic text with code and a link.\none\ntwo\nquoted');
});

test('an unclosed fence (still streaming) is skipped to the end', () => {
  assert.equal(speechText('Here:\n\n```py\nprint(1)\n'), 'Here:');
  assert.equal(speechText('~~~\nx\n~~~\nafter'), 'after');
});

test('tables are read as their cells, without the divider row', () => {
  assert.equal(speechText('| Name | Age |\n|---|:---:|\n| Ann | 30 |'), 'Name, Age.\nAnn, 30.');
});

test('artifacts and math are left out', () => {
  assert.equal(speechText('Made it.\n<antArtifact identifier="a" type="text/html">\n<p>x</p>\n</antArtifact>\nDone.'), 'Made it.\nDone.');
  assert.equal(speechText('Sum: $$a+b$$ ok'), 'Sum: ok');
  assert.equal(speechText(''), '');
});

test('the language comes from the text, else the UI', () => {
  assert.equal(speechLang('Bu bir deneme metnidir, değil mi?', 'en'), 'tr-TR');
  assert.equal(speechLang('This is the answer to the question you asked.', 'tr'), 'en-US');
  assert.equal(speechLang('Merhaba, nasilsin', 'tr'), 'tr-TR');
  assert.equal(speechLang('Hallo Welt', 'en'), 'en-US');
});

test('long text is spoken in sentence-sized pieces', () => {
  assert.deepEqual(chunks('One. Two! Three?', 8), ['One.', 'Two!', 'Three?']);
  assert.deepEqual(chunks('Short one. Short two.', 100), ['Short one. Short two.']);
  const long = chunks('x'.repeat(500), 220);
  assert.deepEqual(long.map((c) => c.length), [220, 220, 60]);
  assert.deepEqual(chunks(''), []);
});
