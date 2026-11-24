// End-to-end smoke test of the critical UI flows, driven over the Chrome
// DevTools Protocol in a running dev copy of the app. `npm run e2e`
// (ui/e2e/run.mjs) starts that copy, runs this and stops it; `npm run
// test:e2e` drives a copy you started as dev/README.md says. With no dev
// copy listening on 127.0.0.1:9333 the tests are skipped (under run.mjs,
// which sets CLAUDERUST_E2E_REQUIRED, anything that would skip fails).
//
// It points the dev copy's API base URL (and an MCP server) at a mock in
// this process, and restores both afterwards.
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { startMock, TITLE, ARTIFACT_ERROR } from './mock_api.mjs';
import { contrast } from './contrast.mjs';
import { attach, cannotRun, CDP, connect, PRELUDE, sleep } from './cdp.mjs';

const { cdp, skip } = await attach();

let mock = null;
let ev = null;
let restore = null;
const made = [];

/** Wait for `expr` to be truthy in the page (or with `run`, another frame's evaluate). */
async function waitFor(what, expr, ms = 20000, run = ev) {
  const t0 = Date.now();
  let last;
  while (Date.now() - t0 < ms) {
    last = await run(expr);
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
  // theme: the dark-mode test emulates the system's colour scheme
  const patch = { base_url: mock.url, chat_engine: 'api', code_engine: 'builtin', code_worktree: 'off', code_checkpoints: false, notifications: false, artifacts_enabled: true, theme: 'system' };
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

test('a Code session asks before it writes a file', { skip, timeout: 60000 }, async (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'clauderust-e2e-'));
  mock.cwd = dir;
  let conv = null;
  try {
    conv = await ev(`__e2e.invoke('create_conversation', { req: { kind: 'code', cwd: ${JSON.stringify(dir)}, options: { engine: 'builtin', worktree: false, permission_mode: 'default' } } })`);
    // a built-in Code session runs your own Claude Code hooks (from its first
    // message on): not in a test. run.mjs starts the copy without them.
    const hooks = await ev(`__e2e.invoke('hooks_info', { conversationId: ${JSON.stringify(conv.id)} })`);
    if (hooks.active.length) {
      cannotRun(t, `the dev copy would run ${hooks.active.length} hook(s) from your Claude Code settings; start it with CLAUDE_CONFIG_DIR on an empty folder (npm run e2e does)`);
      return;
    }
    await ev(`__e2e.emit('navigate', { name: 'chat', params: { id: ${JSON.stringify(conv.id)} } })`);
    // its own view (the route changes first, while the previous chat's composer is still there)
    await waitFor('the session', `document.querySelector('#view .topbar .code-topinfo')?.textContent === ${JSON.stringify(path.basename(dir))} && !!document.querySelector('#view .composer textarea')`);
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

// ------------------------------------------------------------- artifacts

const PANEL_FRAME = `document.querySelector('#artifact-panel:not(.hidden) .ap-body iframe')`;

/** A session in the artifact panel's preview frame other than the one at
 *  `not`, once its document has loaded. The frame has a process of its own,
 *  so a debugging target of its own. */
async function previewFrame(not = '') {
  const src = await waitFor('the artifact preview', `(${PANEL_FRAME}?.src || '') !== ${JSON.stringify(not)} && ${PANEL_FRAME}?.src`);
  const t0 = Date.now();
  let target;
  while (!(target = (await (await fetch(`${CDP}/json`)).json()).find((x) => x.type === 'iframe' && x.url === src))) {
    if (Date.now() - t0 > 10000) throw new Error(`no debugging target for the preview frame ${src}`);
    await sleep(100);
  }
  const frame = await connect(target);
  await waitFor('the preview to load', `document.readyState === 'complete'`, 10000, frame.ev);
  return { frame, src };
}

const closePanel = async () => {
  await ev(`[...document.querySelectorAll('#artifact-panel .ap-head .icon-btn')].find((b) => b.title === __e2e.t('close')).click()`);
  await waitFor('the panel to close', `document.querySelector('#artifact-panel').classList.contains('hidden')`);
};

test('an artifact in a reply opens in the preview panel', { skip, timeout: 60000 }, async () => {
  await ev(`__e2e.emit('navigate', { name: 'new' })`);
  await waitFor('the new chat composer', `!!document.querySelector('#view .welcome .composer textarea')`);
  assert.ok(await ev(`__e2e.type('show me an artifact')`));
  made.push(await convId());
  // it opens by itself as it is written
  await waitFor('the panel to open', `!document.querySelector('#artifact-panel').classList.contains('hidden')`);
  await idle();
  const card = `document.querySelector('#view .artifact-card[data-identifier="page"]')`;
  assert.ok(await ev(`!!${card}`), 'the reply shows the artifact card');
  // and again from its card
  await closePanel();
  await ev(`${card}.click()`);
  const { frame } = await previewFrame();
  try {
    assert.equal(await frame.ev(`document.getElementById('e2e-title').textContent`), 'Hello from the artifact');
  } finally {
    frame.close();
  }
});

test('an HTML artifact is readable in the dark and the light theme', { skip, timeout: 60000 }, async () => {
  let src = await waitFor('the open preview', `${PANEL_FRAME}?.src`);
  try {
    for (const scheme of ['dark', 'light']) {
      const before = await ev(`document.documentElement.dataset.theme`);
      // the system's colour scheme, in the app and in the frame (which has a process of its own)
      const media = { features: [{ name: 'prefers-color-scheme', value: scheme }] };
      await cdp.send('Emulation.setEmulatedMedia', media);
      await waitFor(`the ${scheme} theme`, `document.documentElement.dataset.theme === '${scheme}'`);
      // a theme change draws the preview again, in a new frame
      const { frame, src: now } = await previewFrame(before === scheme ? '' : src);
      src = now;
      try {
        await frame.send('Emulation.setEmulatedMedia', media);
        assert.ok(await frame.ev(`matchMedia('(prefers-color-scheme: ${scheme})').matches`), `the frame did not take the ${scheme} scheme`);
        const texts = await frame.ev(`['e2e-title', 'e2e-text'].map((id) => {
          const el = document.getElementById(id);
          const layers = [];
          for (let n = el; n; n = n.parentElement) layers.push(getComputedStyle(n).backgroundColor);
          return { id, color: getComputedStyle(el).color, layers };
        })`);
        // what shows through a transparent page: the frame element and what is under it
        const under = await ev(`(() => {
          const layers = [];
          for (let n = ${PANEL_FRAME}; n; n = n.parentElement) layers.push(getComputedStyle(n).backgroundColor);
          return layers;
        })()`);
        for (const { id, color, layers } of texts) {
          const all = [...layers, ...under, 'rgb(255, 255, 255)'];
          const ratio = contrast(color, all);
          assert.ok(ratio >= 4.5, `${scheme}: #${id} in ${color} on ${all.join(' / ')} has a contrast of ${ratio.toFixed(2)}:1`);
        }
      } finally {
        frame.close();
      }
    }
    await closePanel();
  } finally {
    await cdp.send('Emulation.setEmulatedMedia', { features: [] });
  }
});

test('an artifact that throws offers to ask Claude for a fix', { skip, timeout: 60000 }, async () => {
  await ev(`__e2e.emit('navigate', { name: 'new' })`);
  await waitFor('the new chat composer', `!!document.querySelector('#view .welcome .composer textarea')`);
  assert.ok(await ev(`__e2e.type('show me a broken artifact')`));
  made.push(await convId());
  await idle();
  const fix = `[...document.querySelectorAll('#artifact-panel .ap-alert button')].find((b) => b.textContent.trim() === __e2e.t('art_try_fix'))`;
  await waitFor('the Try fixing button', `!!${fix}`);
  assert.match(await ev(`document.querySelector('#artifact-panel .ap-alert').innerText`), new RegExp(ARTIFACT_ERROR));
  const sent = mock.requests.length;
  await ev(`${fix}.click()`);
  await until('the request for a fix', () => mock.requests.length > sent);
  const asked = lastSent().at(-1).content.filter((b) => b.type === 'text').map((b) => b.text).join('\n');
  assert.ok(asked.includes(ARTIFACT_ERROR), `the message for a fix has the error: ${asked}`);
  await idle();
  await closePanel();
});

test('the delete menu item removes a conversation from the sidebar', { skip, timeout: 60000 }, async () => {
  await ev(`__e2e.emit('navigate', { name: 'new' })`);
  await waitFor('the new chat composer', `!!document.querySelector('#view .welcome .composer textarea')`);
  assert.ok(await ev(`__e2e.type('e2e delete me')`));
  const id = await convId();
  made.push(id);
  await idle();
  const link = `document.querySelector('#sidebar .chat-link[data-id="${id}"]')`;
  await waitFor('the sidebar link', `!!${link}`);
  await ev(`${link}.querySelector('.more').click()`);
  await waitFor('the menu', `!!document.querySelector('.popover .menu-item.danger')`);
  await ev(`document.querySelector('.popover .menu-item.danger').click()`);
  await waitFor('the confirmation', `!!document.querySelector('.modal-scrim .modal-foot .btn.danger')`);
  await ev(`document.querySelector('.modal-scrim .modal-foot .btn.danger').click()`);
  await waitFor('the link to go', `!${link}`);
  assert.equal(await ev(`__e2e.store.route.name`), 'new', 'the deleted chat is no longer open');
  assert.ok(!(await ev(`__e2e.invoke('list_conversations', {})`)).some((c) => c.id === id), 'deleted from the database');
});

test('a long conversation opens on its last turns and loads the rest as you scroll up', { skip, timeout: 120000 }, async (t) => {
  // imported as a claude.ai export: no model needed for 300 turns
  const title = `E2E long chat ${crypto.randomUUID()}`;
  const n = 150;
  const at = (i) => new Date(Date.UTC(2026, 0, 1) + i * 1000).toISOString();
  const chat_messages = Array.from({ length: n }, (_, i) => [
    { uuid: crypto.randomUUID(), sender: 'human', text: `question ${i} of the long chat`, created_at: at(2 * i) },
    { uuid: crypto.randomUUID(), sender: 'assistant', text: `## Answer ${i}\n\nSome **bold** text, \`code\` and a list:\n\n- one\n- two\n\n\`\`\`js\nconst x${i} = ${i};\n\`\`\``, created_at: at(2 * i + 1) },
  ]).flat();
  const file = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'clauderust-e2e-')), 'conversations.json');
  fs.writeFileSync(file, JSON.stringify([{ uuid: crypto.randomUUID(), name: title, created_at: at(0), chat_messages }]));
  try {
    const r = await ev(`__e2e.invoke('import_claude_export', { path: ${JSON.stringify(file)} })`);
    assert.equal(r.imported, 1, JSON.stringify(r));
  } finally {
    fs.rmSync(path.dirname(file), { recursive: true, force: true });
  }
  const id = (await ev(`__e2e.invoke('list_conversations', {})`)).find((c) => c.title === title).id;
  made.push(id);
  await ev(`__e2e.emit('navigate', { name: 'chat', params: { id: ${JSON.stringify(id)} } })`);
  await waitFor('the last answer', `__e2e.store.route.params.id === ${JSON.stringify(id)} && __e2e.turns().at(-1)?.text.includes('Answer ${n - 1}')`);
  const pageTurns = await ev(`import('./js/turnwindow.js').then((m) => m.PAGE_TURNS)`);
  const opened = await ev(`[document.querySelectorAll('#view .turn').length, document.querySelectorAll('#view *').length]`);
  assert.ok(opened[0] <= pageTurns + 1, `opened with ${opened[0]} turns drawn`);
  // scrolled to the top a page at a time, the first question comes into view
  // (the first one drawn: one far above the view may be parked, empty; and
  // textContent, as innerText is empty for one the browser skips off screen)
  const FIRST = `Number(/question (\\d+) of the long chat/.exec(document.querySelector('#view .turn.user:not(.parked)')?.textContent)?.[1] ?? -1)`;
  for (let was = await ev(FIRST); was !== 0; was = await ev(FIRST)) {
    await ev(`(() => { const s = document.querySelector('#view .chat-scroll'); s.scrollTop = 0; s.dispatchEvent(new Event('scroll')); })()`);
    await waitFor(`the page before question ${was}`, `${FIRST} < ${was}`, 10000);
  }
  // the turns far from the view (more than 6 screens) are parked: emptied,
  // keeping their height, so the page doesn't grow with the conversation
  const top = await waitFor('the far turns to be parked', `(() => {
    const s = document.querySelector('#view .chat-scroll');
    const top = s.getBoundingClientRect().top;
    const view = s.clientHeight;
    const turns = [...document.querySelectorAll('#view .turn')];
    const far = turns.filter((el) => { const r = el.getBoundingClientRect(); return Math.max(top - r.bottom, r.top - top - view, 0) > 6 * view; });
    const left = far.filter((el) => !el.classList.contains('parked')).length;
    return left === 0 && { turns: turns.length, far: far.length, drawn: turns.filter((el) => !el.classList.contains('parked')).length, nodes: document.querySelectorAll('#view *').length };
  })()`);
  t.diagnostic(`opened with ${opened[0]} turns and ${opened[1]} nodes; at the top ${top.drawn} of ${top.turns} turns drawn, ${top.nodes} nodes`);
  assert.equal(top.turns, 2 * n, 'every turn has its place');
  assert.ok(top.far > 0, 'the conversation is longer than 6 screens');
  // and back at the end, the last answer is drawn again
  await ev(`(() => { const s = document.querySelector('#view .chat-scroll'); s.scrollTop = s.scrollHeight; })()`);
  await waitFor('the last answer again', `__e2e.turns().at(-1).text.includes('Answer ${n - 1}')`);
});

test('no uncaught errors in the page', { skip }, () => {
  assert.deepEqual(cdp.exceptions, []);
});
