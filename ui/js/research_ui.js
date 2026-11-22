// Research mode's progress line, from research.rs's research-progress events.
import { t } from './i18n.js';
import { fmtUsd } from './util.js';

const PHASES = { planning: 'rs_planning', researching: 'rs_searching', writing: 'rs_writing' };

/** "Searching the web · 2 of 5 questions · $0.12 of $2.00" for a research-progress event (any field may be missing). */
export function researchLine(p) {
  const e = p || {};
  const bits = [PHASES[e.phase] ? t(PHASES[e.phase]) : t('rs_working')];
  const done = Number(e.done);
  const total = Number(e.total);
  if (total > 0 && Number.isFinite(done)) bits.push(t('rs_steps', { done: Math.min(done, total), total }));
  if (Number(e.budget_usd) > 0) bits.push(t('rs_spent', { spent: fmtUsd(e.spent_usd), budget: fmtUsd(e.budget_usd) }));
  return bits.join(' · ');
}

/** The latest web search of the research, quoted; '' when there is none. */
export function researchQuery(p) {
  const q = p && Array.isArray(p.queries) ? p.queries[p.queries.length - 1] : '';
  return typeof q === 'string' && q ? `“${q}”` : '';
}
