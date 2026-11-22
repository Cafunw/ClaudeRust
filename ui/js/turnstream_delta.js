// One element of `parts` or `artifacts` in a `turn` delta
// (src-tauri/src/turnstream.rs): null is the previous event's element, an
// array is edits of it applied in order, anything else is the element itself.
// An edit is [key, value] (the field is `value`), [key, kept, appended] (a
// string or an array: its first `kept` UTF-16 units or items, then
// `appended`), [key, at, removed, inserted] (a string with `removed` units at
// `at` replaced by `inserted`) or [key, null, edits] (an object, edited in turn).

/** The payloads this decoder expands, announced to the backend. */
export const PROTOCOL = 3;

const within = (n, max) => Number.isInteger(n) && n >= 0 && n <= max;

/** The full element, or undefined when `prev` can't be edited into it. */
export function element(item, prev) {
  if (item === null) return prev;
  if (!Array.isArray(item)) return item;
  if (!prev || typeof prev !== 'object' || Array.isArray(prev)) return undefined;
  const out = { ...prev };
  for (const op of item) {
    if (!Array.isArray(op) || typeof op[0] !== 'string') return undefined;
    const [k, a, b, c] = op;
    const old = out[k];
    if (op.length === 2) {
      out[k] = a;
    } else if (op.length === 3 && a === null) {
      const v = Array.isArray(b) ? element(b, old) : undefined;
      if (v === undefined) return undefined;
      out[k] = v;
    } else if (op.length === 3) {
      const same = typeof old === 'string' ? typeof b === 'string' : Array.isArray(old) && Array.isArray(b);
      if (!same || !within(a, old.length)) return undefined;
      out[k] = old.slice(0, a).concat(b);
    } else if (op.length === 4) {
      if (typeof old !== 'string' || typeof c !== 'string' || !within(a, old.length) || !within(b, old.length - a)) return undefined;
      out[k] = old.slice(0, a) + c + old.slice(a + b);
    } else {
      return undefined;
    }
  }
  return out;
}
