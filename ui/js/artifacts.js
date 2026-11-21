// Artifact side panel and the Artifacts gallery page.
import { icon, ARTIFACT_ICON } from './icons.js';
import { t } from './i18n.js';
import { call, artifactUrl } from './api.js';
import { store, emit, saveLocal, loadLocal } from './store.js';
import { $, esc, h, showMenu, errorToast, relTime } from './util.js';
import { copyText, decorate } from './parts.js';

let panel = null;
let renderTimer = null;
let renderSeq = 0;

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
  panel = { convId, map, identifier, version, view: null, followLatest, lastDocKey: null };
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

function currentArtifact() {
  const list = panel.map.get(panel.identifier) || [];
  if (!list.length) return null;
  if (panel.followLatest || !panel.version) return list[list.length - 1];
  return list.find((v) => v.version === panel.version) || list[list.length - 1];
}

async function render() {
  if (!panel) return;
  const el = aside();
  const a = currentArtifact();
  if (!a) { el.innerHTML = `<div class="ap-empty">${t('no_artifacts')}</div>`; return; }
  const list = panel.map.get(panel.identifier) || [];
  const previewable = a.previewable || a.type === 'text/markdown';
  const view = !a.complete ? 'code' : (panel.view || (previewable ? 'preview' : 'code'));

  if (!el.querySelector('.ap-head')) {
    el.innerHTML = `<div class="resizer"></div><div class="ap-head"></div><div class="ap-body"></div>`;
    bindResizer(el.querySelector('.resizer'));
  }
  const head = el.querySelector('.ap-head');
  head.innerHTML = '';
  const titleBtn = h(`<button class="ap-title">${icon(ARTIFACT_ICON[a.type] || 'file', 16)}<span class="ellipsis">${esc(a.title || a.identifier)}</span>${panel.map.size > 1 ? icon('chevDown', 14) : ''}</button>`);
  titleBtn.onclick = () => {
    if (panel.map.size < 2) return;
    showMenu(titleBtn, [...panel.map.entries()].map(([id, vs]) => {
      const last = vs[vs.length - 1];
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

  const body = el.querySelector('.ap-body');
  const docKey = `${view}|${a.type}|${a.identifier}|${a.version}|${a.complete}|${isDark()}|${a.content.length}|${a.content.slice(-64)}`;
  if (panel.lastDocKey === docKey) return;
  panel.lastDocKey = docKey;
  const seq = ++renderSeq;
  const stale = () => seq !== renderSeq || !panel;
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
      frame.setAttribute('sandbox', 'allow-scripts allow-popups allow-forms allow-modals allow-downloads allow-popups-to-escape-sandbox');
      frame.setAttribute('referrerpolicy', 'no-referrer');
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
  }
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
    <div class="page-head"><h1>${t('artifacts')}</h1></div>
    <p class="muted" style="margin:-10px 0 22px">${t('artifacts_desc')}</p>
    <div class="search-box">${icon('search', 16)}<input class="input" placeholder="${t('search_artifacts')}"></div>
    <div class="grid"></div></div></div>`;
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
      const card = h(`<button class="card"><div class="preview"><pre>${esc(a.content.slice(0, 600))}</pre></div>
        <div class="row">${icon(ARTIFACT_ICON[a.kind] || 'file', 15)}<div class="t ellipsis grow">${esc(a.title || a.identifier)}</div></div>
        <div class="f">${esc(a.label)} · ${esc(a.conversation_title || t('untitled'))} · ${relTime(a.created_at)}</div></button>`);
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
