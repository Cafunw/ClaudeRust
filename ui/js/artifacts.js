// Artifact side panel and the Artifacts gallery page.
import { icon, ARTIFACT_ICON } from './icons.js';
import { t, currentLanguage } from './i18n.js';
import { call, artifactUrl } from './api.js';
import { store, emit, saveLocal, loadLocal } from './store.js';
import { $, esc, h, showMenu, errorToast, relTime, searchToggle, modal, button } from './util.js';
import { openLink } from './artifact_link.js';
import { copyText, decorate, artifactKindText } from './parts.js';
import { storageCall } from './artifact_storage.js';
import { resolveVersions } from './artifact_update.js';
import { createConsent, createGrants, createComplete, createToolCall, toolScope, toolError, fixMessage, errorText } from './artifact_bridge.js';
import { shownArtifact } from './branch_panel.js';
import { reopenId, wrapTab } from './artifact_sheet.js';
import { errorText as failureText } from './neterr.js';
// (chat.js imports this module too: reload is only called once both are loaded)
import { reload as reloadChat } from './chat.js';
import { renderPreview } from './artifact_preview.js';
import { panelView } from './artifact_view.js';

let panel = null;
let renderTimer = null;
let renderSeq = 0;
let fetching = null; // { panel, id } of the document request under way
// the panel covers the whole window (Esc, or the toolbar's button, leaves it)
let full = false;
// icons.js has no "minimize": the toolbar's way out of full screen
const MINIMIZE = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 14h6v6M20 10h-6V4M14 10l7-7M3 21l7-7"/></svg>';

function aside() { return $('#artifact-panel'); }

function isDark() { return document.documentElement.dataset.theme === 'dark'; }

function langFor(a) {
  if (a.language) return a.language;
  return { 'application/vnd.ant.react': 'jsx', 'text/html': 'html', 'image/svg+xml': 'xml', 'application/vnd.ant.mermaid': 'text', 'text/markdown': 'markdown', [SLIDES]: 'html', [DOC]: 'markdown', [DASHBOARD]: 'json' }[a.type] || 'text';
}

// the typed artifacts (artifacts.rs is_typed): the app's own runtime draws them, the person edits them in place
const SLIDES = 'application/vnd.ant.slides', DOC = 'application/vnd.ant.document', SHEET = 'application/vnd.ant.sheet', DASHBOARD = 'application/vnd.ant.dashboard';
const TYPED = new Set([SLIDES, DOC, SHEET, DASHBOARD]);
const iconFor = (type) => ARTIFACT_ICON[type] || 'file';

/** What a typed artifact downloads as: artifacts.rs export_file's formats, and 'print' (PDF, by the frame). */
function exportFormats(type) {
  switch (type) {
    case SLIDES: return [['print', 'art_fmt_pdf'], ['html', 'art_fmt_deck']];
    case DOC: return [['docx', 'art_fmt_docx'], ['print', 'art_fmt_pdf'], ['source', 'art_fmt_md'], ['html', 'art_fmt_html']];
    case SHEET: return [['source', 'art_fmt_csv']];
    case DASHBOARD: return [['source', 'art_fmt_json']];
    default: return [];
  }
}

export function onConversationOpened(convId, map) {
  if (panel && panel.convId !== convId) close();
  else if (panel) { panel.map = map; scheduleRender(); }
}

export function update(convId, map) {
  if (!panel || panel.convId !== convId) return;
  panel.map = map;
  scheduleRender();
}

function scheduleRender() {
  if (renderTimer) return;
  renderTimer = setTimeout(() => { renderTimer = null; render(); }, 120);
}

// as wide as app.css's narrow layout, where the panel is a sheet over the chat
const sheet = window.matchMedia('(max-width: 900px)');
// the artifact shown when the panel last closed: the topbar's toggle brings it back
let lastShown = null;

export function open(convId, map, identifier, version, { streaming = false } = {}) {
  const followLatest = !version || streaming;
  // errors: `${identifier}@${version}` -> the first error its page reported
  panel = { convId, map, identifier, version, view: null, followLatest, lastDocKey: null, errors: new Map() };
  const el = aside();
  const wasHidden = el.classList.contains('hidden');
  if (el.classList.contains('hidden')) window.dispatchEvent(new Event('artifact-opened'));
  el.classList.remove('hidden');
  // only shown in narrow windows (CSS): a click on the dimmed chat goes back to it
  let scrim = $('#artifact-scrim');
  if (!scrim) { scrim = h('<div class="ap-scrim" id="artifact-scrim"></div>'); scrim.onclick = close; el.before(scrim); }
  scrim.classList.remove('hidden');
  if (wasHidden && window.innerWidth < 1500 && !store.sidebarCollapsed) emit('collapse-sidebar');
  const w = loadLocal().artifactWidth;
  if (w) el.style.width = w + 'px';
  render();
  document.querySelectorAll('.artifact-card').forEach((c) => c.classList.toggle('open', c.dataset.identifier === identifier));
  pressed(true);
  // a sheet you opened takes the keyboard with it (not one that opens by itself while a reply streams)
  if (wasHidden && sheet.matches && !streaming) { const b = el.querySelector('.ap-back'); if (b) b.focus(); }
}

export function togglePanel(convId, map) {
  if (panel && panel.convId === convId) { close(); return; }
  const id = reopenId([...map.keys()], convId, lastShown);
  if (id) open(convId, map, id, null);
}

export function close() {
  if (panel) lastShown = { convId: panel.convId, identifier: panel.identifier };
  const back = panel && aside().contains(document.activeElement);
  panel = null;
  full = false;
  const el = aside();
  el.classList.remove('full');
  el.classList.add('hidden');
  el.innerHTML = '';
  const scrim = $('#artifact-scrim');
  if (scrim) scrim.classList.add('hidden');
  document.querySelectorAll('.artifact-card.open').forEach((c) => c.classList.remove('open'));
  pressed(false);
  // focus was in the panel: back to the toggle that reopens it, else the composer
  if (back) { const to = $('.topbar .ap-toggle') || $('#view .composer textarea'); if (to) to.focus(); }
}

export function isOpen() { return !!panel; }

function setFull(on) {
  full = on;
  aside().classList.toggle('full', on);
  render();
  // a deck that asked for the full window stops presenting with it
  const f = aside().querySelector('.ap-body iframe');
  if (!on && f && f.contentWindow && f.dataset.kind === SLIDES) f.contentWindow.postMessage({ __artifact: 'present', on: false }, '*');
  if (!on) aside().querySelectorAll('.ap-head, .ap-alert').forEach((x) => (x.style.display = ''));
}

/** The topbar's toggle shows whether the panel is open. */
function pressed(on) {
  document.querySelectorAll('.ap-toggle').forEach((b) => { b.classList.toggle('on', on); b.setAttribute('aria-pressed', String(on)); });
}

/** An artifact's versions, each update applied to the version before it. */
function versionsOf(identifier) { return resolveVersions(panel.map.get(identifier) || []); }

function currentArtifact() {
  const list = versionsOf(panel.identifier);
  if (!list.length) return null;
  if (panel.followLatest || !panel.version) return list[list.length - 1];
  return list.find((v) => v.version === panel.version) || list[list.length - 1];
}

async function render() {
  if (!panel) return;
  const el = aside();
  // a branch without the artifact shown, but with others: its latest
  const shown = shownArtifact([...panel.map.keys()], panel.identifier);
  if (shown && shown !== panel.identifier) Object.assign(panel, { identifier: shown, version: null, followLatest: true, view: null });
  const a = currentArtifact();
  if (!a) { el.innerHTML = `<div class="ap-empty">${t('no_artifacts')}</div>`; return; }
  const list = versionsOf(panel.identifier);
  // (one cut off is marked so by drawAlert)
  const { live, previewable, view, wrap } = panelView(a, panel.view);

  if (!el.querySelector('.ap-head')) {
    el.innerHTML = `<div class="resizer"></div><div class="ap-head"></div><div class="ap-alert"></div><div class="ap-body"></div>`;
    bindResizer(el.querySelector('.resizer'));
  }
  const head = el.querySelector('.ap-head');
  // the head is drawn anew on each render (every 120 ms while it streams): the focused control keeps the focus
  const focusKey = head.contains(document.activeElement) ? document.activeElement.dataset.k : null;
  head.innerHTML = '';
  const back = h(`<button class="ap-back" data-k="back" title="${esc(t('art_back'))}">${icon('arrowLeft', 16)}<span>${esc(t('art_back'))}</span></button>`);
  back.onclick = close;
  head.appendChild(back);
  const titleBtn = h(`<button class="ap-title" data-k="title" title="${esc(a.title || a.identifier)}" ${panel.map.size > 1 ? 'aria-haspopup="menu"' : ''}>${icon(iconFor(a.type), 16)}<span class="ellipsis">${esc(a.title || a.identifier)}</span>${panel.map.size > 1 ? icon('chevDown', 14) : ''}</button>`);
  titleBtn.onclick = () => {
    if (panel.map.size < 2) return;
    showMenu(titleBtn, [...panel.map.keys()].map((id) => {
      const last = versionsOf(id).at(-1);
      return { label: last.title || id, sub: artifactKindText(last.label), icon: iconFor(last.type), checked: id === panel.identifier, onClick: () => { panel.identifier = id; panel.version = null; panel.followLatest = true; panel.view = null; render(); } };
    }), { width: 280 });
  };
  head.appendChild(titleBtn);
  if (list.length > 1) {
    const vb = h(`<button class="ap-version" data-k="version" aria-haspopup="menu" title="${esc(t('version'))}">v${a.version} ${icon('chevDown', 12)}</button>`);
    vb.onclick = () => showMenu(vb, list.slice().reverse().map((v) => ({
      label: `${t('version')} ${v.version}${v === list[list.length - 1] ? ` · ${t('latest')}` : ''}${!v.complete && v.interrupted ? ` · ${t('art_interrupted')}` : ''}`,
      checked: v.version === a.version,
      onClick: () => { panel.version = v.version; panel.followLatest = v === list[list.length - 1]; render(); },
    })), { width: 200 });
    head.appendChild(vb);
  }
  head.appendChild(h('<div class="grow"></div>'));
  if (previewable && !live) {
    const seg = h(`<div class="segmented" role="group">${['preview', 'code'].map((v) => `<button data-v="${v}" data-k="${v}" class="${view === v ? 'on' : ''}" aria-pressed="${view === v}">${t(v === 'code' ? 'code_view' : 'preview')}</button>`).join('')}</div>`);
    seg.querySelectorAll('button').forEach((b) => b.onclick = () => { panel.view = b.dataset.v; render(); });
    head.appendChild(seg);
  }
  if (view === 'preview' && a.type !== 'text/markdown') {
    const reload = h(`<button class="icon-btn" data-k="reload" title="${esc(t('art_reload'))}" aria-label="${esc(t('art_reload'))}">${icon('refreshCw', 16)}</button>`);
    reload.onclick = () => { panel.lastDocKey = null; render(); };
    head.appendChild(reload);
  }
  const scope = `${panel.convId}/${a.identifier}`;
  if (grantsOf(scope).length) {
    const tools = h(`<button class="icon-btn" data-k="tools" aria-haspopup="dialog" title="${esc(t('art_tools_title'))}" aria-label="${esc(t('art_tools_title'))}">${icon('plug', 16)}</button>`);
    tools.onclick = () => openGrants(scope);
    head.appendChild(tools);
  }
  const fullLabel = esc(t(full ? 'art_full_exit' : 'art_full'));
  const fullB = h(`<button class="icon-btn ap-full${full ? ' on' : ''}" data-k="full" aria-pressed="${full}" title="${fullLabel}" aria-label="${fullLabel}">${full ? MINIMIZE : icon('maximize', 16)}</button>`);
  fullB.onclick = () => setFull(!full);
  head.appendChild(fullB);
  const copyB = h(`<button class="icon-btn" data-k="copy" title="${t('copy')}" aria-label="${t('copy')}">${icon('copy', 16)}</button>`);
  copyB.onclick = async () => { await copyText(a.content); copyB.innerHTML = icon('check', 16); setTimeout(() => (copyB.innerHTML = icon('copy', 16)), 1400); };
  const formats = a.complete ? exportFormats(a.type).filter(([f]) => f !== 'print' || view === 'preview') : [];
  const dl = h(`<button class="icon-btn" data-k="download" title="${t('download')}" aria-label="${t('download')}"${formats.length > 1 ? ' aria-haspopup="menu"' : ''}>${icon('download', 16)}</button>`);
  const save = (format) => {
    if (format === 'print') { const f = aside().querySelector('.ap-body iframe'); if (f && f.contentWindow) f.contentWindow.postMessage({ __artifact: 'print' }, '*'); return; }
    call('export_artifact', { kind: a.type, content: a.content, title: a.title || a.identifier || 'artifact', format }).catch(errorToast);
  };
  dl.onclick = () => {
    if (formats.length > 1) { showMenu(dl, formats.map(([f, key]) => ({ label: t(key), icon: f === 'print' ? 'printer' : 'download', onClick: () => save(f) })), { width: 240 }); return; }
    if (formats.length) { save(formats[0][0]); return; }
    const base = (a.title || a.identifier || 'artifact').replace(/[\\/:*?"<>|]+/g, '-');
    call('save_text_file', { name: `${base}.${a.ext || 'txt'}`, content: a.content }).catch(errorToast);
  };
  const cl = h(`<button class="icon-btn ap-close" data-k="close" title="${t('close')}" aria-label="${t('close')}">${icon('x', 18)}</button>`);
  cl.onclick = close;
  head.append(copyB, dl, cl);
  if (focusKey) { const f = head.querySelector(`[data-k="${focusKey}"]`); if (f) f.focus(); }
  drawAlert(a);

  const body = el.querySelector('.ap-body');
  const docKey = `${view}|${a.type}|${a.identifier}|${a.version}|${a.complete}|${isDark()}|${a.content.length}|${a.content.slice(-64)}`;
  if (panel.lastDocKey === docKey) return;
  // while it streams there's a newer version every 120 ms, and each request
  // highlights all of it: one request at a time, and the latest version is
  // drawn when the one under way is back (newer requests used to make the
  // older ones stale, so a slow highlighter showed nothing until the end)
  // (only for the same artifact: one switched to is drawn right away)
  const myId = a.identifier;
  if (live && fetching && fetching.panel === panel && fetching.id === myId) { panel.again = true; return; }
  panel.lastDocKey = docKey;
  const seq = ++renderSeq;
  // stale: a newer render started, the panel closed or shows another artifact
  const stale = () => seq !== renderSeq || !panel || (currentArtifact() || {}).identifier !== myId;
  const mine = { panel, id: myId };
  fetching = mine;
  try {
    body.classList.remove('loading');
    if (view === 'preview' && a.type === 'text/markdown') {
      const html = await call('render_markdown', { text: a.content });
      if (stale()) return;
      body.innerHTML = `<div class="md">${html}</div>`;
      decorate(body);
    } else if (view === 'preview') {
      const token = await call('serve_artifact', { kind: a.type, content: a.content, dark: isDark() });
      if (stale()) return;
      body.innerHTML = '';
      const frame = document.createElement('iframe');
      // no modals and no popups: links ask the app (openLink), alert and confirm do nothing (artifact_guard.js)
      frame.setAttribute('sandbox', 'allow-scripts allow-forms allow-downloads');
      // a deck or doc prints itself (PDF): its page is the app's runtime, and what it draws runs nothing (artifact_types/)
      if (a.type === SLIDES || a.type === DOC) frame.sandbox.add('allow-modals');
      frame.setAttribute('referrerpolicy', 'no-referrer');
      // whose window.storage and window.claude this frame's calls use (not the panel's: it may show another artifact by then)
      frame.dataset.scope = `${panel.convId}/${myId}`;
      frame.dataset.conv = panel.convId;
      frame.dataset.artifact = myId;
      frame.dataset.version = a.version;
      frame.dataset.title = a.title || myId;
      frame.dataset.kind = a.type;
      // a new page reports its errors anew
      panel.errors.delete(`${myId}@${a.version}`);
      // a typed artifact's runtime speaks the app's language
      frame.src = artifactUrl(token) + (TYPED.has(a.type) ? `#lang=${currentLanguage()}` : '');
      if (a.type !== 'text/html') frame.classList.add('dark-ok');
      frame.title = a.title || myId;
      // a page whose libraries come from a CDN can take a while: a spinner until it's loaded, not a blank panel
      body.classList.add('loading');
      frame.addEventListener('load', () => body.classList.remove('loading'), { once: true });
      body.appendChild(frame);
    } else {
      const code = await call('highlight_code', { code: a.content, language: langFor(a) });
      if (stale()) return;
      const atBottom = body.scrollHeight - body.scrollTop - body.clientHeight < 60;
      body.innerHTML = `<div class="code-block"><pre><code class="hl-code"${wrap ? ' style="white-space:pre-wrap;overflow-wrap:anywhere"' : ''}>${code}</code></pre></div>`;
      if (live && atBottom) body.scrollTop = body.scrollHeight;
    }
  } catch (e) {
    if (!stale()) body.innerHTML = `<div class="ap-empty">${esc(failureText(e))}</div>`;
    panel && (panel.lastDocKey = null);
  } finally {
    if (fetching === mine) fetching = null;
    if (mine.panel.again && panel === mine.panel) { mine.panel.again = false; render(); }
  }
}

// `${conversation}/${identifier}@${version}|${error}` already sent to Claude to fix
const fixesSent = new Set();

/** Above the artifact: an update that couldn't be applied, or the error its
 *  page threw, with "Try fixing" (a message to Claude, sent only on a click
 *  here: the frame can't send one itself). */
function drawAlert(a) {
  const slot = aside().querySelector('.ap-alert');
  if (!slot) return;
  // a version cut off: says so, rather than the errors that follow from what it lacks
  if (!a.complete && a.interrupted) {
    const cut = `cut|${a.identifier}@${a.version}`;
    if (slot.dataset.key === cut) return;
    slot.dataset.key = cut;
    slot.innerHTML = `<div class="notice warn" role="status" style="margin:8px 10px 0">${icon('alert', 15)}<div class="grow small">${esc(t('art_interrupted_note'))}</div></div>`;
    return;
  }
  const err = a.error ? { kind: 'update', message: a.error } : panel.errors.get(`${a.identifier}@${a.version}`);
  const key = err ? `${panel.convId}/${a.identifier}@${a.version}|${err.message}` : '';
  const shown = err && !fixesSent.has(key) ? key : '';
  if ((slot.dataset.key || '') === shown) return;
  slot.dataset.key = shown;
  slot.innerHTML = '';
  if (!shown) return;
  const what = t(err.kind === 'update' ? 'art_update_failed' : 'art_error');
  const note = h(`<div class="notice error" role="alert" style="margin:8px 10px 0">${icon('alert', 15)}<div class="grow small"><div>${esc(what)}</div><div class="err-raw"><div class="${err.blocked ? '' : 'mono'}">${esc(errorText(err, t).slice(0, 300))}</div></div><div class="actions"></div></div></div>`);
  const convId = panel.convId;
  note.querySelector('.actions').appendChild(button(t('art_try_fix'), 'sm', () => {
    fixesSent.add(key);
    const m = fixMessage(a, err);
    emit('send-message', { conversationId: convId, text: t(m.key, m.vars) });
    if (panel) drawAlert(currentArtifact() || a);
  }));
  slot.appendChild(note);
}

/** The person edited typed artifact `frame` in place: `content` is saved as its
 *  newest version (artifacts.rs artifact_edit), which the panel then shows. */
async function saveEdit(frame, content) {
  if (!TYPED.has(frame.dataset.kind) || typeof content !== 'string' || content.length > 2e6) return;
  const convId = frame.dataset.conv;
  try {
    await call('artifact_edit', { conversationId: convId, identifier: frame.dataset.artifact, content });
    if (panel && panel.convId === convId && panel.identifier === frame.dataset.artifact) Object.assign(panel, { followLatest: true, version: null });
    await reloadChat();
  } catch (e) { errorToast(e); }
}

function frameError(frame, d) {
  const key = `${frame.dataset.artifact}@${frame.dataset.version}`;
  // the first is usually the cause; a page that throws on every frame would redraw this all the time
  if (panel.errors.has(key)) return;
  const blocked = d.blocked && typeof d.blocked === 'object' ? { uri: String(d.blocked.uri || '').slice(0, 300), directive: String(d.blocked.directive || '').slice(0, 40) } : null;
  panel.errors.set(key, { kind: 'runtime', message: String(d.message || 'unknown').slice(0, 2000), stack: String(d.stack || '').slice(0, 2000), blocked });
  const a = currentArtifact();
  if (a && `${a.identifier}@${a.version}` === key) drawAlert(a);
}

function askConsent(scope, { title, engine }) {
  return new Promise((resolve) => {
    let done = false;
    const finish = (v) => { if (!done) { done = true; resolve(v); m.close(); } };
    const footer = [
      button(t('deny'), '', () => finish('decline')),
      button(t('allow_once'), '', () => finish('once')),
      button(t('art_ai_allow_artifact'), 'primary', () => finish('always')),
    ];
    const m = modal({
      title: t('art_ai_title'),
      body: `<p style="margin:4px 0 6px;font-size:14.5px;line-height:1.5">${esc(t(engine === 'account' ? 'art_ai_body_account' : 'art_ai_body', { title }))}</p>`,
      footer,
      onClose: () => { if (!done) { done = true; resolve(null); } },
    });
  });
}

/** window.claude.complete of a frame (its dataset): the prompt is all it chooses (artifact_ai.rs). */
const aiConsent = createConsent(askConsent);
const complete = createComplete({
  check: (f) => call('artifact_complete_check', { conversationId: f.conv }),
  allowed: aiConsent,
  run: (f, prompt) => call('artifact_complete', { conversationId: f.conv, artifactId: f.artifact, prompt }),
});

// tool dialogs, from every artifact, one at a time
let toolAsking = Promise.resolve();

/** May the artifact run tool `c` ({ server, tool, read_only, title }) with `input`?
 *  'once', 'always' (only offered with `always`), 'decline', or null when dismissed. */
function askTool(c, input, always) {
  const ask = () => new Promise((resolve) => {
    let done = false;
    const finish = (v) => { if (!done) { done = true; resolve(v); m.close(); } };
    const footer = [button(t('deny'), '', () => finish('decline')), button(t('allow_once'), always ? '' : 'primary', () => finish('once'))];
    if (always) footer.push(button(t('art_ai_allow_artifact'), 'primary', () => finish('always')));
    const m = modal({
      title: t(always ? 'art_tool_title' : 'art_call_title', { server: c.server }),
      body: `<p style="margin:4px 0 8px;font-size:14.5px;line-height:1.5">${esc(t('art_tool_body', { title: c.title, tool: c.tool, server: c.server }))}</p>
        <pre class="mono small" style="max-height:220px;overflow:auto;margin:0 0 8px">${esc(JSON.stringify(input, null, 2).slice(0, 2000))}</pre>
        <p class="small muted" style="margin:0">${esc(t(c.read_only ? 'art_tool_reads' : 'art_tool_changes'))}</p>`,
      footer,
      onClose: () => { if (!done) { done = true; resolve(null); } },
    });
  });
  const next = toolAsking.then(ask, ask);
  toolAsking = next.catch(() => {});
  return next;
}

// the connectors each artifact asked for: scope -> Map(server -> Map(tool -> read only))
const toolsSeen = new Map();
const toolGrants = createGrants((scope, c) => askTool(c, c.input, true));

/** window.claude.callMcpTool of a frame ({ ...its dataset, frame }): the tool and its
 *  arguments are all it chooses; it runs under the tool's policy (mcp/artifact_tools.rs). */
const toolCall = createToolCall({
  check: async (f, name) => {
    const c = await call('artifact_tool_check', { conversationId: f.conv, artifactId: f.artifact, name });
    if (!toolsSeen.has(f.scope)) toolsSeen.set(f.scope, new Map());
    const seen = toolsSeen.get(f.scope);
    if (!seen.has(c.server)) seen.set(c.server, new Map());
    seen.get(c.server).set(c.tool, c.read_only);
    return c;
  },
  grants: toolGrants,
  askCall: async (f, c, input) => (await askTool({ ...c, title: f.title }, input, false)) === 'once',
  run: async (f, name, input, approved) => {
    // the panel closed or moved on while the person was asked
    if (!f.frame.isConnected) throw new Error('The artifact is no longer shown.');
    return call('artifact_tool_call', { conversationId: f.conv, name, arguments: input, approved });
  },
});

/** What artifact `scope` was allowed (or refused) to use: asking Claude, and each connector. */
function grantsOf(scope) {
  const out = [];
  const ai = aiConsent.grants.chosen(scope);
  if (ai) out.push({ name: t('art_grant_claude'), sub: t('art_grant_claude_sub'), state: ai, revoke: () => aiConsent.grants.revoke(scope) });
  for (const [server, tools] of toolsSeen.get(scope) || []) {
    const key = toolScope(scope, server);
    const state = toolGrants.chosen(key);
    const reads = [...tools.values()].every(Boolean);
    if (state) out.push({ name: server, sub: `${[...tools.keys()].join(', ')} · ${t(reads ? 'art_tool_reads' : 'art_tool_changes')}`, state, revoke: () => toolGrants.revoke(key) });
  }
  return out;
}

/** "Tools this artifact can use": each grant of artifact `scope`, and a way to take it back. */
function openGrants(scope) {
  const body = h('<div class="ap-grants"></div>');
  const m = modal({ title: t('art_tools_title'), body });
  const draw = () => {
    const rows = grantsOf(scope);
    body.innerHTML = `<p class="small muted" style="margin:0 0 6px">${esc(t(rows.length ? 'art_grants_note' : 'art_grants_none'))}</p>`;
    for (const g of rows) {
      const row = h(`<div class="ap-grant"><div class="grow"><div class="t">${esc(g.name)}</div><div class="small muted">${esc(g.sub)}</div><div class="small">${esc(t(g.state === 'always' ? 'art_grant_allowed' : 'art_grant_declined'))}</div></div></div>`);
      row.appendChild(button(t(g.state === 'always' ? 'art_grant_revoke' : 'art_grant_reset'), 'sm', () => {
        g.revoke();
        draw();
        if (panel) render();
        const to = body.querySelector('button') || m.el.querySelector('[data-close]');
        if (to) to.focus();
      }));
      body.appendChild(row);
    }
  };
  draw();
}

function bindResizer(r) {
  r.addEventListener('mousedown', (e) => {
    e.preventDefault();
    const el = aside();
    r.classList.add('drag');
    const frames = el.querySelectorAll('iframe');
    frames.forEach((f) => (f.style.pointerEvents = 'none'));
    const move = (ev) => {
      const w = Math.max(360, Math.min(window.innerWidth * 0.75, window.innerWidth - ev.clientX));
      el.style.width = w + 'px';
    };
    const up = () => {
      r.classList.remove('drag');
      frames.forEach((f) => (f.style.pointerEvents = ''));
      document.removeEventListener('mousemove', move);
      document.removeEventListener('mouseup', up);
      saveLocal({ artifactWidth: parseInt(el.style.width, 10) });
    };
    document.addEventListener('mousemove', move);
    document.addEventListener('mouseup', up);
  });
}

// re-render previews on theme change
new MutationObserver(() => { if (panel) { panel.lastDocKey = null; render(); } })
  .observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });

// ------------------------------------------------------------------ gallery

export async function renderGallery(view) {
  view.innerHTML = `<div class="page"><div class="page-inner">
    <div class="page-head"><h1>${t('artifacts')}</h1><div class="grow"></div></div>
    <div class="search-box">${icon('search', 16)}<input class="input" placeholder="${t('search_artifacts')}"></div>
    <div class="grid artifacts"></div></div></div>`;
  searchToggle(view);
  const grid = $('.grid', view);
  let items = [];
  try { items = await call('list_artifacts'); } catch (e) { errorToast(e); }
  const draw = (q) => {
    const f = q ? items.filter((a) => (a.title + ' ' + (a.conversation_title || '')).toLowerCase().includes(q.toLowerCase())) : items;
    if (!f.length) {
      grid.innerHTML = `<div class="empty-state" style="grid-column:1/-1"><div class="big">${t('no_artifacts')}</div></div>`;
      return;
    }
    grid.innerHTML = '';
    if (seen) seen.disconnect();
    // a card's picture is drawn once it comes near the view
    seen = new IntersectionObserver((entries) => entries.forEach((en) => {
      if (!en.isIntersecting) return;
      seen.unobserve(en.target);
      const a = shown.get(en.target);
      renderPreview(en.target, a, { dark: isDark(), lang: currentLanguage(), iconName: iconFor(a.kind) }).catch(() => { en.target.innerHTML = `<pre>${esc(a.content.slice(0, 600))}</pre>`; });
    }), { rootMargin: '200px' });
    for (const a of f) {
      const card = h(`<button class="card artifact-tile"><div class="preview" aria-hidden="true"></div>
        <div class="cap"><div class="row">${icon(iconFor(a.kind), 15)}<div class="t ellipsis grow" title="${esc(a.title || a.identifier)}">${esc(a.title || a.identifier)}</div></div>
        <div class="f ellipsis" title="${esc(a.conversation_title || t('untitled'))}">${esc(artifactKindText(a.label))} · ${esc(a.conversation_title || t('untitled'))} · ${relTime(a.created_at)}</div></div></button>`);
      card.onclick = () => emit('navigate', { name: 'chat', params: { id: a.conversation_id, artifact: a.identifier } });
      const preview = card.querySelector('.preview');
      shown.set(preview, a);
      seen.observe(preview);
      grid.appendChild(card);
    }
  };
  const shown = new WeakMap();
  let seen = null;
  draw('');
  $('.search-box input', view).addEventListener('input', (e) => draw(e.target.value));
}

export function openLatestByIdentifier(convId, map, identifier) {
  if (map.has(identifier)) open(convId, map, identifier, null);
}

export { store };

// one side panel at a time
window.addEventListener('workspace-opened', () => { if (panel) close(); });

// Esc closes the panel from inside it, and the sheet of a narrow window from
// anywhere but a text field (before main.js would stop the turn under it);
// an open menu or dialog closes first
document.addEventListener('keydown', (e) => {
  if (e.key !== 'Escape' || !panel || e.defaultPrevented || document.querySelector('.modal-scrim, .popover')) return;
  if (full) { e.preventDefault(); setFull(false); return; }
  const at = e.target;
  const field = at && (at.isContentEditable || at.tagName === 'TEXTAREA' || (at.tagName === 'INPUT' && at.type !== 'checkbox'));
  if (aside().contains(at) || (sheet.matches && !field)) { e.preventDefault(); close(); }
}, true);

// the sheet of a narrow window and the full-screen view keep Tab inside, as a dialog does (the chat behind isn't reachable)
const sheetControls = () => [...aside().querySelectorAll('button:not(:disabled), a[href], input, select, textarea, iframe, [tabindex]:not([tabindex="-1"])')].filter((x) => x.getClientRects().length);
document.addEventListener('keydown', (e) => {
  if (e.key !== 'Tab' || !panel || !(sheet.matches || full) || !aside().contains(e.target)) return;
  const all = sheetControls();
  const to = wrapTab(all.length, all.indexOf(e.target), e.shiftKey);
  if (to >= 0) { e.preventDefault(); all[to].focus(); }
});
// (Tab out of the preview's frame never reaches the keydown above: focus landing behind the sheet comes back to it)
let focusInSheet = false;
document.addEventListener('focusin', (e) => {
  if (!panel || !(sheet.matches || full)) { focusInSheet = false; return; }
  if (aside().contains(e.target)) { focusInSheet = true; return; }
  // a menu or dialog the sheet opened is above it
  if (e.target.closest && e.target.closest('.popover, .modal-scrim, .toast-root')) return;
  if (focusInSheet) { const first = sheetControls()[0]; if (first) first.focus(); }
});

// what the previewed artifact asks of the app (only from our own frame):
// window.storage and window.claude calls, its errors, links and Esc
window.addEventListener('message', (e) => {
  const d = e.data;
  if (!d || !['call', 'error', 'link', 'escape', 'edit', 'present'].includes(d.__artifact)) return;
  const frame = [...document.querySelectorAll('#artifact-panel iframe')].find((f) => f.contentWindow === e.source);
  if (!frame || !panel) return;
  // the banner above the frame shows it: the page draws no overlay of its own (artifact_hook.js)
  if (d.__artifact === 'error') { e.source.postMessage({ __artifact: 'error-ack' }, '*'); frameError(frame, d); return; }
  if (d.__artifact === 'link') { openLink(d.url); return; }
  if (d.__artifact === 'escape') { if (full) setFull(false); return; }
  if (d.__artifact === 'present') {
    if (frame.dataset.kind !== SLIDES) return;
    // the deck on its own: the panel's bar and notices step aside until it stops
    aside().querySelectorAll('.ap-head, .ap-alert').forEach((x) => (x.style.display = d.on ? 'none' : ''));
    if (!!d.on !== full) setFull(!!d.on);
    return;
  }
  if (d.__artifact === 'edit') { saveEdit(frame, d.content); return; }
  const reply = (r) => e.source.postMessage({ __artifact: 'reply', id: d.id, ...r }, '*');
  const fail = (err) => reply({ error: String((err && err.message) || err) });
  // the toolbar shows what was allowed (or refused) on the way
  const redraw = () => { if (panel) render(); };
  if (d.op === 'complete') { complete(frame.dataset, d.args).then((result) => reply({ result }), fail).finally(redraw); return; }
  // as Claude Desktop's artifacts get it: a refusal or failure is a result with isError, never a rejection
  if (d.op === 'callMcpTool') { toolCall({ ...frame.dataset, frame }, d.args).then((result) => reply({ result }), (err) => reply({ result: toolError(err) })).finally(redraw); return; }
  try { reply({ result: storageCall(localStorage, frame.dataset.scope, d.op, d.args) }); } catch (err) { fail(err); }
});
