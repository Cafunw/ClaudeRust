// New chat (welcome) screen, and the incognito one.
import { icon, spark } from './icons.js';
import { t } from './i18n.js';
import { call } from './api.js';
import { store, emit, displayName, newChatModel, newChatEffort } from './store.js';
import { $, esc, h } from './util.js';
import { createComposer, researchOffered } from './composer.js';
import * as keymap from './keymap.js';
import { notRegistered } from './optional_cmd.js';
import { limitBanner } from './usage_limits.js';

function greeting() {
  const hr = new Date().getHours();
  const g = hr < 5 ? t('good_night') : hr < 12 ? t('good_morning') : hr < 18 ? t('good_afternoon') : t('good_evening');
  const n = displayName();
  return n ? `${g}, ${n}` : g;
}

// incognito.rs is in this build: asked without its argument, the command is refused for that, not as unknown
let incognitoOk = false;
call('close_incognito', {}).catch((e) => { incognitoOk = !notRegistered(e); });
export const incognitoSupported = () => incognitoOk;

export function newIncognitoChat() {
  emit('navigate', { name: 'new', params: { incognito: true } });
}

window.addEventListener('keydown', (e) => {
  if (incognitoOk && !e.defaultPrevented && keymap.matches(e, 'incognito')) { e.preventDefault(); newIncognitoChat(); }
});

/** What send_message makes a new chat or code session from: the composer's choices (`st`). */
export function newConversation(st, { projectId = null, kind = 'chat', cwd = null, engine = null, worktree = null, incognito = false } = {}) {
  const options = { thinking: st.thinking, effort: st.effort, web_search: st.web_search, style: st.style, disabled_servers: st.disabled_servers, disabled_tools: st.disabled_tools, tool_access: st.tool_access, fast: !!st.fast, research: !!st.research };
  if (kind === 'chat' && incognito) options.incognito = true;
  if (kind === 'code') { options.permission_mode = st.permission_mode; options.ultracode = engine === 'claude-code' ? st.ultracode ?? null : null; if (engine) options.engine = engine; if (worktree !== null) options.worktree = worktree; }
  return { kind, model: st.model, project_id: projectId || null, cwd, options };
}

/** The first key typed in a new chat's composer starts that chat's Claude Code CLI ahead of the send
 *  (claudecode::prewarm: nothing on the API engine, for incognito chats or with a spending limit). */
export function prewarmOnInput(composer, opts) {
  $('textarea', composer.el).addEventListener('input', () => call('prewarm_chat', { req: newConversation(composer.state, opts) }).catch(() => {}), { once: true });
}

export async function startConversation({ text, atts, st, projectId, kind = 'chat', cwd = null, engine = null, worktree = null, incognito = false }) {
  const research = kind === 'chat' && !!st.research && researchOffered();
  const r = await call('send_message', {
    req: {
      new_conversation: newConversation(st, { projectId, kind, cwd, engine, worktree, incognito }),
      text,
      attachment_ids: atts.map((a) => a.id).filter(Boolean),
      research,
    },
  });
  if (!incognito) store.drafts.delete('new');
  emit('conversations-changed');
  emit('navigate', { name: 'chat', params: { id: r.conversation_id } });
  return true;
}

export function renderNewChat(view, { projectId = null, text = '', incognito = false } = {}) {
  const proj = projectId ? store.projects.find((p) => p.id === projectId) : null;
  // (an incognito chat is never one of a project's: those are remembered with it)
  const inc = incognito && incognitoOk && !proj;
  view.innerHTML = `<div class="topbar"><div class="spacer"></div></div><div class="welcome ${inc ? 'incognito' : ''}">
    ${inc
    ? `<div class="greeting">${icon('eyeOff', 30)}<span>${esc(t('incog_title'))}</span></div><p class="incog-d muted">${esc(t('incog_d'))}</p>`
    : `<div class="greeting"><span class="spark">${spark(34)}</span><span>${esc(greeting())}</span></div>`}
    ${proj ? `<div class="project-pill"><button class="chip">${icon('folder', 14)} ${esc(proj.name)}</button></div>` : ''}
    <div class="composer-wrap"></div></div>`;
  if (incognitoOk && !proj) {
    const label = inc ? t('incog_leave') : t('incog_new');
    const b = h(`<button class="icon-btn incog-toggle ${inc ? 'on' : ''}" aria-pressed="${inc}" title="${esc(label)}" aria-label="${esc(label)}">${icon('eyeOff', 17)}</button>`);
    b.onclick = () => emit('navigate', { name: 'new', params: inc ? {} : { incognito: true } });
    $('.topbar', view).appendChild(b);
  }
  const composer = createComposer({
    mode: 'chat',
    placeholder: t('help_today'),
    // nothing typed in an incognito chat is kept
    draftKey: inc ? null : 'new',
    initial: { model: newChatModel('chat'), effort: newChatEffort('chat') },
    onSend: (txt, atts, st) => startConversation({ text: txt, atts, st, projectId, incognito: inc }),
  });
  prewarmOnInput(composer, { projectId, incognito: inc });
  $('.composer-wrap', view).append(limitBanner(), composer.el);
  const pill = $('.project-pill button', view);
  if (pill) pill.onclick = () => emit('navigate', { name: 'project', params: { id: projectId } });
  if (text) composer.setText(text);
  composer.focus();
  document.title = inc ? t('incog_title') : 'ClaudeRust';
  return composer;
}
