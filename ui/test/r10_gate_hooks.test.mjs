// npm run test:ui — round 10 edits made across files at the gate: a refresh of the
// sidebar after a turn reads the chats, not the sections (EV8-21); the title
// button's tooltip follows an arriving title, taken from the get_conversation a
// turn's end sends anyway (EV8-16, EV8-21); the Todos and Usage segmented
// controls say which option is chosen (EV8-07); focus tracking holds no strong
// reference to a closed chat's DOM (EV8-01).
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
const read = (p) => readFileSync(new URL(p, import.meta.url), 'utf8');

test('EV8-21: a refresh after a turn reads the chats only; any other refresh the sections too', async () => {
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
  assert.match(read('../js/sidebar.js'), /soon = setTimeout\(\(\) => \{ soon = null; refreshSidebar\(\{ sections: false \}\); \}, 250\);/, 'backend events');
});

test('EV8-16/EV8-21: a title that arrives renames the button and its tooltip, from a reload already on its way', () => {
  const main = read('../js/main.js');
  assert.match(main, /chat\.refreshing\(c\.id\) \? chat\.fetchConversation\(c\.id\)\.then\(\(d\) => d\.conversation\) : call\('conversation_meta', \{ id: c\.id \}\)/);
  assert.match(main, /tt\.textContent = convTitle\(conv\); tt\.parentElement\.title = tt\.textContent;/);
});

test('EV8-07: the Todos and Usage filters say which option is chosen', () => {
  assert.match(read('../js/todos_ui.js'), /class="todos-filters segmented" role="group"><button data-k="" class="\$\{kind \? '' : 'on'\}" aria-pressed="\$\{!kind\}">.*aria-pressed="\$\{kind === k\}"/);
  assert.match(read('../js/usage_ui.js'), /<div class="segmented" role="group">.*aria-pressed="\$\{x === d\}"/);
});

test('EV8-01: where focus was is remembered weakly, so a closed chat can be collected', () => {
  const a11y = read('../js/a11y.js');
  assert.match(a11y, /lastOutside = new WeakRef\(e\.target\)/);
  assert.match(a11y, /ancestors\.push\(new WeakRef\(p\)\)/);
  assert.doesNotMatch(a11y, /lastOutside = e\.target|ancestors\.push\(p\)/);
});
