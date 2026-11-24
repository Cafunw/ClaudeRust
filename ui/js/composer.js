// The message composer: text input, attachments, model / tools menus.
import * as keymap from './keymap.js';
import * as slash from './slash.js';
import { attachSuggest } from './suggest.js';
import { icon } from './icons.js';
import { t, tOr } from './i18n.js';
import { call, isMac } from './api.js';
import { store, modelInfo, emit, rememberModel } from './store.js';
import { $, esc, h, showMenu, closeMenu, autoGrow, fileToBase64, attachmentSub, errorToast, toast, modal, button } from './util.js';
import { hookPreview } from './attachment_preview.js';

const PASTE_AS_FILE = 6000;
/** The preset styles (prompts.rs style_prompt), labelled style_<id>. */
export const PRESET_STYLES = ['normal', 'concise', 'explanatory', 'formal', 'learning'];

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

/** What to send for MCP prompt arguments `defs` (name, description,
 *  required) from the typed `values`, in the same order: the filled ones
 *  (`args`), and the index of a required one left empty (`missing`, else -1). */
export function promptArgValues(defs, values) {
  const filled = (i) => String(values[i] ?? '').trim() !== '';
  return {
    missing: defs.findIndex((a, i) => a.required && !filled(i)),
    args: Object.fromEntries(defs.flatMap((a, i) => (filled(i) ? [[a.name, values[i]]] : []))),
  };
}

/** One form for the arguments of MCP prompt `p`: resolves to them, or null when cancelled. */
function promptArgs(p) {
  const defs = p.arguments || [];
  if (!defs.length) return Promise.resolve({});
  return new Promise((resolve) => {
    let done = false;
    const finish = (v) => { if (!done) { done = true; resolve(v); } };
    const body = h(`<div>${p.description ? `<p class="small muted" style="margin:0 0 14px">${esc(p.description)}</p>` : ''}</div>`);
    const inputs = defs.map((a, i) => {
      const f = h(`<div class="field"><label class="label" for="mcp-arg-${i}">${esc(a.name)}${a.required ? ' *' : ''}</label><input class="input" id="mcp-arg-${i}" ${a.required ? 'aria-required="true"' : ''}></div>`);
      if (a.description) f.appendChild(h(`<div class="help">${esc(a.description)}</div>`));
      body.appendChild(f);
      return f.querySelector('input');
    });
    const submit = () => {
      const { missing, args } = promptArgValues(defs, inputs.map((x) => x.value));
      inputs.forEach((x, i) => x.classList.toggle('invalid', i === missing));
      if (missing >= 0) { inputs[missing].focus(); return; }
      finish(args);
      m.close();
    };
    for (const x of inputs) x.addEventListener('keydown', (e) => { if (e.key === 'Enter' && !e.isComposing) { e.preventDefault(); submit(); } });
    const m = modal({ title: p.name, body, footer: [button(t('cancel'), '', () => m.close()), button(t('mcp_prompt_add'), 'primary', submit)], onClose: () => finish(null) });
    setTimeout(() => inputs[0].focus(), 30);
  });
}

// the refusals attachments.rs and office.rs name by kind (their English text is the fallback)
const ATT_ERRORS = new Set(['image_unreadable', 'office_invalid', 'office_too_large', 'office_damaged', 'office_too_many_parts', 'office_empty', 'pdf_invalid',
  'pdf_too_large', 'pdf_too_many_pages', 'pdf_too_many_pages_for_model', 'file_too_large', 'unsupported_file', 'message_too_large', 'not_a_file']);

/** A refused attachment in the UI's language: `e` an error (or `{kind, message, name}`) for the file `name`; null when it isn't one. */
export function attachmentError(e, name) {
  const kind = e && e.kind;
  if (!ATT_ERRORS.has(kind)) return null;
  // (an Error's own name is "Error": only a plain {kind, message, name} carries the file's)
  const n = name || (!(e instanceof Error) && e.name);
  return t(`atterr_${kind}`, { name: n ? `“${n}”` : t('atterr_file') });
}

/** Say a failure: a refused attachment in the UI's language, anything else as before. */
function attError(e, name) {
  const said = typeof e === 'string' ? e : attachmentError(e, name);
  if (said) toast(said, 'error'); else errorToast(e);
}

/** Whether the thinking switch can turn thinking off for `m`: in chats as `thinking_switch` says for their
 *  engine; in a code session (`codeEngine`) the account's CLI only where `thinking_switch_cli` says it can. */
export function thinkingSupported(m, codeEngine = null) {
  const engineCan = codeEngine ? codeEngine !== 'claude-code' || m.thinking_switch_cli !== false : m.thinking_switch !== false;
  return ['disableable', 'between_tools', 'explicit', 'budget'].includes(m.thinking) && engineCan;
}

/** Research mode runs in chats on the API engine (research.rs), as commands.rs picks the engine. */
export function researchOffered(s = store.settings) {
  return s.chat_engine === 'api' || (s.chat_engine !== 'claude-code' && !!s.has_api_key);
}

/** Fast mode's price, for a model that offers it ([input, output] per million, or just true); null otherwise. */
const fastOf = (m) => m.fast_price || m.fast || null;

/** The note beside the Fast switch: how much more it costs. */
export function fastNote(m) {
  const f = fastOf(m);
  if (!f) return '';
  const x = Array.isArray(f) && Array.isArray(m.price) && m.price[1] > 0 ? Math.round((f[1] / m.price[1]) * 10) / 10 : null;
  return x ? t('fast_d', { x }) : t('fast_d_plain');
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
    // Settings → "Fast mode for new chats"
    fast: !!store.settings.fast_mode,
    style: 'normal',
    disabled_servers: [],
    disabled_tools: [],
    // "deferred" | "all" | null = automatic (tool_load.rs)
    tool_access: null,
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
  // (cut short in a narrow window: the whole of it on hover)
  if (mode === 'chat') Object.assign($('.composer-note', el), { textContent: t('disclaimer'), title: t('disclaimer') });
  const sendBtn = $('[data-act=send]', el);
  const attEl = $('.attachments', el);
  const activeEl = $('.active-tools', el);

  function persist(patch) {
    Object.assign(st, patch);
    if ('model' in patch) rememberModel(mode, patch.model);
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
  const canThink = (m) => thinkingSupported(m, mode === 'code' ? ((opts.context && opts.context()) || {}).engine || 'builtin' : null);
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
    const thinkOn = m.thinking === 'always_on' || (canThink(m) && st.thinking);
    const ucOn = ultracodeOn(m) && ultracodeOffered(m);
    $('[data-act=model]', el).innerHTML = `${esc(m.name)}${thinkOn && m.thinking !== 'always_on' ? ` <span class="think-flag" title="${t('extended_thinking')}">${icon('bulb', 13)}<span class="sr-only">, ${esc(t('extended_thinking'))}</span></span>` : ''}${ucOn ? ` <span class="think-flag" title="${esc(t('ultracode_d'))}">${esc(t('ultracode'))}</span>` : ''} ${icon('chevDown', 14)}`;
    const toolsBtn = $('[data-act=tools]', el);
    const research = mode === 'chat' && st.research && researchOffered();
    const anyOn = (mode === 'chat' && st.web_search) || (st.style && st.style !== 'normal') || research;
    toolsBtn.classList.toggle('on', !!anyOn);
    activeEl.innerHTML = '';
    const chip = (ic, label, off) => {
      const c = h(`<span class="active-tool">${icon(ic, 13)} ${esc(label)} <button aria-label="${esc(t('turn_off_x', { x: label }))}">${icon('x', 12)}</button></span>`);
      c.querySelector('button').onclick = () => persist(off);
      activeEl.appendChild(c);
    };
    if (research) chip('compass', t('research'), { research: false });
    if (mode === 'chat' && st.web_search) chip('globe', t('web_search'), { web_search: false });
    if (st.style && st.style !== 'normal') chip('feather', styleName(st.style), { style: 'normal' });
    if (st.fast && fastOf(m)) chip('zap', t('fast_mode'), { fast: false });
    const mb = $('[data-act=mode]', el);
    if (mb) {
      mb.className = `mode-btn ${st.permission_mode}`;
      const ic = { default: 'shield', auto: 'sparkle', acceptEdits: 'edit', plan: 'compass', bypassPermissions: 'zap' }[st.permission_mode] || 'shield';
      mb.innerHTML = `${icon(ic, 14)} ${esc(t('mode_' + st.permission_mode))}`;
    }
    updateSend();
  }

  function styleName(id) {
    if (PRESET_STYLES.includes(id)) return t(`style_${id}`);
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
    if (running && !(waiting && (ta.value.trim() || attachments.length))) {
      sendBtn.disabled = false;
      sendBtn.classList.add('stop');
      sendIcon('stop');
      sendBtn.setAttribute('aria-label', t('stop'));
      sendBtn.title = `${t('stop')} (Esc)`;
      return;
    }
    sendBtn.classList.remove('stop');
    sendIcon('arrowUp');
    sendBtn.setAttribute('aria-label', t('send'));
    sendBtn.title = `${t('send')} (${store.settings.send_on_enter !== false ? 'Enter' : `${isMac ? '⌘' : 'Ctrl'}+Enter`})`;
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
        <button class="x" aria-label="${esc(t('a11y_remove_x', { x: a.name }))}">${icon('x', 12)}</button></div>`);
      chip.querySelector('.x').onclick = () => {
        attachments = attachments.filter((x) => x !== a);
        if (a.id) call('unstage', { id: a.id }).catch(() => {});
        renderAttachments();
      };
      if (a.id && !a.loading) hookPreview(chip, { stagedId: a.id }, a.name);
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
      for (const e of r.errors) attError(e);
    } catch (e) {
      attachments = attachments.filter((a) => !placeholders.includes(a));
      attError(e, paths.length === 1 ? placeholders[0].name : null);
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
      attError(e, ph.name);
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
      attError(e);
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
    if (canThink(m) || (m.efforts && m.efforts.length) || uc || fastOf(m)) items.push({ sep: true });
    if (canThink(m)) {
      const row = h(`<label class="menu-item" style="cursor:pointer">${icon('bulb', 16)}<span class="mtext">${t('extended_thinking')}<span class="sub">${m.thinking === 'budget' ? '' : ''}</span></span><span class="switch"><input type="checkbox" ${st.thinking ? 'checked' : ''}><span class="track"></span></span></label>`);
      row.querySelector('input').addEventListener('change', (e) => { persist({ thinking: e.target.checked }); });
      items.push({ element: row });
    }
    if (m.efforts && m.efforts.length) {
      const cur = effortOf(m);
      const seg = h(`<div style="padding:6px 10px 8px"><div class="small muted" style="margin-bottom:6px">${t('effort')}</div><div class="segmented fill" role="group" aria-label="${esc(t('effort'))}">${m.efforts.map((e) => `<button data-e="${e}" class="${e === cur ? 'on' : ''}" aria-pressed="${e === cur}">${t('effort_' + e)}</button>`).join('')}</div></div>`);
      seg.querySelectorAll('button').forEach((b) => b.addEventListener('click', () => {
        seg.querySelectorAll('button').forEach((x) => { x.classList.toggle('on', x === b); x.setAttribute('aria-pressed', String(x === b)); });
        persist({ effort: b.dataset.e });
      }));
      items.push({ element: seg });
    }
    if (uc) {
      const row = h(`<label class="menu-item" style="cursor:pointer">${icon('sparkle', 16)}<span class="mtext">${esc(t('ultracode'))}<span class="sub">${esc(t('ultracode_d'))}</span></span><span class="switch"><input type="checkbox" ${ultracodeOn(m) ? 'checked' : ''}><span class="track"></span></span></label>`);
      row.querySelector('input').addEventListener('change', (e) => { persist({ ultracode: e.target.checked }); });
      items.push({ element: row });
    }
    if (fastOf(m)) {
      const row = h(`<label class="menu-item" style="cursor:pointer">${icon('zap', 16)}<span class="mtext">${esc(t('fast_mode'))}<span class="sub">${esc(fastNote(m))}</span></span><span class="switch"><input type="checkbox" ${st.fast ? 'checked' : ''}><span class="track"></span></span></label>`);
      row.querySelector('input').addEventListener('change', (e) => { persist({ fast: e.target.checked }); });
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
      if (researchOffered()) {
        const rr = h(`<label class="menu-item" style="cursor:pointer">${icon('compass', 16)}<span class="mtext">${esc(t('research'))}<span class="sub">${esc(t('research_d'))}</span></span><span class="switch"><input type="checkbox" ${st.research ? 'checked' : ''}><span class="track"></span></span></label>`);
        rr.querySelector('input').addEventListener('change', (e) => persist({ research: e.target.checked }));
        items.push({ element: rr });
      }
      items.push({ sep: true }, { header: t('style') });
      const styles = [...PRESET_STYLES.map((id) => [id, t(`style_${id}`)]), ...(store.settings.custom_styles || []).map((s) => [s.id, s.name])];
      for (const [id, name] of styles) items.push({ label: name, checked: (st.style || 'normal') === id, onClick: () => persist({ style: id }) });
      items.push({ label: t('create_style'), icon: 'edit', onClick: () => emit('open-settings', 'styles') });
    }
    // Claude Code sessions use the CLI's own MCP config, not the app's servers
    const ccCode = mode === 'code' && ((opts.context && opts.context()) || {}).engine === 'claude-code';
    const servers = ccCode ? [] : store.mcpServers.filter((s) => s.status === 'connected');
    if (servers.length) {
      items.push({ sep: true }, { header: 'MCP' });
      for (const s of servers) {
        const on = !(st.disabled_servers || []).includes(s.name);
        const off = offTools(s).length;
        const n = s.tools.length;
        const sub = t(n === 1 ? 'tools_count_one' : 'tools_count', { n }) + (off ? ` · ${t('mcp_tools_off', { off })}` : '');
        // the row opens the server's tools (a switch each); the switch turns the whole server on or off
        const row = h(`<div class="menu-item mcp-row"><button class="mcp-open" aria-haspopup="menu" title="${esc(t('mcp_tools_of', { name: s.name }))}">${icon('plug', 16)}<span class="mtext">${esc(s.name)}<span class="sub">${esc(sub)}</span></span>${icon('chevRight', 14)}</button><label class="switch" title="${esc(t('enabled'))}"><input type="checkbox" aria-label="${esc(s.name)}" ${on ? 'checked' : ''}><span class="track"></span></label></div>`);
        row.querySelector('input').addEventListener('change', (e) => {
          const set = new Set(st.disabled_servers || []);
          if (e.target.checked) set.delete(s.name); else set.add(s.name);
          persist({ disabled_servers: [...set] });
        });
        row.querySelector('.mcp-open').onclick = () => serverTools(anchor, s);
        items.push({ element: row });
      }
      // "load tools when needed" (tool_load.rs, chats only): automatic above 20 of the chat's tools unless it says
      if (mode !== 'code') {
        const policies = store.settings.tool_policies || {};
        const count = servers.filter((s) => !(st.disabled_servers || []).includes(s.name))
          .reduce((n, s) => n + s.tools.filter((tool) => !offTools(s).includes(tool) && policies[`${s.name}/${tool.name}`] !== 'deny').length, 0);
        const deferred = st.tool_access ? st.tool_access === 'deferred' : count > 20;
        items.push({ sep: true }, { header: t('tool_access') },
          { label: t('tool_access_deferred'), sub: t('tool_access_deferred_d'), checked: deferred, onClick: () => { persist({ tool_access: 'deferred' }); toast(t('tool_access_set_deferred')); } },
          { label: t('tool_access_all'), sub: t('tool_access_all_d'), checked: !deferred, onClick: () => { persist({ tool_access: 'all' }); toast(t('tool_access_set_all')); } });
      }
    }
    if (!items.length) items.push({ label: t('nothing_here'), disabled: true });
    showMenu(anchor, items, { placement: 'top-start', width: 280 });
  }

  // a tool is named as the engines know it: "server__tool" (its api_name)
  const toolId = (s, tool) => tool.api_name || `${s.name}__${tool.name}`;
  const offTools = (s) => s.tools.filter((tool) => (st.disabled_tools || []).includes(toolId(s, tool)));

  /** The tools of MCP server `s` in this chat, a switch each (stored in the chat's `disabled_tools`). */
  function serverTools(anchor, s) {
    const serverOn = !(st.disabled_servers || []).includes(s.name);
    const items = [
      { label: t('back'), icon: 'arrowLeft', keepOpen: true, onClick: () => toolsMenu(anchor) },
      { header: serverOn ? s.name : `${s.name} · ${t('mcp_server_off')}` },
    ];
    for (const tool of s.tools) {
      const id = toolId(s, tool);
      const on = !(st.disabled_tools || []).includes(id);
      const row = h(`<label class="menu-item" style="cursor:pointer"><span class="mtext">${esc(tool.title || tool.name)}${tool.description ? `<span class="sub ellipsis">${esc(tool.description)}</span>` : ''}</span><span class="switch"><input type="checkbox" aria-label="${esc(tool.title || tool.name)}" ${on ? 'checked' : ''} ${serverOn ? '' : 'disabled'}><span class="track"></span></span></label>`);
      row.querySelector('input').addEventListener('change', (e) => {
        const set = new Set(st.disabled_tools || []);
        if (e.target.checked) set.delete(id); else set.add(id);
        persist({ disabled_tools: [...set] });
      });
      items.push({ element: row });
    }
    if (!s.tools.length) items.push({ label: t('nothing_here'), disabled: true });
    showMenu(anchor, items, { placement: 'top-start', width: 300 });
    // the keyboard goes on in the new menu
    document.querySelector('.popover input:not(:disabled), .popover button')?.focus();
  }

  /** A still of the screen or window picked in the system's share picker, as a PNG attachment. */
  async function addScreenshot() {
    let stream;
    try {
      stream = await navigator.mediaDevices.getDisplayMedia({ video: true, audio: false });
    } catch (e) {
      if (e && e.name !== 'NotAllowedError') errorToast(e); // (NotAllowedError: the picker was cancelled)
      return;
    }
    try {
      const frame = await new ImageCapture(stream.getVideoTracks()[0]).grabFrame();
      const c = document.createElement('canvas');
      c.width = frame.width;
      c.height = frame.height;
      c.getContext('2d').drawImage(frame, 0, 0);
      const png = await new Promise((res) => c.toBlob(res, 'image/png'));
      await addFile(png, `${t('screenshot')} ${new Date().toLocaleTimeString().replace(/:/g, '.')}.png`);
    } catch (e) {
      errorToast(e);
    } finally {
      stream.getTracks().forEach((tr) => tr.stop());
    }
  }

  async function attachMenu(anchor) {
    const items = [
      { label: t('upload_file'), icon: 'paperclip', onClick: async () => { const paths = await call('pick_files'); addPaths(paths); } },
    ];
    if (navigator.mediaDevices && navigator.mediaDevices.getDisplayMedia) items.push({ label: t('add_screenshot'), icon: 'image', onClick: addScreenshot });
    items.push({ label: t('add_from_github'), icon: 'branch', onClick: () => import('./github_source.js').then((m) => m.openGithubSource({ onAdd: addStaged })).catch(errorToast) },
      { label: t('add_from_link'), icon: 'download', onClick: () => import('./link_source.js').then((m) => m.openLinkSource({ onAdd: addStaged })).catch(errorToast) });
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
    const args = await promptArgs(p);
    if (!args) return;
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
        const n = text.split('\n').length;
        a.name = t(n === 1 ? 'pasted_line_one' : 'pasted_lines', { n });
        attachments[attachments.indexOf(ph)] = a;
        renderAttachments();
      }).catch((err) => { attachments = attachments.filter((x) => x !== ph); renderAttachments(); errorToast(err); });
    }
  });
  ta.addEventListener('focus', () => { store.activeComposer = api; });

  if (opts.draftKey && store.drafts.has(opts.draftKey)) ta.value = store.drafts.get(opts.draftKey);
  refreshBar();
  requestAnimationFrame(() => autoGrow(ta));
  // a narrower box wraps the placeholder (or the text) onto more lines
  let lastWidth = 0;
  new ResizeObserver(() => { if (ta.offsetWidth !== lastWidth) { lastWidth = ta.offsetWidth; autoGrow(ta); } }).observe(ta);

  const api = {
    el,
    state: st,
    // not from behind an open dialog (a view redrawn under Settings), which keeps the keyboard
    focus() { if (document.querySelector('.modal-scrim')) return; ta.focus(); const n = ta.value.length; ta.setSelectionRange(n, n); },
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
        const c = h(`<div class="q-item">${icon('clock', 12)}<span class="ellipsis">${esc(item.text)}</span><button aria-label="${esc(t('a11y_remove_x', { x: item.text.slice(0, 40) }))}">${icon('x', 11)}</button></div>`);
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
