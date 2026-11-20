// Thin wrappers around Tauri IPC.
const T = window.__TAURI__;

export async function call(cmd, args = {}) {
  try {
    return await T.core.invoke(cmd, args);
  } catch (e) {
    const err = new Error(typeof e === 'string' ? e : (e && e.message) || 'Error');
    if (e && typeof e === 'object') { err.kind = e.kind; err.status = e.status; }
    throw err;
  }
}

export function on(event, handler) {
  return T.event.listen(event, (e) => handler(e.payload));
}

export const isMac = navigator.userAgent.includes('Mac');
export const isWindows = navigator.userAgent.includes('Windows');

/** URL for a document served on the artifact:// custom protocol. */
export function artifactUrl(token) {
  return isWindows ? `http://artifact.localhost/${token}` : `artifact://localhost/${token}`;
}

export function windowLabel() {
  try { return T.webviewWindow.getCurrentWebviewWindow().label; } catch { return 'main'; }
}
