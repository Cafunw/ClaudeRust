(function () {
  var seq = 0, waiting = {};
  // the app shows the error above the frame and says so ('error-ack'): then the page draws none of its own
  var acked = false, pendingOverlay = null;
  function overlay(message) {
    pendingOverlay = null;
    if (acked) return;
    var d = document.getElementById('__artifact_error') || document.createElement('pre');
    d.id = '__artifact_error';
    d.style.cssText = 'position:fixed;left:0;right:0;bottom:0;margin:0;padding:10px 14px;background:#fde8e4;color:#7a1f0e;font:12px/1.5 ui-monospace,monospace;white-space:pre-wrap;max-height:40%;overflow:auto;z-index:99999';
    d.textContent = 'Error: ' + message;
    document.body && document.body.appendChild(d);
  }
  function report(message, stack, blocked) {
    window.parent.postMessage({ __artifact: 'error', message: message, stack: stack, blocked: blocked || null }, '*');
    if (pendingOverlay) clearTimeout(pendingOverlay);
    pendingOverlay = setTimeout(function () { overlay(message); }, 400);
  }
  window.addEventListener('error', function (e) {
    report(String(e.message || e.error || 'unknown'), e.error && e.error.stack ? String(e.error.stack).split('\n').slice(0, 6).join('\n') : '');
  });
  // a request the policy (ARTIFACT_CSP) blocked fails without an error of its own
  window.addEventListener('securitypolicyviolation', function (e) {
    var uri = String(e.blockedURI || 'a request'), directive = String(e.effectiveDirective || '');
    report('The artifact sandbox blocked ' + uri + ' (' + directive + '): artifacts cannot reach the network. Only the scripts the artifact instructions list (at those versions) and Google Fonts load, so data and images (inline SVG or data: URLs) must be part of the artifact.', '', { uri: uri, directive: directive });
  });
  window.addEventListener('message', function (e) {
    var d = e.data;
    if (e.source !== window.parent || !d) return;
    if (d.__artifact === 'error-ack') {
      acked = true;
      var shown = document.getElementById('__artifact_error');
      if (shown) shown.remove();
      return;
    }
    if (d.__artifact !== 'reply' || !waiting[d.id]) return;
    var w = waiting[d.id];
    delete waiting[d.id];
    if (d.error) w[1](new Error(d.error)); else w[0](d.result);
  });
  function call(op, args) {
    return new Promise(function (ok, fail) {
      var id = ++seq;
      waiting[id] = [ok, fail];
      window.parent.postMessage({ __artifact: 'call', id: id, op: op, args: args }, '*');
    });
  }
  window.storage = {
    get: function (key, shared) { return call('get', [key, !!shared]); },
    set: function (key, value, shared) { return call('set', [key, value, !!shared]); },
    delete: function (key, shared) { return call('delete', [key, !!shared]); },
    list: function (prefix, shared) { return call('list', [prefix, !!shared]); }
  };
  window.claude = {
    complete: function (prompt) { return call('complete', [prompt]); },
    // a connector's tool, once the user allows it: resolves to {content, structuredContent, isError}
    callMcpTool: function (name, args) { return call('callMcpTool', [name, args]); }
  };
  // Esc the page leaves alone takes the app out of full screen
  window.addEventListener('keydown', function (e) {
    if (e.key === 'Escape' && !e.defaultPrevented) window.parent.postMessage({ __artifact: 'escape' }, '*');
  });
})();
