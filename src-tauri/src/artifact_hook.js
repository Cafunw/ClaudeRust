(function () {
  var seq = 0, waiting = {};
  window.addEventListener('error', function (e) {
    var message = String(e.message || e.error || 'unknown');
    var d = document.getElementById('__artifact_error') || document.createElement('pre');
    d.id = '__artifact_error';
    d.style.cssText = 'position:fixed;left:0;right:0;bottom:0;margin:0;padding:10px 14px;background:#fde8e4;color:#7a1f0e;font:12px/1.5 ui-monospace,monospace;white-space:pre-wrap;max-height:40%;overflow:auto;z-index:99999';
    d.textContent = 'Error: ' + message;
    document.body && document.body.appendChild(d);
    var stack = e.error && e.error.stack ? String(e.error.stack).split('\n').slice(0, 6).join('\n') : '';
    window.parent.postMessage({ __artifact: 'error', message: message, stack: stack }, '*');
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
