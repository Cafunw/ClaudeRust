// MCP Apps (spec 2026-01-26): the page an MCP tool names in
// _meta.ui.resourceUri, shown under the tool's card in the artifact sandbox
// (mcp_apps.rs serves it with its CSP), and the host side of the page's
// postMessage JSON-RPC bridge. The page is untrusted: it gets the tool's
// input and result, may call its own server's tools (asked about like the
// model's calls, and limited, in mcp_apps.rs), open links after a question,
// and say how tall it is. Nothing else is answered.
import { t } from './i18n.js';
import { call, artifactUrl } from './api.js';
import { esc, modal, button } from './util.js';
import { openLink } from './artifact_link.js';

export const PROTOCOL = '2026-01-26';
const MAX_MESSAGE_CHARS = 1024 * 1024;
const MAX_HEIGHT = 720;

/** `d` if it is a JSON-RPC 2.0 request or notification a page may send (a
 *  small object with a short method name and a plain id), else null. */
export function validMessage(d) {
  if (!d || typeof d !== 'object' || Array.isArray(d) || d.jsonrpc !== '2.0') return null;
  if (typeof d.method !== 'string' || !d.method || d.method.length > 128) return null;
  const idOk = d.id === undefined || (typeof d.id === 'number' && Number.isFinite(d.id)) || (typeof d.id === 'string' && d.id.length <= 128);
  if (!idOk || (d.params !== undefined && (typeof d.params !== 'object' || d.params === null || Array.isArray(d.params)))) return null;
  try { if (JSON.stringify(d).length > MAX_MESSAGE_CHARS) return null; } catch { return null; }
  return d;
}

/** The host side of one page's bridge: `handle(message)` answers it through
 *  `env.send`. env: { input, result, hostContext(), resize(h), callTool(name, args), openLink(url) }. */
export function createBridge(env) {
  let ready = false;
  const send = (m) => env.send({ jsonrpc: '2.0', ...m });
  return async function handle(raw) {
    const m = validMessage(raw);
    if (!m) return;
    const { id, method } = m;
    const params = m.params || {};
    const asks = id !== undefined;
    const fail = (code, message) => asks && send({ id, error: { code, message: String(message).slice(0, 500) } });
    try {
      switch (method) {
        case 'ui/initialize':
          if (asks) send({ id, result: { protocolVersion: PROTOCOL, hostInfo: { name: 'ClaudeRust' }, hostCapabilities: { openLinks: {}, serverTools: {}, logging: {} }, hostContext: env.hostContext() } });
          return;
        case 'ui/notifications/initialized':
          // nothing goes to the page before this, the input and the result once
          if (ready) return;
          ready = true;
          send({ method: 'ui/notifications/tool-input', params: { arguments: env.input } });
          if (env.result) send({ method: 'ui/notifications/tool-result', params: env.result });
          return;
        case 'ui/notifications/size-changed':
          env.resize(Number(params.height));
          return;
        case 'ping':
          if (asks) send({ id, result: {} });
          return;
        case 'notifications/message':
          return;
        case 'tools/call':
          if (!asks) return;
          if (!ready) { fail(-32000, 'The app is not initialized.'); return; }
          if (typeof params.name !== 'string' || !params.name || params.name.length > 128) { fail(-32602, 'tools/call needs a tool name.'); return; }
          send({ id, result: await env.callTool(params.name, params.arguments && typeof params.arguments === 'object' && !Array.isArray(params.arguments) ? params.arguments : {}) });
          return;
        case 'ui/open-link':
          if (!asks) return;
          await env.openLink(String(params.url || ''));
          send({ id, result: {} });
          return;
        case 'ui/request-display-mode':
          if (asks) send({ id, result: { mode: 'inline' } });
          return;
        default:
          fail(-32601, `Method not supported: ${method}`);
      }
    } catch (e) {
      fail(-32000, (e && e.message) || e);
    }
  };
}

/** One question at a time: may the app run `tool` of `server` with `args`? */
let asking = Promise.resolve();
function allowCall(server, tool, args) {
  const ask = () => new Promise((resolve) => {
    let done = false;
    const finish = (v) => { if (!done) { done = true; resolve(v); m.close(); } };
    const shown = JSON.stringify(args, null, 2).slice(0, 2000);
    const m = modal({
      title: t('mcpapp_call_title'),
      body: `<p style="margin:4px 0 8px;font-size:14.5px;line-height:1.5">${esc(t('mcpapp_call_body', { server, tool }))}</p><pre class="mono small" style="max-height:220px;overflow:auto;margin:0">${esc(shown)}</pre>`,
      footer: [button(t('elicit_decline'), '', () => finish(false)), button(t('allow_once'), 'primary', () => finish(true))],
      onClose: () => { if (!done) { done = true; resolve(false); } },
    });
  });
  const next = asking.then(ask, ask);
  asking = next.catch(() => {});
  return next;
}

const frames = new Map(); // iframe -> handle

/** Show the app of tool part `p` (a finished MCP tool call) in `el`, if its
 *  tool has one; true when it does. `ctx`: { conversationId }. */
export async function mountApp(el, p, ctx) {
  if (!p || p.kind !== 'tool' || p.status !== 'done' || !p.server || p.server === 'anthropic' || !ctx || !ctx.conversationId) return false;
  if (el.querySelector(':scope > iframe.mcp-app')) return true;
  const info = await call('mcp_app_info', { apiName: p.name, input: p.input || {} }).catch(() => null);
  if (!info || !el.isConnected) return false;
  const opened = await call('mcp_app_open', { server: info.server, uri: info.uri }).catch(() => null);
  if (!opened || !el.isConnected || el.querySelector(':scope > iframe.mcp-app')) return !!opened;
  const frame = document.createElement('iframe');
  frame.className = 'mcp-app' + (opened.prefers_border === false ? '' : ' bordered');
  // no same-origin, no popups, no modals: an opaque origin with scripts
  frame.setAttribute('sandbox', 'allow-scripts allow-forms');
  frame.setAttribute('referrerpolicy', 'no-referrer');
  frame.title = t('mcpapp_frame_title', { tool: info.tool });
  frame.style.height = '160px';
  const callArgs = { conversationId: ctx.conversationId, server: info.server };
  const handle = createBridge({
    send: (m) => frame.contentWindow && frame.contentWindow.postMessage(m, '*'),
    input: p.input || {},
    result: info.result || { content: [{ type: 'text', text: (p.result && p.result.text) || '' }], isError: !!(p.result && p.result.is_error) },
    hostContext: () => ({
      theme: document.documentElement.dataset.theme === 'dark' ? 'dark' : 'light',
      displayMode: 'inline',
      availableDisplayModes: ['inline'],
      locale: navigator.language,
      timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
      platform: 'desktop',
      toolInfo: { tool: { name: info.tool } },
      containerDimensions: { maxHeight: MAX_HEIGHT },
    }),
    resize: (h) => { if (Number.isFinite(h) && h > 0) frame.style.height = `${Math.min(MAX_HEIGHT, Math.max(40, Math.ceil(h)))}px`; },
    callTool: async (tool, args) => {
      let r = await call('mcp_app_call', { ...callArgs, tool, arguments: args, approved: false });
      if (r.needs_approval) {
        if (!(await allowCall(info.server, tool, args))) throw new Error('The user did not allow this tool call.');
        r = await call('mcp_app_call', { ...callArgs, tool, arguments: args, approved: true });
      }
      return r.result;
    },
    openLink,
  });
  frames.set(frame, handle);
  el.appendChild(frame);
  frame.src = artifactUrl(opened.token);
  return true;
}

// what the apps' pages send: only from a frame of ours, each to its own bridge
window.addEventListener('message', (e) => {
  for (const [frame, handle] of frames) {
    if (!frame.isConnected) { frames.delete(frame); continue; }
    if (frame.contentWindow !== e.source) continue;
    // a link the frame guard caught (artifact_guard.js)
    if (e.data && e.data.__artifact === 'link') openLink(String(e.data.url || ''));
    else handle(e.data);
    return;
  }
});
