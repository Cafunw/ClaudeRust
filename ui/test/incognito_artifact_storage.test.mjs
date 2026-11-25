// npm run test:ui — an incognito chat's artifacts leave nothing in localStorage once it closes (artifact_storage.js).
import test from 'node:test';
import assert from 'node:assert/strict';
import { storageCall, forgetConversation } from '../js/artifact_storage.js';

function fakeStorage() {
  const m = new Map();
  return {
    get length() { return m.size; },
    key: (i) => [...m.keys()][i] ?? null,
    getItem: (k) => (m.has(k) ? m.get(k) : null),
    setItem: (k, v) => m.set(k, String(v)),
    removeItem: (k) => m.delete(k),
    keys: () => [...m.keys()],
  };
}

test('closing a chat removes its artifacts’ own and shared keys, and only those', () => {
  const ls = fakeStorage();
  storageCall(ls, 'inc1/a', 'set', ['k', 'secret', false]);
  storageCall(ls, 'inc1/b', 'set', ['k', 'shared secret', true]);
  storageCall(ls, 'inc10/a', 'set', ['k', 'another chat', false]);
  storageCall(ls, 'other/a', 'set', ['k', 'kept', false]);
  ls.setItem('clauderust.ui', 'the app’s own');
  forgetConversation(ls, 'inc1');
  assert.deepEqual(ls.keys().sort(), ['clauderust.artifact:own:inc10/a', 'clauderust.artifact:own:other/a', 'clauderust.ui']);
  assert.equal(storageCall(ls, 'inc1/a', 'get', ['k', false]), null);
  forgetConversation(ls, 'nothing-here');
  assert.equal(ls.length, 3);
});
