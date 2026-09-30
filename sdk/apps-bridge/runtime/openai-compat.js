/* window.openai — ChatGPT Apps SDK compatibility for MCP App Views.
 * Defines window.openai on top of the standard MCP Apps bridge (ui/* JSON-RPC), so a View written
 * for ChatGPT runs unchanged. The host injects this into the View HTML when the resource opts in
 * (skybridge MIME, openai/* metadata); it can also be inlined by hand. Dependency-free, no build.
 * Views that use the MCP Apps `App` class should not load this: it runs its own ui/initialize. */
(function (w) {
  "use strict";
  if (w.openai || w.parent === w) return;
  var parent = w.parent, KEY = "x-openai-compat", STATE_KEY = "openai/widgetState";
  var pending = {}, seq = 0, ctx = {}, sizeTimer = 0, lastHeight = -1;
  var openai = {
    toolInput: null, toolOutput: null, toolResponseMetadata: null, widgetState: null,
    theme: "light", displayMode: "inline", maxHeight: null, locale: "en-US",
    safeArea: { insets: { top: 0, right: 0, bottom: 0, left: 0 } },
    userAgent: { device: { type: "desktop" }, capabilities: { hover: true, touch: false } },
    view: { mode: "inline", params: null }
  };
  function note(msg) { try { console.info("[window.openai] " + msg); } catch (_) {} }
  function send(msg) { parent.postMessage(msg, "*"); }
  function notify(method, params) { send({ jsonrpc: "2.0", method: method, params: params || {} }); }
  function request(method, params) {
    return new Promise(function (ok, no) {
      var id = "oai-" + ++seq;
      pending[id] = [ok, no];
      send({ jsonrpc: "2.0", id: id, method: method, params: params || {} });
    });
  }
  function push(changed) {
    var keys = Object.keys(changed);
    if (!keys.length) return;
    keys.forEach(function (k) { openai[k] = changed[k]; });
    var ev;
    try { ev = new w.CustomEvent("openai:set_globals", { detail: { globals: changed } }); } catch (_) { return; }
    w.dispatchEvent(ev);
  }
  function fromContext(hc, initial) {
    var c = {}, d;
    if (!hc || typeof hc !== "object") return c;
    if (hc.theme) c.theme = hc.theme;
    if (hc.displayMode) { c.displayMode = hc.displayMode; c.view = { mode: hc.displayMode, params: null }; }
    if (hc.locale) c.locale = hc.locale;
    if (hc.containerDimensions) {
      d = hc.containerDimensions;
      c.maxHeight = typeof d.maxHeight === "number" ? d.maxHeight : typeof d.height === "number" ? d.height : null;
    }
    if (hc.safeAreaInsets) c.safeArea = { insets: hc.safeAreaInsets };
    if (hc.platform || hc.deviceCapabilities) {
      c.userAgent = {
        device: { type: hc.platform === "mobile" ? "mobile" : "desktop" },
        capabilities: { hover: !(hc.deviceCapabilities && hc.deviceCapabilities.hover === false), touch: !!(hc.deviceCapabilities && hc.deviceCapabilities.touch) }
      };
    }
    // The host restores the persisted widget state on load only; later writes are ours.
    if (initial && hc[KEY] && hc[KEY].widgetState !== undefined) c.widgetState = hc[KEY].widgetState;
    return c;
  }
  function textOf(result) {
    var c = result && result.content, out = [];
    if (Array.isArray(c)) c.forEach(function (b) { if (b && b.type === "text" && typeof b.text === "string") out.push(b.text); });
    return out.join("\n");
  }
  function onNotification(method, p) {
    p = p || {};
    if (method === "ui/notifications/tool-input") push({ toolInput: p.arguments || {} });
    else if (method === "ui/notifications/tool-result") {
      var meta = {}, k;
      if (p._meta) for (k in p._meta) meta[k] = p._meta[k];
      meta.status = "completed"; meta.call_tool_result = p; meta.mcp_tool_result = p;
      push({ toolOutput: p.structuredContent === undefined ? null : p.structuredContent, toolResponseMetadata: meta });
    } else if (method === "ui/notifications/host-context-changed") { var j; for (j in p) ctx[j] = p[j]; push(fromContext(p, false)); }
  }
  w.addEventListener("message", function (e) {
    var d = e.data, p;
    if (e.source !== parent || !d || d.jsonrpc !== "2.0") return;
    if (d.method) {
      if (d.id != null) {
        // Host → View request: acknowledge teardown/ping, refuse the rest.
        if (d.method === "ui/resource-teardown" || d.method === "ping") send({ jsonrpc: "2.0", id: d.id, result: {} });
        else send({ jsonrpc: "2.0", id: d.id, error: { code: -32601, message: "Method not found" } });
      } else onNotification(d.method, d.params);
      return;
    }
    p = d.id != null && pending[d.id];
    if (!p) return;
    delete pending[d.id];
    if (d.error) { var err = new Error(d.error.message || "request failed"); err.code = d.error.code; p[1](err); }
    else p[0](d.result);
  });
  function reportHeight(h) {
    h = Math.ceil(h);
    if (h === lastHeight || !(h > 0)) return;
    lastHeight = h;
    notify("ui/notifications/size-changed", { width: Math.ceil(w.innerWidth || 0), height: h });
  }
  function watchSize() {
    var el = w.document && w.document.documentElement;
    if (!el || typeof w.ResizeObserver !== "function") return;
    new w.ResizeObserver(function () {
      if (sizeTimer) return;
      sizeTimer = w.setTimeout(function () { sizeTimer = 0; reportHeight(el.scrollHeight); }, 50);
    }).observe(el);
  }

  openai.callTool = function (name, args) {
    return request("tools/call", { name: name, arguments: args || {} }).then(function (r) {
      var out = {}, k;
      for (k in r) out[k] = r[k];
      if (out.result === undefined) out.result = textOf(r);
      return out;
    });
  };
  openai.sendFollowUpMessage = function (a) {
    var prompt = typeof a === "string" ? a : a && a.prompt;
    return request("ui/message", { role: "user", content: [{ type: "text", text: String(prompt == null ? "" : prompt) }] }).then(function () {});
  };
  openai.sendFollowupTurn = openai.sendFollowUpMessage;
  openai.openExternal = function (a) {
    var href = typeof a === "string" ? a : a && a.href;
    return request("ui/open-link", { url: String(href) }).then(function () {});
  };
  openai.requestDisplayMode = function (a) {
    var mode = a && a.mode, avail = ctx.availableDisplayModes;
    if (avail && avail.indexOf(mode) < 0) {
      note("display mode '" + mode + "' is not supported by this host; staying '" + openai.displayMode + "'");
      return Promise.resolve({ mode: openai.displayMode });
    }
    return request("ui/request-display-mode", { mode: mode }).then(function (r) {
      var m = (r && r.mode) || openai.displayMode;
      if (m !== openai.displayMode) push({ displayMode: m, view: { mode: m, params: null } });
      return { mode: m };
    }, function () {
      note("host rejected ui/request-display-mode; staying '" + openai.displayMode + "'");
      return { mode: openai.displayMode };
    });
  };
  openai.requestClose = function () { notify("ui/notifications/request-teardown"); return Promise.resolve(); };
  openai.notifyIntrinsicHeight = function (h) { if (typeof h === "number") reportHeight(h); };
  openai.setOpenInAppUrl = function () { note("setOpenInAppUrl is not supported by this host (no-op)"); return Promise.resolve(); };
  // widgetState: the host persists what we send (per message) and restores it on load; ui/update-model-context
  // is the only standard channel, and ChatGPT also shows widgetState to the model.
  openai.setWidgetState = function (state) {
    push({ widgetState: state === undefined ? null : state });
    var sc = {};
    sc[STATE_KEY] = openai.widgetState;
    return request("ui/update-model-context", { structuredContent: sc }).then(function () {}, function (err) {
      note("widget state was not persisted: " + (err && err.message));
    });
  };
  // Not defined on purpose (Views feature-detect them): requestModal, uploadFile, selectFiles,
  // getFileDownloadUrl, requestCheckout.

  w.openai = openai;
  request("ui/initialize", {
    appInfo: { name: "window.openai compat", version: "1.0.0" },
    appCapabilities: {},
    protocolVersion: "2026-01-26"
  }).then(function (r) {
    ctx = (r && r.hostContext) || {};
    push(fromContext(ctx, true));
    notify("ui/notifications/initialized");
    watchSize();
  }, function (err) { note("ui/initialize failed: " + (err && err.message)); });
})(window);
