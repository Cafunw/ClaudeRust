// Rendering of assistant message parts (text, thinking, tools, notices).
import { icon, ARTIFACT_ICON } from './icons.js';
import { t } from './i18n.js';
import { esc, h, morph, toast, fmtUsd, snapshot, sameSnapshot, relPath } from './util.js';
import { lineDiff } from './diffstat.js';
import { call } from './api.js';
import { store, modelInfo } from './store.js';

// ------------------------------------------------------------- decorating

let katexReady = null;
function loadKatex() {
  if (window.katex) return Promise.resolve();
  if (!katexReady) {
    katexReady = new Promise((res) => {
      const s = document.createElement('script');
      s.src = 'vendor/katex/katex.min.js';
      s.onload = () => res();
      s.onerror = () => res();
      document.head.appendChild(s);
    });
  }
  return katexReady;
}

function renderMath(root) {
  const nodes = [...(root.matches('.math[data-tex]:not([data-done])') ? [root] : []), ...root.querySelectorAll('.math[data-tex]:not([data-done])')];
  if (!nodes.length) return;
  loadKatex().then(() => {
    if (!window.katex) return;
    nodes.forEach((n) => {
      try {
        window.katex.render(n.getAttribute('data-tex'), n, { displayMode: n.classList.contains('math-display'), throwOnError: false, output: 'html' });
        n.setAttribute('data-done', '1');
      } catch { /* leave source */ }
    });
  });
}

/** Post-process rendered markdown: math, copy buttons, artifact cards. */
export function decorate(root) {
  if (!root || root.nodeType !== 1) return;
  if (store.settings.latex_enabled !== false) renderMath(root);
  const all = (sel) => [...(root.matches(sel) ? [root] : []), ...root.querySelectorAll(sel)];
  all('.code-copy').forEach((b) => {
    if (b.dataset.ready) return;
    b.dataset.ready = '1';
    b.innerHTML = `${icon('copy', 14)}<span>${t('copy_code')}</span>`;
    const block = b.closest('.code-block');
    // any code block can be saved as a file
    if (block && !block.querySelector('.code-save')) {
      const save = h(`<button class="code-run code-save" type="button" title="${t('code_save_tip')}">${icon('download', 14)}</button>`);
      b.before(save);
    }
    // shell snippets in a code session can go straight to the terminal
    if (store.codeRoot && block && SHELL_LANGS.has((block.dataset.lang || '').toLowerCase()) && block.closest('.turn.assistant')) {
      const run = h(`<button class="code-run" type="button" title="${t('code_run_tip')}">${icon('terminal', 14)}<span>${t('code_run')}</span></button>`);
      b.before(run);
    }
  });
  all('.artifact-card').forEach((c) => {
    const kind = c.dataset.kind;
    const ic = c.querySelector('.artifact-card-icon');
    if (ic && !ic.innerHTML) ic.innerHTML = icon(ARTIFACT_ICON[kind] || 'file', 20);
    const sub = c.querySelector('.artifact-card-sub');
    if (sub && !sub.dataset.ready) {
      sub.dataset.ready = '1';
      const label = sub.textContent;
      sub.textContent = c.dataset.complete === 'false' ? `${label} · ${t('generating')}` : `${label} · ${t('click_to_open')}`;
    }
  });
}

const SHELL_LANGS = new Set(['bash', 'sh', 'shell', 'zsh', 'fish', 'console', 'shellsession', 'terminal', 'powershell', 'ps1', 'pwsh', 'cmd', 'bat']);

/** The commands in a shell snippet: for console transcripts (`$ cmd` lines
 *  followed by output) only the commands. */
export function shellCommands(code) {
  const lines = String(code).replace(/\n+$/, '').split('\n');
  const prompted = lines.filter((l) => /^\s*[$%] /.test(l));
  if (prompted.length && prompted.length < lines.length) return prompted.map((l) => l.replace(/^\s*[$%] /, '')).join('\n');
  return lines.map((l) => l.replace(/^\s*[$%] /, '')).join('\n');
}

const EXT = {
  javascript: 'js', js: 'js', jsx: 'jsx', typescript: 'ts', ts: 'ts', tsx: 'tsx', python: 'py', py: 'py', rust: 'rs', rs: 'rs', go: 'go',
  java: 'java', kotlin: 'kt', swift: 'swift', c: 'c', cpp: 'cpp', 'c++': 'cpp', csharp: 'cs', cs: 'cs', ruby: 'rb', rb: 'rb', php: 'php',
  html: 'html', css: 'css', scss: 'scss', json: 'json', yaml: 'yaml', yml: 'yml', toml: 'toml', xml: 'xml', sql: 'sql', markdown: 'md', md: 'md',
  bash: 'sh', sh: 'sh', shell: 'sh', zsh: 'sh', powershell: 'ps1', ps1: 'ps1', dockerfile: 'Dockerfile', makefile: 'Makefile', lua: 'lua', r: 'r',
  dart: 'dart', scala: 'scala', elixir: 'ex', haskell: 'hs', vue: 'vue', svelte: 'svelte', graphql: 'graphql', ini: 'ini', csv: 'csv', txt: 'txt',
};

/** A file name for a code block of language `lang`. */
export function codeFileName(lang) {
  const l = String(lang || '').toLowerCase();
  const ext = EXT[l] || 'txt';
  return ext === 'Dockerfile' || ext === 'Makefile' ? ext : `snippet.${ext}`;
}

// Global delegated click handling for rendered content.
document.addEventListener('click', async (e) => {
  const saveBtn = e.target.closest('.code-save');
  if (saveBtn) {
    const block = saveBtn.closest('.code-block');
    const code = block?.querySelector('code')?.innerText || '';
    try { await call('save_text_file', { name: codeFileName(block?.dataset.lang), content: code }); } catch (err) { toast(String((err && err.message) || err), 'error'); }
    return;
  }
  const runBtn = e.target.closest('.code-run');
  if (runBtn) {
    const code = runBtn.closest('.code-block')?.querySelector('code')?.innerText || '';
    const term = await import('./panes/terminal.js');
    if (!(await term.pasteToTerminal(shellCommands(code)))) toast(t('code_run_failed'), 'error');
    return;
  }
  const copyBtn = e.target.closest('.code-copy');
  if (copyBtn) {
    const code = copyBtn.closest('.code-block')?.querySelector('code')?.innerText || '';
    await copyText(code);
    const span = copyBtn.querySelector('span');
    if (span) { span.textContent = t('copied'); setTimeout(() => (span.textContent = t('copy_code')), 1500); }
    return;
  }
  const a = e.target.closest('a[href]');
  if (a && !a.closest('[data-internal]')) {
    // Never navigate the app window itself; web links open in the browser.
    e.preventDefault();
    const href = a.getAttribute('href') || '';
    if (/^(https?:|mailto:)/i.test(href)) call('open_external', { url: href }).catch(() => {});
  }
});

export async function copyText(text) {
  try {
    await call('copy_text', { text });
  } catch {
    try { await navigator.clipboard.writeText(text); } catch { toast('Copy failed', 'error'); }
  }
}

// ------------------------------------------------------------- tool labels

function shortPath(p) {
  return p ? relPath(p, store.codeRoot) : '';
}

export function diffHtml(rows) {
  return `<div class="diff">${rows.map(([k, l]) => `<div class="ln ${k === 'ctx' ? '' : k}">${k === 'add' ? '+' : k === 'del' ? '-' : ' '} ${esc(l)}</div>`).join('')}</div>`;
}

export function unifiedDiffHtml(text) {
  const rows = String(text || '').split('\n').map((l) => {
    if (l.startsWith('+++') || l.startsWith('---')) return '';
    if (l.startsWith('@@')) return `<div class="ln hunk">${esc(l)}</div>`;
    if (l.startsWith('+')) return `<div class="ln add">${esc(l)}</div>`;
    if (l.startsWith('-')) return `<div class="ln del">${esc(l)}</div>`;
    return `<div class="ln">${esc(l)}</div>`;
  });
  return `<div class="diff">${rows.join('')}</div>`;
}

function diffStat(rows) {
  const a = rows.filter((r) => r[0] === 'add').length;
  const d = rows.filter((r) => r[0] === 'del').length;
  return `<span class="diff-stat"><span class="a">+${a}</span> <span class="d">-${d}</span></span>`;
}

const TOOL_ICONS = { MultiEdit: 'edit', NotebookEdit: 'edit', LS: 'folder', WebFetch: 'globe', WebSearch: 'globe', Task: 'sparkle', Agent: 'sparkle', conversation_search: 'history', recent_chats: 'history', Bash: 'terminal', Read: 'fileText', Write: 'file', Edit: 'edit', Glob: 'search', Grep: 'search', TodoWrite: 'list', ExitPlanMode: 'compass', web_search: 'globe', web_fetch: 'globe', code_execution: 'terminal' };

function toolSummary(p) {
  const i = p.input || {};
  switch (p.tool) {
    case 'Bash': return { name: 'Bash', arg: (i.description ? i.description + ' · ' : '') + String(i.command || '').split('\n')[0] };
    case 'Read': return { name: 'Read', arg: shortPath(i.file_path) + (i.offset ? `:${i.offset}` : '') };
    case 'Write': return { name: 'Write', arg: shortPath(i.file_path) };
    case 'Edit': return { name: 'Edit', arg: shortPath(i.file_path) };
    case 'Glob': return { name: 'Glob', arg: i.pattern || '' };
    case 'Grep': return { name: 'Grep', arg: `"${i.pattern || ''}"` + (i.path ? ` in ${shortPath(i.path)}` : '') };
    case 'TodoWrite': return { name: t('todos'), arg: '' };
    case 'ExitPlanMode': return { name: t('plan_ready'), arg: '' };
    case 'conversation_search': return { name: t('searched_chats'), arg: i.query || '' };
    case 'MultiEdit': return { name: 'Edit', arg: shortPath(i.file_path) + (i.edits ? ` (${i.edits.length})` : '') };
    case 'NotebookEdit': return { name: 'Notebook', arg: shortPath(i.notebook_path) };
    case 'LS': return { name: 'List', arg: shortPath(i.path) };
    case 'WebFetch': return { name: 'Fetch', arg: i.url || '' };
    case 'WebSearch': return { name: t('searched_web'), arg: i.query || '' };
    case 'Task': case 'Agent': return { name: i.subagent_type || 'Agent', arg: i.description || '' };
    case 'recent_chats': return { name: t('recent_chats_tool'), arg: '' };
    default:
      if (p.server && p.server !== 'anthropic') return { name: p.tool, arg: p.server };
      return { name: p.tool || p.name, arg: '' };
  }
}

function statusHtml(p) {
  switch (p.status) {
    case 'running': return `<span class="status"><span class="spinner"></span>${t('running')}</span>`;
    case 'streaming': case 'pending': return `<span class="status"><span class="spinner"></span></span>`;
    case 'awaiting_permission': return `<span class="status">${icon('shield', 13)} ${t('awaiting')}</span>`;
    case 'error': return `<span class="status err">${icon('alert', 13)} ${t('failed')}</span>`;
    case 'cancelled': return `<span class="status">${t('cancelled')}</span>`;
    default: return `<span class="status ok">${icon('check', 13)}</span>`;
  }
}

function resultSection(p) {
  const r = p.result;
  if (!r) return '';
  let s = '';
  if (r.text) s += `<pre>${esc(r.text)}</pre>`;
  if (r.images) s += r.images.map((src) => `<img src="${esc(src)}" alt="">`).join('');
  if (!s) return '';
  return `<div class="tool-section"><div class="lbl">${t('output')}</div>${s}</div>`;
}

// Live output of running commands, by tool_use_id (kept across re-renders).
const liveOutput = new Map();

function liveSection(p) {
  if (p.status !== 'running' && p.status !== 'awaiting_permission') return '';
  const text = liveOutput.get(p.id);
  return `<div class="tool-section live-out${text ? '' : ' hidden'}"><div class="lbl">${t('output')}</div><pre>${esc(text || '')}</pre></div>`;
}

/** Show the latest output of a running command in its tool card. */
export function setToolProgress(id, text) {
  if (liveOutput.size > 50) liveOutput.delete(liveOutput.keys().next().value);
  liveOutput.set(id, text);
  // tool cards are only in the conversation view: no need to search the whole document
  const root = document.getElementById('view') || document;
  for (const el of root.querySelectorAll(`.tool[data-tool-id="${CSS.escape(id)}"]`)) {
    const sec = el.querySelector('.live-out');
    if (!sec) continue;
    sec.classList.remove('hidden');
    const pre = sec.querySelector('pre');
    const atEnd = pre.scrollTop + pre.clientHeight >= pre.scrollHeight - 8;
    pre.textContent = text;
    if (atEnd) pre.scrollTop = pre.scrollHeight;
    if (!el.dataset.userClosed) el.classList.add('open');
  }
}

function toolBody(p) {
  const i = p.input || {};
  switch (p.tool) {
    case 'Bash':
      return `<div class="tool-section"><pre>$ ${esc(i.command || '')}</pre></div>${liveSection(p)}${resultSection(p)}`;
    case 'MultiEdit':
      return (i.edits || []).map((ed) => diffHtml(lineDiff(ed.old_string, ed.new_string))).join('') + (p.status === 'error' ? resultSection(p) : '');
    case 'Edit': {
      const rows = lineDiff(i.old_string, i.new_string);
      return diffHtml(rows) + (p.status === 'error' ? resultSection(p) : '');
    }
    case 'Write':
      return `<div class="tool-section"><pre>${esc(String(i.content || '').slice(0, 20000))}</pre></div>` + (p.status === 'error' ? resultSection(p) : '');
    case 'ExitPlanMode':
      return `<div class="tool-section"><pre style="white-space:pre-wrap;font-family:var(--font-ui);font-size:13px">${esc(i.plan || '')}</pre></div>${resultSection(p)}`;
    case 'Task':
    case 'Agent':
      return `<div class="tool-section"><div class="lbl">${t('subagent_task')}</div><pre style="white-space:pre-wrap;font-family:var(--font-ui);font-size:13px">${esc(i.prompt || '')}</pre></div>${liveSection(p)}${resultSection(p)}`;
    default: {
      const inputJson = JSON.stringify(i, null, 2);
      const showInput = inputJson && inputJson !== '{}';
      return `${showInput ? `<div class="tool-section"><div class="lbl">${t('input')}</div><pre>${esc(inputJson)}</pre></div>` : ''}${resultSection(p)}`;
    }
  }
}

function todosHtml(todos) {
  return `<ul class="todos">${(todos || []).map((td) => `<li class="${esc(td.status)}"><span class="box">${td.status === 'completed' ? icon('check', 11) : ''}</span><span>${esc(td.status === 'in_progress' && td.activeForm ? td.activeForm : td.content)}</span></li>`).join('')}</ul>`;
}

// ------------------------------------------------------------- part render

function renderThinking(el, p) {
  if (!el.classList.contains('thinking')) {
    el.className = 'thinking';
    el.innerHTML = `<button class="thinking-head"><span class="ic"></span><span class="lab"></span>${icon('chevRight', 14, 'class="chev"')}</button><div class="thinking-body md"></div>`;
    el.querySelector('.thinking-head').addEventListener('click', () => { el.classList.toggle('open'); el.dataset.userToggled = '1'; });
  }
  const lab = el.querySelector('.lab');
  const ic = el.querySelector('.ic');
  if (p.streaming) {
    lab.textContent = t('thinking') + '…';
    lab.classList.add('shimmer');
    ic.innerHTML = icon('bulb', 15);
    if (!el.dataset.userToggled) el.classList.add('open');
  } else {
    lab.textContent = t('thought_process');
    lab.classList.remove('shimmer');
    ic.innerHTML = icon('clock', 15);
    if (!el.dataset.userToggled) el.classList.remove('open');
  }
  const body = el.querySelector('.thinking-body');
  if (p.redacted && !p.raw) {
    body.textContent = t('redacted_thinking');
  } else if (!p.raw && !p.streaming) {
    body.innerHTML = `<p class="muted">${esc(t('thought_process'))}</p>`;
  } else {
    morph(body, p.html || '').forEach(decorate);
  }
  if (p.streaming) body.scrollTop = body.scrollHeight;
}

function renderTool(el, p) {
  const open = el.classList.contains('open');
  const sum = toolSummary(p);
  const isTodo = p.tool === 'TodoWrite';
  let stat = '';
  if (p.tool === 'Edit' && p.input) stat = diffStat(lineDiff(p.input.old_string, p.input.new_string));
  const ic = TOOL_ICONS[p.tool] || (p.server && p.server !== 'anthropic' ? 'plug' : 'wrench');
  el.className = 'tool' + (open || isTodo ? ' open' : '');
  if (p.id) el.dataset.toolId = p.id;
  if (p.status !== 'running') liveOutput.delete(p.id);
  el.innerHTML = `<button class="tool-head">${icon(ic, 15)}<span class="tname">${esc(sum.name)}</span><span class="targ">${esc(sum.arg)}</span>${stat}${statusHtml(p)}${isTodo ? '' : icon('chevRight', 14, 'class="chev"')}</button>
    <div class="tool-body">${isTodo ? todosHtml(p.input && p.input.todos) : toolBody(p)}</div>`;
  if (!isTodo) el.querySelector('.tool-head').addEventListener('click', () => { el.classList.toggle('open'); if (!el.classList.contains('open')) el.dataset.userClosed = '1'; });
  // file tools: the path opens the file in the editor
  const fp = p.input && (p.input.file_path || p.input.notebook_path);
  if (fp && ['Read', 'Write', 'Edit', 'MultiEdit', 'NotebookEdit'].includes(p.tool)) {
    const targ = el.querySelector('.targ');
    targ.classList.add('file-link');
    targ.title = t('open_in_editor');
    targ.addEventListener('click', (e) => {
      e.stopPropagation();
      const abs = /^([a-zA-Z]:)?[\\/]/.test(fp) || !store.codeRoot ? fp : `${store.codeRoot}/${fp}`;
      call('open_in_editor', { path: abs, line: p.input.offset ? Number(p.input.offset) : null }).catch(() => {});
    });
  }
  const pre = el.querySelector('.live-out pre');
  if (pre) pre.scrollTop = pre.scrollHeight;
}

function favicon(domain) {
  return `<span class="fav">${esc((domain || '?')[0].toUpperCase())}</span>`;
}

function renderSearch(el, p) {
  const open = el.classList.contains('open');
  const n = Array.isArray(p.results) ? p.results.length : 0;
  const running = p.status === 'running';
  el.className = 'tool' + (open ? ' open' : '');
  el.innerHTML = `<button class="tool-head">${icon('globe', 15)}<span class="tname">${running ? `<span class="shimmer">${t('searching_web')}</span>` : t('searched_web')}</span><span class="targ">${esc(p.query || '')}</span>
    ${p.status === 'error' ? `<span class="status err">${esc(p.error || t('failed'))}</span>` : running ? '<span class="status"><span class="spinner"></span></span>' : `<span class="status">${n} ${t('results')}</span>`}
    ${icon('chevRight', 14, 'class="chev"')}</button>
    <div class="tool-body"><div class="tool-section search-results">${(p.results || []).filter((r) => /^https?:/i.test(r.url || '')).map((r) => `<a class="search-result" href="${esc(r.url)}">${favicon(r.domain)}<span class="t">${esc(r.title || r.url)}</span><span class="d">${esc(r.domain || '')}</span></a>`).join('')}</div></div>`;
  el.querySelector('.tool-head').addEventListener('click', () => el.classList.toggle('open'));
}

function renderFetch(el, p) {
  const running = p.status === 'running';
  el.className = 'tool';
  let domain = '';
  try { domain = new URL(p.url).hostname.replace(/^www\./, ''); } catch { domain = p.url || ''; }
  el.innerHTML = `<div class="tool-head">${icon('globe', 15)}<span class="tname">${running ? `<span class="shimmer">${t('fetching')}</span>` : t('fetched')}</span><span class="targ">${esc(p.title || p.url || '')}</span>${p.status === 'error' ? `<span class="status err">${esc(p.error || t('failed'))}</span>` : running ? '<span class="status"><span class="spinner"></span></span>' : `<span class="status">${esc(domain)}</span>`}</div>`;
}

function errorMessage(err) {
  if (!err) return '';
  const k = err.kind;
  if (k === 'no_api_key') return t('err_no_api_key');
  if (err.status === 401 || k === 'authentication_error') return t('err_auth');
  if (err.status === 429 || k === 'rate_limit_error') return t('err_rate_limit');
  if (err.status === 529 || k === 'overloaded_error') return t('err_overloaded');
  if (k === 'network') return t('err_network', { msg: err.message });
  return err.message || t('error_title');
}

function renderNotice(el, p, ctx) {
  el.className = 'notice';
  let html = '';
  switch (p.code) {
    case 'error':
      el.classList.add('error');
      html = `${icon('alert', 16)}<div class="grow"><div>${esc(errorMessage(p.error))}</div><div class="actions"></div></div>`;
      break;
    case 'refusal':
      el.classList.add('warn');
      html = `${icon('shield', 16)}<div class="grow">${esc(t('refusal'))}${p.details && p.details.explanation ? `<div class="small">${esc(p.details.explanation)}</div>` : ''}</div>`;
      break;
    case 'max_tokens':
      html = `${icon('info', 16)}<div class="grow"><div>${esc(t('max_tokens'))}</div><div class="actions"></div></div>`;
      break;
    case 'stopped':
      html = `${icon('stop', 14)}<div class="grow muted">${esc(t('stopped'))}</div>`;
      el.style.background = 'transparent';
      el.style.padding = '2px 0';
      break;
    case 'overloaded_fallback':
      html = `${icon('info', 16)}<div class="grow small">${esc(t('overload_notice', { from: modelInfo(p.from || '').name || p.from, to: modelInfo(p.to || '').name || p.to }))}</div>`;
      break;
    case 'fallback':
      html = `${icon('info', 16)}<div class="grow small">${esc(t('fallback_notice', { from: modelInfo(p.from || '').name || p.from, to: modelInfo(p.to || '').name || p.to }))}</div>`;
      break;
    case 'budget':
      el.classList.add('warn');
      html = `${icon('zap', 16)}<div class="grow"><div>${esc(t('budget_stop', { spent: fmtUsd(p.spent), budget: fmtUsd(p.budget) }))}</div><div class="actions"></div></div>`;
      break;
    case 'compaction':
      html = `${icon('history', 16)}<div class="grow small">${esc(t('compaction'))}</div>`;
      break;
    default:
      html = `<div class="grow">${esc(p.code)}</div>`;
  }
  el.innerHTML = html;
  const actions = el.querySelector('.actions');
  if (actions && ctx) {
    if (p.code === 'error') {
      if (p.error && (p.error.kind === 'no_api_key' || p.error.status === 401)) {
        const b = h(`<button class="btn sm">${t('open_settings')}</button>`);
        b.onclick = () => ctx.openSettings('account');
        actions.appendChild(b);
      }
      if (ctx.onRetry) {
        const b = h(`<button class="btn sm">${icon('retry', 14)} ${t('retry')}</button>`);
        b.onclick = () => ctx.onRetry();
        actions.appendChild(b);
      }
    }
    if ((p.code === 'max_tokens' || p.code === 'budget') && ctx.onContinue) {
      const b = h(`<button class="btn sm">${t('continue')}</button>`);
      b.onclick = () => ctx.onContinue();
      actions.appendChild(b);
    }
  }
}

/** Update the parts container of an assistant turn. Returns whether any
 *  part was drawn or removed (false: the DOM is as it was). */
export function renderParts(container, parts, ctx) {
  const existing = Array.from(container.children);
  let changed = false;
  parts.forEach((p, i) => {
    let el = existing[i];
    if (el && el.dataset.kind !== p.kind) { el.remove(); el = null; }
    if (!el) {
      el = document.createElement('div');
      el.dataset.kind = p.kind;
      const ref = container.children[i] || null;
      container.insertBefore(el, ref);
    }
    // the whole turn comes again with every streamed change (~20 times a
    // second): only parts whose data changed are drawn again. A snapshot,
    // not JSON.stringify, for that: a turn's finished tool outputs can be
    // megabytes, compared on every event
    // an unchanged part of a streamed turn comes as the very same object
    // (turnstream.js reuses the previous event's): nothing to compare
    if (el.__part === p) return;
    el.__part = p;
    const sig = snapshot(p);
    const same = sameSnapshot(el.__sig, sig);
    // keep the newest copy either way, so the old strings can be freed
    el.__sig = sig;
    if (same) return;
    changed = true;
    switch (p.kind) {
      case 'text':
        if (!el.classList.contains('md')) el.className = 'md part-text';
        morph(el, p.html || '').forEach(decorate);
        break;
      case 'thinking': renderThinking(el, p); break;
      case 'tool': renderTool(el, p); break;
      case 'search': renderSearch(el, p); break;
      case 'fetch': renderFetch(el, p); break;
      case 'notice': renderNotice(el, p, ctx); break;
      default: el.textContent = '';
    }
  });
  for (let j = container.children.length - 1; j >= parts.length; j--) { container.children[j].remove(); changed = true; }
  return changed;
}

export function turnPlainText(turn) {
  return (turn.parts || []).filter((p) => p.kind === 'text').map((p) => p.raw || '').join('\n\n')
    .replace(/<antArtifact[^>]*>\n?/g, '').replace(/\n?<\/antArtifact>/g, '').trim();
}
