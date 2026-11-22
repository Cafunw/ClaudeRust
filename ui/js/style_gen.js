// Settings → Profile → a style's editor: its name and instructions, which can be
// drafted from writing samples (style_gen.rs). The samples are sent once to
// draft them and never saved.
import { t } from './i18n.js';
import { call } from './api.js';
import { h, esc, modal, button, toast, errorToast, confirmDialog } from './util.js';

// the caps of style_gen.rs
export const MAX_SAMPLES = 3;
export const MAX_SAMPLE_CHARS = 6000;

/** The samples to send: trimmed, the empty ones left out. */
export function samplesOf(texts) {
  return texts.map((s) => s.trim()).filter(Boolean);
}

/** A file's text as a sample, cut to the cap (`cut` says so); null when it isn't text. */
export function fitSample(text) {
  if (text.includes('\u0000')) return null;
  const chars = [...text];
  return chars.length > MAX_SAMPLE_CHARS ? { text: chars.slice(0, MAX_SAMPLE_CHARS).join(''), cut: true } : { text, cut: false };
}

function labeled(label, control) {
  const f = h(`<label class="field" style="display:block"><span class="label">${esc(label)}</span></label>`);
  f.appendChild(control);
  return f;
}

/** Edit `style` (null: a new one); `onSave({ name, prompt })` keeps it. */
export function styleEditor(style, onSave) {
  const name = h('<input class="input" maxlength="80">');
  name.value = style ? style.name : '';
  const prompt = h(`<textarea class="textarea" rows="7" placeholder="${esc(t('style_prompt_ph'))}"></textarea>`);
  prompt.value = style ? style.prompt : '';
  const box = h(`<details><summary class="label" style="display:list-item;cursor:pointer;margin:0">${esc(t('style_from_samples'))}</summary><p class="help">${esc(t('style_samples_d', { n: MAX_SAMPLES }))}</p><div data-list></div><div class="row" style="flex-wrap:wrap;margin-top:10px"></div></details>`);
  const list = box.querySelector('[data-list]');
  const areas = () => [...list.querySelectorAll('textarea')];

  const addBtn = button(t('style_add_sample'), 'sm', () => addSample().focus());
  const addSample = () => {
    const n = areas().length + 1;
    const wrap = h(`<div style="margin-top:10px"><textarea class="textarea" rows="4" maxlength="${MAX_SAMPLE_CHARS}" aria-label="${esc(t('style_sample_n', { n }))}" placeholder="${esc(t('style_sample_ph'))}"></textarea><div class="small muted" style="text-align:right;margin-top:2px"></div></div>`);
    const ta = wrap.querySelector('textarea');
    const count = () => { wrap.lastChild.textContent = t('style_chars', { n: ta.value.length, max: MAX_SAMPLE_CHARS }); };
    ta.addEventListener('input', count);
    list.appendChild(wrap);
    count();
    addBtn.disabled = n >= MAX_SAMPLES;
    return ta;
  };
  addSample();

  const file = h('<input type="file" accept=".txt,.md,.markdown,.text,text/plain" class="hidden">');
  file.addEventListener('change', async () => {
    const f = file.files && file.files[0];
    file.value = '';
    if (!f) return;
    // enough bytes for the cap in any UTF-8 text, never the whole of a large file
    const fit = fitSample(await f.slice(0, MAX_SAMPLE_CHARS * 4 + 4).text());
    if (!fit) { toast(t('style_not_text'), 'error'); return; }
    const ta = areas().find((a) => !a.value.trim()) || (areas().length < MAX_SAMPLES ? addSample() : null);
    if (!ta) { toast(t('style_samples_full', { n: MAX_SAMPLES }), 'error'); return; }
    ta.value = fit.text;
    ta.dispatchEvent(new Event('input'));
    if (fit.cut) toast(t('style_cut', { n: MAX_SAMPLE_CHARS }));
  });
  const upload = button(t('style_upload'), 'sm', () => file.click());

  const gen = button(t('style_generate'), 'sm', async () => {
    if (prompt.value.trim() && !(await confirmDialog(t('style_replace_q'), { okLabel: t('style_replace'), danger: false }))) return;
    gen.disabled = true;
    gen.innerHTML = `<span class="spinner"></span> ${esc(t('style_generating'))}`;
    try {
      const draft = await call('style_from_samples', { samples: samplesOf(areas().map((a) => a.value)) });
      if (!prompt.isConnected) return;
      prompt.value = draft;
      prompt.focus();
      toast(t('style_draft_ready'));
    } catch (e) { errorToast(e); } finally { gen.disabled = false; gen.textContent = t('style_generate'); }
  });
  box.lastChild.append(addBtn, upload, gen, file);

  const body = h('<div></div>');
  body.append(labeled(t('name'), name), labeled(t('style_instructions'), prompt), box);
  const missing = (el, msg) => { el.classList.add('invalid'); el.focus(); toast(msg, 'error'); };
  const ok = button(t('save'), 'primary', () => {
    name.classList.remove('invalid');
    prompt.classList.remove('invalid');
    if (!name.value.trim()) return missing(name, t('style_need_name'));
    if (!prompt.value.trim()) return missing(prompt, t('style_need_prompt'));
    onSave({ name: name.value.trim(), prompt: prompt.value.trim() });
    m.close();
  });
  const m = modal({ title: style ? t('style_edit') : t('style_new'), body, footer: [button(t('cancel'), '', () => m.close()), ok] });
  body.addEventListener('keydown', (e) => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); ok.click(); } });
  setTimeout(() => name.focus(), 30);
}
