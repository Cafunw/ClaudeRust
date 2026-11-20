// Memory for code sessions: "/memory" edits CLAUDE.md / AGENTS.md files;
// a message starting with "# " is offered as a quick note to remember.
import { t } from './i18n.js';
import { call } from './api.js';
import { h, esc, modal, toast, errorToast, showMenu } from './util.js';
import { registerCommand, registerHashHandler } from './slash.js';

const SCOPES = ['project', 'local', 'agents', 'user', 'folder'];

async function openEditor(ctx) {
  if (!ctx.conversationId) return;
  let files;
  try { files = await call('memory_files', { conversationId: ctx.conversationId }); } catch (e) { errorToast(e); return; }
  files.sort((a, b) => SCOPES.indexOf(a.scope) - SCOPES.indexOf(b.scope));
  let sel = files.findIndex((f) => f.exists);
  if (sel < 0) sel = files.findIndex((f) => f.scope === 'project');
  const body = h(`<div class="mem-ed"><div class="mem-list"></div><div class="mem-main"><div class="mem-path muted small"></div><textarea spellcheck="false"></textarea>
    <div class="mem-foot"><span class="muted small">${esc(t('memory_hint'))}</span><div class="grow"></div><button class="btn primary" data-a="save">${t('save')}</button></div></div></div>`);
  const list = body.querySelector('.mem-list');
  const ta = body.querySelector('textarea');
  const pathEl = body.querySelector('.mem-path');
  const draw = () => {
    list.innerHTML = '';
    files.forEach((f, i) => {
      const b = h(`<button class="mem-item ${i === sel ? 'on' : ''}"><b>${esc(t('memory_scope_' + f.scope))}</b><span class="ellipsis">${esc(f.path.split(/[\\/]/).slice(-2).join('/'))}</span>${f.exists ? '' : `<span class="muted small">${t('memory_new')}</span>`}</button>`);
      b.onclick = () => { files[sel].content = ta.value; sel = i; draw(); };
      list.appendChild(b);
    });
    const f = files[sel];
    pathEl.textContent = f.path;
    ta.value = f.content || '';
  };
  draw();
  modal({ title: t('memory_title'), body, wide: true });
  body.querySelector('[data-a=save]').onclick = async () => {
    const f = files[sel];
    try {
      await call('memory_save', { conversationId: ctx.conversationId, path: f.path, content: ta.value });
      f.content = ta.value; f.exists = true;
      toast(t('memory_saved'));
      draw();
    } catch (e) { errorToast(e); }
  };
}

registerCommand({ name: 'memory', description: () => t('cmd_memory'), kinds: ['code'], run: (_a, ctx) => openEditor(ctx) });

// "# note" → offer to save it to a memory file instead of sending
registerHashHandler((text, ctx) => new Promise((resolve) => {
  if (!ctx.conversationId || !ctx.composer) { resolve(false); return; }
  const note = text.replace(/^#\s+/, '').trim();
  let picked = false;
  const save = async (scope) => {
    picked = true;
    try {
      const p = await call('memory_append', { conversationId: ctx.conversationId, scope, note });
      toast(t('memory_added', { file: p.split(/[\\/]/).slice(-2).join('/') }));
      resolve(true);
    } catch (e) { errorToast(e); resolve(false); }
  };
  showMenu(ctx.composer.el, [
    { header: t('memory_quick_header') },
    { label: t('memory_scope_project'), sub: 'CLAUDE.md', icon: 'folder', onClick: () => save('project') },
    { label: t('memory_scope_local'), sub: 'CLAUDE.local.md', icon: 'key', onClick: () => save('local') },
    { label: t('memory_scope_user'), sub: '~/.claude/CLAUDE.md', icon: 'user', onClick: () => save('user') },
    { sep: true },
    { label: t('memory_send_instead'), icon: 'arrowUp', onClick: () => { picked = true; resolve(false); } },
  ], { placement: 'top-start', width: 300, onClose: () => setTimeout(() => { if (!picked) resolve(null); }, 0) });
}));
