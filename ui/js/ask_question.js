// Claude's questions (Claude Code's AskUserQuestion): a card with each question's options, single or
// multiple choice, and a box to type another answer. The answers go back through the permission reply.
import { icon } from './icons.js';
import { t } from './i18n.js';
import { esc, h } from './util.js';

/**
 * The answers as Claude Code takes them: question text -> the option picked, several joined with ", ",
 * or what was typed under Other (after the options picked, on a multiple-choice question).
 * Questions left unanswered are left out.
 */
export function collectAnswers(questions, picks) {
  const out = {};
  questions.forEach((q, i) => {
    const p = picks[i] || {};
    const labels = (q.options || []).filter((_, j) => (p.chosen || []).includes(j)).map((o) => o.label);
    const other = (p.other || '').trim();
    const parts = q.multiSelect ? [...labels, ...(other ? [other] : [])] : [other || labels[0]].filter(Boolean);
    if (parts.length) out[q.question] = parts.join(', ');
  });
  return out;
}

/** The card for permission request `p`; `respond(decision, message)` answers it. */
export function questionCard(p, respond) {
  const questions = Array.isArray(p.input?.questions) ? p.input.questions : [];
  const card = h(`<div class="perm-card" data-req="${esc(p.request_id)}">
    <div class="ptitle">${icon('help', 17)} ${esc(t('aq_title'))}</div>
    <div style="height:8px"></div>
    <div class="pactions"></div>
  </div>`);
  const picks = questions.map(() => ({ chosen: [], other: '' }));
  const submit = h(`<button class="btn sm primary" disabled>${esc(t('aq_submit'))}</button>`);
  const answers = () => collectAnswers(questions, picks);
  const update = () => { submit.disabled = Object.keys(answers()).length < questions.length || !questions.length; };
  questions.forEach((q, i) => {
    const type = q.multiSelect ? 'checkbox' : 'radio';
    const name = `aq-${p.request_id}-${i}`;
    const set = h(`<fieldset style="border:0;margin:0 0 12px;padding:0;min-width:0">
      <legend style="font-size:14px;margin-bottom:6px;padding:0">${q.header ? `<span style="font-size:11.5px;padding:1px 7px;border-radius:999px;background:var(--bg-2);color:var(--text-2);margin-right:6px">${esc(q.header)}</span>` : ''}${esc(q.question)}</legend>
      ${(q.options || []).map((o, j) => `<label style="display:flex;gap:8px;align-items:flex-start;padding:4px 0;cursor:pointer">
        <input type="${type}" name="${esc(name)}" value="${j}" style="margin-top:3px">
        <span>${esc(o.label)}${o.description ? `<span class="pdesc" style="display:block;margin:2px 0 0">${esc(o.description)}</span>` : ''}</span>
      </label>`).join('')}
      <input class="input" style="margin-top:6px" placeholder="${esc(t('aq_other'))}" aria-label="${esc(t('aq_other'))}">
    </fieldset>`);
    const boxes = [...set.querySelectorAll(`input[type="${type}"]`)];
    const other = set.querySelector('input.input');
    for (const b of boxes) {
      b.addEventListener('change', () => {
        picks[i].chosen = boxes.filter((x) => x.checked).map((x) => Number(x.value));
        // one answer to a single-choice question: an option clears what was typed
        if (!q.multiSelect) { other.value = ''; picks[i].other = ''; }
        update();
      });
    }
    other.addEventListener('input', () => {
      picks[i].other = other.value;
      if (!q.multiSelect && other.value.trim()) { for (const b of boxes) b.checked = false; picks[i].chosen = []; }
      update();
    });
    other.addEventListener('keydown', (e) => { if (e.key === 'Enter' && !submit.disabled) submit.click(); });
    card.insertBefore(set, card.querySelector('.pactions'));
  });
  submit.onclick = () => respond('answer', JSON.stringify(answers()));
  const skip = h(`<button class="btn sm ghost">${esc(t('aq_skip'))}</button>`);
  skip.onclick = () => respond('deny');
  card.querySelector('.pactions').append(submit, skip);
  update();
  return card;
}
