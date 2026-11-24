// How the artifact panel shows a version (artifacts.js): one being written as
// its source as it comes; one cut off (render.rs: a Stop, an error, the app
// closed) as it would be finished, marked so.

const DOC = 'application/vnd.ant.document';

/** `{ live, previewable, view, wrap }` for version `a`; `chosen`: the view the person picked, if any. */
export function panelView(a, chosen) {
  const live = !a.complete && !a.interrupted;
  const previewable = !!a.previewable || a.type === 'text/markdown';
  // prose source wraps at the panel's edge, as the document it is
  return { live, previewable, view: live ? 'code' : chosen || (previewable ? 'preview' : 'code'), wrap: a.type === 'text/markdown' || a.type === DOC };
}
