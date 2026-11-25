// npm run test:ui — a streamed answer's new words appear over the batch
// interval (ui/js/stream_reveal.js), never split a word, and all of the
// text shows the moment the answer ends or when motion is reduced.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { revealAt, coverPlace, reveal, revealAll } from '../js/stream_reveal.js';
import { source } from './source.mjs';

test('the uncovered end moves through the batch on word ends and reaches the end in time', () => {
  const text = 'one two three four five six';
  assert.equal(revealAt(text, 0, text.length, 0), 0, 'nothing yet');
  const mid = revealAt(text, 0, text.length, 40);
  assert.ok(mid > 0 && mid < text.length && /\s/.test(text[mid]), `a word's end: ${mid}`);
  assert.ok(revealAt(text, 0, text.length, 70) >= mid, 'never backwards');
  assert.equal(revealAt(text, 0, text.length, 100), text.length, 'all of it after the interval');
  assert.equal(revealAt(text, 8, 8, 10), 8, 'nothing new');
  assert.equal(revealAt('nospaceshere', 0, 12, 10), 12, 'one long word: at once, never cut');
});

test('the cover takes the rest of the caret\'s line and everything below it', () => {
  assert.deepEqual(coverPlace({ top: 120, bottom: 140, left: 65 }, { top: 100, left: 10 }),
    { line: 'translate(55px, 20px)', lineHeight: '20px', below: 'translateY(40px)' });
});

// a part with one text node on one line, character i at x = i, in a turn at y = 90
function setup() {
  const turn = { getBoundingClientRect: () => ({ top: 90, left: 0 }), clientLeft: 0, clientTop: 0 };
  const body = { covers: [], appendChild(c) { this.covers.push(c); } };
  globalThis.getComputedStyle = () => ({ backgroundColor: 'rgb(1, 2, 3)' });
  globalThis.document.createElement = () => {
    const c = { style: { setProperty(k, v) { this[k] = v; } }, hidden: false, isConnected: true, offsetParent: turn,
      children: [{ style: {} }, { style: {} }], setAttribute() {}, remove() { this.isConnected = false; } };
    Object.defineProperty(c, 'innerHTML', { set() {} });
    return c;
  };
  const part = (text) => ({ node: { nodeValue: text, isConnected: true }, textContent: text, isConnected: true, nodeType: 1, parentElement: null,
    closest: () => body, getBoundingClientRect: () => ({ top: 100, left: 0, width: 500, height: 40 }) });
  return { body, part };
}

test('a batch is uncovered in steps, and all at once when the answer ends', async () => {
  let clock = 1000;
  let reduce = false;
  const frames = [];
  Object.defineProperty(globalThis, 'performance', { configurable: true, value: { now: () => clock } });
  globalThis.matchMedia = () => ({ matches: reduce });
  globalThis.requestAnimationFrame = (cb) => frames.push(cb);
  globalThis.NodeFilter = { SHOW_TEXT: 4 };
  globalThis.document.createTreeWalker = (el) => { let done = false; return { nextNode: () => (done ? null : ((done = true), el.node)) }; };
  globalThis.document.createRange = () => ({ setStart(n, o) { this.o = o; }, collapse() {}, getClientRects() { return [{ top: 120, bottom: 140, left: this.o }]; } });
  const tick = async (ms) => {
    clock += ms;
    await new Promise((r) => setTimeout(r, 40)); // the reveal's own timer
    for (const f of frames.splice(0)) f(clock);
  };
  const { body, part } = setup();

  const a = part('one two three four five six');
  reveal(a);
  const [cover] = body.covers;
  assert.equal(cover.style['--cover'], 'rgb(1, 2, 3)', 'in the colour behind the text');
  assert.deepEqual([cover.hidden, cover.style.transform, cover.style.height], [false, 'translate(0px, 10px)', '40px'], 'over the part');
  assert.equal(cover.children[0].style.transform, 'translate(0px, 20px)', 'a new answer starts covered');
  assert.equal(cover.children[1].style.transform, 'translateY(40px)');
  await tick(50);
  const x = Number(cover.children[0].style.transform.match(/translate\((\d+)px/)[1]);
  assert.ok(x > 0 && x < a.textContent.length, `half way: ${x}`);
  revealAll({ contains: (el) => el === a });
  assert.equal(cover.isConnected, false, 'ended: everything at once');

  // opened mid-answer: what is there shows; the next batch covers only what's new
  const b = part('already here');
  reveal(b, false);
  assert.equal(body.covers.length, 1, 'no cover');
  b.textContent = b.node.nodeValue = 'already here and more';
  reveal(b);
  assert.equal(body.covers[1].children[0].style.transform, 'translate(12px, 20px)');
  await tick(120);
  assert.equal(body.covers[1].hidden, true, 'uncovered within the interval');

  // a part taken out of the page (another chat opened) lets go of it
  const gone = part('a b c d');
  reveal(gone);
  gone.isConnected = false;
  await tick(10);
  assert.equal(body.covers[2].hidden, true);
  revealAll({ contains: () => false });
  assert.equal(body.covers[2].isConnected, false, 'and it is removed with the next turn drawn');

  reduce = true;
  reveal(part('no motion please'));
  assert.equal(body.covers.length, 3, 'reduced motion: no reveal');
});

test('parts of a streaming answer are revealed, and a turn that ends shows all of it', () => {
  const parts = source('../js/parts.js');
  assert.match(parts, /morph\(el, p\.html \|\| ''\)\.forEach\(decorate\);\n\s+if \(ctx && ctx\.streaming\) reveal\(el, ctx\.live\);/);
  assert.match(parts, /if \(p\.streaming\) reveal\(body, !!\(ctx && ctx\.live\)\);/, 'thinking too');
  assert.match(parts, /if \(!p\.streaming\) revealAll\(el\);/, 'and done thinking shows it whole as it folds');
  const chat = source('../js/chat.js');
  assert.match(chat, /if \(!streaming\) revealAll\(partsEl\);/);
  assert.match(chat, /renderAssistantTurn\(el, turn, live\);/, 'only a live event animates');
  // with forced colours the cover would be see-through (measured): no reveal, as with reduced motion
  assert.match(source('../js/stream_reveal.js'), /matchMedia\('\(prefers-reduced-motion: reduce\), \(forced-colors: active\)'\)/);
  const css = source('../css/app.css');
  assert.match(css, /\.reveal-cover \{[^}]*pointer-events: none;/, 'clicks and selection go through');
  assert.match(css, /\.reveal-cover > div \{[^}]*background: var\(--cover\); will-change: transform;/, 'it only moves');
});
