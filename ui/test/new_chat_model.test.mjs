// npm run test:ui — the model new chats start on (ui/js/store.js).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { store, newChatModel, rememberModel } from '../js/store.js';

function setup() {
  localStorage.removeItem('clauderust.ui');
  store.settings = { default_model: 'opus', code_model: null };
  store.models = [{ id: 'opus' }, { id: 'haiku' }, { id: 'sonnet' }, { id: 'gone', available: false }];
}

test('a new chat starts on the model last picked in its mode', () => {
  setup();
  assert.equal(newChatModel('chat'), 'opus', 'nothing picked yet: the default');
  rememberModel('chat', 'haiku');
  assert.equal(newChatModel('chat'), 'haiku');
  assert.equal(newChatModel('code'), 'opus', 'chat and code are remembered apart');
  rememberModel('code', 'sonnet');
  assert.equal(newChatModel('code'), 'sonnet');
  assert.equal(newChatModel('chat'), 'haiku');
});

test('a default changed in Settings after the pick wins', () => {
  setup();
  rememberModel('chat', 'haiku');
  rememberModel('code', 'haiku');
  store.settings.default_model = 'sonnet';
  assert.equal(newChatModel('chat'), 'sonnet');
  // code falls back to the chat default while it has none of its own: that changed too
  assert.equal(newChatModel('code'), 'sonnet');
  rememberModel('code', 'haiku');
  store.settings.code_model = 'opus';
  assert.equal(newChatModel('code'), 'opus');
});

test('a picked model that is no longer offered falls back to the default', () => {
  setup();
  rememberModel('chat', 'gone');
  assert.equal(newChatModel('chat'), 'opus');
  rememberModel('chat', 'removed-model');
  assert.equal(newChatModel('chat'), 'opus');
});
