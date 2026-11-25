// npm run test:ui — a key typed into the integrated terminal goes to the app
// or to the shell, never both: the terminal keeps Alt+1…8 (the shell's
// digit-argument), and xterm skips a key the app's shortcuts already took.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const handlers = [];
window.addEventListener = (type, fn, capture) => { if (type === 'keydown' && capture) handlers.push(fn); };
document.getElementById = () => null;
const { inTerminal } = await import('../js/shortcuts.js');
const workspace = await import('../js/workspace.js');
workspace.registerPane({ id: 'changes', label: () => 'Changes' });
workspace.registerPane({ id: 'terminal', label: () => 'Terminal' });
workspace.setConversation({ kind: 'code', id: 'c1', cwd: 'D:/x' });

const inside = (sel) => ({ closest: (s) => (s === '.xterm' && sel === '.xterm' ? {} : null) });
const altKey = (target, code = 'Digit2') => {
  const e = { altKey: true, ctrlKey: false, metaKey: false, shiftKey: false, key: code.slice(5), code, target, defaultPrevented: false };
  e.preventDefault = () => { e.defaultPrevented = true; };
  return e;
};

test('the terminal keeps Alt+digit; elsewhere it switches panes', () => {
  assert.equal(inTerminal({ target: inside('.xterm') }), true);
  assert.equal(inTerminal({ target: inside('textarea') }), false);
  assert.equal(inTerminal({ target: null }), false);
  const [onKey] = handlers;
  const inShell = altKey(inside('.xterm'));
  onKey(inShell);
  assert.equal(inShell.defaultPrevented, false, 'left to the shell');
  assert.equal(workspace.activePane(), 'changes', 'no pane switched');
  const inChat = altKey(inside('body'));
  onKey(inChat);
  assert.equal(inChat.defaultPrevented, true);
  assert.equal(workspace.activePane(), 'terminal');
});

test('xterm does not also send a key the app took', () => {
  assert.match(readFileSync(new URL('../js/panes/terminal.js', import.meta.url), 'utf8'), /term\.attachCustomKeyEventHandler\(\(e\) => !e\.defaultPrevented\)/);
});

test('the shortcut sheet says the terminal keeps them', async () => {
  const src = readFileSync(new URL('../js/shortcuts.js', import.meta.url), 'utf8');
  assert.match(src, /\$\{esc\(workspace\.paneHints\(\)\)\} \$\{esc\(t\('sc_panes_term'\)\)\}/);
  const { t, setLanguage } = await import('../js/i18n.js');
  setLanguage('tr');
  assert.equal(t('sc_panes_term'), 'Terminaldeyken bu tuşlar kabuğa gider.');
  setLanguage('en');
  assert.equal(t('sc_panes_term'), 'In the terminal, these keys go to the shell.');
});
