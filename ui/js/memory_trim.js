// After a long conversation is closed, WebView2 is asked to give back what it
// kept for it (turnwindow.js trimmer, webview_memory.rs).
import { call } from './api.js';
import { store, on } from './store.js';
import { trimmer } from './turnwindow.js';

const left = trimmer(
  () => call('trim_memory').catch(() => {}),
  () => !!document.querySelector('#view .streaming-indicator:not(.hidden)'),
);

let at = '';
// route-changed comes before the next view is drawn: #view still holds the one being left
on('route-changed', () => {
  const key = `${store.route.name}:${store.route.params.id || ''}`;
  if (key === at) return; // the same conversation drawn again
  at = key;
  left(document.querySelectorAll('#view .turn').length);
});
