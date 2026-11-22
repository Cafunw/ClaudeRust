// The backend's `turn` events (src-tauri/src/turnstream.rs), expanded:
// while a reply streams, most events are deltas, where the elements of
// `parts` and `artifacts` that didn't change since the previous event are
// null, so are the unchanged values named in `same`, and `base` is that
// event's `seq`. Listeners always get the full { conversation_id, turn }
// payload, as if every event carried it.
import { on, call } from './api.js';
import { element, PROTOCOL } from './turnstream_delta.js';

const ELIDED = ['parts', 'artifacts'];
const last = new Map(); // conversation id -> { seq, turn } of the previous event
const resyncing = new Set(); // conversations whose full turn was asked for
const handlers = new Set();
let listening = false;

/** The full payload of event `p`, or null for a delta that can't be
 *  expanded (this page missed the event it was made from: see `receive`).
 *  `state` keeps the previous event of each conversation. */
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
        const v = element(items[i], Array.isArray(old) ? old[i] : undefined);
        if (v === undefined) { state.delete(conv); return null; }
        out[i] = v;
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

/** `expand(p)`; when it can't be expanded, the backend is asked once
 *  (`ask(conv)`) for a full turn of that conversation, which it otherwise
 *  sends only when the turn starts or ends. */
export function receive(p, ask, state = last, waiting = resyncing) {
  const full = expand(p, state);
  const conv = p && p.conversation_id;
  if (full) waiting.delete(conv);
  else if (conv && !waiting.has(conv)) {
    waiting.add(conv);
    ask(conv);
  }
  return full;
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
    call('turn_stream_protocol', { version: PROTOCOL }).catch(() => {});
    on('turn', (p) => {
      const full = receive(p, (conv) => call('turn_stream_resync', { conversationId: conv }).catch(() => {}));
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
