// Save a prompt as a reusable /command (Claude Code's .claude/commands format).
import { t } from './i18n.js';
import { call } from './api.js';
import { h, esc, modal, button, toast, errorToast } from './util.js';

export function openSaveCommand({ text, conversationId, cwd }) {
  const suggested = (text.split('\n')[0] || '').toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').split('-').slice(0, 3).join('-') || 'my-command';
  const body = h(`<div class="rt-form">
    <label>${esc(t('sc_name'))}<div class="row-inline"><span class="sc-slash">/</span><input class="input mono" data-k="name" value="${esc(suggested)}"></div></label>
    <label>${esc(t('sc_desc'))}<input class="input" data-k="desc" placeholder="${esc(t('sc_desc_ph'))}"></label>
    <label>${esc(t('rt_prompt'))}<textarea class="textarea" rows="7" data-k="body">${esc(text)}</textarea></label>
    <div class="muted small">${esc(t('sc_args_help'))}</div>
    <div class="rt-sched"><label>${esc(t('sc_scope'))}<select class="select" data-k="scope"><option value="project">${esc(t('sc_scope_project'))}</option><option value="user">${esc(t('sc_scope_user'))}</option></select></label></div>
  </div>`);
  const q = (k) => body.querySelector(`[data-k=${k}]`);
  if (!cwd) q('scope').value = 'user';
  const save = async (overwrite) => {
    try {
      const path = await call('slash_save', { conversationId: conversationId || null, cwd: cwd || null, scope: q('scope').value, name: q('name').value, description: q('desc').value, body: q('body').value, overwrite });
      toast(t('sc_saved', { name: q('name').value.trim(), path: path.split(/[\\/]/).slice(-3).join('/') }));
      m.close();
    } catch (e) {
      if (!overwrite && /already exists/.test(e.message || '')) {
        if (window.confirm(t('sc_overwrite'))) save(true);
      } else errorToast(e);
    }
  };
  const m = modal({ title: t('sc_title'), body, footer: [button(t('cancel'), 'ghost', () => m.close()), button(t('save'), 'primary', () => save(false))] });
  q('name').select();
}
