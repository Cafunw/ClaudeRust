// npm run test:ui — a refresh of the sidebar after a turn reads the chats,
// not the sections.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

// what sidebar.js's imports touch when they load
globalThis.window.matchMedia = () => ({ matches: false, addEventListener() {} });
globalThis.MutationObserver = class { observe() {} };
globalThis.ResizeObserver = class { observe() {} };
globalThis.document.querySelector = () => null;
const { refreshSidebar } = await import('../js/sidebar.js');
const { store } = await import('../js/store.js');

test('a refresh after a turn reads the chats only; any other refresh the sections too', async () => {
  const asked = [];
  window.__TAURI__.core.invoke = async (cmd) => { asked.push(cmd); return []; };
  const kept = { chat: [{ id: 's1' }], code: [] };
  store.sections = kept;
  await refreshSidebar({ sections: false });
  assert.deepEqual(asked, ['list_conversations']);
  assert.equal(store.sections, kept, 'the sections stay as they were');
  // on('conversations-changed') with no payload, a command's null reply, conversation_set_section's record
  for (const arg of [undefined, null, { id: 'c1', section_id: 's1' }]) {
    asked.length = 0;
    await refreshSidebar(arg);
    assert.deepEqual(asked.sort(), ['list_conversations', 'sections_list', 'sections_list'], String(arg));
  }
  assert.match(readFileSync(new URL('../js/sidebar.js', import.meta.url), 'utf8'), /soon = setTimeout\(\(\) => \{ soon = null; refreshSidebar\(\{ sections: false \}\); \}, 250\);/, 'backend events');
});
