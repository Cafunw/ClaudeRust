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
    this.timer = setTimeout(() => {
      try {
        const keep = [...this.entries()].filter(([, v]) => v && v.trim() && v.length <= 100_000).slice(-50);
        localStorage.setItem(DRAFTS_KEY, JSON.stringify(keep));
      } catch { /* storage full or unavailable */ }
    }, 400);
  }
  set(k, v) {
    // re-insert so the newest drafts are kept when trimming
    super.delete(k);
    if (v && v.trim()) super.set(k, v);
    if (this.timer !== undefined) this.persist();
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
