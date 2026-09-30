/* Allternit view kit — framework-free web components for MCP App Views.
 * <allternit-card>, <allternit-table>, <allternit-list>, <allternit-form>, <allternit-detail>.
 * Inline this file in the View HTML (View CSP allows inline scripts) or load it from resourceDomains.
 * Every visual value is a look-pack CSS variable (--vp-*) with the Allternit default as fallback.
 * Data is untrusted: it is only ever written with textContent; links and images must be https. */
(function (w) {
  "use strict";
  if (!w.customElements || w.AllternitUI) return;

  var STYLE =
    ":host{display:block;box-sizing:border-box;font-family:var(--vp-font-body,system-ui,sans-serif);font-size:13px;" +
    "color:var(--vp-color-text,#1f1e1d);background:var(--vp-color-surface,#fff);border:1px solid var(--vp-color-border,rgba(0,0,0,.12));" +
    "border-radius:var(--vp-radius-card,10px);overflow:hidden}" +
    "*{box-sizing:border-box}.pad{padding:var(--vp-spacing-md,12px)}.gap{display:grid;gap:var(--vp-spacing-sm,8px)}" +
    ".title{font-weight:600;font-size:14px;margin:0}.sub,.muted{color:var(--vp-color-text-muted,#5f5e5b)}" +
    "a{color:inherit}table{width:100%;border-collapse:collapse}th,td{text-align:left;padding:6px var(--vp-spacing-md,12px);" +
    "border-bottom:1px solid var(--vp-color-border,rgba(0,0,0,.08));overflow-wrap:anywhere}th{font-weight:600;" +
    "background:var(--vp-color-fill,rgba(0,0,0,.04))}ul{list-style:none;margin:0;padding:0}li{padding:8px var(--vp-spacing-md,12px);" +
    "border-bottom:1px solid var(--vp-color-border,rgba(0,0,0,.08))}li:last-child,tr:last-child td{border-bottom:0}" +
    "dl{display:grid;grid-template-columns:max-content 1fr;gap:4px 12px;margin:0}dt{color:var(--vp-color-text-muted,#5f5e5b)}dd{margin:0;overflow-wrap:anywhere}" +
    "label{display:grid;gap:4px}input,select,textarea{font:inherit;color:inherit;background:var(--vp-color-surface,#fff);" +
    "border:1px solid var(--vp-color-border,rgba(0,0,0,.25));border-radius:calc(var(--vp-radius-card,10px) / 2);padding:6px 8px}" +
    "button{font:inherit;cursor:pointer;border:0;border-radius:calc(var(--vp-radius-card,10px) / 2);padding:7px 14px;" +
    "background:var(--vp-color-brand,#1f1e1d);color:var(--vp-color-on-brand,#fff)}" +
    "button:focus-visible,input:focus-visible,select:focus-visible,textarea:focus-visible,a:focus-visible{outline:2px solid var(--vp-color-brand,#1f1e1d);outline-offset:2px}" +
    "img{width:40px;height:40px;border-radius:calc(var(--vp-radius-card,10px) / 2);object-fit:cover;background:var(--vp-color-fill,rgba(0,0,0,.06))}" +
    ".row{display:flex;gap:var(--vp-spacing-md,12px);align-items:center}.empty{padding:var(--vp-spacing-md,12px)}" +
    "@media (prefers-reduced-motion:no-preference){button{transition:opacity .12s}button:hover{opacity:.9}}";

  function isObj(v) { return v !== null && typeof v === "object" && !Array.isArray(v); }
  function text(v, max) {
    var s;
    if (v === null || v === undefined) return "";
    if (isObj(v)) { var k = ["name", "title", "label", "login", "email"].filter(function (x) { return typeof v[x] === "string"; })[0]; s = k ? v[k] : ""; }
    else s = String(v);
    return s.length > (max || 500) ? s.slice(0, max || 500) + "…" : s;
  }
  /** https URLs only; anything else (javascript:, data:, http:) is dropped. */
  function safeHref(v) {
    if (typeof v !== "string") return null;
    try { var u = new URL(v); return u.protocol === "https:" ? u.href : null; } catch (_) { return null; }
  }
  function label(key, labels) {
    if (labels && typeof labels[key] === "string") return labels[key];
    var s = String(key).replace(/[_-]+/g, " ").replace(/([a-z])([A-Z])/g, "$1 $2");
    return s.charAt(0).toUpperCase() + s.slice(1);
  }
  function el(tag, cls, content) {
    var n = document.createElement(tag);
    if (cls) n.className = cls;
    if (content !== undefined) n.textContent = content;
    return n;
  }
  function link(href, content) {
    var safe = safeHref(href), a;
    if (!safe) return el("span", "", content);
    a = el("a", "", content);
    a.href = safe; a.target = "_blank"; a.rel = "noopener noreferrer";
    return a;
  }
  function image(src) {
    var safe = safeHref(src), i;
    if (!safe) return null;
    i = document.createElement("img");
    i.src = safe; i.alt = ""; i.referrerPolicy = "no-referrer";
    return i;
  }

  /** Base class: a `data` property, a `fields` mapping property, and a render() into the shadow root. */
  function define(tag, render) {
    if (w.customElements.get(tag)) return;
    var C = class extends HTMLElement {
      constructor() {
        super();
        this._data = undefined; this._fields = {};
        this._root = this.attachShadow({ mode: "open" });
        this._root.appendChild(el("style", "", STYLE));
        this._body = this._root.appendChild(el("div"));
      }
      get data() { return this._data; }
      set data(v) { this._data = v; this._draw(); }
      get fields() { return this._fields; }
      set fields(v) { this._fields = isObj(v) ? v : {}; this._draw(); }
      connectedCallback() { this._draw(); }
      _draw() {
        if (!this.isConnected) return;
        var next = el("div");
        try { render.call(this, next, this._data, this._fields); } catch (_) { next.appendChild(el("div", "empty muted", "Could not show this result.")); }
        this._root.replaceChild(next, this._body);
        this._body = next;
      }
    };
    w.customElements.define(tag, C);
  }

  function metaKeys(data, f, skip) {
    if (Array.isArray(f.meta)) return f.meta;
    return Object.keys(data).filter(function (k) { return skip.indexOf(k) < 0 && (data[k] === null || typeof data[k] !== "object"); }).slice(0, 6);
  }
  function dl(data, keys, f) {
    var rows = keys.filter(function (k) { return data[k] !== undefined && data[k] !== null && data[k] !== ""; });
    if (!rows.length) return null;
    var list = el("dl");
    rows.forEach(function (k) { list.appendChild(el("dt", "", label(k, f.labels))); list.appendChild(el("dd", "", text(data[k], 200))); });
    return list;
  }

  define("allternit-card", function (root, data, f) {
    if (!isObj(data)) { root.appendChild(el("div", "empty muted", "No data.")); return; }
    var wrap = el("div", "pad gap"), head = el("div", "row"), titles = el("div");
    var img = image(f.image ? data[f.image] : undefined);
    if (img) head.appendChild(img);
    var title = f.title ? text(data[f.title], 200) : "";
    if (title) { var t = el("h3", "title"); t.appendChild(link(f.url ? data[f.url] : undefined, title)); titles.appendChild(t); }
    if (f.subtitle && data[f.subtitle]) titles.appendChild(el("div", "sub", text(data[f.subtitle], 200)));
    head.appendChild(titles); wrap.appendChild(head);
    if (f.description && data[f.description]) wrap.appendChild(el("div", "", text(data[f.description], 600)));
    var rows = dl(data, metaKeys(data, f, [f.title, f.subtitle, f.description, f.url, f.image]), f);
    if (rows) wrap.appendChild(rows);
    root.appendChild(wrap);
  });

  define("allternit-detail", function (root, data, f) {
    if (!isObj(data)) { root.appendChild(el("div", "empty muted", "No data.")); return; }
    var wrap = el("div", "pad gap");
    if (f.title && data[f.title]) wrap.appendChild(el("h3", "title", text(data[f.title], 200)));
    var keys = Array.isArray(f.columns) ? f.columns : Object.keys(data).filter(function (k) { return k !== f.title && (data[k] === null || typeof data[k] !== "object"); }).slice(0, 24);
    var rows = dl(data, keys, f);
    if (rows) wrap.appendChild(rows); else wrap.appendChild(el("div", "muted", "Nothing to show."));
    root.appendChild(wrap);
  });

  define("allternit-table", function (root, data, f) {
    var rows = Array.isArray(data) ? data.filter(isObj) : [];
    if (!rows.length) { root.appendChild(el("div", "empty muted", "No rows.")); return; }
    var cols = Array.isArray(f.columns) && f.columns.length ? f.columns : Object.keys(rows[0]).filter(function (k) { return rows[0][k] === null || typeof rows[0][k] !== "object"; }).slice(0, 6);
    var table = el("table"), head = el("tr"), body = el("tbody");
    cols.forEach(function (c) { var th = el("th", "", label(c, f.labels)); th.scope = "col"; head.appendChild(th); });
    table.appendChild(el("thead")).appendChild(head);
    rows.slice(0, 200).forEach(function (r) {
      var tr = el("tr");
      cols.forEach(function (c) { tr.appendChild(el("td", "", text(r[c], 200))); });
      body.appendChild(tr);
    });
    table.appendChild(body); root.appendChild(table);
  });

  define("allternit-list", function (root, data, f) {
    var items = Array.isArray(data) ? data : [];
    if (!items.length) { root.appendChild(el("div", "empty muted", "Nothing here.")); return; }
    var ul = el("ul");
    items.slice(0, 100).forEach(function (it) {
      var li = el("li"), obj = isObj(it);
      var t = obj ? text(it[f.title || "title"] || it.name || it.id, 200) : text(it, 200);
      li.appendChild(el("div", "title", "")).appendChild(obj ? link(f.url ? it[f.url] : it.url, t) : el("span", "", t));
      var s = obj && f.subtitle ? text(it[f.subtitle], 200) : "";
      if (s) li.appendChild(el("div", "sub", s));
      ul.appendChild(li);
    });
    root.appendChild(ul);
  });

  /** data: { fields: [{ name, label?, type?, required?, options?, value? }], submitLabel? }. Fires `allternit-submit` with { values }. */
  define("allternit-form", function (root, data) {
    var spec = isObj(data) && Array.isArray(data.fields) ? data.fields : [];
    if (!spec.length) { root.appendChild(el("div", "empty muted", "No fields.")); return; }
    var host = this, form = el("form", "pad gap"), inputs = {};
    spec.forEach(function (fd, i) {
      if (!isObj(fd) || typeof fd.name !== "string") return;
      var lab = el("label"), id = "f" + i, input;
      lab.appendChild(el("span", "", text(fd.label || label(fd.name), 100)));
      if (Array.isArray(fd.options)) {
        input = document.createElement("select");
        fd.options.forEach(function (o) { var op = el("option", "", text(o, 100)); op.value = String(o); input.appendChild(op); });
      } else if (fd.type === "textarea") { input = document.createElement("textarea"); input.rows = 3; }
      else { input = document.createElement("input"); input.type = ["number", "email", "date", "checkbox", "url"].indexOf(fd.type) >= 0 ? fd.type : "text"; }
      input.id = id; input.name = fd.name; input.required = !!fd.required;
      if (fd.value !== undefined && input.type !== "checkbox") input.value = String(fd.value);
      inputs[fd.name] = input; lab.appendChild(input); form.appendChild(lab);
    });
    var btn = el("button", "", text(data.submitLabel || "Submit", 40)); btn.type = "submit";
    form.appendChild(btn);
    form.addEventListener("submit", function (e) {
      e.preventDefault();
      var values = {};
      Object.keys(inputs).forEach(function (k) { values[k] = inputs[k].type === "checkbox" ? inputs[k].checked : inputs[k].type === "number" && inputs[k].value !== "" ? Number(inputs[k].value) : inputs[k].value; });
      host.dispatchEvent(new CustomEvent("allternit-submit", { detail: { values: values }, bubbles: true, composed: true }));
    });
    root.appendChild(form);
  });

  w.AllternitUI = { version: 1, safeHref: safeHref, tags: ["allternit-card", "allternit-table", "allternit-list", "allternit-form", "allternit-detail"] };
})(window);
