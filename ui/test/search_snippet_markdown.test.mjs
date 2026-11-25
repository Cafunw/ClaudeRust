// npm run test:ui — search (Ctrl+K) snippets show a message's text without
// its inline Markdown, also where the snippet's "…" cut a mark off, and keep
// the match highlight.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { snippetHtml } from '../js/util.js';
import { source } from './source.mjs';

test('emphasis, links, images and code spans become their text', () => {
  assert.equal(snippetHtml('# Heading one\nSome **bold**, *italic*, ~~strike~~, `inline code` and a [link](https…'),
    'Heading one Some bold, italic, strike, inline code and a link');
  assert.equal(snippetHtml('a *\u0001italic\u0002* word and _under_'), 'a <mark>italic</mark> word and under');
  assert.equal(snippetHtml('![pic](a.png) and [wiki](https://en.wikipedia.org/wiki/Foo_(bar)) end'), 'pic and wiki end');
  assert.equal(snippetHtml('…link](https://x.y/z) more <https://a.b/c>'), '…link more https://a.b/c');
  assert.equal(snippetHtml('…italic*, strike and *cut…'), '…italic, strike and cut…');
  assert.equal(snippetHtml('- [ ] todo\n- [x] \u0001done\u0002'), 'todo <mark>done</mark>');
});

test('stars and underscores that are not Markdown stay', () => {
  assert.equal(snippetHtml('2 * 3 = 6 and 2*3 and a * b'), '2 * 3 = 6 and 2*3 and a * b');
  assert.equal(snippetHtml('snake_case_name and [x] not a link'), 'snake_case_name and [x] not a link');
});

test('the match reads at 4.5:1 in both themes, on the selected row too', async () => {
  const css = source('../css/app.css');
  assert.match(css, /\.palette \.res mark, \.list-item \.d mark \{ background: var\(--accent-soft\); color: var\(--text-0\);/);
  const [light, dark] = css.split(':root[data-theme="dark"] {');
  const token = (block, name) => block.split(`${name}: `)[1].split(';')[0].trim();
  const rgba = (v) => (v.startsWith('#') ? [1, 3, 5].map((i) => parseInt(v.slice(i, i + 2), 16)).concat(1) : v.match(/[\d.]+/g).map(Number));
  const over = (top, under) => under.map((u, i) => (i < 3 ? top[i] * top[3] + u * (1 - top[3]) : 1));
  const lum = (c) => c.slice(0, 3).map((v) => { v /= 255; return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }).reduce((s, v, i) => s + v * [0.2126, 0.7152, 0.0722][i], 0);
  for (const block of [light, dark]) {
    const surface = rgba(token(block, '--surface'));
    for (const under of [surface, over(rgba(token(block, '--row-hover')), surface)]) {
      const bg = over(rgba(token(block, '--accent-soft')), under);
      const [a, b] = [lum(rgba(token(block, '--text-0'))), lum(bg)];
      assert.ok((Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05) >= 4.5);
    }
  }
});
