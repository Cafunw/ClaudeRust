// Global app state with a tiny pub/sub.
const listeners = new Map();

const DRAFTS_KEY = 'clauderust-drafts';

/** Unsent drafts, kept in local storage so they survive a restart. */
class Drafts extends Map {
  constructor() {
    super();
    try {
      const saved = JSON.parse(localStorage.getItem(DRAFTS_KEY) || '[]');
      for (const [k, v] of saved) if (typeof v === 'string' && v.trim()) super.set(k, v);
    } catch { /* storage unavailable */ }
    this.timer = null;
  }
  persist() {
    clearTimeout(this.timer);
    this.timer = setTimeout(() => this.flush(), 400);
  }
  /** Write now (also on quit, so the last keystrokes aren't lost). */
  flush() {
    clearTimeout(this.timer);
    this.timer = null;
    try {
      const keep = [...this.entries()].filter(([, v]) => v && v.trim() && v.length <= 100_000).slice(-50);
      localStorage.setItem(DRAFTS_KEY, JSON.stringify(keep));
    } catch { /* storage full or unavailable */ }
  }
  set(k, v) {
    // re-insert so the newest drafts are kept when trimming
    super.delete(k);
    if (v && v.trim()) super.set(k, v);
    this.persist();
    return this;
  }
  delete(k) {
    const r = super.delete(k);
    this.persist();
    return r;
  }
}

export const store = {
  settings: {},
  models: [],
  platform: 'linux',
  version: '',
  dataDir: '',
  mcpConfigPath: '',
  conversations: [],
  projects: [],
  route: { name: 'new', params: {} },
  running: new Set(),
  permissions: new Map(), // request_id -> payload
  drafts: new Drafts(), // conversation id | 'new' -> text (persisted)
  sidebarCollapsed: false,
  mcpServers: [],
  artifactPanel: null, // {conversationId, identifier, version}
};

export function on(evt, fn) {
  if (!listeners.has(evt)) listeners.set(evt, new Set());
  listeners.get(evt).add(fn);
  return () => listeners.get(evt).delete(fn);
}

export function emit(evt, data) {
  const s = listeners.get(evt);
  if (s) for (const fn of [...s]) { try { fn(data); } catch (e) { console.error(e); } }
}

export function modelInfo(id) {
  return store.models.find((m) => m.id === id) || { id, name: id.replace(/^claude-/, ''), thinking: 'explicit', efforts: [], description: '' };
}

export function displayName() {
  const s = store.settings;
  return (s.nickname || s.full_name || '').trim();
}

const LS = 'clauderust.ui';
export function loadLocal() {
  try { return JSON.parse(localStorage.getItem(LS) || '{}'); } catch { return {}; }
}
export function saveLocal(patch) {
  try { localStorage.setItem(LS, JSON.stringify({ ...loadLocal(), ...patch })); } catch { /* ignore */ }
}

function defaultModel(mode) {
  const s = store.settings;
  return mode === 'code' ? s.code_model || s.default_model : s.default_model;
}

/** The model a new chat (mode 'chat') or code session starts on: the one
 *  last picked in that mode, unless the default in Settings has changed
 *  since that pick or the model is no longer offered. */
export function newChatModel(mode) {
  const def = defaultModel(mode);
  const last = loadLocal()[`lastModel_${mode}`];
  const offered = last && store.models.some((m) => m.id === last.model && m.available !== false);
  return offered && last.def === def ? last.model : def;
}

export function rememberModel(mode, model) {
  saveLocal({ [`lastModel_${mode}`]: { model, def: defaultModel(mode) } });
}

/** The thinking effort a new chat or code session starts at: the one last picked in that mode
 *  (null: the model's own). A model without that level runs at its own (composer.js effortOf). */
export const newChatEffort = (mode) => loadLocal()[`lastEffort_${mode}`] ?? null;
export const rememberEffort = (mode, effort) => saveLocal({ [`lastEffort_${mode}`]: effort });

// a pending draft write must not be lost when the window closes
window.addEventListener('pagehide', () => store.drafts.flush());
window.addEventListener('beforeunload', () => store.drafts.flush());
