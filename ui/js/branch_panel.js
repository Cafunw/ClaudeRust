// The artifact panel folds away on a branch without artifacts (it would
// only say "No artifacts yet") and comes back on a branch with them, unless
// you closed it yourself in between.

/** What the panel does as the shown branch changes: 'close' when the branch
 *  has no artifacts and the panel is open, 'reopen' when it has them and the
 *  panel was folded by this rule (`parked`), else null. */
export function panelStep(hasArtifacts, open, parked) {
  if (!hasArtifacts && open) return 'close';
  if (hasArtifacts && !open && parked) return 'reopen';
  return null;
}

/** The artifact the open panel shows among the branch's `ids`: the one it
 *  showed, else the branch's latest (null: the branch has none). */
export function shownArtifact(ids, identifier) {
  if (ids.includes(identifier)) return identifier;
  return ids.length ? ids[ids.length - 1] : null;
}
