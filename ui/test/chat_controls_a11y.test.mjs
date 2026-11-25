// npm run test:ui — chat UI controls a keyboard or a screen reader can use:
// every icon-only button has a name, the find bar's arrows say what they do,
// and on/off controls say whether they are on.
import test from 'node:test';
import assert from 'node:assert/strict';
import { source } from './source.mjs';

const src = (f) => source(`../js/${f}`);
const FILES = ['chat.js', 'parts.js', 'composer.js', 'findbar.js', 'artifacts.js', 'search.js', 'sidebar.js', 'home.js', 'util.js', 'mermaid_block.js', 'usage_limits.js'];

test('icon buttons in the chat UI carry a label or a tooltip', () => {
  for (const f of FILES) {
    for (const tag of src(f).match(/<button[^>]*class="[^"]*icon-btn[^"]*"[^>]*>/g) || []) {
      // the read-aloud button is named as it's drawn (chat.js readAloudButton)
      if (tag.includes('read-aloud')) continue;
      assert.match(tag, /aria-label=|title=/, `${f}: ${tag}`);
    }
  }
});

test('the find bar\'s buttons and field are named', () => {
  const fb = src('findbar.js');
  for (const k of ['find_prev', 'find_next', 'close', 'find_in_chat']) assert.ok(fb.includes(`t('${k}')`), k);
  assert.match(fb, /aria-label="\$\{esc\(t\('find_in_chat'\)\)\}"/);
});

test('toggles say whether they are on', () => {
  assert.match(src('search.js'), /aria-pressed="\$\{k === 'all'\}"/, 'palette tabs');
  assert.match(src('composer.js'), /aria-pressed="\$\{e === cur\}"/, 'effort levels');
  assert.match(src('artifacts.js'), /aria-pressed="\$\{view === v\}"/, 'preview / code');
  assert.match(src('parts.js'), /setAttribute\('aria-expanded'/, 'thinking and tool cards');
  assert.match(src('util.js'), /role="menuitemradio" aria-checked=/, 'a checked menu choice');
});

test('a card or an image that acts as a button opens with the keyboard too', () => {
  const chat = src('chat.js');
  assert.match(chat, /e\.key === 'Enter' \|\| e\.key === ' '\) \{ e\.preventDefault\(\); open\(\); \}/, 'artifact card');
  assert.match(chat, /data-zoom role="button" tabindex="0"/, 'an attached image');
  assert.match(src('util.js'), /e\.key === 'Escape' \|\| e\.key === 'Enter'/, 'the image viewer closes from the keyboard');
});
