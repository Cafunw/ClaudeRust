// npm run test:ui — a web citation's card (ui/js/cite_card.js): it stays in
// the window at any width, shows the page's title, domain and the cited
// passage as text, and a tap or a first click shows it instead of leaving.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { cardPlace, cardHtml } from '../js/cite_card.js';
import { setLanguage, t } from '../js/i18n.js';
import { source } from './source.mjs';

const rect = (left, top, w = 80, h = 19) => ({ left, top, right: left + w, bottom: top + h });

test('the card stays inside a 420 px window, under its chip or above it', () => {
  const [w, h] = [304, 120];
  for (const left of [0, 150, 380, 410]) {
    const p = cardPlace(rect(left, 200), w, h, 420, 800);
    assert.ok(p.left >= 8 && p.left + w <= 412, `chip at ${left}: ${p.left}`);
    assert.equal(p.top, 225, 'under the chip');
  }
  const low = cardPlace(rect(40, 760), w, h, 420, 800);
  assert.equal(low.top, 760 - 6 - h, 'no room below: above it');
  const tight = cardPlace(rect(40, 60), w, 700, 420, 760);
  assert.ok(tight.top >= 8 && tight.top + 700 <= 752, `taller than either side: inside the window (${tight.top})`);
});

test('the card shows the title, the domain and the passage, as text', () => {
  setLanguage('en');
  const chip = { textContent: 'blog.rust-lang.org', dataset: { title: 'Rust <b>Blog</b>', quote: 'It "shipped" <script>x()</script> & more' } };
  const html = cardHtml(chip);
  assert.ok(html.includes('Rust &lt;b&gt;Blog&lt;/b&gt;'), html);
  assert.ok(html.includes('<div class="cite-domain">blog.rust-lang.org</div>'), html);
  assert.ok(html.includes('It &quot;shipped&quot; &lt;script&gt;x()&lt;/script&gt; &amp; more'), html);
  assert.ok(html.includes(`aria-label="${t('cite_passage')}"`));
  assert.ok(!cardHtml({ textContent: 'a.example', dataset: { title: 'A' } }).includes('cite-quote'), 'no passage: none shown');
  assert.ok(cardHtml({ textContent: 'a.example', dataset: {} }).includes('<div class="cite-title">a.example</div>'), 'no title: the domain');
  setLanguage('tr');
  assert.notEqual(t('cite_passage'), 'Cited passage');
  setLanguage('en');
});

test('a tap shows the card, Escape and leaving close it, and its link handling knows', () => {
  const card = source('../js/cite_card.js');
  assert.match(card, /pointerover[^\n]+e\.pointerType !== 'touch'\) show\(a\)/, 'hover, but not a touch (a tap is a click)');
  assert.match(card, /focusin[^\n]+if \(a\) show\(a\)/, 'keyboard focus');
  assert.match(card, /e\.key === 'Escape' && card\) \{ e\.preventDefault\(\); e\.stopPropagation\(\); hide\(\); \}\n\}, true\)/, 'Escape, before the app\'s own');
  assert.match(card, /card\.addEventListener\('pointerleave', hideSoon\)/);
  assert.match(card, /setAttribute\('aria-describedby', card\.id\)/, 'a screen reader hears the card');
  assert.match(source('../js/parts.js'), /if \(cite && citeClick\(cite\)\) \{ e\.preventDefault\(\); return; \}\n\s+const a = e\.target\.closest\('a\[href\]'\);/, 'before the link opens');
  const css = source('../css/app.css');
  assert.match(css, /\.cite-card \{[^}]*max-width: min\(320px, calc\(100vw - 16px\)\)/);
  assert.match(css, /\.cite-card \{[^}]*background: var\(--surface\); color: var\(--text-0\)/, 'theme colours');
});
