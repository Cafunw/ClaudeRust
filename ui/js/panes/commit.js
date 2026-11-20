// Commit from the Changes pane, with a message suggested by Claude.
import { t } from '../i18n.js';
import { icon } from '../icons.js';
import { call } from '../api.js';
import { h, esc, modal, button, toast, errorToast } from '../util.js';
import { onChangesHeader } from './changes.js';
import { refresh } from '../workspace.js';

async function openCommit(ctx, data) {
  const body = h(`<div class="commit-dlg">
    <div class="muted small">${esc(t('commit_files', { n: data.files.length, branch: data.branch || '' }))}</div>
    <textarea class="textarea mono" rows="7" placeholder="${esc(t('commit_msg_ph'))}"></textarea>
    <div class="cm-status muted small"><span class="spinner tiny"></span> ${esc(t('commit_suggesting'))}</div>
    <label class="day"><input type="checkbox" data-k="push"> ${esc(t('commit_push'))}</label>
  </div>`);
  const ta = body.querySelector('textarea');
  const status = body.querySelector('.cm-status');
  const m = modal({
    title: t('commit_title'),
    body,
    footer: [
      button(t('cancel'), 'ghost', () => m.close()),
      button(t('commit_do'), 'primary', async () => {
        try {
          const r = await call('git_commit', { conversationId: ctx.convId, message: ta.value, push: body.querySelector('[data-k=push]').checked });
          toast(r);
          m.close();
          refresh('changes');
        } catch (e) { errorToast(e); }
      }),
    ],
  });
  try {
    const msg = await call('commit_message_suggest', { conversationId: ctx.convId });
    if (!ta.value.trim()) ta.value = msg;
    status.innerHTML = `${icon('sparkle', 12)} ${esc(t('commit_suggested'))}`;
  } catch (e) {
    status.textContent = (e && e.message) || String(e);
  }
  ta.focus();
}

onChangesHeader((slot, ctx, data) => {
  if (!data || !data.repo || !(data.files || []).length) return;
  const b = h(`<button class="btn sm" title="${esc(t('commit_title'))}">${icon('check', 13)} ${t('commit_btn')}</button>`);
  b.onclick = () => openCommit(ctx, data);
  slot.prepend(b);
});
