// window.storage of artifacts. The preview runs sandboxed in an opaque origin
// with no storage of its own, so its calls come here over postMessage
// (artifacts.rs) and each artifact's keys are kept in one entry of the app's
// localStorage, apart from the "shared" ones.
// shortcut: localStorage holds about 5 MB for the whole app, so all artifacts
// together keep at most MAX_CHARS; move to a database table if they need more.
export const MAX_CHARS = 2_000_000;
const PREFIX = 'clauderust.artifact:';
const KEY = /^[^\s/\\'"`]{1,200}$/;

function checkKey(k) {
  if (typeof k !== 'string' || !KEY.test(k)) throw new Error('Storage keys are 1-200 characters, without spaces, slashes or quotes.');
}

/** Run one window.storage call (`op` with `args`, as the artifact sent
 *  them) for the artifact `scope` against `ls`, a Storage. */
export function storageCall(ls, scope, op, args) {
  const [a, b, c] = Array.isArray(args) ? args : [];
  const shared = !!(op === 'set' ? c : b);
  const name = `${PREFIX}${shared ? 'shared' : 'own'}:${scope}`;
  let saved = {};
  try { saved = JSON.parse(ls.getItem(name) || '{}'); } catch { /* unreadable: start over */ }
  // no prototype: a key like "__proto__" is just a key
  const data = Object.assign(Object.create(null), saved);
  switch (op) {
    case 'get':
      checkKey(a);
      return a in data ? { key: a, value: data[a], shared } : null;
    case 'set': {
      checkKey(a);
      if (typeof b !== 'string') throw new Error('Storage values are strings: store objects with JSON.stringify.');
      data[a] = b;
      const json = JSON.stringify(data);
      let total = json.length;
      for (let i = 0; i < ls.length; i++) {
        const k = ls.key(i);
        if (k !== name && k.startsWith(PREFIX)) total += (ls.getItem(k) || '').length;
      }
      if (total > MAX_CHARS) throw new Error(`Artifact storage is full (${MAX_CHARS} characters for all artifacts).`);
      ls.setItem(name, json);
      return { key: a, value: b, shared };
    }
    case 'delete': {
      checkKey(a);
      const deleted = a in data;
      if (deleted) { delete data[a]; ls.setItem(name, JSON.stringify(data)); }
      return { key: a, deleted, shared };
    }
    case 'list': {
      const prefix = a == null ? '' : String(a);
      return { keys: Object.keys(data).filter((k) => k.startsWith(prefix)), prefix, shared };
    }
    default:
      throw new Error(`Unknown storage call: ${op}`);
  }
}
