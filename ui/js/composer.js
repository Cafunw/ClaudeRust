// The message composer: text input, attachments, model / tools menus.
import * as keymap from './keymap.js';
import * as slash from './slash.js';
import { attachSuggest } from './suggest.js';
import { icon } from './icons.js';
import { t, tOr } from './i18n.js';
import { call } from './api.js';
import { store, modelInfo, emit } from './store.js';
import { $, esc, h, showMenu, closeMenu, autoGrow, fileToBase64, attachmentSub, errorToast, toast } from './util.js';

const PASTE_AS_FILE = 6000;

// What the Claude Code CLI applies by itself for a model in a folder: the effort it runs at when none is chosen,
// whether Ultracode can be switched on, and whether its settings switch it on. Asked once per model and folder (it costs a CLI start): undefined
// until it answers, {} when it couldn't say.
const ccInfo = new Map();
function ccInfoOf(model, cwd, onReady) {
  const key = `${model}\n${cwd || ''}`;
  let e = ccInfo.get(key);
  if (!e) {
    e = { info: undefined };
    e.p = call('claude_code_session_info', { model, cwd: cwd || null }).catch(() => ({})).then((r) => { e.info = r || {}; });
    ccInfo.set(key, e);
  }
  if (e.info === undefined && onReady) e.p.then(onReady);
  return e.info;
}

export function thinkingSupported(m) {
  return ['disableable', 'between_tools', 'explicit', 'budget'].includes(m.thinking);
}

export function createComposer(opts) {
  const mode = opts.mode || 'chat';
  const st = {
    model: store.settings.default_model,
    thinking: mode === 'code' ? true : !!store.settings.thinking_default,
    effort: null,
    // null until chosen: the CLI's settings decide
    ultracode: null,
    web_search: !!store.settings.web_search_default,
    style: 'normal',
    disabled_servers: [],
    permission_mode: store.settings.code_permission_mode || 'default',
    ...(opts.initial || {}),
  };
  let attachments = [];
  let running = false;
  // Claude Code is idle while a workflow runs: what you type goes straight in instead of queueing
  let waiting = false;
  let busy = false;

  const el = h(`<div class="composer" data-mode="${mode}">
    <div class="attachments hidden"></div>
    <div class="cbox">
      <textarea rows="1" spellcheck="true" placeholder="${esc(opts.placeholder || t('help_today'))}"></textarea>
      <button class="send-btn" data-act="send" aria-label="${t('send')}" disabled>${icon('arrowUp', 16)}</button>
    </div>
    <div class="composer-bar">
      <button class="tool-btn" data-act="attach" title="${t('attach')}" aria-label="${t('attach')}">${icon('plus', 18)}</button>
      <button class="tool-btn" data-act="tools" title="${t('tools')}" aria-label="${t('tools')}">${icon('sliders', 16)}</button>
      ${mode === 'code' ? '<button class="mode-btn" data-act="mode"></button>' : ''}
      <div class="active-tools"></div>
      <div class="grow composer-note"></div>
      <button class="model-btn" data-act="model"></button>
    </div>
  </div>`);
  const ta = $('textarea', el);
  if (mode === 'chat') $('.composer-note', el).textContent = t('disclaimer');
  const sendBtn = $('[data-act=send]', el);
  const attEl = $('.attachments', el);
  const activeEl = $('.active-tools', el);

  function persist(patch) {
    Object.assign(st, patch);
    refreshBar();
    opts.onOptions && opts.onOptions(patch);
  }

  // Claude Code's own answer for the model (see ccInfoOf); undefined when this isn't a Claude Code session
  const ccInfoNow = (m) => {
    const c = (opts.context && opts.context()) || {};
    return mode === 'code' && c.engine === 'claude-code' ? ccInfoOf(m.id, c.cwd, refreshBar) : undefined;
  };
  // Ultracode is a Claude Code CLI option, for models that support it and while the CLI says it can be on
  // (it can't with Dynamic workflows off)
  const ultracodeOffered = (m) => mode === 'code' && ((opts.context && opts.context()) || {}).engine === 'claude-code' && !!m.ultracode && (ccInfoNow(m) || {}).ultracode_available !== false;
  // the choice made here, else whether the CLI's settings turn it on for every session
  const ultracodeOn = (m) => (st.ultracode ?? (ccInfoNow(m) || {}).ultracode) === true;
  // the level a turn runs at: the one chosen, else the one the CLI really uses (it knows the account's defaults), else ours
  const effortOf = (m) => {
    const levels = m.efforts || [];
    if (!levels.length) return null;
    const own = (ccInfoNow(m) || {}).effort;
    return levels.includes(st.effort) ? st.effort : (levels.includes(own) ? own : (m.default_effort || 'high'));
  };

  function refreshBar() {
    const m = modelInfo(st.model);
    ccInfoNow(m); // asks the CLI now, so the model menu has its answer when it opens
    const thinkOn = m.thinking === 'always_on' || (thinkingSupported(m) && st.thinking);
    const ucOn = ultracodeOn(m) && ultracodeOffered(m);
    $('[data-act=model]', el).innerHTML = `${esc(m.name)}${thinkOn && m.thinking !== 'always_on' ? ` <span class="think-flag" title="${t('extended_thinking')}">${icon('bulb', 13)}</span>` : ''}${ucOn ? ` <span class="think-flag" title="${esc(t('ultracode_d'))}">${esc(t('ultracode'))}</span>` : ''} ${icon('chevDown', 14)}`;
    const toolsBtn = $('[data-act=tools]', el);
    const anyOn = (mode === 'chat' && st.web_search) || (st.style && st.style !== 'normal');
    toolsBtn.classList.toggle('on', !!anyOn);
    activeEl.innerHTML = '';
    if (mode === 'chat' && st.web_search) {
      const c = h(`<span class="active-tool">${icon('globe', 13)} ${t('web_search')} <button aria-label="off">${icon('x', 12)}</button></span>`);
      c.querySelector('button').onclick = () => persist({ web_search: false });
      activeEl.appendChild(c);
    }
    if (st.style && st.style !== 'normal') {
      const c = h(`<span class="active-tool">${icon('feather', 13)} ${esc(styleName(st.style))} <button aria-label="off">${icon('x', 12)}</button></span>`);
      c.querySelector('button').onclick = () => persist({ style: 'normal' });
      activeEl.appendChild(c);
    }
    const mb = $('[data-act=mode]', el);
    if (mb) {
      mb.className = `mode-btn ${st.permission_mode}`;
      const ic = { default: 'shield', auto: 'sparkle', acceptEdits: 'edit', plan: 'compass', bypassPermissions: 'zap' }[st.permission_mode] || 'shield';
      mb.innerHTML = `${icon(ic, 14)} ${esc(t('mode_' + st.permission_mode))}`;
    }
    updateSend();
  }

  function styleName(id) {
    const builtin = { normal: t('style_normal'), concise: t('style_concise'), explanatory: t('style_explanatory'), formal: t('style_formal') };
    if (builtin[id]) return builtin[id];
    const c = (store.settings.custom_styles || []).find((s) => s.id === id);
    return c ? c.name : id;
  }

  // the button's icon, replaced only when it changes: updateSend runs on
  // every keystroke, and new markup is new SVG to lay out each time
  function sendIcon(name) {
    if (sendBtn.__icon === name) return;
    sendBtn.__icon = name;
    sendBtn.innerHTML = icon(name, 16);
  }

  function updateSend() {
    if (running && !waiting && opts.onQueue && ta.value.trim()) {
      sendBtn.disabled = false;
      sendBtn.classList.remove('stop');
      sendBtn.classList.add('queue');
      sendIcon('arrowUp');
      sendBtn.setAttribute('aria-label', t('queue_send'));
      sendBtn.title = t('queue_send');
      return;
    }
    sendBtn.classList.remove('queue');
    sendBtn.title = '';
    if (running && !(waiting && (ta.value.trim() || attachments.length))) {
      sendBtn.disabled = false;
      sendBtn.classList.add('stop');
      sendIcon('stop');
      sendBtn.setAttribute('aria-label', t('stop'));
      return;
    }
    sendBtn.classList.remove('stop');
    sendIcon('arrowUp');
    sendBtn.setAttribute('aria-label', t('send'));
    sendBtn.disabled = busy || (!ta.value.trim() && !attachments.length) || attachments.some((a) => a.loading);
  }

  function renderAttachments() {
    attEl.classList.toggle('hidden', attachments.length === 0);
    attEl.innerHTML = '';
    for (const a of attachments) {
      const ext = (a.name.split('.').pop() || '').slice(0, 4);
      const chip = h(`<div class="att ${a.loading ? 'loading' : ''}" title="${esc(a.name)}">
        ${a.preview ? `<img class="thumb" src="${esc(a.preview)}" alt="">` : `<div class="ficon">${a.kind === 'pdf' ? 'PDF' : esc(ext || 'TXT')}</div>`}
        <div class="info"><div class="n">${esc(a.name)}</div><div class="s">${a.loading ? '…' : attachmentSub(a)}</div></div>
        <button class="x" aria-label="remove">${icon('x', 12)}</button></div>`);
      chip.querySelector('.x').onclick = () => {
        attachments = attachments.filter((x) => x !== a);
        if (a.id) call('unstage', { id: a.id }).catch(() => {});
        renderAttachments();
      };
      attEl.appendChild(chip);
    }
    updateSend();
  }

  async function addPaths(paths) {
    if (!paths || !paths.length) return;
    const placeholders = paths.map((p) => ({ name: p.split(/[\\/]/).pop(), loading: true, size: 0 }));
    attachments.push(...placeholders);
    renderAttachments();
    try {
      const r = await call('stage_paths', { paths });
      attachments = attachments.filter((a) => !placeholders.includes(a));
      attachments.push(...r.attachments);
      for (const e of r.errors) toast(e, 'error');
    } catch (e) {
      attachments = attachments.filter((a) => !placeholders.includes(a));
      errorToast(e);
    }
    renderAttachments();
  }

  // the largest attachment the backend takes (text, before its digest);
  // checked before reading the whole file into the window
  const MAX_ATTACHMENT = 40 * 1024 * 1024;

  async function addFile(file, nameOverride) {
    if (file.size > MAX_ATTACHMENT) { errorToast(new Error(`${t('file_too_large')}: ${file.name || ''}`)); return; }
    const ph = { name: nameOverride || file.name || 'file', loading: true, size: file.size };
    attachments.push(ph);
    renderAttachments();
    try {
      const data = await fileToBase64(file);
      const a = await call('stage_bytes', { name: ph.name, data });
      const at = attachments.indexOf(ph);
      // removed while it was loading: drop it in the backend too
      if (at === -1) { call('unstage', { id: a.id }).catch(() => {}); return; }
      attachments[at] = a;
    } catch (e) {
      attachments = attachments.filter((x) => x !== ph);
      errorToast(e);
    }
    renderAttachments();
  }

  function addStaged(a) { attachments.push(a); renderAttachments(); }

  function cmdContext() {
    const c = (opts.context && opts.context()) || {};
    return { kind: mode, composer: api, ...c };
  }

  /** Send (or queue) what's in the box. Resolves to whether it went: sent,
   *  queued or handled as a command (the box is then cleared); false when
   *  nothing was sent (empty, busy, stopped, failed). */
  async function send() {
    if (running && !(waiting && (ta.value.trim() || attachments.length))) {
      // typing while Claude works queues the message (sent when it finishes)
      if (ta.value.trim() && opts.onQueue && !attachments.some((a) => a.loading)) {
        opts.onQueue(ta.value, attachments.slice());
        ta.value = ''; attachments = []; renderAttachments(); autoGrow(ta);
        if (opts.draftKey) store.drafts.delete(opts.draftKey);
        updateSend();
        return true;
      }
      opts.onStop && opts.onStop();
      return false;
    }
    let text = ta.value;
    if ((!text.trim() && !attachments.length) || busy || attachments.some((a) => a.loading)) return false;
    busy = true; updateSend();
    const atts = attachments.slice();
    try {
      if (mode === 'code' && /^#\s+\S/.test(text) && !text.trim().includes('\n') && !atts.length) {
        const r = await slash.handleHash(text.trim(), cmdContext());
        if (r === true) { ta.value = ''; autoGrow(ta); if (opts.draftKey) store.drafts.delete(opts.draftKey); return true; }
        if (r === null) return false;
      }
      if (text.trim().startsWith('/') && !atts.length) {
        const r = await slash.handle(text, cmdContext());
        if (r && r.handled) {
          ta.value = ''; autoGrow(ta);
          if (opts.draftKey) store.drafts.delete(opts.draftKey);
          return true;
        }
        if (r && r.keep) return false;
        if (r && r.prompt) text = r.prompt;
        if (r && r.attachments && r.attachments.length) atts.push(...r.attachments);
      }
      const ok = await opts.onSend(text, atts, { ...st });
      if (ok === false) return false;
      ta.value = '';
      attachments = [];
      renderAttachments();
      autoGrow(ta);
      if (opts.draftKey) store.drafts.delete(opts.draftKey);
      return true;
    } catch (e) {
      errorToast(e);
      return false;
    } finally {
      busy = false;
      updateSend();
    }
  }

  // ---- menus
  function modelMenu(anchor) {
    const m = modelInfo(st.model);
    const avail = store.models.filter((x) => x.available !== false);
    const featured = avail.filter((x) => x.featured);
    const others = avail.filter((x) => !x.featured);
    const items = [];
    for (const x of featured) items.push({ label: x.name, sub: tOr(`model_d_${x.id}`, x.description), checked: x.id === st.model, onClick: () => { persist({ model: x.id }); } });
    if (others.length) {
      items.push({
        label: t('more_models'), icon: 'chevRight', keepOpen: true,
        onClick: () => {
          showMenu(anchor, [
            { label: t('model'), icon: 'chevLeft', keepOpen: true, onClick: () => modelMenu(anchor) },
            { sep: true },
            ...others.map((x) => ({ label: x.name, sub: x.id, checked: x.id === st.model, onClick: () => persist({ model: x.id }) })),
          ], { placement: 'top-end', width: 280 });
        },
      });
    }
    const uc = ultracodeOffered(m);
    if (thinkingSupported(m) || (m.efforts && m.efforts.length) || uc) items.push({ sep: true });
    if (thinkingSupported(m)) {
      const row = h(`<label class="menu-item" style="cursor:pointer">${icon('bulb', 16)}<span class="mtext">${t('extended_thinking')}<span class="sub">${m.thinking === 'budget' ? '' : ''}</span></span><span class="switch"><input type="checkbox" ${st.thinking ? 'checked' : ''}><span class="track"></span></span></label>`);
      row.querySelector('input').addEventListener('change', (e) => { persist({ thinking: e.target.checked }); });
      items.push({ element: row });
    }
    if (m.efforts && m.efforts.length) {
      const cur = effortOf(m);
      const seg = h(`<div style="padding:6px 10px 8px"><div class="small muted" style="margin-bottom:6px">${t('effort')}</div><div class="segmented">${m.efforts.map((e) => `<button data-e="${e}" class="${e === cur ? 'on' : ''}">${t('effort_' + e)}</button>`).join('')}</div></div>`);
      seg.querySelectorAll('button').forEach((b) => b.addEventListener('click', () => {
        seg.querySelectorAll('button').forEach((x) => x.classList.toggle('on', x === b));
        persist({ effort: b.dataset.e });
      }));
      items.push({ element: seg });
    }
    if (uc) {
      const row = h(`<label class="menu-item" style="cursor:pointer">${icon('sparkle', 16)}<span class="mtext">${esc(t('ultracode'))}<span class="sub">${esc(t('ultracode_d'))}</span></span><span class="switch"><input type="checkbox" ${ultracodeOn(m) ? 'checked' : ''}><span class="track"></span></span></label>`);
      row.querySelector('input').addEventListener('change', (e) => { persist({ ultracode: e.target.checked }); });
      items.push({ element: row });
    }
    showMenu(anchor, items, { placement: 'top-end', width: 300 });
  }

  async function improve() {
    const before = ta.value;
    if (!before.trim() || busy) return;
    busy = true; updateSend();
    ta.classList.add('improving');
    try {
      const out = await call('improve_prompt', { text: before, code: mode === 'code' });
      if (ta.value === before) {
        ta.value = out; autoGrow(ta); updateSend(); saveDraft();
        toast(t('improved'), '', { action: t('undo'), onAction: () => { ta.value = before; autoGrow(ta); updateSend(); saveDraft(); ta.focus(); } });
      }
    } catch (e) { errorToast(e); } finally {
      busy = false; ta.classList.remove('improving'); updateSend(); ta.focus();
    }
  }

  function toolsMenu(anchor) {
    const items = [];
    items.push({ label: t('improve_msg'), sub: t('improve_msg_d'), icon: 'sparkle', disabled: !ta.value.trim() || busy, onClick: improve }, { sep: true });
    if (mode === 'chat') {
      const row = h(`<label class="menu-item" style="cursor:pointer">${icon('globe', 16)}<span class="mtext">${t('web_search')}</span><span class="switch"><input type="checkbox" ${st.web_search ? 'checked' : ''}><span class="track"></span></span></label>`);
      row.querySelector('input').addEventListener('change', (e) => persist({ web_search: e.target.checked }));
      items.push({ element: row });
      items.push({ sep: true }, { header: t('style') });
      const styles = [['normal', t('style_normal')], ['concise', t('style_concise')], ['explanatory', t('style_explanatory')], ['formal', t('style_formal')],
        ...(store.settings.custom_styles || []).map((s) => [s.id, s.name])];
      for (const [id, name] of styles) items.push({ label: name, checked: (st.style || 'normal') === id, onClick: () => persist({ style: id }) });
      items.push({ label: t('create_style'), icon: 'edit', onClick: () => emit('open-settings', 'profile') });
    }
    // Claude Code sessions use the CLI's own MCP config, not the app's servers
    const ccCode = mode === 'code' && ((opts.context && opts.context()) || {}).engine === 'claude-code';
    const servers = ccCode ? [] : store.mcpServers.filter((s) => s.status === 'connected');
    if (servers.length) {
      items.push({ sep: true }, { header: 'MCP' });
      for (const s of servers) {
        const on = !(st.disabled_servers || []).includes(s.name);
        const row = h(`<label class="menu-item" style="cursor:pointer">${icon('plug', 16)}<span class="mtext">${esc(s.name)}<span class="sub">${t('tools_count', { n: s.tools.length })}</span></span><span class="switch"><input type="checkbox" ${on ? 'checked' : ''}><span class="track"></span></span></label>`);
        row.querySelector('input').addEventListener('change', (e) => {
          const set = new Set(st.disabled_servers || []);
          if (e.target.checked) set.delete(s.name); else set.add(s.name);
          persist({ disabled_servers: [...set] });
        });
        items.push({ element: row });
      }
    }
    if (!items.length) items.push({ label: t('nothing_here'), disabled: true });
    showMenu(anchor, items, { placement: 'top-start', width: 280 });
  }

  async function attachMenu(anchor) {
    const items = [
      { label: t('upload_file'), icon: 'paperclip', onClick: async () => { const paths = await call('pick_files'); addPaths(paths); } },
      { label: t('add_screenshot'), icon: 'image', disabled: true },
    ];
    if (store.mcpServers.some((s) => s.status === 'connected')) {
      items.push({ sep: true }, {
        label: t('prompts_resources'), icon: 'plug', keepOpen: true,
        onClick: async () => {
          try {
            const r = await call('mcp_list_prompts');
            const sub = [];
            for (const p of r.prompts) sub.push({ label: p.name, sub: `${p.server}${p.description ? ' · ' + p.description : ''}`, icon: 'zap', onClick: () => usePrompt(p) });
            for (const res of r.resources) sub.push({ label: res.name || res.uri, sub: res.server, icon: 'fileText', onClick: async () => { try { addStaged(await call('mcp_read_resource', { server: res.server, uri: res.uri })); } catch (e) { errorToast(e); } } });
            if (!sub.length) sub.push({ label: t('nothing_here'), disabled: true });
            showMenu(anchor, sub, { placement: 'top-start', width: 300 });
          } catch (e) { errorToast(e); }
        },
      });
    }
    showMenu(anchor, items, { placement: 'top-start', width: 260 });
  }

  async function usePrompt(p) {
    const args = {};
    for (const a of p.arguments || []) {
      const v = window.prompt(`${a.name}${a.description ? ' — ' + a.description : ''}`, '');
      if (v === null) return;
      args[a.name] = v;
    }
    try { addStaged(await call('mcp_get_prompt', { server: p.server, name: p.name, args })); } catch (e) { errorToast(e); }
  }

  function modeMenu(anchor) {
    const modes = ['default', 'auto', 'acceptEdits', 'plan', 'bypassPermissions'];
    showMenu(anchor, modes.map((m) => ({ label: t('mode_' + m), sub: t('mode_' + m + '_d'), checked: st.permission_mode === m, onClick: () => persist({ permission_mode: m }) })), { placement: 'top-start', width: 300 });
  }

  // ---- events
  el.addEventListener('click', (e) => {
    const b = e.target.closest('[data-act]');
    if (!b) { if (e.target === el || e.target.classList.contains('composer-bar') || e.target.classList.contains('cbox')) ta.focus(); return; }
    const act = b.dataset.act;
    if (act === 'send') send();
    else if (act === 'model') modelMenu(b);
    else if (act === 'tools') toolsMenu(b);
    else if (act === 'attach') attachMenu(b);
    else if (act === 'mode') modeMenu(b);
  });
  const saveDraft = () => { if (opts.draftKey) store.drafts.set(opts.draftKey, ta.value); };
  ta.addEventListener('input', () => {
    autoGrow(ta);
    updateSend();
    saveDraft();
  });
  attachSuggest(ta, { context: cmdContext, anchor: el, onPick: (p) => { if (p.type === '/' && p.item.kind === 'builtin' && !p.item.hint) send(); } });
  ta.addEventListener('keydown', (e) => {
    if (e.isComposing || e.keyCode === 229) return;
    const sendOnEnter = store.settings.send_on_enter !== false;
    if (e.key === 'Enter' && ((sendOnEnter && !e.shiftKey && !e.ctrlKey && !e.metaKey) || (!sendOnEnter && (e.ctrlKey || e.metaKey)))) {
      e.preventDefault();
      if (!running || ta.value.trim()) send();
    } else if (keymap.matches(e, 'history')) {
      // search messages sent before (like a shell's history search)
      e.preventDefault();
      import('./prompt_history.js').then((m) => m.openPromptHistory(api, el));
    } else if (e.key === 'ArrowUp' && !ta.value && opts.onEditLast) {
      e.preventDefault();
      opts.onEditLast();
    } else if (e.key === 'Escape' && running && !waiting) {
      // not while idle on a workflow: like Claude Code, Esc leaves background workflows alone
      e.preventDefault();
      opts.onStop && opts.onStop();
    }
  });
  ta.addEventListener('paste', (e) => {
    const items = Array.from(e.clipboardData?.items || []);
    const files = items.filter((i) => i.kind === 'file').map((i) => i.getAsFile()).filter(Boolean);
    if (files.length) {
      e.preventDefault();
      files.forEach((f, i) => addFile(f, f.name && f.name !== 'image.png' ? f.name : `${t('pasted')} ${new Date().toLocaleTimeString().replace(/:/g, '.')}${i ? '-' + i : ''}.png`));
      return;
    }
    const text = e.clipboardData?.getData('text/plain') || '';
    if (text.length > PASTE_AS_FILE) {
      e.preventDefault();
      const ph = { name: `${t('pasted')}.txt`, loading: true, size: text.length };
      attachments.push(ph);
      renderAttachments();
      call('stage_text', { name: 'Pasted text.txt', text }).then((a) => {
        a.name = `${t('pasted')} (${text.split('\n').length} lines)`;
        attachments[attachments.indexOf(ph)] = a;
        renderAttachments();
      }).catch((err) => { attachments = attachments.filter((x) => x !== ph); renderAttachments(); errorToast(err); });
    }
  });
  ta.addEventListener('focus', () => { store.activeComposer = api; });

  if (opts.draftKey && store.drafts.has(opts.draftKey)) ta.value = store.drafts.get(opts.draftKey);
  refreshBar();
  requestAnimationFrame(() => autoGrow(ta));

  const api = {
    el,
    state: st,
    focus() { ta.focus(); const n = ta.value.length; ta.setSelectionRange(n, n); },
    setRunning(r, w) { running = r; waiting = !!(r && w); updateSend(); },
    /** Running, but idle on a Claude Code workflow (a message goes straight in). */
    waiting: () => waiting,
    /** Show messages queued while Claude is responding. */
    showQueue(list, onRemove) {
      let q = el.querySelector('.queued');
      if (!list.length) { if (q) q.remove(); return; }
      if (!q) { q = h('<div class="queued"></div>'); el.insertBefore(q, el.firstChild); }
      q.innerHTML = '';
      list.forEach((item, i) => {
        const c = h(`<div class="q-item">${icon('clock', 12)}<span class="ellipsis">${esc(item.text)}</span><button aria-label="remove">${icon('x', 11)}</button></div>`);
        c.querySelector('button').onclick = () => onRemove(i);
        q.appendChild(c);
      });
    },
    setText(s) { ta.value = s; autoGrow(ta); updateSend(); saveDraft(); },
    getText() { return ta.value; },
    setState(patch) { Object.assign(st, patch); refreshBar(); },
    /** Change options as the menus do (saved to the conversation, if any). */
    persist,
    ultracodeOffered: () => ultracodeOffered(modelInfo(st.model)),
    ultracodeOn: () => ultracodeOn(modelInfo(st.model)),
    /** The effort level the next turn runs at (null when the model has none). */
    effort: () => effortOf(modelInfo(st.model)),
    addPaths,
    addStaged,
    setDrag(on) { el.classList.toggle('drag', on); },
    openModelMenu() { modelMenu($('[data-act=model]', el)); },
    openModeMenu() { const b = $('[data-act=mode]', el); if (b) modeMenu(b); },
    insertText(s) { const p = ta.selectionStart; ta.value = ta.value.slice(0, p) + s + ta.value.slice(ta.selectionEnd); ta.setSelectionRange(p + s.length, p + s.length); autoGrow(ta); updateSend(); ta.focus(); },
    send,
    refresh: refreshBar,
  };
  store.activeComposer = api;
  return api;
}

export { closeMenu };
