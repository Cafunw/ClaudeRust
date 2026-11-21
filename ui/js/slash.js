// Slash command registry. Built-in commands register here (with a run()
// handler); custom commands come from .claude/commands via the backend and
// expand into a prompt.
import { t } from './i18n.js';
import { call } from './api.js';
import { esc, modal, toast } from './util.js';

const builtins = [];
let hashHandler = null;

/** Handler for messages starting with "# " (quick memory). Resolves true (handled), false (send it) or null (cancelled). */
export function registerHashHandler(fn) { hashHandler = fn; }
export async function handleHash(text, ctx) { return hashHandler ? hashHandler(text, ctx) : false; }

/**
 * Register a built-in command.
 * { name, description: () => string, hint?, kinds?: ['code','chat'], engines?: ['builtin','claude-code'],
 *   run: async (args, ctx) => undefined | string }  — a returned string is sent as the prompt.
 */
export function registerCommand(c) {
  const i = builtins.findIndex((x) => x.name === c.name);
  if (i >= 0) builtins[i] = c; else builtins.push(c);
}

export function builtinCommands(ctx) {
  return builtins.filter((c) => (!c.kinds || c.kinds.includes(ctx.kind || 'chat')) && (!c.engines || c.engines.includes(ctx.engine || 'builtin')));
}

let customCache = { key: '', at: 0, list: [] };
export async function customCommands(ctx) {
  const key = `${ctx.conversationId || ''}|${ctx.cwd || ''}`;
  if (customCache.key === key && Date.now() - customCache.at < 5000) return customCache.list;
  let list = [];
  try { list = await call('slash_list', { conversationId: ctx.conversationId || null, cwd: ctx.cwd || null }); } catch { list = []; }
  customCache = { key, at: Date.now(), list };
  return list;
}

/** All commands for the menu: [{ name, description, hint, kind: 'builtin'|'custom', source }]. */
export async function allCommands(ctx) {
  const b = builtinCommands(ctx).map((c) => ({ name: c.name, description: c.description(), hint: c.hint || '', kind: 'builtin' }));
  const custom = (ctx.kind === 'code' ? await customCommands(ctx) : []).filter((c) => !b.some((x) => x.name === c.name))
    .map((c) => ({ name: c.name, description: c.description, hint: c.argument_hint || '', kind: 'custom', source: c.source }));
  return [...b, ...custom];
}

export function parse(text) {
  const m = /^\/([\w:.-]+)(?:\s+([\s\S]*))?$/.exec(text.trim());
  return m ? { name: m[1], args: (m[2] || '').trim() } : null;
}

/**
 * Handle a message that starts with "/". Returns { handled: true } when a
 * built-in ran, { prompt } when the text should be sent (possibly
 * expanded), or null when it isn't a command.
 */
/** Commands that act on an existing session's state. */
const SESSION_ONLY = new Set(['compact', 'context', 'cost', 'budget', 'clear', 'hooks', 'memory', 'todos', 'watch']);

export async function handle(text, ctx) {
  const p = parse(text);
  if (!p) return null;
  const b = builtinCommands(ctx).find((c) => c.name === p.name);
  if (b) {
    // a command that works on a session, run before there is one (the Code
    // page's box): keep what was typed and say why nothing happens
    if (SESSION_ONLY.has(b.name) && !ctx.conversationId) {
      toast(t('cmd_needs_session', { cmd: `/${p.name}` }));
      return { keep: true };
    }
    const r = await b.run(p.args, ctx);
    if (typeof r === 'string') return { prompt: r };
    // false: the command failed; keep what was typed
    if (r === false) return { keep: true };
    // a command can also hand back attachments to send with its prompt
    if (r && typeof r === 'object' && typeof r.prompt === 'string') return { prompt: r.prompt, attachments: r.attachments || [] };
    return { handled: true };
  }
  if (ctx.kind === 'code') {
    try {
      const expanded = await call('slash_expand', { conversationId: ctx.conversationId || null, cwd: ctx.cwd || null, name: p.name, args: p.args });
      if (expanded != null) return { prompt: expanded, display: text };
    } catch { /* fall through */ }
  }
  return null;
}

registerCommand({
  name: 'help',
  description: () => t('cmd_help'),
  run: async (_a, ctx) => {
    const cmds = await allCommands(ctx);
    modal({
      title: t('cmd_help_title'),
      body: `<div class="cmd-help">${cmds.map((c) => `<div class="cmd-row"><code>/${esc(c.name)}${c.hint ? ' ' + esc(c.hint) : ''}</code><span>${esc(c.description || '')}</span>${c.kind === 'custom' ? `<span class="muted"> · ${esc(c.source)}</span>` : ''}</div>`).join('')}
        <p class="muted" style="margin-top:12px">${t('cmd_help_custom')}</p></div>`,
    });
  },
});
