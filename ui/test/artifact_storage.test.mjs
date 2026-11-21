// npm run test:ui
import test from 'node:test';
import assert from 'node:assert/strict';
import { storageCall, MAX_CHARS } from '../js/artifact_storage.js';

// a Storage over a Map
function fakeStorage() {
  const m = new Map();
  return {
    get length() { return m.size; },
    key: (i) => [...m.keys()][i] ?? null,
    getItem: (k) => (m.has(k) ? m.get(k) : null),
    setItem: (k, v) => { m.set(k, String(v)); },
    removeItem: (k) => { m.delete(k); },
  };
}

test('set, get, list and delete, as window.storage answers them', () => {
  const ls = fakeStorage();
  assert.equal(storageCall(ls, 'c1/a', 'get', ['score', false]), null);
  assert.deepEqual(storageCall(ls, 'c1/a', 'set', ['score', '42', false]), { key: 'score', value: '42', shared: false });
  storageCall(ls, 'c1/a', 'set', ['scores:jan', '[1,2]', false]);
  assert.deepEqual(storageCall(ls, 'c1/a', 'get', ['score', false]), { key: 'score', value: '42', shared: false });
  assert.deepEqual(storageCall(ls, 'c1/a', 'list', ['scores:', false]), { keys: ['scores:jan'], prefix: 'scores:', shared: false });
  assert.deepEqual(storageCall(ls, 'c1/a', 'list', [undefined, false]).keys, ['score', 'scores:jan']);
  assert.deepEqual(storageCall(ls, 'c1/a', 'delete', ['score', false]), { key: 'score', deleted: true, shared: false });
  assert.deepEqual(storageCall(ls, 'c1/a', 'delete', ['score', false]), { key: 'score', deleted: false, shared: false });
  assert.equal(storageCall(ls, 'c1/a', 'get', ['score', false]), null);
});

test('each artifact, and its shared keys, are apart', () => {
  const ls = fakeStorage();
  storageCall(ls, 'c1/a', 'set', ['k', 'mine', false]);
  storageCall(ls, 'c1/a', 'set', ['k', 'ours', true]);
  assert.equal(storageCall(ls, 'c1/b', 'get', ['k', false]), null);
  assert.equal(storageCall(ls, 'c1/a', 'get', ['k', false]).value, 'mine');
  assert.deepEqual(storageCall(ls, 'c1/a', 'get', ['k', true]), { key: 'k', value: 'ours', shared: true });
});

test('bad keys, non-string values and unknown calls are refused', () => {
  const ls = fakeStorage();
  for (const k of ['', 'a b', 'a/b', 'q"', 'x'.repeat(201), 5]) assert.throws(() => storageCall(ls, 's', 'set', [k, 'v', false]), /keys/);
  assert.throws(() => storageCall(ls, 's', 'set', ['k', { a: 1 }, false]), /JSON\.stringify/);
  assert.throws(() => storageCall(ls, 's', 'clear', []), /Unknown/);
  // "__proto__" is an ordinary key
  storageCall(ls, 's', 'set', ['__proto__', 'v', false]);
  assert.equal(storageCall(ls, 's', 'get', ['__proto__', false]).value, 'v');
  assert.deepEqual(storageCall(ls, 's', 'list', ['', false]).keys, ['__proto__']);
});

test('all artifacts together stay under the limit, leaving room for the app', () => {
  const ls = fakeStorage();
  ls.setItem('clauderust.ui', 'x'.repeat(MAX_CHARS)); // the app's own state does not count
  const half = 'v'.repeat(MAX_CHARS / 2);
  storageCall(ls, 'c1/a', 'set', ['k', half, false]);
  assert.throws(() => storageCall(ls, 'c2/b', 'set', ['k', half, false]), /full/);
  assert.equal(storageCall(ls, 'c2/b', 'get', ['k', false]), null);
  // replacing a value counts the new one, not both
  storageCall(ls, 'c1/a', 'set', ['k', half, false]);
});

test('an unreadable entry starts over instead of failing', () => {
  const ls = fakeStorage();
  ls.setItem('clauderust.artifact:own:s', '{not json');
  assert.equal(storageCall(ls, 's', 'get', ['k', false]), null);
  storageCall(ls, 's', 'set', ['k', 'v', false]);
  assert.equal(storageCall(ls, 's', 'get', ['k', false]).value, 'v');
});
