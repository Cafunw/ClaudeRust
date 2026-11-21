// npm run test:ui — keyboard shortcut matching (ui/js/keymap.js). Node is
// not macOS, so Mod is Ctrl here.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { store } from '../js/store.js';
import { ACTIONS, DEFAULTS, bindings, fromEvent, label, matches, parse, shown } from '../js/keymap.js';

/** A keydown event. */
const key = (k, mods = {}, code = '') => ({ key: k, code, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...mods });
const ctrl = (k, code, more = {}) => key(k, { ctrlKey: true, ...more }, code);

test('parse reads modifiers and the key', () => {
  assert.deepEqual({ ...parse('Mod+Shift+O') }, { mod: true, ctrl: false, meta: false, alt: false, shift: true, key: 'o' });
  assert.deepEqual({ ...parse('Cmd+Option+K') }, { mod: false, ctrl: false, meta: true, alt: true, shift: false, key: 'k' });
  assert.equal(parse('Mod++').key, '+');
  assert.equal(parse('').key, '');
  assert.equal(parse('Mod+K'), parse('Mod+K'), 'parsed once');
  assert.ok(Object.isFrozen(parse('Mod+K')));
});

test('Mod is Ctrl off macOS, and the other modifiers must match exactly', () => {
  assert.ok(matches(ctrl('k'), 'palette'));
  assert.ok(!matches(key('k', { metaKey: true }), 'palette'));
  assert.ok(!matches(ctrl('k', '', { altKey: true }), 'palette'));
  assert.ok(!matches(key('k'), 'palette'));
});

test('Shift is required when asked for, and refused for letters when not', () => {
  assert.ok(matches(ctrl('O', 'KeyO', { shiftKey: true }), 'new_chat'));
  assert.ok(!matches(ctrl('o', 'KeyO'), 'new_chat'));
  assert.ok(matches(ctrl('n', 'KeyN'), 'new_chat'), 'an alias');
  assert.ok(!matches(ctrl('N', 'KeyN', { shiftKey: true }), 'new_chat'));
  // "/" needs Shift on some layouts (Shift+7 on German)
  assert.ok(matches(ctrl('/', 'Digit7', { shiftKey: true }), 'shortcuts'));
});

test('letters follow the layout, and non-Latin layouts fall back to the physical key', () => {
  assert.ok(matches(ctrl('и', 'KeyB'), 'sidebar'), 'Cyrillic: the B key');
  assert.ok(!matches(ctrl('x', 'KeyB'), 'sidebar'), 'Dvorak: the key gives x');
  assert.ok(matches(ctrl('`', 'Backquote'), 'workspace'));
  assert.ok(matches(ctrl('ё', 'Backquote'), 'workspace'));
});

test('a user binding replaces the defaults, and "" turns an action off', () => {
  store.settings = { keybindings: { palette: 'Mod+P', find: '' } };
  try {
    assert.ok(matches(ctrl('p', 'KeyP'), 'palette'));
    assert.ok(!matches(ctrl('k', 'KeyK'), 'palette'));
    assert.deepEqual(bindings('find'), []);
    assert.ok(!matches(ctrl('f', 'KeyF'), 'find'));
    assert.equal(shown('find'), '');
    assert.deepEqual(bindings('sidebar'), DEFAULTS.sidebar);
  } finally {
    store.settings = {};
  }
});

test('fromEvent records a binding, never a lone modifier or a plain key', () => {
  assert.equal(fromEvent(ctrl('Control', 'ControlLeft')), null);
  assert.equal(fromEvent(key('a', {}, 'KeyA')), null);
  assert.equal(fromEvent(key('A', { shiftKey: true }, 'KeyA')), null, 'Shift alone is typing');
  assert.equal(fromEvent(key('F5', {}, 'F5')), 'F5');
  assert.equal(fromEvent(ctrl('K', 'KeyK', { shiftKey: true })), 'Mod+Shift+K');
  assert.equal(fromEvent(key('k', { metaKey: true, altKey: true }, 'KeyK')), 'Meta+Alt+K');
  // what is recorded matches the same keys again
  for (const e of [ctrl('ф', 'KeyA'), ctrl('`', 'Backquote', { altKey: true }), ctrl('?', 'Slash', { shiftKey: true })]) {
    store.settings = { keybindings: { palette: fromEvent(e) } };
    try {
      assert.ok(matches(e, 'palette'), fromEvent(e));
    } finally {
      store.settings = {};
    }
  }
});

test('Mod+Space can be bound and matched', () => {
  assert.equal(fromEvent(ctrl(' ', 'Space')), 'Mod+Space');
  store.settings = { keybindings: { palette: 'Mod+Space' } };
  try {
    assert.ok(matches(ctrl(' ', 'Space'), 'palette'));
  } finally {
    store.settings = {};
  }
});

test('label shows a binding the way the platform writes it', () => {
  assert.equal(label('Mod+Shift+O'), 'Ctrl+Shift+O');
  assert.equal(label('Meta+Alt+K'), 'Win+Alt+K');
  assert.equal(label('F5'), 'F5');
  assert.equal(label(''), '');
  assert.equal(shown('palette'), 'Ctrl+K');
});

test('every default binding parses and no two actions share one', () => {
  const seen = new Map();
  for (const a of ACTIONS) {
    for (const b of DEFAULTS[a]) {
      assert.ok(parse(b).key, `${a}: ${b}`);
      const k = JSON.stringify(parse(b));
      assert.ok(!seen.has(k), `${b} is bound to ${seen.get(k)} and ${a}`);
      seen.set(k, a);
    }
  }
});
