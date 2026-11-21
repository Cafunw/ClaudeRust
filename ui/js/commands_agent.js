// /review, /init, /doctor.
import { t } from './i18n.js';
import { icon } from './icons.js';
import { call } from './api.js';
import { esc, modal, errorToast } from './util.js';
import { registerCommand } from './slash.js';

const REVIEW_PROMPT = (focus) => `Review the current changes in this repository like a careful senior engineer.

1. Run \`git status\` and \`git diff HEAD\` (and look at new untracked files) to see what changed.
2. Read surrounding code where needed to understand each change.
3. Report problems ordered by severity — bugs and broken behaviour first, then security, performance, error handling, missing tests, and finally readability. For each: file:line, what is wrong, a concrete failure scenario, and the fix.
4. Say clearly if the change looks good to merge.

Do not modify any files.${focus ? `\n\nFocus especially on: ${focus}` : ''}`;

const INIT_PROMPT = `Please analyze this codebase and create a CLAUDE.md file in the project root, which will be given to future coding sessions working in this repository.

What to add:
1. Commands that will be commonly used: how to build, lint, run the tests, and run a single test.
2. High-level architecture and code structure — the "big picture" that requires reading several files to understand.

Usage notes:
- If there's already a CLAUDE.md, suggest improvements to it instead.
- Don't repeat yourself and don't include obvious instructions or generic development practices.
- Don't list every file or component; don't make up information that isn't backed by the project's files.
- If there are Cursor rules (.cursor/rules/, .cursorrules), Copilot instructions (.github/copilot-instructions.md) or an AGENTS.md, include their important parts.
- If there is a README, include its important parts.
- Start the file with:

# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.`;

registerCommand({
  name: 'review',
  description: () => t('cmd_review'),
  hint: '[focus | PR number or URL]',
  kinds: ['code'],
  run: async (args, ctx) => {
    if (ctx.engine === 'claude-code') return `/review ${args}`.trim();
    // `/review 42` or a pull request URL: review that pull request
    const a = (args || '').trim();
    if (/^#?\d+$/.test(a) || /^https?:\/\/\S+\/(pull|merge_requests)\/\d+/.test(a)) {
      try {
        const pr = await call('pr_fetch', { conversationId: ctx.conversationId, pr: a });
        const att = await call('stage_text', { name: `PR-${pr.number}.diff`, text: pr.diff || '' });
        return {
          prompt: t('review_pr_p', {
            n: pr.number, title: pr.title || '', author: (pr.author && pr.author.login) || '', head: pr.headRefName || '', base: pr.baseRefName || '',
            add: pr.additions ?? 0, del: pr.deletions ?? 0, files: pr.changedFiles ?? 0, url: pr.url || '', body: (pr.body || '').trim() || '—',
          }),
          attachments: [att],
        };
      } catch (e) { errorToast(e); return undefined; }
    }
    return REVIEW_PROMPT(args);
  },
});

registerCommand({
  name: 'init',
  description: () => t('cmd_init'),
  kinds: ['code'],
  run: async (_a, ctx) => (ctx.engine === 'claude-code' ? '/init' : INIT_PROMPT),
});

registerCommand({
  name: 'doctor',
  description: () => t('cmd_doctor'),
  run: async (_a, ctx) => {
    const m = modal({ title: t('doctor_title'), body: `<div class="pane-empty"><span class="spinner"></span></div>`, wide: true });
    try {
      const checks = await call('doctor', { conversationId: ctx.conversationId || null });
      const ic = { ok: ['check', 'ok'], warn: ['alert', 'warn'], error: ['x', 'err'] };
      m.body.innerHTML = `<div class="doctor">${checks.map((c) => {
        const [i, cls] = ic[c.status] || ic.warn;
        return `<div class="dr ${cls}"><span class="di">${icon(i, 14)}</span><b>${esc(c.name)}</b><span class="dd">${esc(c.detail || '')}</span></div>`;
      }).join('')}</div>`;
    } catch (e) { m.close(); errorToast(e); }
  },
});
