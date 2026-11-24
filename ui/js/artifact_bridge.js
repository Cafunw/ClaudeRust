// What the app does for an artifact's sandboxed frame besides storage: the
// person's consent to window.claude.complete and window.claude.callMcpTool,
// and the message "Try fixing" sends. No DOM here (artifacts.js draws them).

/** Grants per scope (an artifact, or an artifact and a connector): `decide(scope, info)`
 *  resolves to how the artifact `scope` may go on now: 'granted' by what was
 *  chosen for it before, 'once' or 'always' by `ask(scope, info)` just now, or
 *  false ('decline', or anything else, such as a dismissed dialog, which
 *  declines this call only). 'always' and 'decline' are kept, in memory: a
 *  reload asks again; `revoke(scope)` forgets them. One question at a time per scope. */
export function createGrants(ask) {
  const chosen = new Map();
  const queue = new Map();
  const decide = (scope, info) => {
    const next = (queue.get(scope) || Promise.resolve()).then(async () => {
      if (chosen.has(scope)) return chosen.get(scope) === 'always' && 'granted';
      const a = await ask(scope, info);
      if (a === 'always' || a === 'decline') chosen.set(scope, a);
      return (a === 'always' || a === 'once') && a;
    });
    queue.set(scope, next.catch(() => {}));
    return next;
  };
  return { decide, chosen: (scope) => chosen.get(scope), revoke: (scope) => chosen.delete(scope) };
}

/** A gate for window.claude.complete: `allowed(scope, info)` resolves to
 *  whether the artifact `scope` may ask Claude now (see createGrants; its
 *  grants are `allowed.grants`). */
export function createConsent(ask) {
  const grants = createGrants(ask);
  const allowed = async (scope, info) => !!(await grants.decide(scope, info));
  allowed.grants = grants;
  return allowed;
}

/** window.claude.complete for a frame `info` ({ scope, title, ... }):
 *  `check(info)` rejects where it can't work (other providers) and else
 *  resolves to the engine ('api', 'account'), before `allowed(scope, { title, engine })`
 *  asks anyone; then `run(info, prompt)`. */
export function createComplete({ check, allowed, run }) {
  return async (info, args) => {
    const [prompt] = Array.isArray(args) ? args : [];
    if (typeof prompt !== 'string') throw new Error('window.claude.complete takes a prompt string.');
    const engine = await check(info);
    if (!(await allowed(info.scope, { title: info.title, engine }))) throw new Error('The user did not allow this artifact to ask Claude.');
    return run(info, prompt);
  };
}

const MAX_TOOL_ARGS = 256 * 1024;

/** The tool name and arguments a frame sent to window.claude.callMcpTool
 *  (`args` as posted: [name, arguments]), checked and copied as plain JSON,
 *  or an Error that says what's wrong. */
export function toolCallArgs(args) {
  const [name, input] = Array.isArray(args) ? args : [];
  if (typeof name !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/.test(name)) throw new Error('window.claude.callMcpTool takes a tool name such as "server__tool".');
  if (input !== undefined && input !== null && (typeof input !== 'object' || Array.isArray(input))) throw new Error('Tool arguments must be an object.');
  let json;
  try { json = JSON.stringify(input || {}); } catch { throw new Error('Tool arguments must be plain JSON.'); }
  if (json.length > MAX_TOOL_ARGS) throw new Error('The tool arguments are too large.');
  return { name, input: JSON.parse(json) };
}

/** The scope of the grant for artifact `scope` to use connector `server`. */
// (JSON: an artifact's identifier or a server's name may hold any separator, and two grants must never meet)
export const toolScope = (scope, server) => JSON.stringify([scope, server]);

/** window.claude.callMcpTool for a frame `info` ({ scope, title, ... }):
 *  `check(info, name)` resolves the tool ({ server, tool, read_only, asks })
 *  or rejects where it's refused, before anyone is asked; `grants` asks once
 *  per artifact and connector (that answer covers the call it was asked for);
 *  a call under an earlier grant still asks with `askCall(info, tool, input)`
 *  where the tool's setting asks; then `run(info, name, input, approved)`. */
export function createToolCall({ check, grants, askCall, run }) {
  return async (info, args) => {
    const { name, input } = toolCallArgs(args);
    const c = await check(info, name);
    const how = await grants.decide(toolScope(info.scope, c.server), { ...c, title: info.title, input });
    if (!how) throw new Error(`The user did not allow this artifact to use ${c.server}.`);
    if (how === 'granted' && c.asks && !(await askCall(info, c, input))) throw new Error(`The user did not allow this call of ${c.tool}.`);
    return run(info, name, input, how !== 'granted' || c.asks);
  };
}

/** A refused or failed tool call as the artifact gets it: an MCP result with isError. */
export function toolError(err) {
  return { content: [{ type: 'text', text: String((err && err.message) || err).slice(0, 2000) }], isError: true };
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
