// Projects list and project detail pages.
import { icon } from './icons.js';
import { t } from './i18n.js';
import { call } from './api.js';
import { store, emit, newChatModel } from './store.js';
import { $, esc, h, modal, button, relTime, errorToast, confirmDialog, promptDialog, humanSize, showMenu, fileToBase64, searchToggle, convTitle } from './util.js';
import { createComposer } from './composer.js';
import { startConversation } from './home.js';
import { conversationMenu } from './chat.js';

const CAPACITY_CHARS = 2_000_000;

export async function refreshProjects() {
  try { store.projects = await call('list_projects'); } catch { store.projects = []; }
  return store.projects;
}

export async function renderProjects(view) {
  view.innerHTML = `<div class="page"><div class="page-inner">
    <div class="page-head"><h1>${t('projects')}</h1><div class="grow"></div><button class="btn primary" data-act="new">${icon('plus', 16)} ${t('new_project')}</button></div>
    <div class="search-box">${icon('search', 16)}<input class="input" placeholder="${t('search_projects')}"></div>
    <div class="grid projects"></div></div></div>`;
  $('[data-act=new]', view).onclick = () => newProjectDialog();
  searchToggle(view);
  await refreshProjects();
  const grid = $('.grid', view);
  const draw = (q) => {
    const list = store.projects.filter((p) => !q || (p.name + ' ' + p.description).toLowerCase().includes(q.toLowerCase()));
    if (!list.length) {
      grid.innerHTML = `<div class="empty-state" style="grid-column:1/-1"><div class="big">${t('no_projects')}</div><div>${t('projects_desc')}</div></div>`;
      return;
    }
    grid.innerHTML = '';
    for (const p of list) {
      const card = h(`<button class="card project-card" title="${esc(p.description || '')}"><span class="tile">${icon('folder', 16)}</span><span class="grow"><span class="row"><span class="t ellipsis">${esc(p.name)}</span>${p.starred ? `<span style="color:var(--accent)">${icon('starFill', 13)}</span>` : ''}</span>
        <span class="f">${t('updated')} ${relTime(p.updated_at)} · ${p.conversation_count} ${t('chats').toLowerCase()}</span></span></button>`);
      card.onclick = () => emit('navigate', { name: 'project', params: { id: p.id } });
      grid.appendChild(card);
    }
  };
  draw('');
  $('.search-box input', view).addEventListener('input', (e) => draw(e.target.value));
}

export function newProjectDialog() {
  const body = h(`<div><div class="field"><label class="label">${t('project_name')}</label><input class="input" data-f="name" placeholder="${t('name')}"></div>
    <div class="field"><label class="label">${t('project_desc')}</label><textarea class="textarea" data-f="desc" rows="3" placeholder="${t('description')}"></textarea></div></div>`);
  const create = button(t('create_project'), 'primary', async () => {
    const name = $('[data-f=name]', body).value.trim();
    if (!name) { $('[data-f=name]', body).focus(); return; }
    try {
      const p = await call('create_project', { name, description: $('[data-f=desc]', body).value });
      m.close();
      await refreshProjects();
      emit('navigate', { name: 'project', params: { id: p.id } });
    } catch (e) { errorToast(e); }
  });
  const m = modal({ title: t('new_project'), body, footer: [button(t('cancel'), '', () => m.close()), create] });
  setTimeout(() => $('[data-f=name]', body).focus(), 30);
}

export async function renderProject(view, id) {
  // a newer navigation (or leaving the page) supersedes this render
  const route = store.route;
  const stale = () => store.route !== route || route.name !== 'project';
  let data;
  try { data = await call('get_project', { id }); } catch (e) { if (!stale()) { errorToast(e); emit('navigate', { name: 'projects' }); } return; }
  if (stale()) return;
  const p = data.project;
  view.innerHTML = `<div class="page"><div class="page-inner" style="max-width:1120px">
    <button class="crumb" data-act="back" style="margin-bottom:10px">${icon('arrowLeft', 15)} ${t('projects')}</button>
    <div class="project-layout">
      <div>
        <div class="page-head" style="margin-bottom:6px"><h1 class="grow ellipsis">${esc(p.name)}</h1>
          <button class="icon-btn" data-act="star" title="${t('star')}">${icon(p.starred ? 'starFill' : 'star', 18)}</button>
          <button class="icon-btn" data-act="more">${icon('more', 18)}</button></div>
        <p class="muted" style="margin:0 0 20px">${esc(p.description || '')}</p>
        <div class="composer-slot"></div>
        <div class="section-title" style="padding-left:2px;margin-top:22px">${t('chats_in_project')}</div>
        <div class="proj-chats"></div>
      </div>
      <aside class="knowledge"></aside>
    </div></div></div>`;
  $('[data-act=back]', view).onclick = () => emit('navigate', { name: 'projects' });
  $('[data-act=star]', view).onclick = async () => { await call('update_project', { req: { id, starred: !p.starred } }); renderProject(view, id); };
  $('[data-act=more]', view).onclick = (e) => showMenu(e.currentTarget, [
    { label: t('rename'), icon: 'edit', onClick: async () => { const v = await promptDialog(t('rename'), p.name); if (v) { await call('update_project', { req: { id, name: v } }); await refreshProjects(); renderProject(view, id); } } },
    { label: t('description'), icon: 'fileText', onClick: async () => { const v = await promptDialog(t('description'), p.description, { multiline: true }); if (v !== null) { await call('update_project', { req: { id, description: v } }); renderProject(view, id); } } },
    { sep: true },
    { label: t('delete'), icon: 'trash', danger: true, onClick: async () => { if (await confirmDialog(t('confirm_delete_project'))) { await call('delete_project', { id }); await refreshProjects(); emit('navigate', { name: 'projects' }); } } },
  ], { placement: 'bottom-end', width: 200 });

  const composer = createComposer({
    mode: 'chat', placeholder: t('start_in_project'), draftKey: 'project-' + id,
    initial: { model: newChatModel('chat') },
    onSend: (text, atts, st) => startConversation({ text, atts, st, projectId: id }),
  });
  $('.composer-slot', view).appendChild(composer.el);

  const chatsEl = $('.proj-chats', view);
  const convs = await call('list_conversations', { kind: 'chat', projectId: id });
  if (!convs.length) chatsEl.innerHTML = `<div class="muted small" style="padding:10px 2px">${t('no_chats')}</div>`;
  for (const c of convs) {
    const row = h(`<button class="list-item"><div class="grow"><div class="t">${esc(convTitle(c))}</div><div class="d">${t('last_message')} ${relTime(c.updated_at)}</div></div><span class="icon-btn sm more">${icon('more', 16)}</span></button>`);
    row.addEventListener('click', (e) => {
      if (e.target.closest('.more')) { e.stopPropagation(); conversationMenu(e.target.closest('.more'), c, { onChange: () => renderProject(view, id), onDelete: () => renderProject(view, id) }); return; }
      emit('navigate', { name: 'chat', params: { id: c.id } });
    });
    chatsEl.appendChild(row);
  }
  renderKnowledge($('.knowledge', view), data, () => renderProject(view, id));
}

function renderKnowledge(el, data, rerender) {
  const p = data.project;
  const pct = Math.min(100, (data.knowledge_chars / CAPACITY_CHARS) * 100);
  el.innerHTML = `<div class="row" style="justify-content:space-between"><h3>${t('project_knowledge')}</h3><button class="btn sm" data-act="add">${icon('plus', 14)} ${t('add_content')}</button></div>
    <div class="k-section"><div class="row" style="justify-content:space-between"><div style="font-weight:500">${t('instructions')}</div><button class="icon-btn sm" data-act="instr">${icon('edit', 14)}</button></div>
      <div class="small ${p.instructions ? '' : 'muted'}" style="margin-top:6px;white-space:pre-wrap;max-height:160px;overflow:auto">${esc(p.instructions || t('instructions_help'))}</div></div>
    <div class="k-section"><div class="capacity"><div style="width:${pct.toFixed(1)}%"></div></div><div class="small muted" style="margin-top:6px">${pct < 1 && pct > 0 ? '<1' : Math.round(pct)}% ${t('of_capacity')}</div></div>
    <div class="k-section files"></div>`;
  const files = $('.files', el);
  if (!data.files.length) files.innerHTML = `<div class="muted small">${t('no_files')}</div>`;
  for (const f of data.files) {
    const row = h(`<div class="k-file">${icon(f.kind === 'image' ? 'image' : 'fileText', 15)}<div class="grow ellipsis">${esc(f.name)}</div><span class="small muted">${humanSize(f.size)}</span><button class="icon-btn sm x">${icon('x', 14)}</button></div>`);
    row.querySelector('.x').onclick = async () => { await call('remove_project_file', { id: f.id }); rerender(); };
    files.appendChild(row);
  }
  $('[data-act=instr]', el).onclick = async () => {
    const v = await promptDialog(t('set_instructions'), p.instructions, { multiline: true, placeholder: t('instructions_help') });
    if (v === null) return;
    await call('update_project', { req: { id: p.id, instructions: v } });
    rerender();
  };
  $('[data-act=add]', el).onclick = (e) => showMenu(e.currentTarget, [
    { label: t('upload_files'), icon: 'paperclip', onClick: async () => {
      const paths = await call('pick_files');
      if (!paths.length) return;
      const r = await call('add_project_paths', { projectId: p.id, paths });
      r.errors.forEach((x) => errorToast(new Error(x)));
      rerender();
    } },
    { label: t('add_text_content'), icon: 'fileText', onClick: () => addTextDialog(p.id, rerender) },
  ], { placement: 'bottom-end', width: 220 });
  // drop files onto knowledge panel (HTML5, when available)
  el.addEventListener('dragover', (e) => e.preventDefault());
  el.addEventListener('drop', async (e) => {
    e.preventDefault();
    for (const f of Array.from(e.dataTransfer?.files || [])) {
      try { await call('add_project_bytes', { projectId: p.id, name: f.name, data: await fileToBase64(f) }); } catch (err) { errorToast(err); }
    }
    rerender();
  });
}

function addTextDialog(projectId, done) {
  const body = h(`<div><div class="field"><label class="label">${t('name')}</label><input class="input" data-f="n" placeholder="notes.md"></div>
    <div class="field"><label class="label">${t('add_text_content')}</label><textarea class="textarea" data-f="c" rows="12"></textarea></div></div>`);
  const save = button(t('save'), 'primary', async () => {
    const name = $('[data-f=n]', body).value.trim() || 'notes.md';
    const text = $('[data-f=c]', body).value;
    if (!text.trim()) return;
    try { await call('add_project_text', { projectId, name, text }); m.close(); done(); } catch (e) { errorToast(e); }
  });
  const m = modal({ title: t('add_text_content'), body, footer: [button(t('cancel'), '', () => m.close()), save] });
}
