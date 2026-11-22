// New chat (welcome) screen.
import { icon, spark } from './icons.js';
import { t } from './i18n.js';
import { call } from './api.js';
import { store, emit, displayName, newChatModel } from './store.js';
import { $, esc } from './util.js';
import { createComposer } from './composer.js';

function greeting() {
  const hr = new Date().getHours();
  const g = hr < 5 ? t('good_night') : hr < 12 ? t('good_morning') : hr < 18 ? t('good_afternoon') : t('good_evening');
  const n = displayName();
  return n ? `${g}, ${n}` : g;
}

export async function startConversation({ text, atts, st, projectId, kind = 'chat', cwd = null, engine = null, worktree = null }) {
  const options = { thinking: st.thinking, effort: st.effort, web_search: st.web_search, style: st.style, disabled_servers: st.disabled_servers };
  if (kind === 'code') { options.permission_mode = st.permission_mode; options.ultracode = engine === 'claude-code' ? st.ultracode ?? null : null; if (engine) options.engine = engine; if (worktree !== null) options.worktree = worktree; }
  const r = await call('send_message', {
    req: {
      new_conversation: { kind, model: st.model, project_id: projectId || null, cwd, options },
      text,
      attachment_ids: atts.map((a) => a.id).filter(Boolean),
    },
  });
  store.drafts.delete('new');
  emit('conversations-changed');
  emit('navigate', { name: 'chat', params: { id: r.conversation_id } });
  return true;
}

export function renderNewChat(view, { projectId = null, text = '' } = {}) {
  const proj = projectId ? store.projects.find((p) => p.id === projectId) : null;
  view.innerHTML = `<div class="topbar"></div><div class="welcome">
    <div class="greeting"><span class="spark">${spark(34)}</span><span>${esc(greeting())}</span></div>
    ${proj ? `<div class="project-pill"><button class="chip">${icon('folder', 14)} ${esc(proj.name)}</button></div>` : ''}
    <div class="composer-wrap"></div></div>`;
  const composer = createComposer({
    mode: 'chat',
    placeholder: t('help_today'),
    draftKey: 'new',
    initial: { model: newChatModel('chat') },
    onSend: (txt, atts, st) => startConversation({ text: txt, atts, st, projectId }),
  });
  $('.composer-wrap', view).appendChild(composer.el);
  const pill = $('.project-pill button', view);
  if (pill) pill.onclick = () => emit('navigate', { name: 'project', params: { id: projectId } });
  if (text) composer.setText(text);
  composer.focus();
  document.title = 'ClaudeRust';
  return composer;
}
