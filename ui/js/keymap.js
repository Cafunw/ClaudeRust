// Keyboard shortcuts that can be changed (Settings → Keyboard). A binding is
// a string like "Mod+Shift+O": Mod is ⌘ on macOS and Ctrl elsewhere.
import { isMac } from './api.js';
import { store } from './store.js';

/** action -> default bindings (the first is shown; the rest are aliases). */
export const DEFAULTS = {
  new_chat: ['Mod+Shift+O', 'Mod+N'],
  palette: ['Mod+K'],
  sidebar: ['Mod+B', 'Mod+.'],
  settings: ['Mod+,'],
  workspace: ['Mod+`'],
  find: ['Mod+F'],
  history: ['Mod+R'],
  shortcuts: ['Mod+/'],
};

export const ACTIONS = Object.keys(DEFAULTS);

/** The bindings in effect for `action` (a user's choice replaces the defaults; "" turns it off). */
export function bindings(action) {
  const user = (store.settings && store.settings.keybindings) || {};
  if (Object.prototype.hasOwnProperty.call(user, action)) return user[action] ? [String(user[action])] : [];
  return DEFAULTS[action] || [];
}

const CODE_KEYS = {
  Backquote: '`', Minus: '-', Equal: '=', BracketLeft: '[', BracketRight: ']', Backslash: '\\',
  Semicolon: ';', Quote: "'", Comma: ',', Period: '.', Slash: '/', Space: 'space',
};

/** The key of an event, independent of Shift and (for letters/digits) of the layout. */
function eventKeys(e) {
  const out = new Set();
  if (e.key) out.add(e.key.toLowerCase());
  // the physical key only stands in when the layout gives a non-Latin
  // character (e.g. Cyrillic, Greek): on Dvorak / Colemak `e.key` is right,
  // and the key at that position means something else (Ctrl+V is at `.`)
  if (e.key && /^[\x20-\x7e]$/.test(e.key)) return out;
  const c = e.code || '';
  if (/^Key[A-Z]$/.test(c)) out.add(c.slice(3).toLowerCase());
  else if (/^Digit\d$/.test(c)) out.add(c.slice(5));
  else if (CODE_KEYS[c]) out.add(CODE_KEYS[c]);
  if (e.key === ' ') out.add('space');
  return out;
}

export function parse(binding) {
  const parts = String(binding || '').split('+');
  // "Mod++" style (the key itself is "+")
  let key = parts.pop();
  if (key === '' && parts.length && parts[parts.length - 1] === '') { parts.pop(); key = '+'; }
  const has = (m) => parts.some((p) => p.toLowerCase() === m);
  return { mod: has('mod'), ctrl: has('ctrl'), meta: has('meta') || has('cmd'), alt: has('alt') || has('option'), shift: has('shift'), key: (key || '').toLowerCase() };
}

function matchesOne(e, binding) {
  const b = parse(binding);
  if (!b.key) return false;
  const wantCtrl = b.ctrl || (b.mod && !isMac);
  const wantMeta = b.meta || (b.mod && isMac);
  if (!!e.ctrlKey !== wantCtrl || !!e.metaKey !== wantMeta || !!e.altKey !== b.alt) return false;
  // Shift is part of typing some symbols ("?" is Shift+/), so only a binding
  // that asks for it requires it; one that doesn't rejects it for letters
  if (b.shift && !e.shiftKey) return false;
  const keys = eventKeys(e);
  if (!b.shift && e.shiftKey && /^[a-z0-9]$/.test(b.key)) return false;
  return keys.has(b.key);
}

/** Whether keydown `e` triggers `action`. */
export function matches(e, action) {
  return bindings(action).some((b) => matchesOne(e, b));
}

/** A binding string for a keydown (null for a lone modifier, or a plain
 *  key without a modifier, which would get in the way of typing). */
export function fromEvent(e) {
  const k = e.key || '';
  if (['Control', 'Shift', 'Alt', 'Meta', 'OS', 'Dead', 'Unidentified'].includes(k)) return null;
  const mods = [];
  if (isMac ? e.metaKey : e.ctrlKey) mods.push('Mod');
  if (isMac && e.ctrlKey) mods.push('Ctrl');
  if (!isMac && e.metaKey) mods.push('Meta');
  if (e.altKey) mods.push('Alt');
  if (e.shiftKey) mods.push('Shift');
  const fkey = /^F\d{1,2}$/.test(k);
  if (!fkey && !mods.some((m) => m !== 'Shift')) return null;
  const keys = [...eventKeys(e)];
  const key = keys.find((x) => x.length === 1) || keys[0] || k.toLowerCase();
  return [...mods, key.length === 1 ? key.toUpperCase() : key[0].toUpperCase() + key.slice(1)].join('+');
}

/** How a binding is shown: ⌘⇧O on macOS, Ctrl+Shift+O elsewhere. */
export function label(binding) {
  if (!binding) return '';
  const b = parse(binding);
  const key = b.key.length === 1 ? b.key.toUpperCase() : b.key[0].toUpperCase() + b.key.slice(1);
  if (isMac) return `${b.ctrl ? '⌃' : ''}${b.alt ? '⌥' : ''}${b.shift ? '⇧' : ''}${b.mod || b.meta ? '⌘' : ''}${key}`;
  return [b.mod || b.ctrl ? 'Ctrl' : '', b.meta ? 'Win' : '', b.alt ? 'Alt' : '', b.shift ? 'Shift' : '', key].filter(Boolean).join('+');
}

/** The shown binding of an action ("" when turned off). */
export function shown(action) {
  return label(bindings(action)[0] || '');
}
