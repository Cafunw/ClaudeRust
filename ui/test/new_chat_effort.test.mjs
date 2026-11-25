// npm run test:ui — the thinking effort new chats and code sessions start at: the one last picked
// in that mode, like the model (a model without that level runs at its own).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { newChatEffort, rememberEffort } from '../js/store.js';
import { source } from './source.mjs';

test('the effort last picked in a mode is where the next new one starts', () => {
  localStorage.removeItem('clauderust.ui');
  assert.equal(newChatEffort('chat'), null, 'nothing picked yet: the model’s own');
  rememberEffort('chat', 'low');
  assert.equal(newChatEffort('chat'), 'low');
  assert.equal(newChatEffort('code'), null, 'chat and code are remembered apart');
  rememberEffort('code', 'xhigh');
  assert.equal(newChatEffort('code'), 'xhigh');
  assert.equal(newChatEffort('chat'), 'low');
  rememberEffort('chat', null);
  assert.equal(newChatEffort('chat'), null, '/effort auto goes back to the model’s own');
});

test('the composer remembers a picked effort, and every new chat and session starts from it', () => {
  const composer = source('../js/composer.js');
  assert.match(composer, /if \('effort' in patch\) rememberEffort\(mode, patch\.effort\);/);
  // a level the model doesn't have falls back to the model's own
  assert.match(composer, /return levels\.includes\(st\.effort\) \? st\.effort : /);
  for (const [file, mode] of [['home.js', 'chat'], ['projects.js', 'chat'], ['code.js', 'code']]) {
    assert.match(source(`../js/${file}`), new RegExp(`initial: \\{ model: newChatModel\\('${mode}'\\), effort: newChatEffort\\('${mode}'\\)`), file);
  }
  // an existing chat keeps its own
  assert.match(source('../js/chat.js'), /effort: o\.effort \|\| null,/);
});
