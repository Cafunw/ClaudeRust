// A streamed answer's new words shown over the next batch interval instead of
// all at once: the backend sends what changed every 100 ms (engine.rs
// EMIT_EVERY), rendered whole, and each batch's text is uncovered in a few
// steps. The text itself is never touched: the markdown is always rendered
// whole, and layout, scrolling, the find bar, selection and screen readers see
// all of it at once. What is not uncovered yet sits under a cover in the
// colour behind the text (aria-hidden, clicks go through), which only moves:
// a CSS highlight or a clip on the text would repaint it at every step, which
// measured as half the stream's CPU again.

const SPAN_MS = 100; // engine.rs EMIT_EVERY
// steps at 30 a second, 3 a batch; one a frame would draw every frame of a fast display
const STEP_MS = 33;
const active = new Map(); // element -> { text, from, to, t0, at, nodes }
const covers = new Map(); // element -> its cover
const shown = new WeakMap(); // element -> how much of its text has been uncovered
let timer = 0;

/** The end of what is uncovered of `text` `elapsed` ms after a batch took it
 *  from `from` to `to` characters: on a word's end, all of it after `span`. */
export function revealAt(text, from, to, elapsed, span = SPAN_MS) {
  if (elapsed >= span || from >= to) return to;
  const n = from + Math.ceil(((to - from) * Math.max(0, elapsed)) / span);
  if (n === from) return from;
  const gap = text.slice(n, to).search(/\s/);
  return gap < 0 ? to : n + gap;
}

/** Where the cover's two blocks go for a caret at `c` in a part at `box`
 *  (viewport rects): the rest of the caret's line, and everything below it. */
export function coverPlace(c, box) {
  const x = c.left - box.left;
  const top = c.top - box.top;
  return { line: `translate(${x}px, ${top}px)`, lineHeight: `${c.bottom - c.top}px`, below: `translateY(${c.bottom - box.top}px)` };
}

// (forced colours keep a background's alpha: the page's colours are the system's, so no cover can match them)
const reduced = () => !!(globalThis.matchMedia && matchMedia('(prefers-reduced-motion: reduce), (forced-colors: active)').matches);

/** The colour behind `el` (its first ancestor's that has one). */
function backdrop(el) {
  for (let n = el; n && n.nodeType === 1; n = n.parentElement) {
    const c = getComputedStyle(n).backgroundColor;
    if (c && c !== 'transparent' && !/,\s*0\)$/.test(c)) return c;
  }
  return 'transparent';
}

function coverOf(el) {
  let cover = covers.get(el);
  if (!cover || !cover.isConnected) {
    cover = document.createElement('div');
    cover.className = 'reveal-cover';
    cover.setAttribute('aria-hidden', 'true');
    cover.innerHTML = '<div></div><div></div>';
    // next to the parts, not among them (renderParts counts its children)
    (el.closest('.assistant-body') || el.parentElement).appendChild(cover);
    covers.set(el, cover);
  }
  cover.style.setProperty('--cover', backdrop(el));
  return cover;
}

function drop(el) {
  const s = active.get(el);
  if (!s) return;
  const cover = covers.get(el);
  if (cover) cover.hidden = true;
  active.delete(el);
  shown.set(el, s.to);
}

/** The text nodes of `el` with where each starts in its text. */
// shortcut: walks the whole part once a batch (a few thousand nodes for a long answer); walk only
// its last blocks if answers of hundreds of KB show up in a profile
function textNodes(el) {
  const out = [];
  const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
  let at = 0;
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    out.push([n, at]);
    at += n.nodeValue.length;
  }
  return out;
}

/** The text node `at` is in (the last one that starts at or before it). */
function locate(nodes, at) {
  let i = nodes.length - 1;
  while (i > 0 && nodes[i][1] > at) i--;
  return nodes[i];
}

let caret = null;

/** Cover what is after character `at` of `s`'s element. */
function hideFrom(s, el, at) {
  let hit = locate(s.nodes, at);
  // the find bar splits text nodes into marks (the text stays the same)
  if (hit && !hit[0].isConnected) hit = locate((s.nodes = textNodes(el)), at);
  const cover = covers.get(el);
  if (!hit || !cover) return;
  const [node, start] = hit;
  caret ??= document.createRange();
  caret.setStart(node, Math.min(at - start, node.nodeValue.length));
  caret.collapse(true);
  const c = caret.getClientRects()[0];
  const box = el.getBoundingClientRect();
  cover.hidden = false;
  const host = cover.offsetParent;
  // (text not laid out, such as a formula's source, or a part folded away: nothing to cover)
  if (!c || !box.height || !host) { cover.hidden = true; return; }
  const o = host.getBoundingClientRect();
  cover.style.transform = `translate(${box.left - o.left - host.clientLeft}px, ${box.top - o.top - host.clientTop}px)`;
  cover.style.width = `${box.width}px`;
  cover.style.height = `${box.height}px`;
  const p = coverPlace(c, box);
  const [line, below] = cover.children;
  line.style.transform = p.line;
  line.style.height = p.lineHeight;
  below.style.transform = p.below;
}

function step(now) {
  timer = 0;
  for (const [el, s] of active) {
    const at = el.isConnected ? revealAt(s.text, s.from, s.to, now - s.t0) : s.to;
    if (at >= s.to) drop(el);
    else if (at !== s.at) {
      s.at = at;
      hideFrom(s, el, at);
    }
  }
  if (active.size) later();
}

function later() {
  if (!timer) timer = setTimeout(() => requestAnimationFrame(step), STEP_MS);
}

/** After `el` (a part of a streaming answer) was drawn again: uncover its new
 *  text over the next batch interval, or at once when `animate` is false (a
 *  conversation opened mid-answer) or the system asks for reduced motion. */
export function reveal(el, animate = true) {
  const text = el.textContent;
  const s = active.get(el);
  const from = Math.min(text.length, s ? revealAt(s.text, s.from, s.to, performance.now() - s.t0) : shown.get(el) || 0);
  if (!animate || reduced() || from >= text.length) {
    if (s) drop(el);
    shown.set(el, text.length);
    return;
  }
  const next = { text, from, to: text.length, t0: performance.now(), at: from, nodes: textNodes(el) };
  active.set(el, next);
  coverOf(el);
  hideFrom(next, el, from);
  later();
}

/** The answer in `root` ended (finished, stopped or failed): all of its text, at once. */
export function revealAll(root) {
  if (!covers.size) return;
  for (const [el, cover] of [...covers]) {
    if (el.isConnected && !root.contains(el)) continue;
    drop(el);
    cover.remove();
    covers.delete(el);
  }
}
