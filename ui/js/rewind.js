// Rewind a code session to before one of the user's messages: restore the
// files from the checkpoint, rewind the conversation, or both.
import { t } from './i18n.js';
import { icon } from './icons.js';
import { call } from './api.js';
import { h, esc, modal, toast, errorToast } from './util.js';
import { registerCommand } from './slash.js';

/**
 * Open the rewind dialog for a user message.
 * ctx: { conversationId, messageId, preview (text), onConversationRewound(text) }
 */
export async function openRewind(ctx) {
  let info = { checkpoint: false, files: [] };
  try { info = await call('rewind_info', { conversationId: ctx.conversationId, messageId: ctx.messageId }); } catch (e) { errorToast(e); return; }
  const files = info.files || [];
  const body = h(`<div class="rewind">
    <div class="rw-msg">${icon('history', 14)} <span>${esc((ctx.preview || '').slice(0, 200))}</span></div>
    ${info.checkpoint
      ? (files.length
        ? `<div class="muted small">${esc(t('rewind_files', { n: files.length }))}</div><ul class="rw-files">${files.slice(0, 30).map((f) => `<li><code>${esc(f)}</code></li>`).join('')}${files.length > 30 ? `<li class="muted">… +${files.length - 30}</li>` : ''}</ul>`
        : `<div class="muted small">${esc(t('rewind_no_file_changes'))}</div>`)
      : `<div class="muted small">${esc(t('rewind_no_checkpoint'))}</div>`}
    <div class="rw-options"></div>
  </div>`);
  const opts = body.querySelector('.rw-options');
  const m = modal({ title: t('rewind_title'), body });
  const choose = async (code, conversation) => {
    try {
      const r = await call('rewind', { conversationId: ctx.conversationId, messageId: ctx.messageId, code, conversation });
      m.close();
      if (conversation && ctx.onConversationRewound) ctx.onConversationRewound(r.text);
      if (code && r.undo) {
        toast(t('rewind_done_code'), '', { action: t('undo'), onAction: () => call('rewind_undo', { conversationId: ctx.conversationId, undo: r.undo }).then(() => toast(t('rewind_undone'))).catch(errorToast) });
      } else if (conversation) toast(t('rewind_done_conv'));
    } catch (e) { errorToast(e); }
  };
  const canCode = info.checkpoint && files.length > 0;
  const options = [
    { label: t('rewind_both'), sub: t('rewind_both_d'), code: true, conv: true, enabled: canCode },
    { label: t('rewind_conv'), sub: t('rewind_conv_d'), code: false, conv: true, enabled: true },
    { label: t('rewind_code'), sub: t('rewind_code_d'), code: true, conv: false, enabled: canCode },
  ];
  for (const o of options) {
    const b = h(`<button class="rw-opt" ${o.enabled ? '' : 'disabled'}><b>${esc(o.label)}</b><span>${esc(o.sub)}</span></button>`);
    b.onclick = () => choose(o.code, o.conv);
    opts.appendChild(b);
  }
}

registerCommand({
  name: 'rewind',
  description: () => t('cmd_rewind'),
  kinds: ['code'],
  run: async (_a, ctx) => {
    const chat = await import('./chat.js');
    chat.openRewindPicker(ctx);
  },
});
