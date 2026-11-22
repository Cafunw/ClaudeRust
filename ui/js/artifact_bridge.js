// What the app does for an artifact's sandboxed frame besides storage: the
// person's consent to window.claude.complete, and the message "Try fixing"
// sends. No DOM here (artifacts.js draws them).

/** A gate for window.claude.complete: `allowed(scope, info)` resolves to
 *  whether the artifact `scope` may ask Claude now, by what was chosen for it
 *  before or by `ask(scope, info)`: 'once', 'always' or 'decline' (anything
 *  else, such as a dismissed dialog, declines this call only). 'always' and
 *  'decline' are kept, in memory: a reload asks again. One question at a time
 *  per artifact. */
export function createConsent(ask) {
  const chosen = new Map();
  const queue = new Map();
  return function allowed(scope, info) {
    const next = (queue.get(scope) || Promise.resolve()).then(async () => {
      if (chosen.has(scope)) return chosen.get(scope) === 'always';
      const a = await ask(scope, info);
      if (a === 'always' || a === 'decline') chosen.set(scope, a);
      return a === 'always' || a === 'once';
    });
    queue.set(scope, next.catch(() => {}));
    return next;
  };
}

/** window.claude.complete for a frame `info` ({ scope, title, ... }):
 *  `check(info)` rejects where it can't work (the account's engine, other
 *  providers), before `allowed(scope, title)` asks anyone; then `run(info, prompt)`. */
export function createComplete({ check, allowed, run }) {
  return async (info, args) => {
    const [prompt] = Array.isArray(args) ? args : [];
    if (typeof prompt !== 'string') throw new Error('window.claude.complete takes a prompt string.');
    await check(info);
    if (!(await allowed(info.scope, info.title))) throw new Error('The user did not allow this artifact to ask Claude.');
    return run(info, prompt);
  };
}

/** What the banner says for `err`: a request the sandbox blocked in the
 *  app's language (the page reports it in English, for Claude), else the
 *  page's own message. */
export function errorText(err, t) {
  return err.blocked ? t('art_csp_blocked', { uri: err.blocked.uri, directive: err.blocked.directive }) : err.message;
}

const MAX_ERROR = 1500;

/** The i18n key and values of the message that asks Claude to fix
 *  `err` of the artifact version `a`: a failed update asks for a full
 *  rewrite, an error the page threw for a fix (with the top of its stack). */
export function fixMessage(a, err) {
  const vars = { title: a.title || a.identifier, id: a.identifier, version: a.version };
  if (err.kind === 'update') return { key: 'art_rewrite_msg', vars: { ...vars, error: err.message } };
  const stack = err.stack && !err.stack.includes(err.message) ? `${err.message}\n${err.stack}` : err.stack || err.message;
  return { key: 'art_fix_msg', vars: { ...vars, error: stack.slice(0, MAX_ERROR) } };
}
