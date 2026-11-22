// Links a sandboxed page follows (an artifact's, an MCP App's): the frame
// guard (src-tauri/src/artifact_guard.js) hands them to the app instead.
import { t } from './i18n.js';
import { call } from './api.js';
import { confirmDialog, errorToast } from './util.js';

let asked = false;

/** Open web link `url` in the browser once the person says so, one question at a time. */
export async function openLink(url) {
  if (asked || typeof url !== 'string' || !/^(https?:|mailto:)/i.test(url)) return;
  asked = true;
  try {
    if (await confirmDialog(t('art_open_link', { url: url.slice(0, 500) }), { okLabel: t('open'), danger: false })) await call('open_external', { url });
  } catch (err) { errorToast(err); } finally { asked = false; }
}
