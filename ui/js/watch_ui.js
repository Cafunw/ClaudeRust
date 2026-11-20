// Watch mode badge (topbar of code sessions): re-run a check command when
// files change; show the result; send errors to Claude.
import { t } from './i18n.js';
import { icon } from './icons.js';
import { call, on } from './api.js';
import { h, esc, modal, errorToast } from './util.js';
import { registerCommand } from './slash.js';

const last = new Map(); // convId -> status summary

function badgeHtml(s) {
  if (!s || !s.watching) return `${icon('eye', 14)}<span>${esc(t('watch_off'))}</span>`;
  if (s.status === 'running') return `<span class="spinner tiny"></span><span>${esc(t('watch_running'))}</span>`;
  if (s.status === 'ok') return `<span class="wb-ok">${icon('check', 13)}</span><span>${esc(t('watch_ok'))}</span>`;
  if (s.status === 'failed' || s.status === 'error') return `<span class="wb-err">${icon('x', 13)}</span><span>${esc(s.errors ? t('watch_errors', { n: s.errors }) : t('watch_failed'))}</span>`;
  return `${icon('eye', 14)}<span>${esc(t('watch_idle'))}</span>`;
}

function paint(el) {
  const s = last.get(el.dataset.conv);
  el.className = `watch-badge ${s && s.watching ? s.status : 'off'}`;
  el.innerHTML = badgeHtml(s);
  el.title = s && s.command ? s.command : t('watch_help');
}

async function refresh(convId) {
  try { last.set(convId, await call('watch_status', { conversationId: convId })); } catch { /* ignore */ }
  document.querySelectorAll(`.watch-badge[data-conv="${CSS.escape(convId)}"]`).forEach(paint);
}

async function sendFix(convId, s) {
  const chat = await import('./chat.js');
  const c = chat.current();
  if (!c || c.id !== convId) return;
  const out = (s.output || '').slice(-12000);
  c.composer.setText(`${t('watch_fix_prompt', { cmd: s.command, n: s.errors || 0 })}\n\n\`\`\`\n${out.trim()}\n\`\`\``);
  c.composer.send();
}

function openPanel(convId, conv) {
  const s = last.get(convId) || {};
  const body = h(`<div class="watch-dlg">
    <p class="muted small">${esc(t('watch_help'))}</p>
    <label class="small">${esc(t('watch_command'))}</label>
    <div class="row-inline"><input class="input mono" data-k="cmd" value="${esc(s.command || (conv && conv.options && conv.options.watch_command) || s.detected || '')}" placeholder="npm test"></div>
    <label class="day"><input type="checkbox" data-k="auto" ${s.auto_fix ? 'checked' : ''}> ${esc(t('watch_autofix'))}</label>
    ${s.watching ? `<div class="watch-res ${esc(s.status || '')}"><div class="wr-head">${badgeHtml(s)}${s.duration_ms ? `<span class="muted small">· ${(s.duration_ms / 1000).toFixed(1)}s</span>` : ''}</div>${s.output ? `<pre class="task-out">${esc(s.output.slice(-8000))}</pre>` : ''}</div>` : ''}
    <div class="rt-actions">
      ${s.watching ? `<button class="btn ghost" data-a="stop">${t('watch_stop')}</button><button class="btn" data-a="run">${icon('play', 13)} ${t('watch_run')}</button>` : ''}
      ${s.watching && (s.status === 'failed' || s.status === 'error') ? `<button class="btn primary" data-a="fix">${t('watch_fix')}</button>` : ''}
      ${s.watching ? '' : `<button class="btn primary" data-a="start">${icon('eye', 13)} ${t('watch_start')}</button>`}
    </div></div>`);
  const m = modal({ title: t('watch_title'), body, wide: true });
  const q = (k) => body.querySelector(`[data-k=${k}]`);
  const a = (k) => body.querySelector(`[data-a=${k}]`);
  q('auto').onchange = () => { if (s.watching) call('watch_auto_fix', { conversationId: convId, on: q('auto').checked }).catch(errorToast); };
  if (a('start')) a('start').onclick = async () => {
    try { last.set(convId, await call('watch_start', { conversationId: convId, command: q('cmd').value, autoFix: q('auto').checked })); m.close(); refresh(convId); } catch (e) { errorToast(e); }
  };
  if (a('stop')) a('stop').onclick = async () => { await call('watch_stop', { conversationId: convId }).catch(errorToast); m.close(); refresh(convId); };
  if (a('run')) a('run').onclick = async () => {
    if (q('cmd').value.trim() && q('cmd').value.trim() !== s.command) {
      try { await call('watch_start', { conversationId: convId, command: q('cmd').value, autoFix: q('auto').checked }); } catch (e) { errorToast(e); return; }
    } else await call('watch_run', { conversationId: convId }).catch(errorToast);
    m.close();
  };
  if (a('fix')) a('fix').onclick = () => { m.close(); sendFix(convId, s); };
}

/** The topbar badge for a code conversation. */
export function watchBadge(conv) {
  const el = h(`<button class="watch-badge off" data-conv="${esc(conv.id)}"></button>`);
  el.onclick = () => openPanel(conv.id, conv);
  paint(el);
  refresh(conv.id).then(() => {
    const s = last.get(conv.id);
    // resume watching after a restart
    if (s && !s.watching && conv.options && conv.options.watch_enabled && conv.options.watch_command) {
      call('watch_start', { conversationId: conv.id, command: conv.options.watch_command, autoFix: !!conv.options.watch_auto_fix }).then(() => refresh(conv.id)).catch(() => {});
    }
  });
  return el;
}

on('watch-status', (s) => {
  last.set(s.conversation_id, s);
  document.querySelectorAll(`.watch-badge[data-conv="${CSS.escape(s.conversation_id)}"]`).forEach(paint);
});

registerCommand({
  name: 'watch',
  description: () => t('cmd_watch'),
  kinds: ['code'],
  run: async (_a, ctx) => { if (ctx.conversationId) { await refresh(ctx.conversationId); openPanel(ctx.conversationId, ctx.conv); } },
});
