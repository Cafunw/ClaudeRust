// window.fs.readFile of an artifact, as claude.ai's: the files attached to
// its chat, by name. The app reads them for the frame it showed in that chat
// (ui/js/artifact_files.js); the page never names a conversation or a path.
(function () {
  var seq = 0, waiting = {};
  window.addEventListener('message', function (e) {
    var d = e.data;
    if (e.source !== window.parent || !d || d.__artifact !== 'fs-reply' || !waiting[d.id]) return;
    var w = waiting[d.id];
    delete waiting[d.id];
    if (d.error) w[1](new Error(d.error)); else w[0](d.result);
  });
  window.fs = {
    // resolves to a Uint8Array, or a string with { encoding: 'utf8' } (or 'utf8')
    readFile: function (name, options) {
      var encoding = options && typeof options === 'object' ? options.encoding : options;
      return new Promise(function (ok, fail) {
        var id = ++seq;
        waiting[id] = [ok, fail];
        window.parent.postMessage({ __artifact: 'fs', id: id, name: String(name), encoding: encoding == null ? null : String(encoding) }, '*');
      });
    }
  };
})();
