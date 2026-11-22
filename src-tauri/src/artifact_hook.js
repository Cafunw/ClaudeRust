(function () {
  var seq = 0, waiting = {};
  function report(message, stack) {
    var d = document.getElementById('__artifact_error') || document.createElement('pre');
    d.id = '__artifact_error';
    d.style.cssText = 'position:fixed;left:0;right:0;bottom:0;margin:0;padding:10px 14px;background:#fde8e4;color:#7a1f0e;font:12px/1.5 ui-monospace,monospace;white-space:pre-wrap;max-height:40%;overflow:auto;z-index:99999';
    d.textContent = 'Error: ' + message;
    document.body && document.body.appendChild(d);
    window.parent.postMessage({ __artifact: 'error', message: message, stack: stack }, '*');
  }
  window.addEventListener('error', function (e) {
    report(String(e.message || e.error || 'unknown'), e.error && e.error.stack ? String(e.error.stack).split('\n').slice(0, 6).join('\n') : '');
  });
  // a request the policy (ARTIFACT_CSP) blocked fails without an error of its own
  window.addEventListener('securitypolicyviolation', function (e) {
    report('The artifact sandbox blocked ' + (e.blockedURI || 'a request') + ' (' + e.effectiveDirective + '): artifacts cannot reach the network. Only scripts from the allowed CDNs and Google Fonts load, so data and images (inline SVG or data: URLs) must be part of the artifact.', '');
  });
  window.addEventListener('message', function (e) {
    var d = e.data;
    if (e.source !== window.parent || !d || d.__artifact !== 'reply' || !waiting[d.id]) return;
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
  window.claude = { complete: function (prompt) { return call('complete', [prompt]); } };
})();
