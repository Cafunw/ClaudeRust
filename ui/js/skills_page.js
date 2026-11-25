// Settings → Skills: the skills in ~/.claude/skills, in a session folder's
// .claude/skills and in Claude Code plugins (skills.rs); on/off, install
// from a .zip/.skill after a look at what it holds, remove, and "Create
// skill" as a drafted chat.
import { t } from './i18n.js';
import { icon } from './icons.js';
import { call } from './api.js';
import { store, emit } from './store.js';
import { esc, h, modal, button, toast, errorToast, confirmDialog, promptDialog, humanSize, hasWords } from './util.js';
import { notRegistered } from './optional_cmd.js';
import { errorText } from './neterr.js';

const SCRIPT = /\.(py|sh|bash|zsh|js|mjs|cjs|ts|ps1|psm1|bat|cmd|vbs|rb|pl|php|exe|dll|so|dylib|bin|jar)$/i;

/** The files of a skill (paths, or {path}) that are programs or scripts. */
export function scriptFiles(files) {
  return (files || []).map((f) => (typeof f === 'string' ? f : f.path)).filter((p) => p && (SCRIPT.test(p) || /(^|[\\/])scripts[\\/]./i.test(p)));
}

/** The folders of Code sessions, the open conversation's first, each once. */
export function sessionFolders(convs, currentId) {
  const withCwd = (convs || []).filter((c) => c.cwd);
  const cur = withCwd.find((c) => c.id === currentId);
  return [...new Set([...(cur ? [cur.cwd] : []), ...withCwd.map((c) => c.cwd)])];
}

const folderName = (p) => String(p).split(/[\\/]/).filter(Boolean).pop() || p;

/** The skills whose name or description hold every word of `q` (any case, Turkish İ/ı too). */
export const filterSkills = (skills, q) => skills.filter((s) => hasWords(`${s.name} ${s.description || ''}`, q));

/** `skills` by where they come from, in this order, the groups that have some: [[source, skills]]. */
export function groupSkills(skills) {
  const of = (s) => (s.source === 'project' || s.source === 'plugin' ? s.source : 'personal');
  return ['personal', 'project', 'plugin'].map((g) => [g, skills.filter((s) => of(s) === g)]).filter(([, l]) => l.length);
}

/** Settings → Skills; `tab.close()` closes Settings (for "Create skill"). */
export async function renderSkills(body, tab) {
  body.appendChild(h(`<h3>${esc(t('sk_title'))}</h3>`));
  body.appendChild(h(`<p class="muted" style="margin:-6px 0 12px">${esc(t('sk_desc'))}</p>`));
  body.appendChild(h(`<div class="notice">${icon('info', 15)}<div class="grow small">${esc(t('sk_where'))}</div></div>`));
  const actions = h('<div class="row" style="margin-bottom:12px;flex-wrap:wrap"></div>');
  const folders = sessionFolders(store.conversations, store.route.params && store.route.params.id);
  const pick = h(`<select class="select" style="width:auto;max-width:min(260px,100%);min-width:0" aria-label="${esc(t('sk_folder'))}"><option value="">${esc(t('sk_no_folder'))}</option>${folders.map((f, i) => `<option value="${esc(f)}" ${i === 0 ? 'selected' : ''} title="${esc(f)}">${esc(folderName(f))}</option>`).join('')}</select>`);
  // (Esc empties it first, then closes Settings: a11y.js, as in every search field)
  const filter = h(`<div class="search-box sk-filter">${icon('search', 15)}<input class="input" type="search" placeholder="${esc(t('sk_filter'))}" aria-label="${esc(t('sk_filter'))}"></div>`);
  const input = filter.querySelector('input');
  const list = h('<div class="sk-list"></div>');
  body.append(actions, filter, list);
  const project = () => pick.value || null;
  let skills = [];
  const show = () => {
    list.replaceChildren();
    if (!skills.length) { list.appendChild(h(`<div class="empty-state" style="padding:30px">${icon('book', 28)}<div style="margin-top:8px">${esc(t('sk_none'))}</div></div>`)); return; }
    const shown = filterSkills(skills, input.value);
    if (!shown.length) { list.appendChild(h(`<div class="empty-state" role="status" style="padding:30px">${esc(t('sk_no_match', { q: input.value.trim() }))}</div>`)); return; }
    for (const [g, group] of groupSkills(shown)) {
      const label = g === 'project' ? t('sk_src_project', { dir: folderName(project() || '') }) : g === 'plugin' ? t('sk_group_plugin') : t('sk_src_user');
      list.appendChild(h(`<h4 class="sk-group">${esc(label)} <span class="muted">${group.length}</span></h4>`));
      for (const s of group) list.appendChild(card(s, project(), draw));
    }
    clampDescriptions(list);
  };
  input.addEventListener('input', show);

  // the folder picked last wins: an earlier, slower list is dropped
  let drawn = 0;
  const draw = async () => {
    const mine = ++drawn;
    list.innerHTML = '<div style="padding:6px 0"><span class="spinner"></span></div>';
    try { skills = await call('skills_list', { project: project() }); } catch (e) {
      if (mine !== drawn) return;
      list.replaceChildren(h(`<div class="empty-state" style="padding:30px">${esc(notRegistered(e) ? t('sk_unavailable') : errorText(e))}</div>`));
      actions.querySelectorAll('button').forEach((b) => { b.disabled = notRegistered(e); });
      return;
    }
    if (mine !== drawn) return;
    show();
  };
  actions.append(
    button(t('sk_install'), 'primary', () => installSkill(project(), draw)),
    button(t('sk_create'), '', () => createSkill(tab)),
  );
  if (folders.length) {
    const w = h(`<label class="row small muted" style="gap:6px;margin-left:auto;min-width:0;max-width:100%">${esc(t('sk_folder'))}</label>`);
    w.appendChild(pick);
    actions.appendChild(w);
  }
  pick.addEventListener('change', draw);
  await draw();
}

/** About two lines of a description in a wide Settings: the start of a longer one, cut at a word, is
 *  what a card shows (and a screen reader reads) until it is opened; CSS keeps it to two lines when narrower. */
export function shortDescription(text, max = 170) {
  if (text.length <= max) return text;
  const at = text.lastIndexOf(' ', max);
  return `${text.slice(0, at > max / 2 ? at : max).trimEnd()}…`;
}

/** Each description in `root` (its whole text in `__full`) is two lines at most; one cut short gets a button that shows it all. */
function clampDescriptions(root) {
  // (measured once drawn; shortcut: a later resize of Settings doesn't add or drop a button, the next draw does)
  requestAnimationFrame(() => root.querySelectorAll('.sk-desc').forEach((d) => {
    const short = d.textContent;
    if (short === d.__full && d.scrollHeight <= d.clientHeight + 1) return;
    const b = h(`<button class="sk-more" type="button" aria-expanded="false" aria-controls="${d.id}">${esc(t('sk_more'))}</button>`);
    b.onclick = () => {
      const open = d.classList.toggle('open');
      d.textContent = open ? d.__full : short;
      b.setAttribute('aria-expanded', String(open));
      b.textContent = t(open ? 'sk_less' : 'sk_more');
    };
    d.after(b);
  }));
}

let descId = 0;

function card(s, project, redraw) {
  // plugin skills belong to the CLI's plugin manager (the group says where the others come from)
  const own = s.source !== 'plugin';
  const c = h(`<div class="server"><div class="head">${icon('book', 16)}<div class="grow" style="min-width:0">
    <div style="font-weight:600;overflow-wrap:anywhere">${esc(s.name)}${own ? '' : ` <span class="cd-cat">${esc(t('sk_src_plugin', { name: s.plugin || '' }))}</span>`}</div>
    <div class="small muted sk-desc" id="sk-desc-${++descId}">${esc(shortDescription(s.description || ''))}</div></div>
    ${own ? `<button class="icon-btn sm" data-a="remove" title="${esc(t('sk_remove'))}" aria-label="${esc(t('sk_remove'))}: ${esc(s.name)}">${icon('trash', 15)}</button>
    <label class="switch" title="${esc(t('enabled'))}"><input type="checkbox" aria-label="${esc(t('enabled'))}: ${esc(s.name)}" ${s.enabled === false ? '' : 'checked'}><span class="track"></span></label>` : ''}</div></div>`);
  c.querySelector('.sk-desc').__full = s.description || '';
  if (!own) return c;
  const where = { name: s.name, scope: s.source, project };
  c.querySelector('input').addEventListener('change', async (e) => {
    try { await call('skill_set_enabled', { ...where, enabled: e.target.checked }); } catch (err) { e.target.checked = !e.target.checked; errorToast(err); }
  });
  c.querySelector('[data-a=remove]').onclick = async () => {
    if (!(await confirmDialog(t('sk_remove_confirm', { name: s.name }), { okLabel: t('sk_remove') }))) return;
    try { await call('skill_uninstall', where); toast(t('sk_removed', { name: s.name })); } catch (e) { errorToast(e); }
    redraw();
  };
  return c;
}

/** Pick a .zip/.skill, show what it holds, and install exactly that file only on a second, explicit click. */
async function installSkill(project, redraw) {
  const path = await call('skill_pick_file');
  if (!path) return;
  const body = h('<div></div>');
  const into = h(`<select class="select" style="width:auto" aria-label="${esc(t('sk_into'))}"><option value="personal">${esc(t('sk_src_user'))}</option>${project ? `<option value="project">${esc(t('sk_src_project', { dir: folderName(project) }))}</option>` : ''}</select>`);
  let p = null;
  const ok = button(t('install'), 'primary', async () => {
    ok.disabled = true;
    try {
      const r = await call('skill_install', { path, scope: into.value, project, sha256: p.sha256 });
      toast(t('sk_installed', { name: (r && r.name) || p.name }));
      m.close();
      redraw();
    } catch (e) { errorToast(e); ok.disabled = false; }
  });
  const show = async () => {
    ok.disabled = true;
    try { p = await call('skill_install_preview', { path, scope: into.value, project }); } catch (e) { errorToast(e); m.close(); return; }
    const files = p.files || [];
    const scripts = scriptFiles(files);
    body.replaceChildren(h(`<div>
      <div class="notice warn">${icon('shield', 15)}<div class="grow small">${esc(t('sk_trust'))}</div></div>
      ${scripts.length ? `<div class="notice error">${icon('alert', 15)}<div class="grow small">${esc(t('sk_scripts', { list: scripts.join(', ') }))}</div></div>` : ''}
      ${p.exists ? `<div class="notice error">${icon('alert', 15)}<div class="grow small">${esc(t('sk_exists', { name: p.name }))}</div></div>` : ''}
      <p style="margin:0 0 4px;font-weight:600">${esc(p.name || '')}</p>
      ${p.description ? `<p class="small" style="margin:0 0 12px">${esc(p.description)}</p>` : ''}
      <div class="row small" style="gap:8px;margin-bottom:10px"><span class="muted">${esc(t('sk_into'))}</span></div>
      <div class="label">SKILL.md</div><pre class="sk-md mono small"></pre>
      <div class="label" style="margin-top:12px">${esc(t('sk_files', { n: files.length }))}${p.total_size != null ? ` · ${esc(humanSize(p.total_size))}` : ''}</div>
      <ul class="sk-files mono small">${files.map((f) => `<li>${esc(f.path)} <span class="muted">${esc(humanSize(f.size || 0))}</span></li>`).join('')}</ul>
      ${p.target ? `<p class="small muted mono" style="margin:8px 0 0;word-break:break-all">${esc(p.target)}</p>` : ''}</div>`));
    body.querySelector('.row.small').appendChild(into);
    body.querySelector('.sk-md').textContent = p.skill_md || '';
    ok.disabled = !!p.exists;
  };
  into.addEventListener('change', show);
  const m = modal({ title: t('sk_install_title'), body, wide: true, footer: [button(t('cancel'), '', () => m.close()), ok] });
  await show();
}

/** "Create skill": a new chat that asks Claude to draft the SKILL.md. */
async function createSkill(tab) {
  const what = await promptDialog(t('sk_create_title'), '', { multiline: true, placeholder: t('sk_create_ph'), okLabel: t('sk_create_go') });
  if (!what || !what.trim()) return;
  tab.close();
  emit('navigate', { name: 'new', params: { text: t('sk_create_msg', { what: what.trim() }) } });
}
