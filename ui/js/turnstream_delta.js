// One element of `parts` or `artifacts` in a `turn` delta
// (src-tauri/src/turnstream.rs, protocol 2): null is the previous event's
// element, an array edits its strings ([key, kept, appended]: the first
// `kept` UTF-16 units of the old string, then `appended`), anything else is
// the element itself.

/** The payloads this decoder expands, announced to the backend. */
export const PROTOCOL = 2;

/** The full element, or undefined when `prev` can't be edited into it. */
export function element(item, prev) {
  if (item === null) return prev;
  if (!Array.isArray(item)) return item;
  if (!prev || typeof prev !== 'object' || Array.isArray(prev)) return undefined;
  const out = { ...prev };
  for (const op of item) {
    const [k, kept, added] = Array.isArray(op) ? op : [];
    const old = prev[k];
    if (typeof old !== 'string' || !Number.isInteger(kept) || kept < 0 || kept > old.length || typeof added !== 'string') return undefined;
    out[k] = old.slice(0, kept) + added;
  }
  return out;
}
