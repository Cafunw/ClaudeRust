// The backend's `turn` events (src-tauri/src/turnstream.rs), expanded:
// while a reply streams, most events are deltas, where the elements of
// `parts` and `artifacts` that didn't change since the previous event are
// null, so are the unchanged values named in `same`, and `base` is that
// event's `seq`. Listeners always get the full { conversation_id, turn }
// payload, as if every event carried it.
import { on } from './api.js';

const ELIDED = ['parts', 'artifacts'];
const last = new Map(); // conversation id -> { seq, turn } of the previous event
const handlers = new Set();
let listening = false;

/** The full payload of event `p`, or null for a delta that can't be
 *  expanded (this page missed the event it was made from; the next full
 *  turn comes within a second). `state` keeps the previous event of each
 *  conversation. */
export function expand(p, state = last) {
  if (!p || typeof p !== 'object') return null;
  const conv = p.conversation_id;
  let turn = p.turn;
  if (p.base != null) {
    const prev = state.get(conv);
    if (!prev || prev.seq !== p.base || !turn || typeof turn !== 'object') return null;
    // a new object: the event's payload is shared by every listener of
    // `turn`, so it's left as it came
    const full = { ...turn };
    for (const k of ELIDED) {
      const items = turn[k];
      const old = prev.turn[k];
      if (!Array.isArray(items)) continue;
      const out = new Array(items.length);
      for (let i = 0; i < items.length; i++) {
        if (items[i] !== null) { out[i] = items[i]; continue; }
        if (!Array.isArray(old) || old[i] === undefined) { state.delete(conv); return null; }
        out[i] = old[i];
      }
      full[k] = out;
    }
    for (const k of p.same || []) {
      if (!(k in prev.turn)) { state.delete(conv); return null; }
      full[k] = prev.turn[k];
    }
    turn = full;
  }
  if (turn && turn.streaming === true && p.seq != null) state.set(conv, { seq: p.seq, turn });
  else state.delete(conv);
  return { conversation_id: conv, turn };
}

/** Forget the stream of `conv` (its turn ended). */
export function forget(conv, state = last) {
  state.delete(conv);
}

/** Call `fn` with each full `turn` payload. Returns a function that stops it. */
export function onTurn(fn) {
  handlers.add(fn);
  if (!listening) {
    listening = true;
    on('turn', (p) => {
      const full = expand(p);
      if (!full) return;
      // each listener on its own, as when they listened separately
      for (const h of [...handlers]) {
        try { h(full); } catch (e) { console.error(e); }
      }
    });
    on('generation-state', (p) => { if (p && !p.running) forget(p.conversation_id); });
  }
  return () => handlers.delete(fn);
}
