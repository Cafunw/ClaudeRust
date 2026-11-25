// Read aloud: an answer spoken with the system's voices (speechSynthesis).
// The button is only offered where the WebView has voices.
import { t, currentLanguage } from './i18n.js';
import { toast } from './util.js';

/** What of a Markdown answer is worth saying: prose, list items and table
 *  cells, without code blocks, markup, links' targets or artifacts. */
export function speechText(md) {
  return String(md || '')
    .replace(/<antArtifact[\s\S]*?(<\/antArtifact>|$)/g, ' ')
    .replace(/(^|\n) {0,3}(`{3}|~{3})[\s\S]*?(\n {0,3}\2[^\n]*|(?![\s\S]))/g, '$1')
    .replace(/!\[[^\]]*\]\([^)]*\)/g, ' ')
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/`([^`]*)`/g, '$1')
    .replace(/<[^>\n]+>/g, ' ')
    .replace(/^ {0,3}\|?(\s*:?-{3,}:?\s*\|)+\s*:?-*:?\s*$/gm, ' ')
    .replace(/^ *\|(.*)\|? *$/gm, (_m, row) => `${row.split('|').map((c) => c.trim()).filter(Boolean).join(', ')}.`)
    .replace(/^ {0,3}(#{1,6}|>+|[-*+]|\d+[.)])\s+(\[[ xX]\]\s+)?/gm, '')
    .replace(/(\*\*|__|~~)(?=\S)([^\n]*?\S)\1/g, '$2')
    // *italic* and _italic_, not 2*3*4 or snake_case_name
    .replace(/(^|[^\w*])\*(?=\S)([^*\n]*?\S)\*(?![\w*])/g, '$1$2')
    .replace(/(^|[^\w])_(?=\S)([^_\n]*?\S)_(?!\w)/g, '$1$2')
    .replace(/\$\$[\s\S]*?\$\$/g, ' ')
    .replace(/\s*\n\s*/g, '\n')
    .replace(/[ \t]{2,}/g, ' ')
    .trim();
}

/** The language to speak `text` in: Turkish when its letters say so,
 *  English when it reads like English, else the UI's. */
export function speechLang(text, uiLang) {
  const s = String(text || '');
  if (/[ğşıİĞŞ]/.test(s)) return 'tr-TR';
  const english = (s.toLowerCase().match(/\b(the|and|is|are|of|to|you|this|that|with)\b/g) || []).length >= 3;
  return english || uiLang !== 'tr' ? 'en-US' : 'tr-TR';
}

/** Sentences of `text`, packed into pieces of at most `max` characters
 *  (long utterances stop short on some voices). */
export function chunks(text, max = 220) {
  const out = [];
  let cur = '';
  for (const s of String(text).match(/[^.!?\n]+[.!?]*\s*|\n+/g) || []) {
    if (cur && (cur + s).length > max) { out.push(cur.trim()); cur = ''; }
    cur += s;
    while (cur.length > max) { out.push(cur.slice(0, max)); cur = cur.slice(max); }
  }
  if (cur.trim()) out.push(cur.trim());
  return out.filter(Boolean);
}

const synth = () => (typeof window !== 'undefined' && window.speechSynthesis) || null;

/** Whether this WebView can read aloud (it has voices). */
export function readAloudAvailable() {
  const s = synth();
  return !!s && s.getVoices().length > 0;
}

// voices come in a moment after start on some platforms: callers redraw then
export function onVoicesReady(fn) {
  const s = synth();
  if (s && !s.getVoices().length) s.addEventListener('voiceschanged', fn, { once: true });
}

let reading = null; // { key, onEnd }

/** The answer being read (its key), or null. */
export function readingKey() { return reading && reading.key; }

export function stopReading() {
  const r = reading;
  reading = null;
  const s = synth();
  if (s) s.cancel();
  if (r && r.onEnd) r.onEnd();
}

/** Read Markdown `md` aloud; `key` names it (an answer), `onEnd` runs when it stops. */
export function readAloud(key, md, onEnd) {
  stopReading();
  const s = synth();
  const text = speechText(md);
  if (!s || !text) return;
  const lang = speechLang(text, currentLanguage());
  const voices = s.getVoices().filter((v) => v.lang.replace('_', '-').slice(0, 2) === lang.slice(0, 2));
  const voice = voices.find((v) => v.lang.replace('_', '-') === lang && v.localService) || voices.find((v) => v.lang.replace('_', '-') === lang) || voices[0];
  const me = { key, onEnd };
  reading = me;
  const parts = chunks(text);
  parts.forEach((p, i) => {
    const u = new SpeechSynthesisUtterance(p);
    u.lang = lang;
    if (voice) u.voice = voice;
    if (i === parts.length - 1) u.onend = () => { if (reading === me) stopReading(); };
    u.onerror = (e) => {
      if (reading !== me) return;
      if (e.error !== 'interrupted' && e.error !== 'canceled') toast(t('ra_failed'), 'error');
      stopReading();
    };
    s.speak(u);
  });
}

// nothing goes on reading once the window is closed or reloaded
if (typeof window !== 'undefined') window.addEventListener('pagehide', stopReading);
