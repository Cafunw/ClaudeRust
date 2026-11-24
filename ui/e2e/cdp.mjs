// Driving a running dev copy of the app over the Chrome DevTools Protocol, for the e2e tests: its page, a
// connection to it, and page-side helpers. `npm run e2e` (run.mjs) starts the copy; with none listening on
// CLAUDERUST_E2E_PORT (default 9333) the tests are skipped (under run.mjs, which sets CLAUDERUST_E2E_REQUIRED,
// anything that would skip fails).
import assert from 'node:assert/strict';

export const CDP = `http://127.0.0.1:${process.env.CLAUDERUST_E2E_PORT || 9333}`;
export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const required = !!process.env.CLAUDERUST_E2E_REQUIRED;

/** Skip `t`, or fail it when every test has to run. */
export function cannotRun(t, why) {
  if (required) assert.fail(why);
  t.skip(why);
}

async function devPage() {
  try {
    const targets = await (await fetch(`${CDP}/json`, { signal: AbortSignal.timeout(2000) })).json();
    return targets.find((t) => t.type === 'page' && !t.url.startsWith('devtools')) || null;
  } catch {
    return null;
  }
}

export async function connect(page) {
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
export const PRELUDE = `(async () => {
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

/** The dev copy's page, connected; `skip` says why the tests can't run on it (they fail instead under run.mjs). */
export async function attach() {
  const page = await devPage();
  let skip = page ? false : `no dev copy listening on ${CDP} (see dev/README.md)`;
  let cdp = null;
  let boot = null;
  if (page) {
    cdp = await connect(page);
    // (a copy just started may still be loading its page)
    for (let i = 0; i < 20 && !boot; i++) {
      boot = await cdp.ev(`window.__TAURI__ ? window.__TAURI__.core.invoke('bootstrap') : null`).catch(() => null);
      if (!boot) await sleep(500);
    }
    // a dev copy on the installed app's identifier shares its settings and chats
    if (!boot) skip = `the page on ${CDP} is not ClaudeRust`;
    else if (/dev\.clauderust\.desktop([\\/]|$)/.test(boot.data_dir)) skip = 'the dev copy uses the installed app\'s data folder; start it with its own identifier (see dev/README.md)';
    else if (!boot.settings.has_api_key) skip = 'the dev copy has no API key; start it with CLAUDERUST_API_KEY=sk-ant-test (see dev/README.md)';
    if (skip) cdp.close();
  }
  if (skip && required) throw new Error(`e2e: ${skip}`);
  return { cdp: skip ? null : cdp, skip };
}
