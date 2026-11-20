// Built-in slash commands (more are registered by the features that own
// them: /compact, /init, /review, /doctor, /memory, /rewind …).
import { t } from './i18n.js';
import { call } from './api.js';
import { emit, modelInfo, store } from './store.js';
import { modal, fmtTokens, errorToast, toast, fmtUsd } from './util.js';
import { registerCommand } from './slash.js';
import * as workspace from './workspace.js';

registerCommand({ name: 'model', description: () => t('cmd_model'), run: (_a, ctx) => { if (ctx.composer) ctx.composer.openModelMenu(); } });
registerCommand({ name: 'permissions', description: () => t('cmd_permissions'), kinds: ['code'], run: (_a, ctx) => { if (ctx.composer) ctx.composer.openModeMenu(); } });
registerCommand({
  name: 'clear',
  description: () => t('cmd_clear'),
  kinds: ['code'],
  run: async (_a, ctx) => {
    if (!ctx.conversationId || !ctx.cwd) return;
    const c = ctx.conv || {};
    const conv = await call('create_conversation', { req: { kind: 'code', model: c.model || null, project_id: c.project_id || null, cwd: ctx.cwd, options: { permission_mode: (c.options || {}).permission_mode, engine: ctx.engine } } });
    emit('conversations-changed');
    emit('navigate', { name: 'chat', params: { id: conv.id } });
  },
});
for (const tab of ['terminal', 'changes', 'tasks']) {
  registerCommand({ name: tab, description: () => t('cmd_open_pane', { pane: t('ws_' + tab) }), kinds: ['code'], run: () => { workspace.show(tab); } });
}
registerCommand({
  name: 'cost',
  description: () => t('cmd_cost'),
  run: async (_a, ctx) => {
    if (!ctx.conversationId) return;
    try {
      const u = await call('usage_stats', { conversationId: ctx.conversationId });
      const m = ctx.conv ? modelInfo(ctx.conv.model).name : '';
      modal({
        title: t('cmd_cost_title'),
        body: `<table class="kv">
          <tr><td>${t('model')}</td><td>${m}</td></tr>
          <tr><td>${t('input_tokens')}</td><td>${fmtTokens(u.input_tokens)}</td></tr>
          <tr><td>${t('output_tokens')}</td><td>${fmtTokens(u.output_tokens)}</td></tr>
          <tr><td>${t('cache_tokens')}</td><td>${fmtTokens(u.cache_read_input_tokens)} / ${fmtTokens(u.cache_creation_input_tokens)}</td></tr>
          <tr><td><b>${t('cost')}</b></td><td><b>$${(u.cost_usd || 0).toFixed(4)}</b></td></tr>
          ${u.context_tokens != null ? `<tr><td>${t('context_used')}</td><td>${fmtTokens(u.context_tokens)} / ${fmtTokens(u.context_window || 0)}</td></tr>` : ''}
        </table>`,
      });
    } catch (e) { errorToast(e); }
  },
});

// /budget [amount | off | default]: spending limit for each agent turn here.
registerCommand({
  name: 'budget',
  description: () => t('cmd_budget'),
  hint: '[USD | off | default]',
  kinds: ['code'],
  run: async (args, ctx) => {
    if (!ctx.conversationId) return;
    const a = (args || '').trim().replace(/^\$/, '').replace(',', '.').toLowerCase();
    const own = ((ctx.conv && ctx.conv.options) || {}).budget_usd;
    const global = Number(store.settings.turn_budget_usd || 0);
    const fmt = (n) => (n > 0 ? fmtUsd(n) : t('budget_none'));
    if (!a) {
      toast(own != null ? t('budget_current', { amount: fmt(own) }) : t('budget_current_default', { amount: fmt(global) }));
      return;
    }
    let value;
    if (a === 'off' || a === '0') value = 0;
    else if (a === 'default' || a === 'reset') value = null;
    else {
      value = Number(a);
      if (!Number.isFinite(value) || value < 0) { errorToast(new Error(t('budget_invalid'))); return; }
    }
    try {
      await call('update_conversation', { req: { id: ctx.conversationId, options: { budget_usd: value } } });
      if (ctx.conv) ctx.conv.options = { ...(ctx.conv.options || {}), budget_usd: value };
      toast(value == null ? t('budget_current_default', { amount: fmt(global) }) : value === 0 ? t('budget_off') : t('budget_set', { amount: fmt(value) }));
    } catch (e) { errorToast(e); }
  },
});
