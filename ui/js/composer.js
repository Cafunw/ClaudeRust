// The message composer: text input, attachments, model / tools menus.
import * as keymap from './keymap.js';
import * as slash from './slash.js';
import { attachSuggest } from './suggest.js';
import { icon } from './icons.js';
import { t } from './i18n.js';
import { call } from './api.js';
import { store, modelInfo, emit } from './store.js';
import { $, esc, h, showMenu, closeMenu, autoGrow, fileToBase64, attachmentSub, errorToast, toast } from './util.js';

const PASTE_AS_FILE = 6000;

export function thinkingSupported(m) {
  return ['disableable', 'between_tools', 'explicit', 'budget'].includes(m.thinking);
}

export function createComposer(opts) {
  const mode = opts.mode || 'chat';
  const st = {
    model: store.settings.default_model,
    thinking: mode === 'code' ? true : !!store.settings.thinking_default,
    effort: null,
    web_search: !!store.settings.web_search_default,
    style: 'normal',
    disabled_servers: [],
    permission_mode: store.settings.code_permission_mode || 'default',
    ...(opts.initial || {}),
  };
  let attachments = [];
  let running = false;
  let busy = false;

  const el = h(`<div class="composer" data-mode="${mode}">
    <div class="attachments hidden"></div>
    <textarea rows="1" spellcheck="true" placeholder="${esc(opts.placeholder || t('help_today'))}"></textarea>
    <div class="composer-bar">
      <button class="tool-btn" data-act="attach" title="${t('attach')}" aria-label="${t('attach')}">${icon('plus', 18)}</button>
      <button class="tool-btn" data-act="tools" title="${t('tools')}" aria-label="${t('tools')}">${icon('sliders', 16)}</button>
      ${mode === 'code' ? '<button class="mode-btn" data-act="mode"></button>' : ''}
      <div class="active-tools"></div>
      <div class="grow"></div>
      <button class="model-btn" data-act="model"></button>
      <button class="send-btn" data-act="send" aria-label="${t('send')}" disabled>${icon('arrowUp', 18)}</button>
    </div>
  </div>`);
  const ta = $('textarea', el);
  const sendBtn = $('[data-act=send]', el);
  const attEl = $('.attachments', el);
  const activeEl = $('.active-tools', el);

  function persist(patch) {
    Object.assign(st, patch);
    refreshBar();
    opts.onOptions && opts.onOptions(patch);
  }

  function refreshBar() {
    const m = modelInfo(st.model);
    const thinkOn = m.thinking === 'always_on' || (thinkingSupported(m) && st.thinking);
    $('[data-act=model]', el).innerHTML = `${esc(m.name)}${thinkOn && m.thinking !== 'always_on' ? ` <span class="think-flag" title="${t('extended_thinking')}">${icon('bulb', 13)}</span>` : ''} ${icon('chevDown', 14)}`;
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

  function updateSend() {
    if (running && opts.onQueue && ta.value.trim()) {
      sendBtn.disabled = false;
      sendBtn.classList.remove('stop');
      sendBtn.classList.add('queue');
      sendBtn.innerHTML = icon('arrowUp', 18);
      sendBtn.setAttribute('aria-label', t('queue_send'));
      sendBtn.title = t('queue_send');
      return;
    }
    sendBtn.classList.remove('queue');
    sendBtn.title = '';
    if (running) {
      sendBtn.disabled = false;
      sendBtn.classList.add('stop');
      sendBtn.innerHTML = icon('stop', 18);
      sendBtn.setAttribute('aria-label', t('stop'));
      return;
    }
    sendBtn.classList.remove('stop');
    sendBtn.innerHTML = icon('arrowUp', 18);
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

  async function addFile(file, nameOverride) {
    const ph = { name: nameOverride || file.name || 'file', loading: true, size: file.size };
    attachments.push(ph);
    renderAttachments();
    try {
      const data = await fileToBase64(file);
      const a = await call('stage_bytes', { name: ph.name, data });
      attachments[attachments.indexOf(ph)] = a;
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

  async function send() {
    if (running) {
      // typing while Claude works queues the message (sent when it finishes)
      if (ta.value.trim() && opts.onQueue && !attachments.some((a) => a.loading)) {
        opts.onQueue(ta.value, attachments.slice());
        ta.value = ''; attachments = []; renderAttachments(); autoGrow(ta);
        if (opts.draftKey) store.drafts.delete(opts.draftKey);
        updateSend();
        return;
      }
      opts.onStop && opts.onStop();
      return;
    }
    let text = ta.value;
    if ((!text.trim() && !attachments.length) || busy || attachments.some((a) => a.loading)) return;
    busy = true; updateSend();
    const atts = attachments.slice();
    try {
      if (mode === 'code' && /^#\s+\S/.test(text) && !text.trim().includes('\n') && !atts.length) {
        const r = await slash.handleHash(text.trim(), cmdContext());
        if (r === true) { ta.value = ''; autoGrow(ta); if (opts.draftKey) store.drafts.delete(opts.draftKey); return; }
        if (r === null) return;
      }
      if (text.trim().startsWith('/') && !atts.length) {
        const r = await slash.handle(text, cmdContext());
        if (r && r.handled) {
          ta.value = ''; autoGrow(ta);
          if (opts.draftKey) store.drafts.delete(opts.draftKey);
          return;
        }
        if (r && r.prompt) text = r.prompt;
        if (r && r.attachments && r.attachments.length) atts.push(...r.attachments);
      }
      const ok = await opts.onSend(text, atts, { ...st });
      if (ok !== false) {
        ta.value = '';
        attachments = [];
        renderAttachments();
        autoGrow(ta);
        if (opts.draftKey) store.drafts.delete(opts.draftKey);
      }
    } catch (e) {
      errorToast(e);
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
    for (const x of featured) items.push({ label: x.name, sub: x.description, checked: x.id === st.model, onClick: () => { persist({ model: x.id }); } });
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
    if (thinkingSupported(m) || (m.efforts && m.efforts.length)) items.push({ sep: true });
    if (thinkingSupported(m)) {
      const row = h(`<label class="menu-item" style="cursor:pointer">${icon('bulb', 16)}<span class="mtext">${t('extended_thinking')}<span class="sub">${m.thinking === 'budget' ? '' : ''}</span></span><span class="switch"><input type="checkbox" ${st.thinking ? 'checked' : ''}><span class="track"></span></span></label>`);
      row.querySelector('input').addEventListener('change', (e) => { persist({ thinking: e.target.checked }); });
      items.push({ element: row });
    }
    if (m.efforts && m.efforts.length) {
      const cur = st.effort || m.default_effort || 'high';
      const seg = h(`<div style="padding:6px 10px 8px"><div class="small muted" style="margin-bottom:6px">${t('effort')}</div><div class="segmented">${m.efforts.map((e) => `<button data-e="${e}" class="${e === cur ? 'on' : ''}">${t('effort_' + e)}</button>`).join('')}</div></div>`);
      seg.querySelectorAll('button').forEach((b) => b.addEventListener('click', () => {
        seg.querySelectorAll('button').forEach((x) => x.classList.toggle('on', x === b));
        persist({ effort: b.dataset.e });
      }));
      items.push({ element: seg });
    }
    showMenu(anchor, items, { placement: 'top-end', width: 300 });
  }

  function toolsMenu(anchor) {
    const items = [];
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
    const servers = store.mcpServers.filter((s) => s.status === 'connected');
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
    if (!b) { if (e.target === el || e.target.classList.contains('composer-bar')) ta.focus(); return; }
    const act = b.dataset.act;
    if (act === 'send') send();
    else if (act === 'model') modelMenu(b);
    else if (act === 'tools') toolsMenu(b);
    else if (act === 'attach') attachMenu(b);
    else if (act === 'mode') modeMenu(b);
  });
  ta.addEventListener('input', () => {
    autoGrow(ta);
    updateSend();
    if (opts.draftKey) store.drafts.set(opts.draftKey, ta.value);
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
    } else if (e.key === 'Escape' && running) {
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
    setRunning(r) { running = r; updateSend(); },
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
    setText(s) { ta.value = s; autoGrow(ta); updateSend(); },
    getText() { return ta.value; },
    setState(patch) { Object.assign(st, patch); refreshBar(); },
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
