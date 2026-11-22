// The artifact panel closed and opened again (in a narrow window it is a
// sheet over the chat: "Back to chat", then the topbar's toggle). No DOM here.

/** Which of `ids` (a conversation's artifacts, oldest first) the topbar's
 *  toggle opens: the one shown when the panel of `convId` last closed
 *  (`last`: { convId, identifier }), while it's still there, else the newest. */
export function reopenId(ids, convId, last) {
  if (last && last.convId === convId && ids.includes(last.identifier)) return last.identifier;
  return ids.length ? ids[ids.length - 1] : null;
}

/** Where Tab (Shift+Tab with `back`) goes from control `at` of the sheet's
 *  `n`: the other end when it would leave the sheet (as in a dialog), else
 *  -1 (the browser moves it). */
export function wrapTab(n, at, back) {
  if (!n) return -1;
  if (back) return at <= 0 ? n - 1 : -1;
  return at === n - 1 || at < 0 ? 0 : -1;
}
