// Routines: scheduled prompts that run as new code sessions.
import { t } from './i18n.js';
import { icon } from './icons.js';
import { call, on } from './api.js';
import { store, emit, loadLocal } from './store.js';
import { h, esc, modal, button, toast, errorToast, confirmDialog, relTime } from './util.js';
import { registerCommand } from './slash.js';

let openModal = null;

function scheduleText(s) {
  if (s.kind === 'interval') {
    const m = s.minutes;
    return m % 1440 === 0 ? t('rt_every_days', { n: m / 1440 }) : m % 60 === 0 ? t('rt_every_hours', { n: m / 60 }) : t('rt_every_minutes', { n: m });
  }
  const days = (s.weekdays || []).length && s.weekdays.length < 7 ? s.weekdays.map((d) => t('wd_' + d)).join(', ') : t('rt_every_day');
  return `${days} · ${s.time}`;
}

function fmtWhen(ms) {
  if (!ms) return '—';
  const d = new Date(ms);
  return d.toLocaleString(undefined, { weekday: 'short', hour: '2-digit', minute: '2-digit', month: 'short', day: 'numeric' });
}

async function drawList(body) {
  let list = [];
  try { list = await call('routines_list'); } catch (e) { errorToast(e); }
  body.innerHTML = '';
  const head = h(`<div class="rt-head"><p class="muted">${esc(t('rt_desc'))}</p><button class="btn primary sm" data-a="new">${icon('plus', 14)} ${t('rt_new')}</button></div>`);
  head.querySelector('[data-a=new]').onclick = () => drawEditor(body, null);
  body.appendChild(head);
  if (!list.length) { body.appendChild(h(`<div class="pane-empty">${icon('clock', 22)}<div>${esc(t('rt_empty'))}</div></div>`)); return; }
  for (const r of list) {
    const row = h(`<div class="rt-row ${r.enabled ? '' : 'off'}">
      <label class="switch"><input type="checkbox" ${r.enabled ? 'checked' : ''}><span class="track"></span></label>
      <div class="rt-main"><div class="rt-name">${esc(r.name)}</div>
        <div class="rt-sub muted">${esc(scheduleText(r.schedule))} · <span title="${esc(r.cwd)}">${esc(r.cwd.split(/[\\/]/).filter(Boolean).pop() || r.cwd)}</span></div>
        <div class="rt-sub muted">${t('rt_next')}: ${esc(r.enabled ? fmtWhen(r.next_run_at) : '—')}${r.last_run_at ? ` · ${t('rt_last')}: ${esc(relTime(r.last_run_at))}` : ''}${r.last_error ? ` · <span style="color:var(--danger)">${esc(r.last_error)}</span>` : ''}</div></div>
      ${r.last_conversation_id ? `<button class="btn sm ghost" data-a="open">${t('rt_open_last')}</button>` : ''}
      <button class="icon-btn sm" data-a="run" title="${t('rt_run_now')}">${icon('play', 14)}</button>
      <button class="icon-btn sm" data-a="edit" title="${t('edit')}">${icon('edit', 14)}</button>
      <button class="icon-btn sm" data-a="del" title="${t('delete')}">${icon('trash', 14)}</button>
    </div>`);
    row.querySelector('input').onchange = async (e) => {
      try { await call('routine_save', { routine: { ...r, enabled: e.target.checked } }); } catch (err) { errorToast(err); }
      drawList(body);
    };
    const op = row.querySelector('[data-a=open]');
    if (op) op.onclick = () => { if (openModal) openModal.close(); emit('navigate', { name: 'chat', params: { id: r.last_conversation_id } }); };
    row.querySelector('[data-a=run]').onclick = async () => {
      try {
        const id = await call('routine_run', { id: r.id });
        emit('conversations-changed');
        if (openModal) openModal.close();
        emit('navigate', { name: 'chat', params: { id } });
      } catch (e) { errorToast(e); drawList(body); }
    };
    row.querySelector('[data-a=edit]').onclick = () => drawEditor(body, r);
    row.querySelector('[data-a=del]').onclick = async () => {
      if (!(await confirmDialog(t('rt_delete_confirm', { name: r.name })))) return;
      await call('routine_delete', { id: r.id }).catch(errorToast);
      drawList(body);
    };
    body.appendChild(row);
  }
}

function drawEditor(body, r) {
  const s = r ? r.schedule : { kind: 'daily', time: '09:00', weekdays: [1, 2, 3, 4, 5] };
  const cwd = r ? r.cwd : (loadLocal().codeFolder || '');
  const f = h(`<div class="rt-form">
    <label>${t('rt_name')}<input class="input" data-k="name" value="${esc(r ? r.name : '')}" placeholder="${esc(t('rt_name_ph'))}"></label>
    <label>${t('rt_prompt')}<textarea class="textarea" rows="5" data-k="prompt" placeholder="${esc(t('rt_prompt_ph'))}">${esc(r ? r.prompt : '')}</textarea></label>
    <label>${t('rt_folder')}<div class="row-inline"><input class="input" data-k="cwd" value="${esc(cwd)}"><button class="btn sm" data-a="pick">${t('choose')}</button></div></label>
    <div class="rt-sched">
      <label>${t('rt_schedule')}<select class="select" data-k="kind"><option value="daily">${t('rt_daily')}</option><option value="interval">${t('rt_interval')}</option></select></label>
      <label class="k-daily">${t('rt_time')}<input class="input" type="time" data-k="time" value="${esc(s.time || '09:00')}"></label>
      <label class="k-interval">${t('rt_minutes')}<input class="input" type="number" min="5" step="5" data-k="minutes" value="${s.minutes || 60}"></label>
    </div>
    <div class="k-daily rt-days">${[1, 2, 3, 4, 5, 6, 7].map((d) => `<label class="day"><input type="checkbox" value="${d}" ${(s.weekdays || []).length === 0 || (s.weekdays || []).includes(d) ? 'checked' : ''}> ${t('wd_' + d)}</label>`).join('')}</div>
    <div class="rt-sched">
      <label>${t('permission_mode')}<select class="select" data-k="mode">${['auto', 'acceptEdits', 'default', 'plan', 'bypassPermissions'].map((m) => `<option value="${m}" ${(r ? r.permission_mode : 'auto') === m ? 'selected' : ''}>${esc(t('mode_' + m))}</option>`).join('')}</select></label>
      <label>${t('code_engine')}<select class="select" data-k="engine"><option value="">${t('engine_auto')}</option><option value="builtin" ${r && r.engine === 'builtin' ? 'selected' : ''}>${t('engine_builtin')}</option><option value="claude-code" ${r && r.engine === 'claude-code' ? 'selected' : ''}>${t('engine_cc')}</option></select></label>
    </div>
    <label class="day"><input type="checkbox" data-k="worktree" ${r && r.worktree ? 'checked' : ''}> ${t('rt_worktree')}</label>
    <div class="rt-actions"><button class="btn ghost" data-a="cancel">${t('cancel')}</button><button class="btn primary" data-a="save">${t('save')}</button></div>
  </div>`);
  const q = (k) => f.querySelector(`[data-k=${k}]`);
  q('kind').value = s.kind;
  const sync = () => {
    const daily = q('kind').value === 'daily';
    f.querySelectorAll('.k-daily').forEach((x) => x.classList.toggle('hidden', !daily));
    f.querySelectorAll('.k-interval').forEach((x) => x.classList.toggle('hidden', daily));
  };
  q('kind').onchange = sync;
  sync();
  f.querySelector('[data-a=pick]').onclick = async () => { const d = await call('pick_directory').catch(() => null); if (d) q('cwd').value = d; };
  f.querySelector('[data-a=cancel]').onclick = () => drawList(body);
  f.querySelector('[data-a=save]').onclick = async () => {
    const kind = q('kind').value;
    const schedule = kind === 'daily'
      ? { kind, time: q('time').value || '09:00', weekdays: [...f.querySelectorAll('.rt-days input:checked')].map((x) => +x.value) }
      : { kind, minutes: Math.max(5, parseInt(q('minutes').value, 10) || 60) };
    if (schedule.weekdays && schedule.weekdays.length === 7) schedule.weekdays = [];
    const routine = {
      ...(r || {}),
      id: r ? r.id : '',
      name: q('name').value.trim(),
      prompt: q('prompt').value.trim(),
      cwd: q('cwd').value.trim(),
      schedule,
      permission_mode: q('mode').value,
      engine: q('engine').value,
      worktree: q('worktree').checked,
      enabled: r ? r.enabled : true,
      model: r ? r.model : (store.settings.code_model || null),
    };
    try { await call('routine_save', { routine }); toast(t('rt_saved')); drawList(body); } catch (e) { errorToast(e); }
  };
  body.innerHTML = '';
  body.appendChild(f);
  q('name').focus();
}

export function openRoutines() {
  const body = h('<div class="routines"></div>');
  openModal = modal({ title: t('rt_title'), body, wide: true, onClose: () => { openModal = null; } });
  drawList(body);
  void button;
}

registerCommand({ name: 'schedule', description: () => t('cmd_schedule'), kinds: ['code'], run: () => openRoutines() });
on('routines-changed', () => { if (openModal) drawList(openModal.body.querySelector('.routines') || openModal.body); });
