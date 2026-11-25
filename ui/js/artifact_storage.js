// window.storage of artifacts. The preview runs sandboxed in an opaque origin
// with no storage of its own, so its calls come here over postMessage
// (artifacts.js) and go to the app's database (commands.rs artifact_storage):
// each artifact's keys apart, its "shared" ones apart from its own, at most
// MAX_BYTES for one artifact, all of them gone with their conversation.
import { call } from './api.js';

export const MAX_BYTES = 20 * 1024 * 1024;
const KEY = /^[^\s/\\'"`]{1,200}$/;

function checkKey(k) {
  if (typeof k !== 'string' || !KEY.test(k)) throw new Error('Storage keys are 1-200 characters, without spaces, slashes or quotes.');
}

/** Run one window.storage call (`op` with `args`, as the artifact sent
 *  them) for the artifact `scope`: resolves to what the artifact gets. */
export async function storageCall(scope, op, args) {
  const [a, b, c] = Array.isArray(args) ? args : [];
  const shared = !!(op === 'set' ? c : b);
  const ask = (extra) => call('artifact_storage', { scope, op, shared, ...extra });
  await migrated;
  switch (op) {
    case 'get': {
      checkKey(a);
      const value = await ask({ key: a });
      return value === null ? null : { key: a, value, shared };
    }
    case 'set':
      checkKey(a);
      if (typeof b !== 'string') throw new Error('Storage values are strings: store objects with JSON.stringify.');
      if (!(await ask({ key: a, value: b }))) throw new Error(`Artifact storage is full (${MAX_BYTES / 1024 / 1024} MB for each artifact).`);
      return { key: a, value: b, shared };
    case 'delete':
      checkKey(a);
      return { key: a, deleted: await ask({ key: a }), shared };
    case 'list': {
      const prefix = a == null ? '' : String(a);
      return { keys: await ask({ key: prefix }), prefix, shared };
    }
    default:
      throw new Error(`Unknown storage call: ${op}`);
  }
}

const OLD_PREFIX = 'clauderust.artifact:';

/** What older versions kept in `ls` (a Storage): [{scope, shared, data}] and the keys they were under. */
export function oldEntries(ls) {
  const found = [];
  for (let i = 0; i < ls.length; i++) {
    const k = ls.key(i);
    const m = k && k.startsWith(OLD_PREFIX) && /^(own|shared):(.+)$/.exec(k.slice(OLD_PREFIX.length));
    if (!m) continue;
    let data = {};
    try { data = JSON.parse(ls.getItem(k) || '{}'); } catch { /* unreadable: nothing to keep */ }
    found.push({ key: k, scope: m[2], shared: m[1] === 'shared', data: data && typeof data === 'object' ? data : {} });
  }
  return found;
}

/** Move what older versions kept in `ls` into the database, then remove it there (once: nothing is left after). */
export async function migrate(ls) {
  const found = oldEntries(ls);
  if (!found.length) return;
  await call('artifact_storage_import', { scopes: found.map(({ scope, shared, data }) => ({ scope, shared, data })) });
  for (const f of found) ls.removeItem(f.key);
}

// before the first call: an artifact opened right after an update reads what it kept
const migrated = (async () => {
  try { await migrate(localStorage); } catch { /* kept where it was: tried again at the next start */ }
})();
