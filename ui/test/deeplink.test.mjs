// npm run test:ui
import test from 'node:test';
import assert from 'node:assert/strict';
import { linkTarget } from '../js/deeplink.js';

test('each route the backend sends has its place', () => {
  assert.deepEqual(linkTarget({ route: 'chat', id: 'abc-1' }), { name: 'chat', params: { id: 'abc-1' } });
  assert.deepEqual(linkTarget({ route: 'new', prompt: 'hi' }), { name: 'new', params: { text: 'hi' } });
  assert.deepEqual(linkTarget({ route: 'code', prompt: '' }), { name: 'code', params: { text: '' } });
  assert.deepEqual(linkTarget({ route: 'settings' }), { settings: true });
});

test('nothing, or anything else, goes nowhere', () => {
  for (const r of [null, undefined, {}, { route: 'chat' }, { route: 'install', url: 'x' }]) assert.equal(linkTarget(r), null);
});

test('a prompt is never marked to be sent', () => {
  assert.equal(JSON.stringify(linkTarget({ route: 'new', prompt: 'x', send: true })).includes('send'), false);
});
