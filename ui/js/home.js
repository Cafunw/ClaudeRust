// New chat (welcome) screen.
import { icon, spark } from './icons.js';
import { t } from './i18n.js';
import { call } from './api.js';
import { store, emit, displayName } from './store.js';
import { $, esc } from './util.js';
import { createComposer } from './composer.js';

function greeting() {
  const hr = new Date().getHours();
  const g = hr < 5 ? t('good_night') : hr < 12 ? t('good_morning') : hr < 18 ? t('good_afternoon') : t('good_evening');
  const n = displayName();
  return n ? `${g}, ${n}` : g;
}

const STARTERS = {
  write: { en: 'Help me write ', tr: 'Şunu yazmama yardım et: ' },
  learn: { en: 'Explain to me ', tr: 'Bana şunu açıkla: ' },
  code: { en: 'Help me write code that ', tr: 'Şunu yapan bir kod yazmama yardım et: ' },
  life: { en: 'I need advice on ', tr: 'Şu konuda tavsiyeye ihtiyacım var: ' },
  choice: { en: 'Surprise me with something interesting I probably don’t know.', tr: 'Bana muhtemelen bilmediğim ilginç bir şey anlat.' },
};

export async function startConversation({ text, atts, st, projectId, kind = 'chat', cwd = null, engine = null, worktree = null }) {
  const options = { thinking: st.thinking, effort: st.effort, web_search: st.web_search, style: st.style, disabled_servers: st.disabled_servers };
  if (kind === 'code') { options.permission_mode = st.permission_mode; if (engine) options.engine = engine; if (worktree !== null) options.worktree = worktree; }
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
  const lang = document.documentElement.lang === 'tr' ? 'tr' : 'en';
  const proj = projectId ? store.projects.find((p) => p.id === projectId) : null;
  view.innerHTML = `<div class="topbar"></div><div class="welcome">
    <div class="greeting"><span class="spark">${spark(34)}</span><span>${esc(greeting())}</span></div>
    ${proj ? `<div class="project-pill"><button class="chip">${icon('folder', 14)} ${esc(proj.name)}</button></div>` : ''}
    <div class="composer-wrap"></div>
    <div class="suggestions">
      <button class="chip" data-s="write">${icon('pen', 14)} ${t('suggestions_write')}</button>
      <button class="chip" data-s="learn">${icon('gradcap', 14)} ${t('suggestions_learn')}</button>
      <button class="chip" data-s="code">${icon('code', 14)} ${t('suggestions_code')}</button>
      <button class="chip" data-s="life">${icon('coffee', 14)} ${t('suggestions_life')}</button>
      <button class="chip" data-s="choice">${icon('sparkle', 14)} ${t('suggestions_choice')}</button>
    </div></div>`;
  const composer = createComposer({
    mode: 'chat',
    placeholder: t('help_today'),
    draftKey: 'new',
    initial: { model: store.settings.default_model },
    onSend: (txt, atts, st) => startConversation({ text: txt, atts, st, projectId }),
  });
  $('.composer-wrap', view).appendChild(composer.el);
  view.querySelectorAll('[data-s]').forEach((b) => b.addEventListener('click', () => {
    const s = STARTERS[b.dataset.s][lang];
    composer.setText(s);
    composer.focus();
    if (b.dataset.s === 'choice') composer.send();
  }));
  const pill = $('.project-pill button', view);
  if (pill) pill.onclick = () => emit('navigate', { name: 'project', params: { id: projectId } });
  if (text) composer.setText(text);
  composer.focus();
  document.title = 'ClaudeRust';
  return composer;
}
