// clauderust:// links: where the window goes for a route the backend checked
// (src-tauri/src/deeplink.rs). A link's prompt only waits in the composer: it is never sent.

/** The navigate() target of route `r` ({ settings: true } for Settings), or null. */
export function linkTarget(r) {
  if (!r) return null;
  if (r.route === 'chat' && r.id) return { name: 'chat', params: { id: r.id } };
  if (r.route === 'new' || r.route === 'code') return { name: r.route, params: { text: r.prompt || '' } };
  if (r.route === 'settings') return { settings: true };
  return null;
}
