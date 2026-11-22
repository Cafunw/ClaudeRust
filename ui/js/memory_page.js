// Settings → Memory: chat memory (memory_cmds.rs), off until turned on;
// one memory for the chats outside projects and, if wanted, one per
// project, to read, edit, update now, reset, add pasted notes to, or delete.
import { t } from './i18n.js';
import { icon } from './icons.js';
import { call } from './api.js';
import { store, emit } from './store.js';
import { esc, h, button, toast, errorToast, confirmDialog, promptDialog, relTime } from './util.js';
import { notRegistered } from './optional_cmd.js';
import { errorText } from './neterr.js';

/** The memory of `scope` ('global' or a project id) in a chat_memory_list reply. */
export function memoryOf(list, scope) {
  return ((list && list.memories) || []).find((m) => (m.scope ?? m.project_id ?? 'global') === scope) || { scope, text: '', updated_at: 0 };
}

export async function renderMemory(parent) {
  // drawn into its own box: one that arrives after another tab was opened lands nowhere
  const body = parent.appendChild(h('<div></div>'));
  body.appendChild(h(`<h3>${esc(t('mem_title'))}</h3>`));
  body.appendChild(h(`<div class="notice">${icon('shield', 15)}<div class="grow small">${esc(t('mem_privacy'))}</div></div>`));
  let list;
  try { list = await call('chat_memory_list'); } catch (e) {
    body.appendChild(h(`<div class="empty-state" style="padding:30px">${esc(notRegistered(e) ? t('mem_unavailable') : errorText(e))}</div>`));
    return;
  }
  const switchRow = (key, title, desc, on) => {
    const r = h(`<div class="setting-row"><div class="txt"><div class="t">${esc(title)}</div><div class="d">${esc(desc)}</div></div><label class="switch"><input type="checkbox" aria-label="${esc(title)}" ${on ? 'checked' : ''}><span class="track"></span></label></div>`);
    body.appendChild(r);
    const input = r.querySelector('input');
    input.addEventListener('change', () => emit('save-settings', { [key]: input.checked }));
    return input;
  };
  const enabled = switchRow('memory_enabled', t('mem_enable'), t('mem_enable_d'), !!store.settings.memory_enabled);
  const perProject = switchRow('memory_project_scope', t('mem_per_project'), t('mem_per_project_d'), store.settings.memory_project_scope !== false);

  const sel = h(`<select class="select" style="width:auto;min-width:min(200px,100%);max-width:100%" aria-label="${esc(t('mem_scope'))}"></select>`);
  const fillScopes = () => {
    const scopes = [['global', t('mem_scope_global')], ...(perProject.checked ? store.projects || [] : []).map((p) => [p.id, t('mem_scope_project', { name: p.name })])];
    const cur = sel.value;
    sel.innerHTML = scopes.map(([v, l]) => `<option value="${esc(v)}">${esc(l)}</option>`).join('');
    sel.value = scopes.some(([v]) => v === cur) ? cur : 'global';
  };
  const when = h('<span class="small muted"></span>');
  const excl = h(`<label class="row small" style="gap:6px"><input type="checkbox"> ${esc(t('mem_exclude'))}</label>`);
  const exclIn = excl.querySelector('input');
  exclIn.addEventListener('change', () => {
    const set = new Set(store.settings.memory_excluded_projects || []);
    if (exclIn.checked) set.add(sel.value); else set.delete(sel.value);
    emit('save-settings', { memory_excluded_projects: [...set] });
  });
  const head = h('<div class="row" style="gap:12px;margin:16px 0 8px;flex-wrap:wrap"></div>');
  head.append(sel, when, excl);
  const ta = h(`<textarea class="textarea" rows="12" spellcheck="false" aria-label="${esc(t('mem_title'))}" placeholder="${esc(t('mem_empty'))}"></textarea>`);
  if (list.max_chars) ta.maxLength = list.max_chars;
  const btns = h('<div class="row" style="gap:8px;margin-top:10px;flex-wrap:wrap;align-items:center"></div>');
  const runs = h('<span class="small muted"></span>');
  body.append(head, ta, btns);

  const show = () => {
    const m = memoryOf(list, sel.value);
    ta.value = m.text || '';
    when.textContent = m.updated_at ? t('mem_updated', { when: relTime(m.updated_at) }) : t('mem_never');
    excl.classList.toggle('hidden', sel.value === 'global');
    exclIn.checked = (store.settings.memory_excluded_projects || []).includes(sel.value);
    runs.textContent = list.daily_runs ? t('mem_runs', { n: list.runs_today || 0, max: list.daily_runs }) : '';
    upd.disabled = !enabled.checked;
  };
  const refresh = async () => { list = await call('chat_memory_list'); show(); };
  const saveB = button(t('save'), 'primary', async () => {
    try { await call('chat_memory_set', { scope: sel.value, text: ta.value }); await refresh(); toast(t('mem_saved')); } catch (e) { errorToast(e); }
  });
  const upd = button(t('mem_update_now'), '', async () => {
    upd.disabled = true;
    upd.textContent = t('mem_updating');
    try { await call('chat_memory_update_now', { scope: sel.value }); await refresh(); toast(t('mem_updated_now')); } catch (e) { errorToast(e); } finally { upd.textContent = t('mem_update_now'); show(); }
  });
  const imp = button(t('mem_import'), '', async () => {
    const text = await promptDialog(t('mem_import_title'), '', { multiline: true, placeholder: t('mem_import_ph'), okLabel: t('mem_import_go') });
    if (!text || !text.trim()) return;
    try { await call('chat_memory_import', { scope: sel.value, text }); await refresh(); toast(t('mem_saved')); } catch (e) { errorToast(e); }
  });
  const reset = button(t('mem_reset'), 'ghost', async () => {
    if (!(await confirmDialog(t('mem_reset_confirm', { scope: sel.selectedOptions[0].textContent }), { okLabel: t('mem_reset') }))) return;
    try { await call('chat_memory_delete', { scope: sel.value }); await refresh(); } catch (e) { errorToast(e); }
  });
  btns.append(saveB, upd, imp, reset, runs);
  enabled.addEventListener('change', show);
  perProject.addEventListener('change', () => { fillScopes(); show(); });
  sel.addEventListener('change', show);
  fillScopes();
  show();

  body.appendChild(h(`<h4>${esc(t('mem_delete_all'))}</h4>`));
  const del = button(t('mem_delete_all'), 'danger', async () => {
    if (!(await confirmDialog(t('mem_delete_all_confirm'), { okLabel: t('mem_delete_all') }))) return;
    try {
      await call('chat_memory_delete_all');
      await refresh();
      toast(t('mem_deleted'));
    } catch (e) { errorToast(e); }
  });
  const dr = h(`<div class="setting-row"><div class="txt"><div class="d">${esc(t('mem_delete_all_d'))}</div></div></div>`);
  dr.appendChild(del);
  body.appendChild(dr);
}
