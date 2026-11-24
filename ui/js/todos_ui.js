// /todos: TODO / FIXME / HACK / XXX / BUG comments in the project, by file.
// Click one to see it in the Files pane; "Fix" hands it to Claude.
import { t } from './i18n.js';
import { icon } from './icons.js';
import { call } from './api.js';
import { h, esc, modal, errorToast } from './util.js';
import { registerCommand } from './slash.js';

const KINDS = ['TODO', 'FIXME', 'HACK', 'XXX', 'BUG'];

async function openTodos(ctx) {
  if (!ctx.conversationId) return;
  const body = h(`<div class="todos-view"><div class="pane-empty"><span class="spinner"></span></div></div>`);
  const m = modal({ title: t('todos_title'), body, wide: true });
  let data;
  try { data = await call('scan_todos', { conversationId: ctx.conversationId }); } catch (e) { m.close(); errorToast(e); return; }
  const items = data.items || [];
  const counts = Object.fromEntries(KINDS.map((k) => [k, items.filter((i) => i.kind === k).length]));
  let kind = '';
  const draw = () => {
    const shown = kind ? items.filter((i) => i.kind === kind) : items;
    body.innerHTML = '';
    const filters = h(`<div class="todos-filters segmented" role="group"><button data-k="" class="${kind ? '' : 'on'}" aria-pressed="${!kind}">${esc(t('todos_all'))} ${items.length}</button>${KINDS.filter((k) => counts[k]).map((k) => `<button data-k="${k}" class="${kind === k ? 'on' : ''}" aria-pressed="${kind === k}">${k} ${counts[k]}</button>`).join('')}</div>`);
    filters.querySelectorAll('button').forEach((b) => { b.onclick = () => { kind = b.dataset.k; draw(); }; });
    body.appendChild(filters);
    if (!shown.length) { body.appendChild(h(`<div class="pane-empty">${icon('check', 22)}<div>${esc(t('todos_none'))}</div></div>`)); return; }
    const list = h('<div class="todos-list"></div>');
    let lastPath = null;
    for (const it of shown) {
      if (it.path !== lastPath) {
        list.appendChild(h(`<div class="todos-file mono">${esc(it.path)}</div>`));
        lastPath = it.path;
      }
      const row = h(`<div class="todos-row"><span class="no mono">${it.line}</span><span class="todo-kind k-${esc(it.kind)}">${esc(it.kind)}</span><span class="txt ellipsis" title="${esc(it.text)}">${esc(it.text || '—')}</span><button class="btn sm ghost" data-a="fix">${icon('wrench', 12)} ${esc(t('todos_fix'))}</button></div>`);
      row.onclick = async (e) => {
        if (e.target.closest('[data-a=fix]')) return;
        m.close();
        const files = await import('./panes/files.js');
        files.showFile(it.path, it.line);
      };
      row.querySelector('[data-a=fix]').onclick = async () => {
        m.close();
        const chat = await import('./chat.js');
        const c = chat.current();
        if (!c || !c.composer) return;
        const target = `${it.path}#L${it.line}`;
        const draft = c.composer.getText().trim();
        const ask = t('todos_fix_p', { kind: it.kind, at: `@${/\s/.test(target) ? `"${target}"` : target}`, text: it.text || '' });
        c.composer.setText(draft ? `${draft}\n\n${ask}` : ask);
        c.composer.send();
      };
      list.appendChild(row);
    }
    body.appendChild(list);
    if (data.truncated) body.appendChild(h(`<div class="muted small">${esc(t('todos_truncated', { n: items.length }))}</div>`));
  };
  draw();
}

registerCommand({ name: 'todos', description: () => t('cmd_todos'), kinds: ['code'], run: (_a, ctx) => openTodos(ctx) });
