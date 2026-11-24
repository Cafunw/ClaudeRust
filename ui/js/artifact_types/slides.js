// Slide decks (application/vnd.ant.slides), as Claude Desktop's: each
// `<section>` a 1920x1080 slide, `<style>` shared by all, an `<aside>` as a
// slide's last child its speaker notes, `class="fragment"` a click step and
// `data-transition` how the next slide comes in. Runs in the artifact's frame:
// pages through the deck, presents it full screen, prints it (PDF) and edits
// its words and notes in place (Save is a new version). Nothing in the deck
// runs: scripts and event handlers are taken out before it is drawn.

/** The click step of each fragment and how many steps there are: those with a
 *  `data-fragment-index` by it (one index, one step), the rest after them in
 *  their order on the slide. `indices`: each fragment's index, or null. */
export function fragmentSteps(indices) {
  const keys = indices.map((v, i) => (Number.isFinite(v) ? v : 1e6 + i));
  const order = [...new Set(keys)].sort((a, b) => a - b);
  return { stepOf: keys.map((k) => order.indexOf(k)), count: order.length };
}

/** Where `dir` (next, prev, first, last) goes from `{slide, step}` in a deck
 *  whose slides have `counts[i]` click steps: through a slide's steps before
 *  the next slide; back to a slide with all its steps shown. */
export function navigate(at, dir, counts) {
  const n = counts.length;
  if (!n) return { slide: 0, step: 0 };
  const slide = Math.max(0, Math.min(n - 1, at.slide));
  const step = Math.max(0, Math.min(counts[slide], at.step));
  if (dir === 'first') return { slide: 0, step: 0 };
  if (dir === 'last') return { slide: n - 1, step: counts[n - 1] };
  if (dir === 'next') {
    if (step < counts[slide]) return { slide, step: step + 1 };
    return slide < n - 1 ? { slide: slide + 1, step: 0 } : { slide, step };
  }
  if (dir === 'prev') {
    if (step > 0) return { slide, step: step - 1 };
    return slide > 0 ? { slide: slide - 1, step: counts[slide - 1] } : { slide, step };
  }
  return { slide, step };
}

/** Whether URL `v` of an attribute is one a slide may keep. */
export function safeUrl(v) {
  const s = String(v).replace(/[\u0000- ]/g, '').toLowerCase();
  return !/^(javascript|vbscript|data:text|data:application)/.test(s);
}

// ---------------------------------------------------------------- the frame

const STRINGS = {
  en: { prev: 'Previous slide', next: 'Next slide', grid: 'All slides', notes: 'Notes', present: 'Present', edit: 'Edit text', save: 'Save', cancel: 'Cancel', exit: 'Exit (Esc)', slide: 'Slide {n}', saved: 'Saved as a new version', empty: 'This deck has no slides (no <section> elements).', noNotes: 'No notes for this slide.', notesPh: 'Speaker notes' },
  tr: { prev: 'Önceki slayt', next: 'Sonraki slayt', grid: 'Tüm slaytlar', notes: 'Notlar', present: 'Sun', edit: 'Metni düzenle', save: 'Kaydet', cancel: 'Vazgeç', exit: 'Çık (Esc)', slide: 'Slayt {n}', saved: 'Yeni sürüm olarak kaydedildi', empty: 'Bu destede slayt yok (<section> öğesi yok).', noNotes: 'Bu slaytta not yok.', notesPh: 'Konuşmacı notları' },
};

function sanitize(root) {
  root.querySelectorAll('script, iframe, frame, frameset, object, embed, base, meta, form, portal').forEach((x) => x.remove());
  // an SVG animation can set a link's href (to javascript:) or a handler after the attributes are checked
  root.querySelectorAll('animate, set').forEach((x) => { if (/^(on|href$|xlink:href$)/i.test(x.getAttribute('attributeName') || '')) x.remove(); });
  root.querySelectorAll('link').forEach((l) => { if (!(/stylesheet/i.test(l.getAttribute('rel') || '') && /^https:\/\/fonts\.googleapis\.com\//.test(l.getAttribute('href') || ''))) l.remove(); });
  root.querySelectorAll('*').forEach((el) => {
    for (const a of [...el.attributes]) {
      const n = a.name.toLowerCase();
      if (n.startsWith('on') || n === 'srcdoc' || n === 'contenteditable' || (['href', 'src', 'xlink:href', 'action', 'formaction', 'poster', 'background'].includes(n) && !safeUrl(a.value))) el.removeAttribute(a.name);
    }
  });
}

function parseDeck(html) {
  const tpl = document.createElement('template');
  tpl.innerHTML = html;
  sanitize(tpl.content);
  const sections = [...tpl.content.querySelectorAll('section')].filter((s) => !s.parentElement || !s.parentElement.closest('section'));
  const shared = [...tpl.content.querySelectorAll('style, link')].filter((s) => !s.closest('section'));
  return { sections, shared };
}

function boot(content) {
  const lang = /(^|[#&])lang=tr\b/.test(location.hash) ? 'tr' : 'en';
  const S = STRINGS[lang];
  const editable = window.parent !== window;
  const deck = parseDeck(content);
  document.getElementById('deck')?.remove();
  for (const s of deck.shared) document.head.appendChild(s.cloneNode(true));
  const body = document.body;
  if (!deck.sections.length) { body.insertAdjacentHTML('beforeend', `<div class="err-box"></div>`); body.querySelector('.err-box').textContent = S.empty; return; }
  body.insertAdjacentHTML('afterbegin', `<div class="tb" role="toolbar">
    <button data-a="prev" aria-label="${S.prev}" title="${S.prev}">‹</button><span class="status count" aria-live="polite"></span><button data-a="next" aria-label="${S.next}" title="${S.next}">›</button>
    <span class="sep"></span><button data-a="grid" aria-pressed="false">${S.grid}</button><button data-a="notes" aria-pressed="false">${S.notes}</button>
    <span class="grow"></span><span class="status msg" role="status"></span>
    <button data-a="cancel" hidden>${S.cancel}</button><button data-a="save" class="primary" hidden>${S.save}</button><button data-a="edit"${editable ? '' : ' hidden'}>${S.edit}</button><button data-a="present" class="primary">▶ ${S.present}</button></div>
    <div class="deck"><nav class="rail" aria-label="${S.grid}"></nav><div class="main"><div class="stage"></div><div class="grid-view" hidden></div><div class="notes" hidden></div></div></div><div id="print"></div>`);
  const $ = (q) => body.querySelector(q);
  const tb = $('.tb'), rail = $('.rail'), stage = $('.stage'), grid = $('.grid-view'), notes = $('.notes'), msg = $('.tb .msg');
  const counts = deck.sections.map((s) => fragmentSteps([...s.querySelectorAll('.fragment')].map((f) => parseFloat(f.getAttribute('data-fragment-index')))).count);
  let at = { slide: 0, step: 0 };
  const fromHash = location.hash.match(/slide-(\d+)/);
  if (fromHash) at = navigate({ slide: Number(fromHash[1]) - 1, step: 0 }, 'stay', counts);
  let presenting = false, editing = false, dirty = false, showNotes = deck.sections.some((s) => s.querySelector(':scope > aside'));
  let box = null;

  const boxOf = (section) => { const b = document.createElement('div'); b.className = 'slide-box'; b.appendChild(section.cloneNode(true)); return b; };
  const notesOf = (s) => { const a = s.querySelector(':scope > aside'); return a ? a.textContent.trim() : ''; };
  // `b` scaled into a w x h box, centred, `pad` from its edges
  const fit = (b, w, h, pad = 0) => {
    const k = Math.max(0.01, Math.min((w - 2 * pad) / 1920, (h - 2 * pad) / 1080));
    b.style.transform = `translate(${(w - 1920 * k) / 2}px, ${(h - 1080 * k) / 2}px) scale(${k})`;
  };
  const layout = () => {
    if (box) fit(box, stage.clientWidth, stage.clientHeight, presenting ? 0 : 16);
    body.querySelectorAll('.thumb .slide-box').forEach((b) => { const t = b.parentElement; fit(b, t.clientWidth, t.clientHeight); });
  };
  const thumbs = (into) => {
    into.innerHTML = '';
    deck.sections.forEach((s, i) => {
      const t = document.createElement('button');
      t.className = 'thumb' + (i === at.slide ? ' on' : '');
      t.dataset.i = i;
      t.setAttribute('aria-label', S.slide.replace('{n}', i + 1));
      if (i === at.slide) t.setAttribute('aria-current', 'true');
      t.innerHTML = `<span class="n">${i + 1}</span>`;
      const b = boxOf(s);
      b.querySelectorAll('.fragment').forEach((f) => f.classList.add('visible'));
      t.appendChild(b);
      into.appendChild(t);
    });
  };
  const commitSlide = () => {
    if (!editing || !box) return;
    const live = box.firstElementChild;
    const clean = live.cloneNode(true);
    clean.removeAttribute('contenteditable');
    clean.querySelectorAll('.fragment').forEach((f) => { f.classList.remove('visible', 'current-fragment'); if (!f.className) f.removeAttribute('class'); });
    const area = notes.querySelector('textarea');
    const target = deck.sections[at.slide];
    if (clean.innerHTML !== target.innerHTML) { target.innerHTML = clean.innerHTML; dirty = true; }
    if (area) {
      let aside = target.querySelector(':scope > aside');
      if (area.value.trim() !== notesOf(target)) {
        if (!aside && area.value.trim()) { aside = document.createElement('aside'); target.appendChild(aside); }
        if (aside) { if (area.value.trim()) aside.textContent = area.value.trim(); else aside.remove(); }
        dirty = true;
      }
    }
  };
  const draw = (enter) => {
    stage.innerHTML = '';
    box = boxOf(deck.sections[at.slide]);
    const steps = fragmentSteps([...box.querySelectorAll('.fragment')].map((f) => parseFloat(f.getAttribute('data-fragment-index'))));
    box.querySelectorAll('.fragment').forEach((f, i) => {
      f.classList.toggle('visible', steps.stepOf[i] < at.step || editing);
      f.classList.toggle('current-fragment', steps.stepOf[i] === at.step - 1);
    });
    if (enter) box.classList.add('enter-' + enter);
    if (editing) box.firstElementChild.setAttribute('contenteditable', 'true');
    stage.appendChild(box);
    $('.tb .count').textContent = `${at.slide + 1} / ${deck.sections.length}`;
    tb.querySelector('[data-a=prev]').disabled = at.slide === 0 && at.step === 0;
    tb.querySelector('[data-a=next]').disabled = at.slide === deck.sections.length - 1 && at.step >= counts[at.slide];
    rail.querySelectorAll('.thumb').forEach((t) => { const on = Number(t.dataset.i) === at.slide; t.classList.toggle('on', on); if (on) { t.setAttribute('aria-current', 'true'); t.scrollIntoView({ block: 'nearest' }); } else t.removeAttribute('aria-current'); });
    const text = notesOf(deck.sections[at.slide]);
    notes.hidden = !(showNotes || editing) || (presenting && !notes.classList.contains('over'));
    if (editing) { notes.innerHTML = '<textarea></textarea>'; const ta = notes.firstChild; ta.value = text; ta.placeholder = S.notesPh; ta.setAttribute('aria-label', S.notesPh); }
    else notes.textContent = text || S.noNotes;
    layout();
  };
  const go = (dir) => {
    commitSlide();
    const from = at.slide;
    const next = navigate(at, dir, counts);
    if (next.slide === at.slide && next.step === at.step) return;
    const t = next.slide === from + 1 ? (deck.sections[from].getAttribute('data-transition') || '') : '';
    at = next;
    if (next.slide === from && !editing) {
      // a click step: only the fragments change
      const steps = fragmentSteps([...box.querySelectorAll('.fragment')].map((f) => parseFloat(f.getAttribute('data-fragment-index'))));
      box.querySelectorAll('.fragment').forEach((f, i) => { f.classList.toggle('visible', steps.stepOf[i] < at.step); f.classList.toggle('current-fragment', steps.stepOf[i] === at.step - 1); });
      tb.querySelector('[data-a=prev]').disabled = at.slide === 0 && at.step === 0;
      tb.querySelector('[data-a=next]').disabled = at.slide === deck.sections.length - 1 && at.step >= counts[at.slide];
      return;
    }
    draw(/fade|magic/.test(t) ? 'fade' : /push/.test(t) ? 'push' : '');
  };
  const setPresent = (on, tell = true) => {
    if (presenting === on) return;
    commitSlide();
    presenting = on;
    body.classList.toggle('presenting', on);
    notes.classList.remove('over');
    if (on) { setGrid(false); stage.setAttribute('title', S.exit); }
    else stage.removeAttribute('title');
    if (tell && editable) window.parent.postMessage({ __artifact: 'present', on }, '*');
    // the panel grows to the window first: fit once it has
    setTimeout(() => { draw(); window.focus(); }, 60);
  };
  const setGrid = (on) => {
    grid.hidden = !on;
    stage.hidden = on;
    tb.querySelector('[data-a=grid]').classList.toggle('on', on);
    tb.querySelector('[data-a=grid]').setAttribute('aria-pressed', String(on));
    if (on) { commitSlide(); thumbs(grid); requestAnimationFrame(layout); }
    else draw();
  };
  let base = content;
  const setEditing = (on, discard) => {
    if (on) { editing = true; setGrid(false); }
    else if (discard) { editing = false; dirty = false; deck.sections.splice(0, deck.sections.length, ...parseDeck(base).sections); thumbs(rail); }
    else { commitSlide(); editing = false; }
    ['save', 'cancel'].forEach((a) => (tb.querySelector(`[data-a=${a}]`).hidden = !on));
    tb.querySelector('[data-a=edit]').hidden = on || !editable;
    tb.querySelector('[data-a=present]').disabled = on;
    draw();
    if (on) box.firstElementChild.focus();
  };
  const save = () => {
    if (!editing) return;
    setEditing(false);
    if (!dirty) return;
    const out = [...deck.shared.map((s) => s.outerHTML), ...deck.sections.map((s) => s.outerHTML)].join('\n\n');
    window.parent.postMessage({ __artifact: 'edit', content: out }, '*');
    base = out;
    dirty = false;
    msg.textContent = S.saved;
    thumbs(rail);
    layout();
  };
  tb.addEventListener('click', (e) => {
    const a = e.target.closest('button') && e.target.closest('button').dataset.a;
    if (a === 'prev' || a === 'next') go(a);
    else if (a === 'grid') setGrid(grid.hidden);
    else if (a === 'notes') { showNotes = !showNotes; tb.querySelector('[data-a=notes]').classList.toggle('on', showNotes); tb.querySelector('[data-a=notes]').setAttribute('aria-pressed', String(showNotes)); draw(); }
    else if (a === 'present') setPresent(true);
    else if (a === 'edit') setEditing(true);
    else if (a === 'save') save();
    else if (a === 'cancel') setEditing(false, true);
  });
  const pick = (e) => { const t = e.target.closest('.thumb'); if (!t) return; commitSlide(); at = { slide: Number(t.dataset.i), step: 0 }; if (!grid.hidden) setGrid(false); else draw(); };
  rail.addEventListener('click', pick);
  grid.addEventListener('click', pick);
  stage.addEventListener('click', (e) => { if (presenting) go(e.clientX < stage.clientWidth / 4 ? 'prev' : 'next'); });
  document.addEventListener('keydown', (e) => {
    const t = e.target;
    if (t && (t.isContentEditable || t.tagName === 'TEXTAREA' || t.tagName === 'INPUT')) {
      if (e.key === 'Escape' && editing) { e.preventDefault(); t.blur(); }
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') { e.preventDefault(); save(); }
      return;
    }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') { e.preventDefault(); save(); return; }
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const k = e.key;
    if (['ArrowRight', 'ArrowDown', 'PageDown', ' '].includes(k) || (k === 'Enter' && presenting)) { e.preventDefault(); go('next'); }
    else if (['ArrowLeft', 'ArrowUp', 'PageUp', 'Backspace'].includes(k)) { e.preventDefault(); go('prev'); }
    else if (k === 'Home') { e.preventDefault(); go('first'); }
    else if (k === 'End') { e.preventDefault(); go('last'); }
    else if (presenting && (k === 'n' || k === 'N')) { notes.classList.toggle('over'); notes.hidden = !notes.classList.contains('over'); }
    else if (k === 'Escape' && presenting) { e.preventDefault(); setPresent(false); }
    else if (!presenting && (k === 'f' || k === 'F5')) { e.preventDefault(); setPresent(true); }
  });
  window.addEventListener('message', (e) => {
    if (e.source !== window.parent || !e.data) return;
    if (e.data.__artifact === 'present') setPresent(!!e.data.on, false);
    if (e.data.__artifact === 'print') {
      const p = $('#print');
      p.innerHTML = '';
      deck.sections.forEach((s) => p.appendChild(s.cloneNode(true)));
      window.print();
    }
  });
  new ResizeObserver(layout).observe(stage);
  new ResizeObserver(layout).observe(grid);
  thumbs(rail);
  tb.querySelector('[data-a=notes]').classList.toggle('on', showNotes);
  tb.querySelector('[data-a=notes]').setAttribute('aria-pressed', String(showNotes));
  draw();
}

// only in an artifact's own page (the gallery and the tests import the functions above)
const DATA = typeof document !== 'undefined' && typeof document.getElementById === 'function' && document.getElementById('art-data');
if (DATA && DATA.dataset.type === 'slides') boot(JSON.parse(DATA.textContent));
