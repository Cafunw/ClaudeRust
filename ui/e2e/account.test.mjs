// End-to-end test of a chat on the Claude account: the path a subscriber takes by default, where the app runs
// the Claude Code CLI for each turn. The dev copy's Claude Code path setting points at fake_claude.cjs (through
// a claude.cmd shim, as an npm-installed CLI is started), and its MCP server and API base URL at a mock in this
// process; all three are restored afterwards. Driven like smoke.test.mjs (see cdp.mjs).
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startMock } from './mock_api.mjs';
import { attach, PRELUDE, sleep } from './cdp.mjs';

const { cdp, skip } = await attach();
const ev = cdp ? cdp.ev : null;
let mock = null;
let dir = null;
let restore = null;
const made = [];

async function waitFor(what, expr, ms = 20000) {
  const t0 = Date.now();
  let last;
  while (Date.now() - t0 < ms) {
    last = await ev(expr);
    if (last) return last;
    await sleep(100);
  }
  const page = await ev(`(location.href + ' ' + (document.querySelector('#view')?.innerText || document.body.innerText)).slice(0, 300)`).catch((e) => e.message);
  throw new Error(`timed out waiting for ${what}; last: ${JSON.stringify(last)}; the page: ${page}; the fake CLI's starts: ${runs().map((r) => r.argv.slice(0, 3).join(' ')).join(' | ')}`);
}

async function idle() {
  await waitFor('the turn to end', `!__e2e.running() && !__e2e.turns().some((t) => t.streaming)`, 30000);
  return ev('__e2e.turns()');
}

/** Every line the fake CLI logged. */
function logLines() {
  const log = dir && path.join(dir, 'log.jsonl');
  const text = log && fs.existsSync(log) ? fs.readFileSync(log, 'utf8') : '';
  return text.split('\n').filter(Boolean).map((l) => JSON.parse(l));
}

/** The fake CLI's starts: the arguments of each. */
function runs() {
  return logLines().filter((l) => l.argv);
}

const STARTED = `document.readyState === 'complete' && !!document.querySelector('#sidebar .nav') && !!document.querySelector('#view .composer, #view .chat-col')`;

const turnRuns = () => runs().filter((r) => r.argv.includes('--system-prompt-file'));

async function newChat(text) {
  await ev(`__e2e.emit('navigate', { name: 'new' })`);
  await waitFor('the new chat composer', `!!document.querySelector('#view .welcome .composer textarea')`);
  assert.ok(await ev(`__e2e.type(${JSON.stringify(text)})`), 'the send button is enabled');
  const id = await waitFor('an open conversation', `__e2e.store.route.name === 'chat' && __e2e.store.route.params.id`);
  made.push(id);
  return id;
}

before(async () => {
  if (skip) return;
  mock = await startMock();
  // the API would write the title first: it can't here, so the title is the CLI's
  mock.noTitles = true;
  dir = fs.mkdtempSync(path.join(os.tmpdir(), 'clauderust-e2e-cli-'));
  fs.copyFileSync(path.join(path.dirname(fileURLToPath(import.meta.url)), 'fake_claude.cjs'), path.join(dir, 'fake.cjs'));
  const win = process.platform === 'win32';
  const program = path.join(dir, win ? 'claude.cmd' : 'claude');
  fs.writeFileSync(program, win ? '@ECHO off\r\nSET dp0=%~dp0\r\nnode "%dp0%\\fake.cjs" %*\r\n' : '#!/bin/sh\nexec node "$(dirname "$0")/fake.cjs" "$@"\n', { mode: 0o755 });
  // the first test file meets a copy that has only just started: its first load settles before the reload
  await waitFor('the app to start', STARTED, 60000);
  await ev(PRELUDE);
  const settings = await ev(`__e2e.invoke('get_settings')`);
  const patch = { base_url: mock.url, chat_engine: 'claude-code', claude_code_path: program, notifications: false, artifacts_enabled: true };
  const old = Object.fromEntries(Object.keys(patch).map((k) => [k, settings[k]]));
  const mcp = (await ev(`__e2e.invoke('mcp_status')`)).config;
  restore = async () => {
    await ev(`__e2e.invoke('save_settings', { patch: ${JSON.stringify(old)} })`);
    await ev(`__e2e.invoke('mcp_save_config', { config: ${JSON.stringify(JSON.stringify(mcp))} })`);
  };
  await ev(`__e2e.invoke('save_settings', { patch: ${JSON.stringify(patch)} })`);
  await cdp.send('Page.reload', { ignoreCache: true });
  await sleep(500);
  await waitFor('the app to start', STARTED, 60000);
  await ev(PRELUDE);
});

after(async () => {
  if (skip) return;
  try {
    await ev(PRELUDE);
    for (const id of made) await ev(`__e2e.invoke('delete_conversation', { id: ${JSON.stringify(id)} })`).catch(() => {});
    if (restore) await restore();
    await ev(`__e2e.emit('navigate', { name: 'new' })`);
  } finally {
    cdp.close();
    await mock.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('a chat on the Claude account streams its reply from the CLI', { skip, timeout: 60000 }, async () => {
  await newChat('e2e account hello');
  let partial = false;
  for (let i = 0; i < 200 && !partial; i++) {
    const a = (await ev('__e2e.turns()')).find((x) => x.role === 'assistant');
    if (a && a.streaming && a.text.includes('Account answer') && !a.text.includes('in pieces.')) partial = true;
    else await sleep(20);
  }
  assert.ok(partial, 'saw the reply while it streamed');
  const turns = await idle();
  assert.deepEqual(turns.map((x) => x.role), ['user', 'assistant']);
  assert.match(turns[1].text, /Account answer to e2e account hello\. The fake CLI streams this reply in pieces\./);
  // the chat ran the CLI on its stream-json protocol with the app's prompt, and never the API
  const run = turnRuns()[0];
  assert.ok(run && run.argv.includes('stream-json'), JSON.stringify(runs()));
  assert.equal(mock.requests.length, 0, 'no Messages request went to the API');
});

test('its title is written by the CLI, in the chat\'s own session', { skip, timeout: 60000 }, async () => {
  await waitFor('the title', `document.querySelector('#sidebar .chat-link[data-id="${made[0]}"] .title')?.textContent === 'E2E account chat'`);
  // the chat's CLI is kept after its turn and asked for the title: no second CLI starts for it
  assert.ok(logLines().some((l) => l.in === 'control_request' && l.subtype === 'generate_session_title'), JSON.stringify(logLines()));
  assert.equal(runs().filter((r) => r.argv.includes('json') && r.argv.includes('haiku')).length, 0, JSON.stringify(runs()));
});

test('the chat\'s next message goes to its kept CLI', { skip, timeout: 60000 }, async () => {
  const before = turnRuns().length;
  assert.ok(await ev(`__e2e.type('e2e account again')`), 'the send button is enabled');
  await waitFor('the answer', `__e2e.turns().some((t) => t.text.includes('Account answer to e2e account again.'))`);
  await idle();
  assert.equal(turnRuns().length, before, 'no CLI started for it');
});

test('an artifact it writes is saved and listed', { skip, timeout: 60000 }, async () => {
  await newChat('make an e2e artifact');
  await idle();
  assert.ok(await ev(`!!document.querySelector('#view .artifact-card[data-identifier="account-page"]')`), 'the reply shows the artifact card');
  const listed = await ev(`__e2e.invoke('list_artifacts')`);
  assert.ok(listed.some((a) => a.identifier === 'account-page' && a.title === 'E2E account page' && a.content.includes('Made on the account')), JSON.stringify(listed.map((a) => a.identifier)));
  if (await ev(`!document.querySelector('#artifact-panel').classList.contains('hidden')`)) {
    await ev(`[...document.querySelectorAll('#artifact-panel .ap-head .icon-btn')].find((b) => b.title === __e2e.t('close')).click()`);
  }
  await ev(`__e2e.emit('navigate', { name: 'artifacts' })`);
  await waitFor('the Artifacts page to list it', `[...document.querySelectorAll('#view .artifact-tile .t')].some((e) => e.textContent === 'E2E account page')`);
});

test('a tool call through the CLI waits on the permission card', { skip, timeout: 60000 }, async () => {
  const status = await ev(`__e2e.invoke('mcp_save_config', { config: ${JSON.stringify(JSON.stringify({ mcpServers: { e2e: { url: `${mock.url}/mcp` } } }))} })`);
  assert.ok(status.servers.some((s) => s.name === 'e2e' && s.status === 'connected'), JSON.stringify(status.servers));
  await newChat('please echo hi');
  await waitFor('the permission card', `!!document.querySelector('#view .perm-card')`);
  assert.match(await ev(`document.querySelector('#view .perm-card').innerText`), /echo/);
  assert.equal(mock.mcpCalls.length, 0, 'nothing runs before the answer');
  // Allow once: the CLI's call comes through the app's connection to the server
  await ev(`document.querySelector('#view .perm-card .pactions button').click()`);
  await waitFor('the answer after the tool', `__e2e.turns().some((t) => t.text.includes('Done: hi'))`);
  await idle();
  assert.equal(await ev(`document.querySelectorAll('#view .perm-card').length`), 0);
  assert.deepEqual(mock.mcpCalls, [{ name: 'echo', arguments: { text: 'hi' } }]);
});

test('no uncaught errors in the page', { skip }, () => {
  assert.deepEqual(cdp.exceptions, []);
});
