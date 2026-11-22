// Hooks: ask before running a project's hooks, and /hooks to review them.
import { icon } from './icons.js';
import { t } from './i18n.js';
import { call, on } from './api.js';
import { h, esc, modal, button, toast, errorToast } from './util.js';
import { registerCommand } from './slash.js';
import { errorText } from './neterr.js';

const asked = new Set();

function trustDialog({ root, hash, commands }) {
  const body = h(`<div class="hooks-dlg"><p>${esc(t('hooks_untrusted_d', { root }))}</p><ul class="hook-list">${commands.map((c) => `<li><code>${esc(c)}</code></li>`).join('')}</ul><p class="muted small">${esc(t('hooks_untrusted_warn'))}</p></div>`);
  const m = modal({
    title: t('hooks_untrusted_title'),
    body,
    footer: [
      button(t('not_now'), 'ghost', () => m.close()),
      button(t('hooks_trust'), 'primary', async () => {
        try { await call('hooks_trust', { root, hash }); toast(t('hooks_trusted')); m.close(); } catch (e) { errorToast(e); }
      }),
    ],
  });
}

on('hooks-untrusted', (p) => {
  const key = `${p.root}|${p.hash}`;
  if (asked.has(key)) return;
  asked.add(key);
  trustDialog(p);
});

registerCommand({
  name: 'hooks',
  description: () => t('cmd_hooks'),
  kinds: ['code'],
  engines: ['builtin'],
  run: async (_a, ctx) => {
    if (!ctx.conversationId) return;
    let info;
    try { info = await call('hooks_info', { conversationId: ctx.conversationId }); } catch (e) { errorToast(e); return; }
    const rows = info.active.map((x) => `<tr><td>${esc(x.event)}</td><td><code>${esc(x.matcher || '*')}</code></td><td><code>${esc(x.command)}</code></td></tr>`).join('');
    const problems = (info.problems || []).map((p) => `<div class="notice error" style="margin:0 0 10px">${icon('alert', 15)}<div class="grow small">${esc(errorText(p))}</div></div>`).join('');
    const body = h(`<div class="hooks-dlg">
      ${problems}
      ${info.active.length ? `<table class="kv hooks-table"><tr><th>${t('hooks_event')}</th><th>${t('hooks_matcher')}</th><th>${t('hooks_command')}</th></tr>${rows}</table>` : `<p class="muted">${esc(t('hooks_none'))}</p>`}
      ${info.untrusted ? `<p style="margin-top:12px">${esc(t('hooks_untrusted_d', { root: info.untrusted.root }))}</p><button class="btn" data-a="trust">${t('hooks_review')}</button>` : ''}
      <p class="muted small" style="margin-top:12px">${esc(t('hooks_help'))}</p></div>`);
    const m = modal({ title: t('hooks_title'), body, wide: true });
    const tb = body.querySelector('[data-a=trust]');
    if (tb) tb.onclick = () => { m.close(); trustDialog(info.untrusted); };
  },
});
