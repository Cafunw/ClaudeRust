// End-to-end smoke test of the critical UI flows, driven over the Chrome
// DevTools Protocol in a running dev copy of the app. It starts nothing
// itself: run the app as dev/README.md says, then `npm run test:e2e`. With
// no dev copy listening on 127.0.0.1:9333 the tests are skipped.
//
// It points the dev copy's API base URL (and an MCP server) at a mock in
// this process, and restores both afterwards.
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { startMock, TITLE } from './mock_api.mjs';

const CDP = 'http://127.0.0.1:9333';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function devPage() {
  try {
    const targets = await (await fetch(`${CDP}/json`, { signal: AbortSignal.timeout(2000) })).json();
    return targets.find((t) => t.type === 'page' && !t.url.startsWith('devtools')) || null;
  } catch {
    return null;
  }
}

async function connect(page) {
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = () => reject(new Error('CDP connection failed')); });
  let id = 0;
  const pending = new Map();
  const exceptions = [];
  ws.onmessage = (m) => {
    const d = JSON.parse(m.data);
    if (d.id && pending.has(d.id)) { pending.get(d.id)(d); pending.delete(d.id); return; }
    if (d.method === 'Runtime.exceptionThrown') exceptions.push(d.params.exceptionDetails.exception?.description || d.params.exceptionDetails.text);
  };
  const send = (method, params = {}) => new Promise((resolve) => { const i = ++id; pending.set(i, resolve); ws.send(JSON.stringify({ id: i, method, params })); });
  const ev = async (expression) => {
    const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (r.error) throw new Error(r.error.message);
    if (r.result.exceptionDetails) throw new Error(`${expression.slice(0, 120)}: ${r.result.exceptionDetails.exception?.description || r.result.exceptionDetails.text}`);
    return r.result.result.value;
  };
  await send('Runtime.enable');
  return { send, ev, exceptions, close: () => ws.close() };
}

// page-side helpers, installed again after every load
const PRELUDE = `(async () => {
  if (window.__e2e) return true;
  const [i18n, store] = await Promise.all([import('./js/i18n.js'), import('./js/store.js')]);
  const view = () => document.querySelector('#view');
  window.__e2e = {
    t: i18n.t, store: store.store, emit: store.emit, invoke: window.__TAURI__.core.invoke,
    turns: () => [...view().querySelectorAll('.turn')].map((el) => ({
      role: el.classList.contains('user') ? 'user' : 'assistant',
      text: el.innerText,
      versions: el.querySelector('.branch-nav .count')?.textContent.replace(/\\s/g, '') || null,
      streaming: !!el.querySelector('.streaming-indicator:not(.hidden)'),
    })),
    running: () => !!view().querySelector('.composer [data-act=send].stop'),
    type: (text) => {
      const ta = view().querySelector('.composer textarea');
      ta.focus();
      ta.value = text;
      ta.dispatchEvent(new Event('input', { bubbles: true }));
      const b = view().querySelector('.composer [data-act=send]');
      if (b.disabled) return false;
      b.click();
      return true;
    },
    // a turn action by its tooltip
    action: (turn, title) => [...turn.querySelectorAll('.turn-actions button')].find((b) => b.title === title),
    key: (key, init = {}) => document.body.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...init })),
  };
  return true;
})()`;

const page = await devPage();
let skip = page ? false : `no dev copy listening on ${CDP} (see dev/README.md)`;
let cdp = null;
let boot = null;
if (page) {
  cdp = await connect(page);
  boot = await cdp.ev(`window.__TAURI__ ? window.__TAURI__.core.invoke('bootstrap') : null`).catch(() => null);
  // a dev copy on the installed app's identifier shares its settings and chats
  if (!boot) skip = `the page on ${CDP} is not ClaudeRust`;
  else if (/dev\.clauderust\.desktop([\\/]|$)/.test(boot.data_dir)) skip = 'the dev copy uses the installed app\'s data folder; start it with its own identifier (see dev/README.md)';
  else if (!boot.settings.has_api_key) skip = 'the dev copy has no API key; start it with CLAUDERUST_API_KEY=sk-ant-test (see dev/README.md)';
  if (skip) cdp.close();
}

let mock = null;
let ev = null;
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
  throw new Error(`timed out waiting for ${what}; last: ${JSON.stringify(last)}; mock rejected ${mock.rejected} requests`);
}

async function until(what, f, ms = 10000) {
  const t0 = Date.now();
  while (!f()) {
    if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
    await sleep(50);
  }
}

/** The current conversation's id. */
const convId = () => waitFor('an open conversation', `__e2e.store.route.name === 'chat' && __e2e.store.route.params.id`);

async function reloadPage() {
  await cdp.send('Page.reload', { ignoreCache: true });
  await sleep(500);
  await waitFor('the app to start', `document.readyState === 'complete' && !!document.querySelector('#sidebar .nav') && !!document.querySelector('#view .composer, #view .chat-col')`, 30000);
  await ev(PRELUDE);
}

/** Wait for the turn to end; the conversation's turns then. */
async function idle() {
  await waitFor('the turn to end', `!__e2e.running() && !__e2e.turns().some((t) => t.streaming)`, 30000);
  return ev('__e2e.turns()');
}

const lastSent = () => mock.requests[mock.requests.length - 1].messages;
const user = (text) => ({ role: 'user', content: [{ type: 'text', text }] });

before(async () => {
  if (skip) return;
  ev = cdp.ev;
  mock = await startMock();
  await ev(PRELUDE);
  const settings = await ev(`__e2e.invoke('get_settings')`);
  const patch = { base_url: mock.url, chat_engine: 'api', code_engine: 'builtin', code_worktree: 'off', code_checkpoints: false, notifications: false };
  const old = Object.fromEntries(Object.keys(patch).map((k) => [k, settings[k]]));
  const mcp = (await ev(`__e2e.invoke('mcp_status')`)).config;
  restore = async () => {
    await ev(`__e2e.invoke('save_settings', { patch: ${JSON.stringify(old)} })`);
    await ev(`__e2e.invoke('mcp_save_config', { config: ${JSON.stringify(JSON.stringify(mcp))} })`);
  };
  await ev(`__e2e.invoke('save_settings', { patch: ${JSON.stringify(patch)} })`);
  await reloadPage();
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
  }
});

test('a message is sent and its reply streams in', { skip, timeout: 60000 }, async () => {
  await ev(`__e2e.emit('navigate', { name: 'new' })`);
  await waitFor('the new chat composer', `!!document.querySelector('#view .welcome .composer textarea')`);
  assert.ok(await ev(`__e2e.type('e2e hello')`), 'the send button is enabled');
  made.push(await convId());
  // the reply arrives in pieces: part of it is shown before the end
  let partial = false;
  for (let i = 0; i < 200 && !partial; i++) {
    const a = (await ev('__e2e.turns()')).find((x) => x.role === 'assistant');
    if (a && a.streaming && a.text.includes('Answer 1') && !a.text.includes('in pieces.')) partial = true;
    else await sleep(20);
  }
  assert.ok(partial, 'saw the reply while it streamed');
  const turns = await idle();
  assert.deepEqual(turns.map((x) => x.role), ['user', 'assistant']);
  assert.match(turns[0].text, /e2e hello/);
  assert.match(turns[1].text, /Answer 1 to e2e hello\. The mock streams this reply in pieces\./);
  assert.deepEqual(lastSent(), [user('e2e hello')]);
  // the generated title, in the sidebar too
  await waitFor('the title', `document.querySelector('#sidebar .chat-link[data-id="${made[0]}"] .title')?.textContent === ${JSON.stringify(TITLE)}`);
});

test('Stop ends a slow reply and keeps what had arrived', { skip, timeout: 60000 }, async () => {
  assert.ok(await ev(`__e2e.type('slow e2e')`));
  await waitFor('the slow reply', `(__e2e.turns().at(-1).text.includes('w5 ')) && __e2e.running()`);
  await ev(`document.querySelector('#view .composer [data-act=send].stop').click()`);
  const turns = await idle();
  const reply = turns.at(-1);
  assert.equal(reply.role, 'assistant');
  assert.match(reply.text, /w5 /);
  assert.doesNotMatch(reply.text, /w399/);
  assert.ok(reply.text.includes(await ev(`__e2e.t('stopped')`)), reply.text);
  await until('the app to close the stream', () => mock.aborted === 1);
});

// the numbers of the two replies to the edited message
let editReply = 0;
let retryReply = 0;

test('editing a message makes a second version of it', { skip, timeout: 60000 }, async () => {
  await ev(`__e2e.action(document.querySelector('#view .turn.user'), __e2e.t('edit')).click()`);
  await waitFor('the edit box', `!!document.querySelector('#view .edit-box textarea')`);
  await ev(`(() => { document.querySelector('#view .edit-box textarea').value = 'e2e edited'; document.querySelector('#view .edit-box [data-a=save]').click(); })()`);
  await waitFor('the reply to the edit', `__e2e.turns().some((t) => t.text.includes('to e2e edited'))`);
  const turns = await idle();
  editReply = mock.requests.length;
  assert.deepEqual(turns.map((x) => x.role), ['user', 'assistant']);
  assert.match(turns[0].text, /e2e edited/);
  assert.match(turns[1].text, new RegExp(`Answer ${editReply} to e2e edited`));
  assert.equal(turns[0].versions, '2/2');
  // only the edited branch went to the API
  assert.deepEqual(lastSent(), [user('e2e edited')]);
});

test('Retry answers again as another version of the reply', { skip, timeout: 60000 }, async () => {
  retryReply = mock.requests.length + 1;
  await ev(`__e2e.action([...document.querySelectorAll('#view .turn.assistant')].at(-1), __e2e.t('retry')).click()`);
  await waitFor('the new reply', `__e2e.turns().some((t) => t.text.includes('Answer ${retryReply} to e2e edited'))`);
  const turns = await idle();
  assert.equal(turns.length, 2);
  assert.equal(turns.at(-1).versions, '2/2');
  assert.deepEqual(lastSent(), [user('e2e edited')]);
});

test('the version arrows move between the branches', { skip, timeout: 60000 }, async () => {
  const nav = (role, d) => `document.querySelector('#view .turn.${role} .branch-nav [data-d="${d}"]').click()`;
  // back to the first reply to the edit
  await ev(nav('assistant', -1));
  await waitFor('the first reply', `__e2e.turns().at(-1).versions === '1/2'`);
  assert.match((await ev('__e2e.turns()')).at(-1).text, new RegExp(`Answer ${editReply} to e2e edited`));
  // the message as first written, with everything that followed it
  await ev(nav('user', -1));
  await waitFor('the first version', `__e2e.turns()[0].versions === '1/2'`);
  let turns = await idle();
  assert.deepEqual(turns.map((x) => x.role), ['user', 'assistant', 'user', 'assistant']);
  assert.match(turns[0].text, /e2e hello/);
  assert.match(turns[2].text, /slow e2e/);
  // and back, to its newest reply
  await ev(nav('user', 1));
  await waitFor('the second version', `__e2e.turns()[0].versions === '2/2'`);
  turns = await idle();
  assert.equal(turns.length, 2);
  assert.match(turns[0].text, /e2e edited/);
  assert.match(turns[1].text, new RegExp(`Answer ${retryReply} to e2e edited`));
});

test('a tool call waits on the permission card', { skip, timeout: 60000 }, async () => {
  const status = await ev(`__e2e.invoke('mcp_save_config', { config: ${JSON.stringify(JSON.stringify({ mcpServers: { e2e: { url: `${mock.url}/mcp` } } }))} })`);
  assert.ok(status.servers.some((s) => s.name === 'e2e' && s.status === 'connected'), JSON.stringify(status.servers));
  assert.ok(await ev(`__e2e.type('please echo hi')`));
  await waitFor('the permission card', `!!document.querySelector('#view .perm-card')`);
  assert.match(await ev(`document.querySelector('#view .perm-card').innerText`), /echo/);
  assert.equal(mock.mcpCalls.length, 0, 'nothing runs before the answer');
  // Allow once
  await ev(`document.querySelector('#view .perm-card .pactions button').click()`);
  await waitFor('the answer after the tool', `__e2e.turns().some((t) => t.text.includes('Done: hi'))`);
  await idle();
  assert.equal(await ev(`document.querySelectorAll('#view .perm-card').length`), 0);
  assert.deepEqual(mock.mcpCalls, [{ name: 'echo', arguments: { text: 'hi' } }]);
});

// A built-in Code session runs the hooks of ~/.claude/settings.json (your own
// Claude Code hooks): this test is skipped where there are any.
const homeHooks = (() => {
  try { return Object.keys(JSON.parse(fs.readFileSync(path.join(os.homedir(), '.claude', 'settings.json'), 'utf8')).hooks || {}).length > 0; } catch { return false; }
})();

test('a Code session asks before it writes a file', { skip: skip || (homeHooks && 'a Code session would run the hooks in ~/.claude/settings.json'), timeout: 60000 }, async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'clauderust-e2e-'));
  mock.cwd = dir;
  let conv = null;
  try {
    conv = await ev(`__e2e.invoke('create_conversation', { req: { kind: 'code', cwd: ${JSON.stringify(dir)}, options: { engine: 'builtin', worktree: false, permission_mode: 'default' } } })`);
    await ev(`__e2e.emit('navigate', { name: 'chat', params: { id: ${JSON.stringify(conv.id)} } })`);
    await waitFor('the session', `__e2e.store.route.params.id === ${JSON.stringify(conv.id)} && !!document.querySelector('#view .composer textarea')`);
    assert.ok(await ev(`__e2e.type('please write the file')`));
    await waitFor('the permission card', `!!document.querySelector('#view .perm-card')`);
    assert.ok((await ev(`document.querySelector('#view .perm-card').innerText`)).includes(await ev(`__e2e.t('write_file')`)));
    assert.ok(!fs.existsSync(path.join(dir, 'e2e.txt')), 'nothing written before the answer');
    await ev(`document.querySelector('#view .perm-card .pactions button').click()`);
    await waitFor('the answer after the tool', `__e2e.turns().some((t) => t.text.includes('Done:'))`);
    await idle();
    assert.equal(fs.readFileSync(path.join(dir, 'e2e.txt'), 'utf8'), 'written by the e2e test\n');
  } finally {
    // gone before the search test, which looks for the chat by its title
    if (conv) await ev(`__e2e.invoke('delete_conversation', { id: ${JSON.stringify(conv.id)} })`);
    mock.cwd = null;
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('Settings opens and shows its sections', { skip, timeout: 60000 }, async () => {
  await ev(`__e2e.key(',', { code: 'Comma', ctrlKey: !navigator.userAgent.includes('Mac'), metaKey: navigator.userAgent.includes('Mac') })`);
  await waitFor('Settings', `!!document.querySelector('.modal-scrim .settings')`);
  for (const tab of ['general', 'appearance', 'features', 'developer', 'code', 'shortcuts']) {
    await ev(`document.querySelector('.settings-nav [data-tab=${tab}]').click()`);
    await waitFor(`the ${tab} section`, `document.querySelector('.settings-nav [data-tab=${tab}]').classList.contains('active') && document.querySelector('.settings-body').children.length > 0`);
  }
  // the MCP server of the permission test is listed under Developer
  await ev(`document.querySelector('.settings-nav [data-tab=developer]').click()`);
  await waitFor('the MCP server', `document.querySelector('.settings-body').innerText.includes('e2e')`);
  await ev(`__e2e.key('Escape', { code: 'Escape' })`);
  await waitFor('Settings to close', `!document.querySelector('.modal-scrim')`);
});

test('the sidebar search finds the conversation', { skip, timeout: 60000 }, async () => {
  await ev(`__e2e.emit('navigate', { name: 'new' })`);
  await waitFor('the new chat page', `__e2e.store.route.name === 'new'`);
  await ev(`document.querySelector('#sidebar [data-nav=search]').click()`);
  await waitFor('the search box', `!!document.querySelector('.palette input')`);
  await ev(`(() => { const i = document.querySelector('.palette input'); i.value = 'smoke chat'; i.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  await waitFor('the result', `[...document.querySelectorAll('.palette .res .t')].some((e) => e.textContent === ${JSON.stringify(TITLE)})`);
  await ev(`[...document.querySelectorAll('.palette .res')].find((b) => b.querySelector('.t').textContent === ${JSON.stringify(TITLE)}).click()`);
  assert.equal(await convId(), made[0]);
  assert.equal(await ev(`document.querySelector('.palette')`), null);
  await waitFor('the conversation', `__e2e.turns().some((t) => t.text.includes('e2e edited'))`);
});

test('no uncaught errors in the page', { skip }, () => {
  assert.deepEqual(cdp.exceptions, []);
});
