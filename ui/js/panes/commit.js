// Commit from the Changes pane, with a message suggested by Claude.
import { t } from '../i18n.js';
import { icon } from '../icons.js';
import { call } from '../api.js';
import { branchFor } from './pr_util.js';
import { h, esc, modal, button, toast, errorToast } from '../util.js';
import { onChangesHeader } from './changes.js';
import { refresh } from '../workspace.js';

async function openCommit(ctx, data) {
  const body = h(`<div class="commit-dlg">
    <div class="muted small">${esc(t('commit_files', { n: data.files.length, branch: data.branch || '' }))}</div>
    <textarea class="textarea mono" rows="7" placeholder="${esc(t('commit_msg_ph'))}"></textarea>
    <div class="cm-status muted small"><span class="spinner tiny"></span> ${esc(t('commit_suggesting'))}</div>
    <label class="day"><input type="checkbox" data-k="push"> ${esc(t('commit_push'))}</label>
    <div class="secret-warn hidden"></div>
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
          const push = body.querySelector('[data-k=push]').checked;
          const r = await call('git_commit', { conversationId: ctx.convId, message: ta.value, push });
          if (push && /Pushed\./.test(r)) toast(r, '', { action: t('pr_create'), onAction: () => openPr(ctx) });
          else toast(r);
          m.close();
          refresh('changes');
        } catch (e) { errorToast(e); }
      }),
    ],
  });
  // secrets about to be committed: list them and ask for confirmation
  call('secret_scan', { conversationId: ctx.convId }).then((found) => {
    if (!found || !found.length) return;
    const warn = body.querySelector('.secret-warn');
    const commitBtn = m.el.querySelector('.modal-foot .btn.primary');
    warn.classList.remove('hidden');
    warn.innerHTML = `<div class="sw-title">${icon('alert', 14)} ${esc(t('secrets_found', { n: found.length }))}</div>
      <ul>${found.slice(0, 8).map((f) => `<li><code>${esc(f.path)}${f.line ? `:${f.line}` : ''}</code> — ${esc(f.kind)}${f.preview ? ` <code>${esc(f.preview)}</code>` : ''}</li>`).join('')}</ul>
      <label class="day"><input type="checkbox" data-k="anyway"> ${esc(t('secrets_anyway'))}</label>`;
    if (commitBtn) commitBtn.disabled = true;
    warn.querySelector('[data-k=anyway]').onchange = (e) => { if (commitBtn) commitBtn.disabled = !e.target.checked; };
  }).catch(() => {});
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

// ------------------------------------------------------------- pull request

async function openPr(ctx) {
  let info;
  try { info = await call('pr_info', { conversationId: ctx.convId }); } catch (e) { errorToast(e); return; }
  const body = h(`<div class="pr-dlg">
    <div class="pr-meta"><code>${esc(info.branch)}</code> → <code>${esc(info.base)}</code> · ${esc(info.ahead === 1 ? t('pr_ahead_one') : t('pr_ahead', { n: info.ahead }))}</div>
    ${info.commits && info.commits.length ? `<details class="pr-commits"><summary class="muted small">${esc(t('pr_commits'))}</summary><pre>${esc(info.commits.join('\n'))}</pre></details>` : ''}
    ${info.existing && info.existing.url ? `<div class="pr-note">${esc(t('pr_exists'))} <a href="#" data-a="open-existing">${esc(info.existing.title || info.existing.url)}</a></div>` : ''}
    ${info.dirty ? `<div class="pr-note warn">${esc(t('pr_dirty'))}</div>` : ''}
    ${!info.ahead ? `<div class="pr-note warn">${esc(t('pr_nothing'))}</div>` : ''}
    ${info.on_base ? `<label class="pr-field"><span>${esc(t('pr_new_branch'))}</span><input class="input mono" data-k="branch" spellcheck="false"></label>` : ''}
    <label class="pr-field"><span>${esc(t('pr_title_label'))}</span><input class="input" data-k="title"></label>
    <label class="pr-field"><span>${esc(t('pr_body_label'))}</span><textarea class="textarea" rows="10" data-k="body"></textarea></label>
    <div class="cm-status muted small"><span class="spinner tiny"></span> ${esc(t('pr_drafting'))}</div>
    <label class="day"><input type="checkbox" data-k="draft"> ${esc(t('pr_draft'))}</label>
    ${info.gh ? '' : `<div class="muted small">${esc(info.compare_url ? t('pr_no_gh') : t('pr_no_gh_host'))}</div>`}
  </div>`);
  const $k = (k) => body.querySelector(`[data-k=${k}]`);
  const status = body.querySelector('.cm-status');
  const ex = body.querySelector('[data-a=open-existing]');
  if (ex) ex.onclick = (e) => { e.preventDefault(); call('open_external', { url: info.existing.url }).catch(errorToast); };
  let branchTouched = false;
  if ($k('branch')) $k('branch').addEventListener('input', () => { branchTouched = true; });
  $k('title').addEventListener('input', () => { if ($k('branch') && !branchTouched) $k('branch').value = branchFor($k('title').value); });
  const canCreate = info.ahead > 0 && !(info.existing && info.existing.url) && (info.gh || info.compare_url);
  const m = modal({
    title: t('pr_title'),
    body,
    footer: [
      button(t('cancel'), 'ghost', () => m.close()),
      button(info.gh ? t('pr_create') : t('pr_open_compare'), 'primary', async (ev) => {
        if (!canCreate) return;
        const btn = ev && ev.currentTarget;
        if (btn) btn.disabled = true;
        try {
          const r = await call('pr_create', {
            conversationId: ctx.convId, title: $k('title').value, body: $k('body').value, base: info.base,
            draft: $k('draft').checked, newBranch: $k('branch') ? $k('branch').value : null,
          });
          m.close();
          refresh('changes');
          if (r.created) toast(t('pr_created'), '', { action: t('open'), onAction: () => call('open_external', { url: r.url }).catch(errorToast) });
          else { call('open_external', { url: r.url }).catch(errorToast); toast(t('pr_compare_opened')); }
        } catch (e) { errorToast(e); if (btn) btn.disabled = false; }
      }),
    ],
  });
  if (!canCreate) {
    const last = m.el ? m.el.querySelector('.modal-foot .btn.primary, .footer .btn.primary') : null;
    if (last) last.disabled = true;
  }
  if (!info.ahead) { status.textContent = ''; return; }
  try {
    const d = await call('pr_suggest', { conversationId: ctx.convId, base: info.base });
    if (!$k('title').value.trim()) $k('title').value = d.title || '';
    if (!$k('body').value.trim()) $k('body').value = d.body || '';
    if ($k('branch') && !branchTouched) $k('branch').value = branchFor($k('title').value);
    status.innerHTML = `${icon('sparkle', 12)} ${esc(t('pr_drafted'))}`;
  } catch (e) {
    status.textContent = (e && e.message) || String(e);
    if ($k('branch') && !$k('branch').value) $k('branch').value = branchFor('');
  }
  $k('title').focus();
}

onChangesHeader((slot, ctx, data) => {
  if (!data || !data.repo) return;
  const b = h(`<button class="btn sm ghost" title="${esc(t('pr_title'))}">${icon('external', 13)} ${t('pr_btn')}</button>`);
  b.onclick = () => openPr(ctx);
  slot.appendChild(b);
});
