// npm run test:ui — a new chat's composer starts its Claude Code CLI on the first key typed, with exactly the
// new_conversation send_message then gets (home.js newConversation; claudecode.rs prewarm decides whether to).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { source } from './source.mjs';

const calls = [];
window.__TAURI__.core.invoke = async (cmd, a) => { calls.push([cmd, a]); return cmd === 'send_message' ? { conversation_id: 'c' } : null; };
const { newConversation, startConversation } = await import('../js/home.js');

const st = { model: 'm', thinking: true, effort: 'low', web_search: false, style: null, disabled_servers: ['s'], disabled_tools: [], tool_access: null, fast: false, research: false };

test('the CLI started ahead is started for the conversation the first message makes', async () => {
  await startConversation({ text: 'hi', atts: [], st, projectId: 'p' });
  const sent = calls.find(([c]) => c === 'send_message')[1].req.new_conversation;
  assert.deepEqual(sent, newConversation(st, { projectId: 'p' }));
  assert.equal(sent.options.effort, 'low');
  assert.equal(newConversation(st, { incognito: true }).options.incognito, true);
  assert.equal(newConversation(st, { kind: 'code', incognito: true }).options.incognito, undefined);
});

test('the new chat and the project page ask for it on the first key typed, once', () => {
  for (const f of ['../js/home.js', '../js/projects.js']) assert.match(source(f), /prewarmOnInput\(composer, \{ projectId/, f);
  assert.match(source('../js/home.js'), /addEventListener\('input', \(\) => call\('prewarm_chat', \{ req: newConversation\(composer\.state, opts\) \}\)[^\n]*\{ once: true \}\)/);
});
