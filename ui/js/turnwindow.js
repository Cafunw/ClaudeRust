// Long conversations draw only their newest turns; older ones come a page at
// a time as you scroll up (chat.js), so opening one costs what's on screen.

export const PAGE_TURNS = 40;
const PAGE_CHARS = 250_000;

/** About how much a turn puts on screen: its rendered text. */
function weight(turn) {
  let n = 200;
  for (const p of turn.parts || []) n += (p.html || p.text || '').length;
  return n;
}

/** Where the page of `turns` that ends before index `end` starts: up to
 *  PAGE_TURNS turns or PAGE_CHARS of content (at least one turn), and never
 *  on an answer without its question when the question is there. */
export function pageStart(turns, end) {
  let i = end;
  let chars = 0;
  while (i > 0 && end - i < PAGE_TURNS && (i === end || chars + weight(turns[i - 1]) <= PAGE_CHARS)) {
    i--;
    chars += weight(turns[i]);
  }
  if (i > 0 && turns[i].role === 'assistant' && turns[i - 1].role === 'user') i--;
  return i;
}

// A page of older turns (and parked turns coming back) is drawn a frame's
// worth at a time: this much work (script, and the layout of what it drew)
// leaves room for the frame's paint within 16 ms.
export const FRAME_BUDGET = 6;
/** Opening a conversation draws this much of its last page at once (a short
 *  one whole), in the frame that opens it. */
export const OPEN_BUDGET = 12;

/** `draw(i)` for i = 0, 1, … below `n` while `budget` ms last (at least
 *  once); how many were drawn. */
export function drawWithin(n, draw, budget = FRAME_BUDGET, now = () => performance.now()) {
  const t0 = now();
  let i = 0;
  while (i < n) {
    draw(i++);
    if (now() - t0 >= budget) break;
  }
  return i;
}

// Turns drawn by scrolling up stay in the DOM; the ones far off screen are
// parked (emptied, keeping their height) and drawn again from their data
// when they come near, so what's mounted stays about a few screens' worth.
const PARK_FAR = 4;
const PARK_NEAR = 2;

/** Whether a drawn turn `dist` px off screen (0 when on screen) is parked,
 *  in a viewport `view` px tall, given whether it is now (`was`): beyond
 *  PARK_FAR screens it is, within PARK_NEAR it's drawn, and in between it
 *  stays as it is, so scrolling to and fro doesn't redraw turns at the edge. */
export function parked(dist, view, was) {
  if (dist > PARK_FAR * view) return true;
  if (dist < PARK_NEAR * view) return false;
  return was;
}

// A closed conversation leaves the WebView holding what it drew (raster
// tiles, glyph and image caches) until Windows runs short of memory: a moment
// after you leave a long one, it is asked to let go (memory_trim.js). Turns
// count, not elements: a parked turn is an empty box, but it was drawn.
export const TRIM_TURNS = 20;
const TRIM_WAIT = 3000;

/** `left(turns)` for each view you leave, with the turns it drew: `trim` runs
 *  once, `wait` ms after the last of a run of quick switches, if a view left
 *  in that run drew TRIM_TURNS; while `busy()` (an answer streaming in) it
 *  waits another round, so a purge never lands on a moving answer. */
export function trimmer(trim, busy, wait = TRIM_WAIT) {
  let peak = 0;
  let timer = null;
  const fire = () => {
    if (busy()) { timer = setTimeout(fire, wait); return; }
    if (peak >= TRIM_TURNS) trim();
    peak = 0;
  };
  return (turns) => {
    peak = Math.max(peak, turns);
    clearTimeout(timer);
    timer = setTimeout(fire, wait);
  };
}
