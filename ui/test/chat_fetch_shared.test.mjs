// npm run test:ui — the refreshes around a turn ask the backend once: the
// end of a turn brings generation-state and conversation-updated back to
// back, and a conversation-updated while get_conversation is about to be
// sent needs no conversation_meta of its own (main.js asks `refreshing`).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';

// what chat.js's imports touch when they load
globalThis.window.matchMedia = () => ({ matches: false, addEventListener() {} });
globalThis.MutationObserver = class { observe() {} };
globalThis.ResizeObserver = class { observe() {} };
const { fetchConversation, refreshing } = await import('../js/chat.js');

test('callers a moment apart share one get_conversation, and are told one is coming', async () => {
  const asked = [];
  window.__TAURI__.core.invoke = async (cmd, args) => { asked.push([cmd, args.id]); return { conversation: { id: args.id }, turns: [] }; };
  assert.equal(refreshing('c1'), false);
  const a = fetchConversation('c1');
  assert.equal(refreshing('c1'), true, 'a conversation-updated now gets the record from this request');
  assert.equal(refreshing('c2'), false, 'only for that conversation');
  const b = fetchConversation('c1');
  assert.equal(a, b);
  await a;
  assert.equal(refreshing('c1'), false, 'sent: what comes after it needs its own request');
  assert.deepEqual(asked, [['get_conversation', 'c1']]);
});
