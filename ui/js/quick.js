// Quick Entry window: a floating prompt that opens a new chat in the main window.
import { call, on } from './api.js';
import { setLanguage, t } from './i18n.js';

const q = document.getElementById('q');
const go = document.getElementById('go');
const mq = window.matchMedia('(prefers-color-scheme: dark)');

async function init() {
  try {
    const b = await call('bootstrap');
    const s = b.settings;
    setLanguage(s.language);
    const theme = s.theme === 'light' || s.theme === 'dark' ? s.theme : (mq.matches ? 'dark' : 'light');
    document.documentElement.dataset.theme = theme;
    q.placeholder = t('help_today');
  } catch { /* ignore */ }
  q.focus();
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
  await call('quick_entry_submit', { text }).catch(() => {});
}

q.addEventListener('input', grow);
q.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); submit(); }
  if (e.key === 'Escape') { e.preventDefault(); call('quick_entry_hide').catch(() => {}); }
});
go.addEventListener('click', submit);
on('quick-entry-shown', () => { q.focus(); q.select(); });
window.addEventListener('focus', () => q.focus());
init();
