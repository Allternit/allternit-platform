/* window.allternit — optional Allternit host extensions for MCP App Views.
 * Inline this file in the View HTML (the View CSP is script-src 'self' 'unsafe-inline' + resourceDomains).
 * Feature-detect: if (window.allternit && await window.allternit.detect()) { ... }
 * Reference: docs/ALLTERNIT_MCP_APP_EXTENSIONS.md */
(function (w) {
  "use strict";
  if (w.allternit || w.parent === w) return;
  var KEY = "x-allternit", pending = {}, seq = 0, ad = null;
  function note(v) { if (v && v[KEY] && !ad) ad = v[KEY]; }
  w.addEventListener("message", function (e) {
    var d = e.data;
    if (e.source !== w.parent || !d || d.jsonrpc !== "2.0") return;
    if (d.result) { note(d.result.hostCapabilities); note(d.result.hostContext); }
    if (d.params) note(d.params.hostContext || d.params);
    var p = d.id != null && pending[d.id];
    if (!p) return;
    delete pending[d.id];
    if (d.error) { var err = new Error(d.error.message || "request failed"); err.code = d.error.code; p[1](err); }
    else p[0](d.result);
  });
  function call(method, params) {
    return new Promise(function (ok, no) {
      var id = "allternit-" + ++seq;
      pending[id] = [ok, no];
      w.parent.postMessage({ jsonrpc: "2.0", id: id, method: KEY + "/" + method, params: params || {} }, "*");
    });
  }
  function has(m) { return !!ad && ad.methods.indexOf(KEY + "/" + m) >= 0; }
  w.allternit = {
    version: 1,
    /** Resolves true when the host advertises x-allternit (checks the host's capabilities). */
    detect: function () {
      if (ad) return Promise.resolve(true);
      return call("capabilities").then(function (r) { note({ "x-allternit": r }); return !!ad; }, function () { return false; });
    },
    supports: has,
    saveArtifact: function (a) { return call("saveArtifact", a); },
    openInAci: function (a) { return call("openInAci", a); },
    dispatchAgent: function (a) { return call("dispatchAgent", a); },
    handoffToComputer: function (a) { return call("handoffToComputer", a); },
    requestCheckout: function (a) { return call("requestCheckout", a); }
  };
})(window);
