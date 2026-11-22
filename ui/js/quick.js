// Quick Entry window: a floating prompt that opens a new chat in the main window.
import { call, on } from './api.js';
import { setLanguage, t } from './i18n.js';

if (window.__QUICK_OPAQUE__) document.body.classList.add('opaque');
const q = document.getElementById('q');
const go = document.getElementById('go');
const mq = window.matchMedia('(prefers-color-scheme: dark)');
let themePref = 'system';

function applyTheme() {
  document.documentElement.dataset.theme = themePref === 'light' || themePref === 'dark' ? themePref : (mq.matches ? 'dark' : 'light');
}

/** The main window's language and theme, as they are now (they may have changed since the last time this was shown). */
async function applySettings() {
  try {
    const s = await call('get_settings');
    setLanguage(s.language);
    themePref = s.theme;
    applyTheme();
    q.placeholder = t('help_today');
  } catch { /* keep what was shown */ }
}

function grow() {
  q.style.height = 'auto';
  q.style.height = Math.min(q.scrollHeight, 80) + 'px';
}

async function submit() {
  const text = q.value.trim();
  if (!text) return;
  q.value = '';
  grow();
  // not sent: what was typed comes back rather than being lost
  await call('quick_entry_submit', { text }).catch(() => { if (!q.value) { q.value = text; grow(); } });
}

q.addEventListener('input', grow);
q.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); submit(); }
  if (e.key === 'Escape') { e.preventDefault(); call('quick_entry_hide').catch(() => {}); }
});
go.addEventListener('click', submit);
on('quick-entry-shown', () => { applySettings(); q.focus(); q.select(); });
mq.addEventListener('change', applyTheme);
window.addEventListener('focus', () => q.focus());
applySettings().then(() => q.focus());
