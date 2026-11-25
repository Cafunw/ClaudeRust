// What Settings fields read and show, without the page (tested in Node).

/** `raw` as a whole number from `min` to `max`, or null for anything else (empty included). */
export function wholeIn(raw, min = 0, max = Infinity) {
  const n = Number(raw);
  return String(raw).trim() !== '' && Number.isInteger(n) && n >= min && n <= max ? n : null;
}

const ACCEL_MOD = { commandorcontrol: 'Mod', commandorctrl: 'Mod', cmdorctrl: 'Mod', cmdorcontrol: 'Mod', control: 'Ctrl', ctrl: 'Ctrl', alt: 'Alt', option: 'Alt', shift: 'Shift', super: 'Meta', command: 'Meta', cmd: 'Meta' };
const CODE_CHAR = { Backquote: '`', Minus: '-', Equal: '=', BracketLeft: '[', BracketRight: ']', Backslash: '\\', Semicolon: ';', Quote: "'", Comma: ',', Period: '.', Slash: '/' };

/** A global shortcut as Tauri writes it ("CommandOrControl+Alt+Space") as a keymap binding
 *  ("Mod+Alt+Space"), so it is shown with this system's key names like every other shortcut. */
export function acceleratorBinding(accel) {
  if (!accel) return '';
  return String(accel).split('+').map((p) => {
    const k = p.trim();
    return ACCEL_MOD[k.toLowerCase()] || CODE_CHAR[k] || k.replace(/^(?:Key|Digit)(?=\w$)/, '');
  }).join('+');
}

// the keys a global shortcut can be on (global-hotkey's names, the same as KeyboardEvent.code)
const ACCEL_KEY = /^(?:Key[A-Z]|Digit\d|F\d{1,2}|Numpad\d|Arrow(?:Up|Down|Left|Right)|Space|Enter|Tab|Backspace|Delete|Insert|Home|End|PageUp|PageDown|Backquote|Backslash|BracketLeft|BracketRight|Comma|Equal|Minus|Period|Quote|Semicolon|Slash)$/;

/** A keydown as a global shortcut in Tauri's syntax, or null: a lone modifier, a key a global
 *  shortcut can't be on, or one without Ctrl/⌘/Alt/Win (it would take that key from every app). */
export function acceleratorFromEvent(e, mac) {
  const code = e.code || '';
  if (!ACCEL_KEY.test(code)) return null;
  const mods = [];
  if (mac ? e.metaKey : e.ctrlKey) mods.push('CommandOrControl');
  if (mac && e.ctrlKey) mods.push('Control');
  if (!mac && e.metaKey) mods.push('Super');
  if (e.altKey) mods.push('Alt');
  if (!mods.length) return null;
  if (e.shiftKey) mods.push('Shift');
  return [...mods, code.replace(/^(?:Key|Digit)/, '')].join('+');
}

/** What is wrong with the Claude Code found for the path field `want` ('' = detect it): null when
 *  it's there, else the i18n key of the sentence and its path. */
export function cliProblem(want, info) {
  if (!info || !info.found) return want ? { key: 'cc_not_at', path: want } : { key: 'cc_missing' };
  // something is there, but `--version` gave nothing: not the Claude Code CLI
  if (want && !info.version) return { key: 'cc_not_cli', path: want };
  return null;
}
