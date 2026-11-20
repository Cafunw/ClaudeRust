// Helpers for the pull request dialog (mirrors pullrequest::branch_for).

/** A branch name for a pull request titled `title`. */
export function branchFor(title) {
  let slug = '';
  for (const c of String(title || '').toLowerCase()) {
    if (/[a-z0-9]/.test(c)) slug += c;
    else if (slug && !slug.endsWith('-')) slug += '-';
    if (slug.length >= 40) break;
  }
  slug = slug.replace(/^-+|-+$/g, '');
  return `clauderust/${slug || 'changes'}`;
}
