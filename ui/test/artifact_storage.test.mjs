// npm run test:ui — artifacts' window.storage (ui/js/artifact_storage.js) over the app's database, as
// commands.rs artifact_storage answers it (faked here with its budget per artifact).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { storageCall, migrate, oldEntries, MAX_BYTES } from '../js/artifact_storage.js';
import { source } from './source.mjs';

// the database: scope|shared|key -> value, the budget per scope as db.rs storage_set counts it
const rows = new Map();
const calls = [];
const bytes = (s) => Buffer.byteLength(s);
window.__TAURI__.core.invoke = async (cmd, a) => {
  calls.push([cmd, a]);
  if (cmd === 'artifact_storage_import') {
    for (const s of a.scopes) for (const [k, v] of Object.entries(s.data)) if (typeof v === 'string') rows.set(`${s.scope}|${s.shared}|${k}`, v);
    return 0;
  }
  assert.equal(cmd, 'artifact_storage');
  const id = `${a.scope}|${a.shared}|${a.key}`;
  if (a.op === 'get') return rows.has(id) ? rows.get(id) : null;
  if (a.op === 'delete') return rows.delete(id);
  if (a.op === 'list') return [...rows.keys()].filter((r) => r.startsWith(`${a.scope}|${a.shared}|`)).map((r) => r.split('|')[2]).filter((k) => k.startsWith(a.key)).sort();
  let used = 0;
  for (const [r, v] of rows) if (r.startsWith(`${a.scope}|`) && r !== id) used += bytes(r.split('|')[2]) + bytes(v);
  if (used + bytes(a.key) + bytes(a.value) > MAX_BYTES) return false;
  rows.set(id, a.value);
  return true;
};

test('set, get, list and delete, as window.storage answers them', async () => {
  assert.equal(await storageCall('c1/a', 'get', ['score', false]), null);
  assert.deepEqual(await storageCall('c1/a', 'set', ['score', '42', false]), { key: 'score', value: '42', shared: false });
  await storageCall('c1/a', 'set', ['scores:jan', '[1,2]', false]);
  assert.deepEqual(await storageCall('c1/a', 'get', ['score', false]), { key: 'score', value: '42', shared: false });
  assert.deepEqual(await storageCall('c1/a', 'list', ['scores:', false]), { keys: ['scores:jan'], prefix: 'scores:', shared: false });
  assert.deepEqual((await storageCall('c1/a', 'list', [undefined, false])).keys, ['score', 'scores:jan']);
  assert.deepEqual(await storageCall('c1/a', 'delete', ['score', false]), { key: 'score', deleted: true, shared: false });
  assert.deepEqual(await storageCall('c1/a', 'delete', ['score', false]), { key: 'score', deleted: false, shared: false });
  assert.equal(await storageCall('c1/a', 'get', ['score', false]), null);
  // an empty value is a value
  await storageCall('c1/a', 'set', ['empty', '', false]);
  assert.deepEqual(await storageCall('c1/a', 'get', ['empty', false]), { key: 'empty', value: '', shared: false });
});

test('each artifact, and its shared keys, are apart', async () => {
  await storageCall('c2/a', 'set', ['k', 'mine', false]);
  await storageCall('c2/a', 'set', ['k', 'ours', true]);
  assert.equal(await storageCall('c2/b', 'get', ['k', false]), null);
  assert.equal((await storageCall('c2/a', 'get', ['k', false])).value, 'mine');
  assert.deepEqual(await storageCall('c2/a', 'get', ['k', true]), { key: 'k', value: 'ours', shared: true });
});

test('bad keys, non-string values and unknown calls are refused before the database is asked', async () => {
  const before = calls.length;
  for (const k of ['', 'a b', 'a/b', 'q"', 'x'.repeat(201), 5]) await assert.rejects(storageCall('s/x', 'set', [k, 'v', false]), /keys/);
  await assert.rejects(storageCall('s/x', 'set', ['k', { a: 1 }, false]), /JSON\.stringify/);
  await assert.rejects(storageCall('s/x', 'clear', []), /Unknown/);
  assert.equal(calls.length, before);
  // "__proto__" is an ordinary key
  await storageCall('s/x', 'set', ['__proto__', 'v', false]);
  assert.equal((await storageCall('s/x', 'get', ['__proto__', false])).value, 'v');
});

test('one artifact can keep far more than all of them could in localStorage, and each has its own budget', async () => {
  const big = 'v'.repeat(6 * 1024 * 1024); // over the old 2,000,000 characters for all artifacts together
  await storageCall('c3/a', 'set', ['k', big, false]);
  await storageCall('c3/b', 'set', ['k', big, false]);
  await storageCall('c3/a', 'set', ['k2', big, false]);
  await assert.rejects(storageCall('c3/a', 'set', ['k3', big + big, false]), /full \(20 MB for each artifact\)/);
  assert.equal(await storageCall('c3/a', 'get', ['k3', false]), null);
  // another artifact still has room
  await storageCall('c3/c', 'set', ['k', big + big, false]);
});

function fakeStorage(entries) {
  const m = new Map(Object.entries(entries));
  return {
    get length() { return m.size; },
    key: (i) => [...m.keys()][i] ?? null,
    getItem: (k) => (m.has(k) ? m.get(k) : null),
    setItem: (k, v) => m.set(k, String(v)),
    removeItem: (k) => m.delete(k),
    keys: () => [...m.keys()],
  };
}

test('what an older version kept in localStorage moves to the database once, the app’s own keys stay', async () => {
  const ls = fakeStorage({
    'clauderust.artifact:own:old/a': JSON.stringify({ score: '7' }),
    'clauderust.artifact:shared:old/a': JSON.stringify({ board: '[1]' }),
    'clauderust.artifact:own:old/broken': '{not json',
    'clauderust.ui': 'the app’s own',
  });
  assert.deepEqual(oldEntries(ls).map((e) => [e.scope, e.shared]), [['old/a', false], ['old/a', true], ['old/broken', false]]);
  await migrate(ls);
  assert.deepEqual(ls.keys(), ['clauderust.ui']);
  assert.deepEqual(await storageCall('old/a', 'get', ['score', false]), { key: 'score', value: '7', shared: false });
  assert.equal((await storageCall('old/a', 'get', ['board', true])).value, '[1]');
  // nothing left: a second start asks nothing
  const before = calls.length;
  await migrate(ls);
  assert.equal(calls.length, before);
});

test('a migration that fails keeps localStorage as it was', async () => {
  const ls = fakeStorage({ 'clauderust.artifact:own:x/a': JSON.stringify({ k: 'v' }) });
  const invoke = window.__TAURI__.core.invoke;
  window.__TAURI__.core.invoke = async () => { throw new Error('database busy'); };
  try { await assert.rejects(migrate(ls), /busy/); } finally { window.__TAURI__.core.invoke = invoke; }
  assert.deepEqual(ls.keys(), ['clauderust.artifact:own:x/a']);
});

test('the panel answers an artifact with what the database call resolves to, or its error', () => {
  assert.match(source('../js/artifacts.js'), /storageCall\(frame\.dataset\.scope, d\.op, d\.args\)\.then\(\(result\) => reply\(\{ result \}\), fail\)/);
});
