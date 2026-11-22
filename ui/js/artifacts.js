// Artifact side panel and the Artifacts gallery page.
import { icon, ARTIFACT_ICON } from './icons.js';
import { t } from './i18n.js';
import { call, artifactUrl } from './api.js';
import { store, emit, saveLocal, loadLocal } from './store.js';
import { $, esc, h, showMenu, errorToast, relTime, searchToggle, modal, button } from './util.js';
import { openLink } from './artifact_link.js';
import { copyText, decorate } from './parts.js';
import { storageCall } from './artifact_storage.js';
import { resolveVersions } from './artifact_update.js';
import { createConsent, createComplete, fixMessage } from './artifact_bridge.js';

let panel = null;
let renderTimer = null;
let renderSeq = 0;
let fetching = null; // { panel, id } of the document request under way

function aside() { return $('#artifact-panel'); }

function isDark() { return document.documentElement.dataset.theme === 'dark'; }

function langFor(a) {
  if (a.language) return a.language;
  return { 'application/vnd.ant.react': 'jsx', 'text/html': 'html', 'image/svg+xml': 'xml', 'application/vnd.ant.mermaid': 'text', 'text/markdown': 'markdown' }[a.type] || 'text';
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

export function open(convId, map, identifier, version, { streaming = false } = {}) {
  const followLatest = !version || streaming;
  // errors: `${identifier}@${version}` -> the first error its page reported
  panel = { convId, map, identifier, version, view: null, followLatest, lastDocKey: null, errors: new Map() };
  const el = aside();
  const wasHidden = el.classList.contains('hidden');
  if (el.classList.contains('hidden')) window.dispatchEvent(new Event('artifact-opened'));
  el.classList.remove('hidden');
  if (wasHidden && window.innerWidth < 1500 && !store.sidebarCollapsed) emit('collapse-sidebar');
  const w = loadLocal().artifactWidth;
  if (w) el.style.width = w + 'px';
  render();
  document.querySelectorAll('.artifact-card').forEach((c) => c.classList.toggle('open', c.dataset.identifier === identifier));
}

export function togglePanel(convId, map) {
  if (panel && panel.convId === convId) { close(); return; }
  const ids = [...map.keys()];
  if (!ids.length) return;
  const last = ids[ids.length - 1];
  open(convId, map, last, null);
}

export function close() {
  panel = null;
  const el = aside();
  el.classList.add('hidden');
  el.innerHTML = '';
  document.querySelectorAll('.artifact-card.open').forEach((c) => c.classList.remove('open'));
}

export function isOpen() { return !!panel; }

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
  const a = currentArtifact();
  if (!a) { el.innerHTML = `<div class="ap-empty">${t('no_artifacts')}</div>`; return; }
  const list = versionsOf(panel.identifier);
  const previewable = a.previewable || a.type === 'text/markdown';
  const view = !a.complete ? 'code' : (panel.view || (previewable ? 'preview' : 'code'));

  if (!el.querySelector('.ap-head')) {
    el.innerHTML = `<div class="resizer"></div><div class="ap-head"></div><div class="ap-alert"></div><div class="ap-body"></div>`;
    bindResizer(el.querySelector('.resizer'));
  }
  const head = el.querySelector('.ap-head');
  head.innerHTML = '';
  const titleBtn = h(`<button class="ap-title">${icon(ARTIFACT_ICON[a.type] || 'file', 16)}<span class="ellipsis">${esc(a.title || a.identifier)}</span>${panel.map.size > 1 ? icon('chevDown', 14) : ''}</button>`);
  titleBtn.onclick = () => {
    if (panel.map.size < 2) return;
    showMenu(titleBtn, [...panel.map.keys()].map((id) => {
      const last = versionsOf(id).at(-1);
      return { label: last.title || id, sub: last.label, icon: ARTIFACT_ICON[last.type] || 'file', checked: id === panel.identifier, onClick: () => { panel.identifier = id; panel.version = null; panel.followLatest = true; panel.view = null; render(); } };
    }), { width: 280 });
  };
  head.appendChild(titleBtn);
  if (list.length > 1) {
    const vb = h(`<button class="ap-version">v${a.version}${a.version === list[list.length - 1].version ? '' : ''} ${icon('chevDown', 12)}</button>`);
    vb.onclick = () => showMenu(vb, list.slice().reverse().map((v) => ({
      label: `${t('version')} ${v.version}${v === list[list.length - 1] ? ` · ${t('latest')}` : ''}`,
      checked: v.version === a.version,
      onClick: () => { panel.version = v.version; panel.followLatest = v === list[list.length - 1]; render(); },
    })), { width: 200 });
    head.appendChild(vb);
  }
  head.appendChild(h('<div class="grow"></div>'));
  if (previewable && a.complete) {
    const seg = h(`<div class="segmented"><button data-v="preview" class="${view === 'preview' ? 'on' : ''}">${t('preview')}</button><button data-v="code" class="${view === 'code' ? 'on' : ''}">${t('code_view')}</button></div>`);
    seg.querySelectorAll('button').forEach((b) => b.onclick = () => { panel.view = b.dataset.v; render(); });
    head.appendChild(seg);
  }
  const copyB = h(`<button class="icon-btn" title="${t('copy')}">${icon('copy', 16)}</button>`);
  copyB.onclick = async () => { await copyText(a.content); copyB.innerHTML = icon('check', 16); setTimeout(() => (copyB.innerHTML = icon('copy', 16)), 1400); };
  const dl = h(`<button class="icon-btn" title="${t('download')}">${icon('download', 16)}</button>`);
  dl.onclick = () => {
    const base = (a.title || a.identifier || 'artifact').replace(/[\\/:*?"<>|]+/g, '-');
    call('save_text_file', { name: `${base}.${a.ext || 'txt'}`, content: a.content }).catch(errorToast);
  };
  const cl = h(`<button class="icon-btn" title="${t('close')}">${icon('x', 18)}</button>`);
  cl.onclick = close;
  head.append(copyB, dl, cl);
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
  if (!a.complete && fetching && fetching.panel === panel && fetching.id === myId) { panel.again = true; return; }
  panel.lastDocKey = docKey;
  const seq = ++renderSeq;
  // stale: a newer render started, the panel closed or shows another artifact
  const stale = () => seq !== renderSeq || !panel || (currentArtifact() || {}).identifier !== myId;
  const mine = { panel, id: myId };
  fetching = mine;
  try {
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
      frame.setAttribute('referrerpolicy', 'no-referrer');
      // whose window.storage and window.claude this frame's calls use (not the panel's: it may show another artifact by then)
      frame.dataset.scope = `${panel.convId}/${myId}`;
      frame.dataset.conv = panel.convId;
      frame.dataset.artifact = myId;
      frame.dataset.version = a.version;
      frame.dataset.title = a.title || myId;
      // a new page reports its errors anew
      panel.errors.delete(`${myId}@${a.version}`);
      frame.src = artifactUrl(token);
      if (a.type !== 'text/html') frame.classList.add('dark-ok');
      body.appendChild(frame);
    } else {
      const code = await call('highlight_code', { code: a.content, language: langFor(a) });
      if (stale()) return;
      const atBottom = body.scrollHeight - body.scrollTop - body.clientHeight < 60;
      body.innerHTML = `<div class="code-block"><pre><code class="hl-code">${code}</code></pre></div>`;
      if (!a.complete && atBottom) body.scrollTop = body.scrollHeight;
    }
  } catch (e) {
    if (!stale()) body.innerHTML = `<div class="ap-empty">${esc(e.message)}</div>`;
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
  const err = a.error ? { kind: 'update', message: a.error } : panel.errors.get(`${a.identifier}@${a.version}`);
  const key = err ? `${panel.convId}/${a.identifier}@${a.version}|${err.message}` : '';
  const shown = err && !fixesSent.has(key) ? key : '';
  if ((slot.dataset.key || '') === shown) return;
  slot.dataset.key = shown;
  slot.innerHTML = '';
  if (!shown) return;
  const what = t(err.kind === 'update' ? 'art_update_failed' : 'art_error');
  const note = h(`<div class="notice error" style="margin:8px 10px 0">${icon('alert', 15)}<div class="grow small"><div>${esc(what)}</div><div class="err-raw"><div class="mono">${esc(err.message.slice(0, 300))}</div></div><div class="actions"></div></div></div>`);
  const convId = panel.convId;
  note.querySelector('.actions').appendChild(button(t('art_try_fix'), 'sm', () => {
    fixesSent.add(key);
    const m = fixMessage(a, err);
    emit('send-message', { conversationId: convId, text: t(m.key, m.vars) });
    if (panel) drawAlert(currentArtifact() || a);
  }));
  slot.appendChild(note);
}

function frameError(frame, d) {
  const key = `${frame.dataset.artifact}@${frame.dataset.version}`;
  // the first is usually the cause; a page that throws on every frame would redraw this all the time
  if (panel.errors.has(key)) return;
  panel.errors.set(key, { kind: 'runtime', message: String(d.message || 'unknown').slice(0, 2000), stack: String(d.stack || '').slice(0, 2000) });
  const a = currentArtifact();
  if (a && `${a.identifier}@${a.version}` === key) drawAlert(a);
}

function askConsent(scope, title) {
  return new Promise((resolve) => {
    let done = false;
    const finish = (v) => { if (!done) { done = true; resolve(v); m.close(); } };
    const footer = [
      button(t('elicit_decline'), '', () => finish('decline')),
      button(t('allow_once'), '', () => finish('once')),
      button(t('art_ai_allow_artifact'), 'primary', () => finish('always')),
    ];
    const m = modal({
      title: t('art_ai_title'),
      body: `<p style="margin:4px 0 6px;font-size:14.5px;line-height:1.5">${esc(t('art_ai_body', { title }))}</p>`,
      footer,
      onClose: () => { if (!done) { done = true; resolve(null); } },
    });
  });
}

/** window.claude.complete of a frame (its dataset): the prompt is all it chooses (artifact_ai.rs). */
const complete = createComplete({
  check: (f) => call('artifact_complete_check', { conversationId: f.conv }),
  allowed: createConsent(askConsent),
  run: (f, prompt) => call('artifact_complete', { conversationId: f.conv, artifactId: f.artifact, prompt }),
});

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
    for (const a of f) {
      const card = h(`<button class="card artifact-tile"><div class="preview"><pre>${esc(a.content.slice(0, 600))}</pre></div>
        <div class="cap"><div class="row">${icon(ARTIFACT_ICON[a.kind] || 'file', 15)}<div class="t ellipsis grow">${esc(a.title || a.identifier)}</div></div>
        <div class="f ellipsis">${esc(a.label)} · ${esc(a.conversation_title || t('untitled'))} · ${relTime(a.created_at)}</div></div></button>`);
      card.onclick = () => emit('navigate', { name: 'chat', params: { id: a.conversation_id, artifact: a.identifier } });
      grid.appendChild(card);
    }
  };
  draw('');
  $('.search-box input', view).addEventListener('input', (e) => draw(e.target.value));
}

export function openLatestByIdentifier(convId, map, identifier) {
  if (map.has(identifier)) open(convId, map, identifier, null);
}

export { store };

// one side panel at a time
window.addEventListener('workspace-opened', () => { if (panel) close(); });

// what the previewed artifact asks of the app (only from our own frame):
// window.storage and window.claude.complete calls, its errors and links
window.addEventListener('message', (e) => {
  const d = e.data;
  if (!d || (d.__artifact !== 'call' && d.__artifact !== 'error' && d.__artifact !== 'link')) return;
  const frame = [...document.querySelectorAll('#artifact-panel iframe')].find((f) => f.contentWindow === e.source);
  if (!frame || !panel) return;
  if (d.__artifact === 'error') { frameError(frame, d); return; }
  if (d.__artifact === 'link') { openLink(d.url); return; }
  const reply = (r) => e.source.postMessage({ __artifact: 'reply', id: d.id, ...r }, '*');
  const fail = (err) => reply({ error: String((err && err.message) || err) });
  if (d.op === 'complete') { complete(frame.dataset, d.args).then((result) => reply({ result }), fail); return; }
  try { reply({ result: storageCall(localStorage, frame.dataset.scope, d.op, d.args) }); } catch (err) { fail(err); }
});
