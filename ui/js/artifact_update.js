// Artifact versions written as edits (`command="update"`, prompts.rs): pairs
// of <old_str>/<new_str> applied to the version before. All of them apply or
// none: an old string that isn't there exactly once leaves the previous
// content and says why, so the panel can ask Claude for a full rewrite.

const PAIR = /<old_str>([\s\S]*?)<\/old_str>\s*<new_str>([\s\S]*?)<\/new_str>/g;

// a newline right inside a tag is layout, not text
const inner = (s) => s.replace(/^\r?\n/, '').replace(/\r?\n$/, '');

/** The complete edits in `body` (one still streaming is left out). */
export function parseEdits(body) {
  return [...String(body).matchAll(PAIR)].map((m) => ({ old: inner(m[1]), new: inner(m[2]) }));
}

/** `content` with the edits of `body` applied: `{ content }`, or the
 *  unchanged `content` and an `error`. */
export function applyEdits(content, body) {
  const edits = parseEdits(body);
  if (!edits.length) return { content, error: 'The update has no <old_str>…</old_str><new_str>…</new_str> pair.' };
  // the model writes \n; an artifact that came in with \r\n is edited in kind
  const eol = content.includes('\r\n') ? (s) => s.replace(/\r?\n/g, '\r\n') : (s) => s;
  let out = content;
  for (const [i, e] of edits.entries()) {
    const old = eol(e.old);
    const n = edits.length > 1 ? ` ${i + 1}` : '';
    if (!old) return { content, error: `old_str${n} is empty.` };
    const at = out.indexOf(old);
    if (at < 0) return { content, error: `old_str${n} was not found in the current version.` };
    // overlapping matches count: "aa" is twice in "aaa"
    if (out.indexOf(old, at + 1) >= 0) return { content, error: `old_str${n} appears more than once in the current version.` };
    out = out.slice(0, at) + eol(e.new) + out.slice(at + old.length);
  }
  return { content: out };
}

/** The versions of one artifact, in order, with each update applied to the
 *  version before it: it takes that version's type, and its title unless it
 *  has one. A failed update has the content before it and an `error` (only
 *  once it is complete: while it streams, the edits so far are shown). */
export function resolveVersions(list) {
  const out = [];
  for (const v of list) {
    const base = out[out.length - 1];
    if (!v.update) { out.push(v); continue; }
    if (!base) { out.push({ ...v, error: v.complete ? 'There is no earlier version of this artifact to update.' : undefined }); continue; }
    const r = applyEdits(base.content, v.content);
    out.push({
      ...base,
      identifier: v.identifier,
      version: v.version,
      complete: v.complete,
      interrupted: v.interrupted,
      update: true,
      title: v.title || base.title,
      content: r.content,
      error: v.complete ? r.error : undefined,
    });
  }
  return out;
}
