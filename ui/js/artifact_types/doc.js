// Doc artifacts (application/vnd.ant.document): markdown drawn as a formatted
// document (artifacts.rs renders it, raw HTML as text) that the person edits
// in place; Save writes it back as markdown, a new version. Runs in the
// artifact's frame; htmlToMarkdown is pure (it reads nodes, never the page).

// a node's children as an array (the page gives a NodeList)
const kids = (n) => [...(n.childNodes || [])];

const BLOCKS = new Set(['P', 'DIV', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'UL', 'OL', 'LI', 'BLOCKQUOTE', 'PRE', 'TABLE', 'HR', 'SECTION', 'ARTICLE']);

// characters markdown would read as formatting
const escText = (s) => s.replace(/([\\`*_[\]])/g, '\\$1');

function inline(node) {
  let out = '';
  for (const n of kids(node)) {
    if (n.nodeType === 3) { out += escText(n.nodeValue.replace(/\s+/g, ' ')); continue; }
    if (n.nodeType !== 1) continue;
    const tag = n.nodeName.toUpperCase();
    const inner = () => inline(n);
    const wrap = (m) => { const s = inner(); const t = s.trim(); return t ? s.replace(t, m + t + m) : s; };
    switch (tag) {
      case 'STRONG': case 'B': out += wrap('**'); break;
      case 'EM': case 'I': out += wrap('*'); break;
      case 'DEL': case 'S': case 'STRIKE': out += wrap('~~'); break;
      case 'CODE': { const t = n.textContent || ''; const tick = t.includes('`') ? '``' : '`'; out += tick + t + tick; break; }
      case 'A': { const href = n.getAttribute('href') || ''; const t = inner(); out += href && href !== '#' ? `[${t}](${href.replace(/[()\s]/g, encodeURIComponent)})` : t; break; }
      case 'BR': out += '  \n'; break;
      case 'IMG': out += `![${escText(n.getAttribute('alt') || '')}](${n.getAttribute('src') || ''})`; break;
      case 'INPUT': if ((n.getAttribute('type') || '') === 'checkbox') out += n.hasAttribute && n.hasAttribute('checked') ? '[x] ' : '[ ] '; break;
      default: out += BLOCKS.has(tag) ? ' ' + inner().trim() + ' ' : inner();
    }
  }
  return out;
}

function listItems(list, depth, ordered) {
  const out = [];
  let n = Number(list.getAttribute && list.getAttribute('start')) || 1;
  for (const li of kids(list)) {
    if (li.nodeType !== 1 || li.nodeName.toUpperCase() !== 'LI') continue;
    const marker = ordered ? `${n++}. ` : '- ';
    const pad = '   '.repeat(depth);
    const own = { childNodes: kids(li).filter((c) => !(c.nodeType === 1 && ['UL', 'OL'].includes(c.nodeName.toUpperCase()))) };
    // a loose item's text sits in a paragraph: its lines are the item's
    out.push(pad + marker + inline(own).trim().replace(/\n/g, '\n' + pad + '   '));
    for (const c of kids(li)) {
      if (c.nodeType === 1 && ['UL', 'OL'].includes(c.nodeName.toUpperCase())) out.push(...listItems(c, depth + 1, c.nodeName.toUpperCase() === 'OL'));
    }
  }
  return out;
}

function cellText(td) { return inline(td).trim().replace(/\|/g, '\\|').replace(/\n/g, ' '); }

function table(t) {
  const rows = [];
  const walk = (n) => { for (const c of kids(n)) { if (c.nodeType !== 1) continue; const tag = c.nodeName.toUpperCase(); if (tag === 'TR') rows.push(kids(c).filter((x) => x.nodeType === 1 && ['TH', 'TD'].includes(x.nodeName.toUpperCase())).map(cellText)); else walk(c); } };
  walk(t);
  if (!rows.length) return '';
  const w = Math.max(...rows.map((r) => r.length));
  const line = (r) => '| ' + Array.from({ length: w }, (_, i) => r[i] || '').join(' | ') + ' |';
  return [line(rows[0]), '|' + ' --- |'.repeat(w), ...rows.slice(1).map(line)].join('\n');
}

function blocks(node) {
  const out = [];
  let loose = [];
  const flush = () => { const s = inline({ childNodes: loose }).trim(); if (s) out.push(s); loose = []; };
  for (const n of kids(node)) {
    const tag = n.nodeType === 1 ? n.nodeName.toUpperCase() : '';
    if (!BLOCKS.has(tag)) { loose.push(n); continue; }
    flush();
    const h = tag.match(/^H([1-6])$/);
    if (h) { const s = inline(n).trim(); if (s) out.push('#'.repeat(Number(h[1])) + ' ' + s); }
    else if (tag === 'P') { const s = inline(n).trim(); if (s) out.push(s); }
    else if (tag === 'UL' || tag === 'OL') out.push(listItems(n, 0, tag === 'OL').join('\n'));
    else if (tag === 'BLOCKQUOTE') out.push(blocks(n).split('\n').map((l) => (l ? '> ' + l : '>')).join('\n'));
    else if (tag === 'PRE') { const t = (n.textContent || '').replace(/\n$/, ''); const fence = t.includes('```') ? '~~~' : '```'; out.push(fence + '\n' + t + '\n' + fence); }
    else if (tag === 'TABLE') { const s = table(n); if (s) out.push(s); }
    else if (tag === 'HR') out.push('---');
    else { const s = blocks(n); if (s) out.push(s); }
  }
  flush();
  return out.join('\n\n');
}

/** Markdown of a document's nodes (headings, paragraphs, lists, quotes, code, tables). */
export function htmlToMarkdown(root) { return blocks(root).replace(/\n{3,}/g, '\n\n').trim() + '\n'; }

/** Words in a text, for the counter. */
export function wordCount(text) { const m = String(text).match(/[\p{L}\p{N}][\p{L}\p{N}'’-]*/gu); return m ? m.length : 0; }

// ---------------------------------------------------------------- the frame

const STRINGS = {
  en: { edit: 'Edit', save: 'Save', cancel: 'Cancel', words: '{n} words', saved: 'Saved as a new version', b: 'Bold', i: 'Italic', h1: 'Heading 1', h2: 'Heading 2', h3: 'Heading 3', p: 'Text', ul: 'Bulleted list', ol: 'Numbered list', q: 'Quote', code: 'Code' },
  tr: { edit: 'Düzenle', save: 'Kaydet', cancel: 'Vazgeç', words: '{n} sözcük', saved: 'Yeni sürüm olarak kaydedildi', b: 'Kalın', i: 'İtalik', h1: 'Başlık 1', h2: 'Başlık 2', h3: 'Başlık 3', p: 'Metin', ul: 'Madde işaretli liste', ol: 'Numaralı liste', q: 'Alıntı', code: 'Kod' },
};

function boot() {
  const lang = /(^|[#&])lang=tr\b/.test(location.hash) ? 'tr' : 'en';
  const S = STRINGS[lang];
  const doc = document.getElementById('doc');
  const editable = window.parent !== window;
  const tools = [['b', 'bold', '<b>B</b>'], ['i', 'italic', '<i>I</i>'], ['h1', 'formatBlock:H1', 'H1'], ['h2', 'formatBlock:H2', 'H2'], ['h3', 'formatBlock:H3', 'H3'], ['p', 'formatBlock:P', '¶'], ['ul', 'insertUnorderedList', '•'], ['ol', 'insertOrderedList', '1.'], ['q', 'formatBlock:BLOCKQUOTE', '❝'], ['code', 'formatBlock:PRE', '&lt;/&gt;']];
  const bar = document.createElement('div');
  bar.className = 'tb';
  bar.setAttribute('role', 'toolbar');
  bar.innerHTML = `${tools.map(([k, cmd, label]) => `<button data-cmd="${cmd}" title="${S[k]}" aria-label="${S[k]}" hidden>${label}</button>`).join('')}<span class="grow"></span><span class="status" role="status"></span><button data-a="cancel" hidden>${S.cancel}</button><button data-a="save" class="primary" hidden>${S.save}</button><button data-a="edit">${S.edit}</button>`;
  document.body.prepend(bar);
  const status = bar.querySelector('.status');
  const count = () => { status.textContent = S.words.replace('{n}', wordCount(doc.innerText || doc.textContent)); };
  if (!editable) bar.querySelector('[data-a=edit]').hidden = true;
  let before = '';
  const setEditing = (on) => {
    doc.contentEditable = on ? 'true' : 'false';
    bar.querySelectorAll('[data-cmd], [data-a=save], [data-a=cancel]').forEach((b) => (b.hidden = !on));
    bar.querySelector('[data-a=edit]').hidden = on || !editable;
    if (on) { before = doc.innerHTML; doc.focus(); }
  };
  const save = () => {
    if (doc.contentEditable !== 'true') return;
    if (doc.innerHTML !== before) window.parent.postMessage({ __artifact: 'edit', content: htmlToMarkdown(doc) }, '*');
    setEditing(false);
    count();
    if (doc.innerHTML !== before) status.textContent = S.saved;
  };
  bar.addEventListener('mousedown', (e) => { if (e.target.closest('[data-cmd]')) e.preventDefault(); });
  bar.addEventListener('click', (e) => {
    const b = e.target.closest('button');
    if (!b) return;
    if (b.dataset.cmd) { const [cmd, arg] = b.dataset.cmd.split(':'); document.execCommand(cmd, false, arg); doc.focus(); return; }
    if (b.dataset.a === 'edit') setEditing(true);
    else if (b.dataset.a === 'save') save();
    else if (b.dataset.a === 'cancel') { doc.innerHTML = before; setEditing(false); count(); }
  });
  doc.addEventListener('input', count);
  doc.addEventListener('dblclick', () => { if (editable && doc.contentEditable !== 'true') setEditing(true); });
  document.addEventListener('keydown', (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') { e.preventDefault(); save(); }
    else if (e.key === 'Escape' && doc.contentEditable === 'true') { e.preventDefault(); doc.innerHTML = before; setEditing(false); count(); }
  }, true);
  window.addEventListener('message', (e) => { if (e.source === window.parent && e.data && e.data.__artifact === 'print') window.print(); });
  count();
}

// only in an artifact's own page (the gallery and the tests import the functions above)
const DATA = typeof document !== 'undefined' && typeof document.getElementById === 'function' && document.getElementById('art-data');
if (DATA && DATA.dataset.type === 'document') boot();
