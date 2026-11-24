// window.fs.readFile of the artifact shown in the side panel
// (src-tauri/src/artifact_fs.js asks): the files attached to the chat that
// frame was shown for, read by the backend. Only our own frames are answered.
import { call } from './api.js';
import { readFileFor } from './artifact_bridge.js';

window.addEventListener('message', (e) => {
  const d = e.data;
  if (!d || d.__artifact !== 'fs') return;
  const frame = [...document.querySelectorAll('#artifact-panel iframe')].find((f) => f.contentWindow === e.source);
  if (!frame) return;
  const reply = (r) => e.source.postMessage({ __artifact: 'fs-reply', id: d.id, ...r }, '*');
  readFileFor(frame.dataset.conv, d, (args) => call('artifact_read_file', args))
    .then((result) => reply({ result }), (err) => reply({ error: String((err && err.message) || err) }));
});
