// Global app state with a tiny pub/sub.
const listeners = new Map();

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
  drafts: new Map(), // conversation id | 'new' -> text
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
