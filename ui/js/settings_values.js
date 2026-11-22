// What Settings fields read and show, without the page (tested in Node).

/** `raw` as a whole number from `min` to `max`, or null for anything else (empty included). */
export function wholeIn(raw, min = 0, max = Infinity) {
  const n = Number(raw);
  return String(raw).trim() !== '' && Number.isInteger(n) && n >= min && n <= max ? n : null;
}

/** A global shortcut as Tauri writes it ("CommandOrControl+Alt+Space") with this system's key name. */
export function acceleratorLabel(accel, mod) {
  return String(accel || '').replace(/CommandOrControl|CmdOrCtrl/g, mod);
}
