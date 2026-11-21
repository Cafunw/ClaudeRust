// Line diffs of the file tools' inputs: the rows a tool card draws and the
// +/- counts of the "N files edited" summary under a code turn.
import { relPath } from './util.js';

/** Rows (['ctx'|'del'|'add', line]) between two texts: their common first
 *  and last lines trimmed, with two lines of context either side. */
export function lineDiff(oldS, newS) {
  const a = String(oldS || '').split('\n');
  const b = String(newS || '').split('\n');
  let s = 0;
  while (s < a.length && s < b.length && a[s] === b[s]) s++;
  let ea = a.length - 1, eb = b.length - 1;
  while (ea >= s && eb >= s && a[ea] === b[eb]) { ea--; eb--; }
  const out = [];
  const ctxStart = Math.max(0, s - 2);
  for (let i = ctxStart; i < s; i++) out.push(['ctx', a[i]]);
  for (let i = s; i <= ea; i++) out.push(['del', a[i]]);
  for (let i = s; i <= eb; i++) out.push(['add', b[i]]);
  for (let i = ea + 1; i < Math.min(a.length, ea + 3); i++) out.push(['ctx', a[i]]);
  return out;
}

const EDIT_TOOLS = new Set(['Write', 'Edit', 'MultiEdit', 'NotebookEdit']);
const lineCount = (s) => (s ? String(s).replace(/\n$/, '').split('\n').length : 0);

function addEdit(f, oldS, newS) {
  for (const [k] of lineDiff(oldS, newS)) {
    if (k === 'add') f.added++;
    else if (k === 'del') f.removed++;
  }
}

/** Files a turn wrote or edited, relative to `cwd`, with the +/- line counts
 *  its tool cards show (a Write counts all its lines as added). */
export function editedFiles(parts, cwd) {
  const files = new Map();
  for (const p of parts || []) {
    if (p.kind !== 'tool' || !EDIT_TOOLS.has(p.name) || p.status === 'error' || p.status === 'cancelled' || p.status === 'denied') continue;
    if (p.result && p.result.is_error) continue;
    const inp = p.input || {};
    const abs = inp.file_path || inp.notebook_path;
    if (!abs) continue;
    const rel = relPath(String(abs), cwd);
    const f = files.get(rel) || { path: rel, added: 0, removed: 0, created: false };
    if (/File created successfully/.test((p.result && p.result.text) || '')) f.created = true;
    if (p.name === 'Write') f.added += lineCount(inp.content);
    else if (p.name === 'Edit') addEdit(f, inp.old_string, inp.new_string);
    else if (p.name === 'MultiEdit') for (const e of inp.edits || []) addEdit(f, e.old_string, e.new_string);
    files.set(rel, f);
  }
  return [...files.values()];
}
