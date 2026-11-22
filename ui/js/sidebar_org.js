// The sidebar's own organisation (sidebar_cmds.rs): pins, the archive,
// unread marks and sections, and moving a conversation into a section by
// dragging it there.
import { call } from './api.js';
import { notRegistered } from './optional_cmd.js';

/** The lists the sidebar draws from this mode's `convs`: pinned (newest pin
 *  first), each section's, starred, and the rest. A conversation is in one
 *  of them only, in that order of precedence. */
export function groupConversations(convs, sections) {
  const known = new Set(sections.map((s) => s.id));
  const pinned = convs.filter((c) => c.pinned_at).sort((a, b) => b.pinned_at - a.pinned_at);
  const free = convs.filter((c) => !c.pinned_at);
  const inSection = (c) => c.section_id && known.has(c.section_id);
  return {
    pinned,
    sections: sections.map((s) => ({ section: s, items: free.filter((c) => c.section_id === s.id) })),
    starred: free.filter((c) => c.starred && !inSection(c)),
    recents: free.filter((c) => !c.starred && !inSection(c)),
  };
}

/** Where a conversation dropped on `zone` (a drop zone's section id, '' for
 *  outside any section, undefined for no zone) goes: a section id, null to
 *  take it out of its section, or undefined when nothing changes. */
export function dropTarget(from, zone) {
  if (zone === undefined) return undefined;
  const to = zone || null;
  return to === (from || null) ? undefined : to;
}

/** Whether a press that moved (dx, dy) is a drag rather than a click. */
export const isDrag = (dx, dy) => dx * dx + dy * dy >= 36;

// ---- the backend (null where this build doesn't have it)

let supported = null;

/** This mode's sections, or null when the backend has none (an older build). */
export async function loadSections(kind) {
  if (supported === false) return null;
  try {
    const list = await call('sections_list', { kind });
    supported = true;
    return list;
  } catch (e) {
    if (notRegistered(e)) supported = false;
    return supported ? [] : null;
  }
}

export const orgSupported = () => supported === true;

export const setPinned = (id, pinned) => call('conversation_set_pinned', { id, pinned });
export const setArchived = (id, archived) => call('conversation_set_archived', { id, archived });
export const setUnread = (id, unread) => call('conversation_set_unread', { id, unread });
export const setSection = (id, sectionId) => call('conversation_set_section', { id, sectionId });
export const createSection = (kind, name) => call('section_create', { kind, name });
export const renameSection = (id, name) => call('section_rename', { id, name });
export const deleteSection = (id) => call('section_delete', { id });
export const listArchived = (kind) => call('list_archived_conversations', { kind });

/** Tell the backend which conversation is on screen (null: none), so it marks it read. */
export function opened(id) {
  if (supported !== false) call('conversation_opened', { id: id || null }).catch(() => {});
}

// ---- dragging a conversation onto a section (pointer events: the WebView's
// own drag and drop is taken by native file drops on Windows)

/** Make `link` (a conversation's row) draggable onto `[data-drop]` zones;
 *  `onDrop(zone)` gets the zone's section id ('' for none). */
export function draggable(link, onDrop) {
  link.addEventListener('pointerdown', (e) => {
    if (e.button !== 0 || e.target.closest('.more')) return;
    const x0 = e.clientX;
    const y0 = e.clientY;
    let ghost = null;
    let over = null;
    const zoneAt = (x, y) => { const el = document.elementFromPoint(x, y); return el && el.closest('[data-drop]'); };
    const move = (ev) => {
      if (!ghost && !isDrag(ev.clientX - x0, ev.clientY - y0)) return;
      if (!ghost) {
        ghost = link.cloneNode(true);
        ghost.classList.add('drag-ghost');
        ghost.style.width = `${link.offsetWidth}px`;
        document.body.appendChild(ghost);
        link.classList.add('dragging');
      }
      ghost.style.transform = `translate(${ev.clientX + 8}px, ${ev.clientY + 4}px)`;
      const z = zoneAt(ev.clientX, ev.clientY);
      if (z !== over) { if (over) over.classList.remove('drop-over'); over = z; if (over) over.classList.add('drop-over'); }
    };
    const up = () => {
      document.removeEventListener('pointermove', move);
      document.removeEventListener('pointerup', up);
      document.removeEventListener('pointercancel', up);
      if (!ghost) return;
      ghost.remove();
      link.classList.remove('dragging');
      if (over) over.classList.remove('drop-over');
      // the click that ends a drag (it comes before any timer) doesn't open the conversation
      const block = (ev) => { ev.stopPropagation(); ev.preventDefault(); };
      link.addEventListener('click', block, true);
      setTimeout(() => link.removeEventListener('click', block, true));
      onDrop(over ? over.dataset.drop : undefined);
    };
    document.addEventListener('pointermove', move);
    document.addEventListener('pointerup', up);
    document.addEventListener('pointercancel', up);
  });
}
