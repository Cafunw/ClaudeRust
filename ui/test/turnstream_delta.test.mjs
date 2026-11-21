// npm run test:ui
// The fixture is written by the Rust encoder (turnstream::tests::the_js_fixture_is_current).
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { element } from '../js/turnstream_delta.js';

globalThis.window ??= { __TAURI__: {} }; // api.js reads it on import
const { expand } = await import('../js/turnstream.js');

test('null is the previous element, an object replaces it', () => {
  const prev = { kind: 'text', raw: 'a' };
  assert.equal(element(null, prev), prev);
  assert.equal(element(null, undefined), undefined);
  assert.deepEqual(element({ kind: 'tool' }, prev), { kind: 'tool' });
});

test('edits keep a prefix of each string and append', () => {
  const prev = { kind: 'text', html: '<p>Ça 😀</p>', raw: 'Ça 😀', sources: [] };
  const got = element([['html', 8, ' ğ</p>'], ['raw', 5, ' ğ']], prev);
  assert.deepEqual(got, { kind: 'text', html: '<p>Ça 😀 ğ</p>', raw: 'Ça 😀 ğ', sources: [] });
  assert.equal(prev.raw, 'Ça 😀', 'the previous element is left as it was');
  // a cut before a whole surrogate pair
  assert.deepEqual(element([['a', 1, '😁b']], { a: 'a😀b' }), { a: 'a😁b' });
});

test('edits that do not fit the previous element are refused', () => {
  const prev = { raw: 'abc', n: 1 };
  for (const bad of [[['raw', 4, 'x']], [['raw', -1, 'x']], [['raw', 1.5, 'x']], [['n', 0, 'x']], [['missing', 0, 'x']], [['raw', 1, 2]], ['raw']]) {
    assert.equal(element(bad, prev), undefined, JSON.stringify(bad));
  }
  assert.equal(element([['raw', 0, 'x']], undefined), undefined);
  assert.equal(element([['0', 0, 'x']], ['abc']), undefined);
});

test('expand decodes what the Rust encoder sends', () => {
  const fixture = JSON.parse(readFileSync(new URL('./turnstream_delta.fixture.json', import.meta.url), 'utf8'));
  const state = new Map();
  let edits = 0;
  for (const [i, { payload, full }] of fixture.entries()) {
    if (payload.includes('[["')) edits++;
    assert.deepEqual(expand(JSON.parse(payload), state), full, `event ${i}`);
  }
  assert.ok(edits > 10, `${edits} payloads with edits`);
});
