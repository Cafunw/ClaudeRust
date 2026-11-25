// npm run test:ui — the Quick Entry shortcut (Settings → Desktop app): recorded with the keys
// like the app's own shortcuts, saved in Tauri's accelerator syntax, shown with this system's names.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { acceleratorBinding, acceleratorFromEvent } from '../js/settings_values.js';
import * as keymap from '../js/keymap.js';
import { source } from './source.mjs';

const key = (code, mods = {}) => ({ code, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...mods });

test('a key combination becomes the accelerator the global shortcut is registered with', () => {
  assert.equal(acceleratorFromEvent(key('Space', { ctrlKey: true, altKey: true }), false), 'CommandOrControl+Alt+Space');
  assert.equal(acceleratorFromEvent(key('KeyK', { ctrlKey: true, shiftKey: true }), false), 'CommandOrControl+Shift+K');
  assert.equal(acceleratorFromEvent(key('Digit1', { altKey: true }), false), 'Alt+1');
  assert.equal(acceleratorFromEvent(key('F9', { metaKey: true }), false), 'Super+F9', 'the Windows key');
  assert.equal(acceleratorFromEvent(key('Space', { metaKey: true, ctrlKey: true }), true), 'CommandOrControl+Control+Space', 'macOS: ⌘ and ⌃ apart');
  assert.equal(acceleratorFromEvent(key('Backquote', { altKey: true }), true), 'Alt+Backquote');
});

test('what a global shortcut can’t be is refused', () => {
  assert.equal(acceleratorFromEvent(key('KeyK'), false), null, 'a plain key would be taken from every app');
  assert.equal(acceleratorFromEvent(key('KeyK', { shiftKey: true }), false), null, 'Shift alone too');
  assert.equal(acceleratorFromEvent(key('AltLeft', { altKey: true }), false), null, 'a lone modifier');
  assert.equal(acceleratorFromEvent(key('MediaPlayPause', { ctrlKey: true }), false), null, 'a key it can’t be on');
});

test('it is shown with this system’s key names, like the other shortcuts', () => {
  assert.equal(acceleratorBinding('CommandOrControl+Alt+Space'), 'Mod+Alt+Space');
  assert.equal(keymap.label(acceleratorBinding('CommandOrControl+Alt+Space')), 'Ctrl+Alt+Space');
  assert.equal(keymap.label(acceleratorBinding('CmdOrCtrl+Shift+K')), 'Ctrl+Shift+K');
  assert.equal(keymap.label(acceleratorBinding('Super+F9')), 'Win+F9');
  assert.equal(keymap.label(acceleratorBinding('Alt+Backquote')), 'Alt+`');
  assert.equal(keymap.label(acceleratorBinding('Control+Option+KeyA')), 'Ctrl+Alt+A');
  assert.equal(acceleratorBinding(''), '');
  // what the recorder saves reads back the same
  assert.equal(keymap.label(acceleratorBinding(acceleratorFromEvent(key('Digit1', { ctrlKey: true, altKey: true }), false))), 'Ctrl+Alt+1');
});

test('the Desktop page records the shortcut instead of taking typed accelerator text', () => {
  const src = source('../js/settings.js');
  const desktop = src.slice(src.indexOf('  desktop(body) {'), src.indexOf('  shortcuts(body) {'));
  assert.doesNotMatch(desktop, /textField\('quick_entry_shortcut'/);
  assert.match(desktop, /recordKeys\(qeChip, \(e\) => acceleratorFromEvent\(e, isMac\), /);
  assert.match(desktop, /qeChip\.textContent = keymap\.label\(acceleratorBinding\(accel\)\) \|\| t\('kb_off'\)/);
  // and the Shortcuts page records with the same recorder
  assert.match(src, /const record = \(action, chip\) => recordKeys\(chip, keymap\.fromEvent, /);
});
