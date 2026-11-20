// Worktree actions in the Changes pane header: commit on the session's
// branch, or apply the changes to the original folder.
import { t } from '../i18n.js';
import { call } from '../api.js';
import { h, esc, toast, errorToast, promptDialog, confirmDialog } from '../util.js';
import { onChangesHeader } from './changes.js';
import { refresh } from '../workspace.js';

onChangesHeader((slot, ctx) => {
  const w = ctx.conv && ctx.conv.options && ctx.conv.options.worktree;
  if (!w || !w.branch) return;
  const commit = h(`<button class="btn sm ghost" title="${esc(t('wt_commit_d', { branch: w.branch }))}">${t('wt_commit')}</button>`);
  commit.onclick = async () => {
    const msg = await promptDialog(t('wt_commit_msg'), '', { placeholder: 'Describe the change' });
    if (msg === null) return;
    try { toast(await call('worktree_commit', { conversationId: ctx.convId, message: msg })); } catch (e) { errorToast(e); }
    refresh('changes');
  };
  const apply = h(`<button class="btn sm" title="${esc(t('wt_apply_d', { path: w.repo_root }))}">${t('wt_apply', { branch: w.base_branch || 'main' })}</button>`);
  apply.onclick = async () => {
    if (!(await confirmDialog(t('wt_apply_confirm', { path: w.repo_root }), { okLabel: t('wt_apply', { branch: w.base_branch || 'main' }), danger: false }))) return;
    try { toast(await call('worktree_apply', { conversationId: ctx.convId })); } catch (e) { errorToast(e); }
  };
  slot.append(commit, apply);
});
