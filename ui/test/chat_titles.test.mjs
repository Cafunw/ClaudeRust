// npm run test:ui — a chat's title: one name for an untitled chat, and the
// title button follows a title that arrives after a turn.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setLanguage } from '../js/i18n.js';
import { convTitle } from '../js/util.js';

test('an untitled chat is "New chat" everywhere; a code session stays "Untitled"', () => {
  setLanguage('tr');
  assert.equal(convTitle({ title: '', kind: 'chat' }), 'Yeni sohbet');
  assert.equal(convTitle({ title: null }), 'Yeni sohbet');
  assert.equal(convTitle({ title: '', kind: 'code' }), 'Adsız');
  assert.equal(convTitle({ title: 'Plan', kind: 'chat' }), 'Plan');
  setLanguage('en');
});

test('a title that arrives renames the button and its tooltip, from a reload already on its way', () => {
  const main = readFileSync(new URL('../js/main.js', import.meta.url), 'utf8');
  assert.match(main, /chat\.refreshing\(c\.id\) \? chat\.fetchConversation\(c\.id\)\.then\(\(d\) => d\.conversation\) : call\('conversation_meta', \{ id: c\.id \}\)/);
  assert.match(main, /tt\.textContent = convTitle\(conv\); tt\.parentElement\.title = tt\.textContent;/);
});
