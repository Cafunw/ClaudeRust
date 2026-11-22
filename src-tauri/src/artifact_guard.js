(function () {
  // The webview runs the app's start-up scripts in every frame, this one too:
  // their alert and confirm ask the app (confirm returns a Promise, so
  // `if (confirm(...))` is always true) and their click handler opens links
  // in the browser. A sandbox without allow-modals gives a page none of that.
  window.alert = function () {};
  window.confirm = function () { return false; };
  window.prompt = function () { return null; };
  // links to the web leave through the app, which asks the person first
  function leave(href) {
    var u;
    try { u = new URL(href, location.href); } catch (_) { return false; }
    if (u.origin === location.origin || !/^(https?|mailto):$/.test(u.protocol)) return false;
    window.parent.postMessage({ __artifact: 'link', url: u.href }, '*');
    return true;
  }
  window.open = function (url) { if (url) leave(String(url)); return null; };
  window.addEventListener('click', function (e) {
    var path = e.composedPath ? e.composedPath() : [e.target], a = null;
    for (var i = 0; i < path.length && !a; i++) if (path[i] && path[i].nodeName && path[i].nodeName.toUpperCase() === 'A') a = path[i];
    var href = a && (typeof a.href === 'string' ? a.href : a.href && a.href.baseVal);
    if (!href || (a.getAttribute('href') || '').charAt(0) === '#') return;
    if (leave(href)) e.preventDefault();
  }, true);
})();
