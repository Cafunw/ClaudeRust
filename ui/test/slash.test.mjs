// npm run test:ui — the slash-command parser and registry (ui/js/slash.js).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { parse, registerCommand, builtinCommands, allCommands, handle } from '../js/slash.js';

/** Answer backend calls with `fn(cmd, args)`; returns the calls made. */
function backend(fn) {
  const calls = [];
  window.__TAURI__.core.invoke = async (cmd, args) => { calls.push([cmd, args]); return fn(cmd, args); };
  return calls;
}

const names = (ctx) => builtinCommands(ctx).map((c) => c.name);

test('parse finds the command name and its arguments', () => {
  assert.deepEqual(parse('/help'), { name: 'help', args: '' });
  assert.deepEqual(parse('  /review:pr  fix the  bug \n and more  '), { name: 'review:pr', args: 'fix the  bug \n and more' });
  assert.deepEqual(parse('/my-cmd.v2'), { name: 'my-cmd.v2', args: '' });
  assert.equal(parse('hello /help'), null);
  assert.equal(parse('/'), null);
  assert.equal(parse('/usr/bin is a path'), null, 'a path is not a command');
});

test('built-ins are offered by conversation kind and engine; registering a name again replaces it', () => {
  registerCommand({ name: 't-code', kinds: ['code'], description: () => 'code only', run: async () => {} });
  registerCommand({ name: 't-cli', engines: ['claude-code'], description: () => 'cli only', run: async () => {} });
  assert.ok(names({}).includes('help'), 'a chat by default');
  assert.ok(!names({ kind: 'chat' }).includes('t-code'));
  assert.ok(names({ kind: 'code' }).includes('t-code'));
  assert.ok(!names({ kind: 'code' }).includes('t-cli'), 'the built-in engine by default');
  assert.ok(names({ kind: 'chat', engine: 'claude-code' }).includes('t-cli'));
  registerCommand({ name: 't-code', kinds: ['code'], description: () => 'v2', run: async () => {} });
  const mine = builtinCommands({ kind: 'code' }).filter((c) => c.name === 't-code');
  assert.deepEqual(mine.map((c) => c.description()), ['v2']);
});

test('handle runs a built-in and passes on what it hands back', async () => {
  let got;
  registerCommand({ name: 't-prompt', description: () => '', run: async (args, ctx) => { got = [args, ctx]; return `Expanded: ${args}`; } });
  registerCommand({ name: 't-fail', description: () => '', run: async () => false });
  registerCommand({ name: 't-files', description: () => '', run: async (a) => (a ? { prompt: 'Look', attachments: [{ id: 'f1' }] } : { prompt: 'Look' }) });
  registerCommand({ name: 't-done', description: () => '', run: async () => undefined });
  const ctx = { kind: 'chat', conversationId: 'c1' };
  assert.deepEqual(await handle('/t-prompt  a b ', ctx), { prompt: 'Expanded: a b' });
  assert.deepEqual(got, ['a b', ctx]);
  assert.deepEqual(await handle('/t-fail', ctx), { keep: true }, 'what was typed is kept');
  assert.deepEqual(await handle('/t-files x', ctx), { prompt: 'Look', attachments: [{ id: 'f1' }] });
  assert.deepEqual(await handle('/t-files', ctx), { prompt: 'Look', attachments: [] });
  assert.deepEqual(await handle('/t-done', ctx), { handled: true });
});

test('handle leaves other messages alone', async () => {
  const calls = backend(() => 'never');
  assert.equal(await handle('just text', { kind: 'chat' }), null);
  assert.equal(await handle('/no-such-command', { kind: 'chat' }), null);
  assert.deepEqual(calls, [], 'a chat has no custom commands');
});

test('in a code session an unknown command is expanded by the backend', async () => {
  const calls = backend((cmd, a) => (a.name === 'deploy' ? `Deploy to ${a.args}` : null));
  const ctx = { kind: 'code', conversationId: 'c1', cwd: '/work' };
  assert.deepEqual(await handle('/deploy prod', ctx), { prompt: 'Deploy to prod', display: '/deploy prod' });
  assert.deepEqual(calls[0], ['slash_expand', { conversationId: 'c1', cwd: '/work', name: 'deploy', args: 'prod' }]);
  assert.equal(await handle('/unknown', ctx), null);
  backend(() => { throw new Error('backend down'); });
  assert.equal(await handle('/deploy prod', ctx), null);
});

test('the menu lists custom commands of a code session; a built-in keeps its name', async () => {
  const calls = backend((cmd) => (cmd === 'slash_list'
    ? [{ name: 'help', description: 'shadowed' }, { name: 'ship', description: 'Ship it', argument_hint: '[env]', source: 'project' }]
    : null));
  const ctx = { kind: 'code', conversationId: 'c9', cwd: '/w9' };
  const all = await allCommands(ctx);
  assert.deepEqual(all.filter((c) => c.name === 'help').map((c) => c.kind), ['builtin']);
  assert.deepEqual(all.find((c) => c.name === 'ship'), { name: 'ship', description: 'Ship it', hint: '[env]', kind: 'custom', source: 'project' });
  await allCommands(ctx);
  assert.equal(calls.length, 1, 'asked once, then cached');
  assert.ok(!(await allCommands({ kind: 'chat' })).some((c) => c.kind === 'custom'));
});
