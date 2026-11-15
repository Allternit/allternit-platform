---
title: On-device models — Model Lab for every device
template: doc
---
Lead: All picks are in (11-15). The tab keeps the name Freeweights Studio. Grid is the default view. Freeweights Studio (view `model-lab`) becomes the one place to find, download and run open models, on every device Allternit runs on. Its catalog uses the Freeweights card system (list, grid, deck), so a model looks the same on freeweights.allternit.com and in the app. Each card adds one line for the device you are on: a fit label, the file to use and a Run button.

The mockups run on real Freeweights catalog rows. Switch the device at the top of each mockup to see Mac, Windows, Windows on Arm, Linux, CPU-only Linux, browser, iPhone and Android.

## A Your picks and the new Model Lab

```kv
iOS engines: llama.cpp xcframework + MLX Swift + Apple Foundation Models
Android: native llama.cpp engine in the app + WebGPU in the PWA
Phone downloads: any network, in the background
Home of the feature: Model Lab (today titled "Freeweights Studio")
Catalog look: Freeweights card system, list / grid / deck, same as the site
```

```tree
Model Lab
  ~ Catalog: Freeweights cards (list, grid, deck) + fit label and Run for this device
  + This device: hardware, engines, running models, downloads, storage, recommended
  - Engine tab (moves into This device)
  - Studio tab (Local Studio controller moves into This device > Advanced)
  ~ Playground: model picker grouped by device, stats line on each reply
  Train (Freeweights Studio trainer lane, not changed by this work)
  Cloud (not changed)
```

```callout ok Decided 11-15
Name: Freeweights Studio (view id `model-lab` stays).
```

```callout ok Decided 11-15
Engine and Studio fold into one "This device" tab.
```

## B What Model Lab and Freeweights look like today

The real Model Lab, captured from the dev preview of origin/main. It has six tabs and one fixed card grid with no list, grid or deck switch. On a machine without the engine, every card says "Fit Unknown".

![Model Lab today: Catalog tab (desktop, origin/main dev preview)](ml-catalog.png)

![Model Lab today: Engine tab, engine not running](ml-engine.png)

![Model Lab today: phone layout](ml-phone.png)

The live Freeweights site (collections/gguf) with its three views. Model Lab's new catalog uses these exact faces from `site/assets/fw-cards.js` and `fw.css`.

![freeweights.allternit.com, List view with preview pane](fw-d-list.png)

![freeweights.allternit.com, Grid view](fw-d-grid.png)

![freeweights.allternit.com, Deck view](fw-d-deck.png)

## C How other apps present their catalog and setup

Real screenshots from each project's own docs or repo.

![Lemonade (AMD): Model Manager. Rail with All / Downloaded / Favorites, task and backend chips, storage bar; list with Running dot, size and backend per row](https://raw.githubusercontent.com/lemonade-sdk/assets/refs/heads/main/docs/model_manager_03.png)

![Google AI Edge Gallery (Android): model cards with size, license, Benchmark and Try it](https://github.com/user-attachments/assets/ebff12e8-60d5-4efa-96dd-2eaa5f5457c4)

![Google AI Edge Gallery: chat with a stats strip (tok/s, first token, RAM, context used)](https://github.com/user-attachments/assets/c4891c86-e114-44cf-be99-5f948fc77bc3)

![LM Studio: chat with the model loader on top and RAM/CPU in the status bar](https://lmstudio.ai/assets/marketing/docs/chat.png)

![Intel AI Playground: settings with backend status and Manage Backend Components](https://raw.githubusercontent.com/intel/AI-Playground/main/docs/readme/ui-dark-settings.png)

![Hugging Face: "Use this model" lists the local apps that can run the model](https://huggingface.co/datasets/huggingface/documentation-images/resolve/main/local-apps/button.png)

```table
| Project | Catalog | First run and engines | Running state | What Model Lab takes |
|---|---|---|---|---|
| Llama (llama.app, llama.cpp + Hugging Face) | "Recommended for this Mac/PC", then the web catalog | Picks quant, context and batch from the hardware | Unloads after 5 min idle; API on localhost | "Recommended for this device"; auto quant; idle unload |
| Hugging Face | Model pages; hardware compatibility panel per quant | You save your hardware once | n/a | Fit per quant from a device profile |
| Lemonade (AMD) | Rail filters + Downloaded above catalog | Backend matrix: Metal, CUDA, ROCm, Vulkan, CPU, XDNA2 NPU | Running dot on the row | Downloaded first; engine label on each row; storage bar |
| Foundry Local (Microsoft) | One alias per model, hardware variants behind it | Picks CUDA, DirectML/WebGPU, NPU or CPU at load | n/a | One card per model; the app picks GGUF, MLX or WebGPU file |
| Jan | Hugging Face ID search; quants as Small / Balanced / Large | llama.cpp inside the app; backend dropdown per GPU | Top-bar download progress | Three-state fit label with a reason |
| LM Studio | Discover with staff picks; quant list per model | Engine packs per backend | Loader at the top; RAM/CPU bar | Model loader in Playground |
| Intel AI Playground | Presets | First-run hardware mode; backend status list with Manage | n/a | Engines list with status and update button |
| Google AI Edge Gallery | Task first, then compatible models | Engine inside the app | Stats per reply: tok/s, first token, RAM, context | Stats line on each on-device reply |
| PocketPal AI | Curated list + Hugging Face GGUF search | llama.cpp inside the app | Benchmark screen | Hugging Face search inside the catalog |
```

Not found as real screenshots: Ollama app, Jan model hub, Msty, GPT4All, Private LLM, Locally AI, MLC Chat, Foundry Local.

## D Model Lab Catalog on every device

Click List, Grid and Deck, and switch the device at the top. The card faces come from `fw-cards.js`, the same code the site runs. Sizes are the file you download (the catalog shows the whole repo today). File sizes are illustrative until the per-quant field lands.

```react css=fw.css title="Model Lab › Catalog: Freeweights cards + fit and Run per device" h=900
import React from 'react';
// Freeweights shared rendering: the Allternit dot lettering (same 10-unit grid, 8.5 dots,
// 1.5 radius as the A://TERNIT wordmark) and the four preview card styles (A to D).
// Loaded by site/build.mjs at build time and by the browser for grid, deck and style switching.
(function (g) {
  var FONT = {
    A: ['..#..', '.#.#.', '#####', '#...#', '#...#'], B: ['####.', '#...#', '####.', '#...#', '####.'],
    C: ['.####', '#....', '#....', '#....', '.####'], D: ['####.', '#...#', '#...#', '#...#', '####.'],
    E: ['#####', '#....', '####.', '#....', '#####'], F: ['#####', '#....', '####.', '#....', '#....'],
    G: ['.####', '#....', '#.###', '#...#', '.###.'], H: ['#...#', '#...#', '#####', '#...#', '#...#'],
    I: ['###', '.#.', '.#.', '.#.', '###'], J: ['..###', '...#.', '...#.', '#..#.', '.##..'],
    K: ['#...#', '#..#.', '###..', '#..#.', '#...#'], L: ['#....', '#....', '#....', '#....', '#####'],
    M: ['#...#', '##.##', '#.#.#', '#...#', '#...#'], N: ['#...#', '##..#', '#.#.#', '#..##', '#...#'],
    O: ['.###.', '#...#', '#...#', '#...#', '.###.'], P: ['####.', '#...#', '####.', '#....', '#....'],
    Q: ['.###.', '#...#', '#.#.#', '#..#.', '.##.#'], R: ['####.', '#...#', '####.', '#.#..', '#..##'],
    S: ['.####', '#....', '.###.', '....#', '####.'], T: ['#####', '..#..', '..#..', '..#..', '..#..'],
    U: ['#...#', '#...#', '#...#', '#...#', '.###.'], V: ['#...#', '#...#', '#...#', '.#.#.', '..#..'],
    W: ['#...#', '#...#', '#.#.#', '##.##', '#...#'], X: ['#...#', '.#.#.', '..#..', '.#.#.', '#...#'],
    Y: ['#...#', '.#.#.', '..#..', '..#..', '..#..'], Z: ['#####', '...#.', '..#..', '.#...', '#####'],
    0: ['.###.', '#..##', '#.#.#', '##..#', '.###.'], 1: ['.#.', '##.', '.#.', '.#.', '###'],
    2: ['####.', '....#', '.###.', '#....', '#####'], 3: ['####.', '....#', '.###.', '....#', '####.'],
    4: ['#..#.', '#..#.', '#####', '...#.', '...#.'], 5: ['#####', '#....', '####.', '....#', '####.'],
    6: ['.###.', '#....', '####.', '#...#', '.###.'], 7: ['#####', '....#', '...#.', '..#..', '..#..'],
    8: ['.###.', '#...#', '.###.', '#...#', '.###.'], 9: ['.###.', '#...#', '.####', '....#', '.###.'],
    '.': ['.', '.', '.', '.', '#'], ':': ['.', '#', '.', '#', '.'], '/': ['..#', '..#', '.#.', '#..', '#..'],
    '-': ['...', '...', '###', '...', '...'], ' ': ['..', '..', '..', '..', '..']
  };
  var CELL = 10;
  function rect(c, r, fill) { return '<rect x="' + (c * CELL + 0.75) + '" y="' + (r * CELL + 0.75) + '" width="8.5" height="8.5" rx="1.5"' + (fill ? ' fill="' + fill + '"' : '') + '/>'; }
  // Lay out text on the dot grid. Returns { cols, rows, body }. Letters are separated by one empty column.
  function dots(text, col0, row0) {
    var c = col0 || 0, r0 = row0 || 0, body = '', chars = String(text).toUpperCase().split('');
    chars.forEach(function (ch, i) {
      var gl = FONT[ch] || FONT[' '];
      for (var r = 0; r < 5; r++) for (var x = 0; x < gl[r].length; x++) if (gl[r][x] === '#') body += rect(c + x, r0 + r);
      c += gl[0].length + (i < chars.length - 1 ? 1 : 0);
    });
    return { cols: c - (col0 || 0), body: body };
  }
  // The A://TERNIT wordmark cells, copied from the brand SVG (o = the coral dot).
  var WM = ['..#.......#...#.#####.#####.####..#...#..###..#####', '.#.#..#...#...#...#...#.....#...#.##..#...#.....#..', '##o##....#...#....#...####..####..#.#.#...#.....#..', '#...#.#.#...#.....#...#.....#.#...#..##...#.....#..', '#...#...#...#.....#...#####.#..##.#...#..###....#..'];
  function wordmark(col0, row0) {
    var b = '';
    WM.forEach(function (line, r) { for (var x = 0; x < line.length; x++) { if (line[x] === '#') b += rect(col0 + x, row0 + r); else if (line[x] === 'o') b += rect(col0 + x, row0 + r, '#D97757'); } });
    return { cols: WM[0].length, body: b };
  }
  function svg(cols, rows, body, h, label, cls) {
    var w = Math.round((h * cols) / rows * 100) / 100;
    return '<svg class="' + (cls || 'dotw') + '" viewBox="0 0 ' + cols * CELL + ' ' + rows * CELL + '" height="' + h + '" width="' + w + '" fill="currentColor" shape-rendering="geometricPrecision"' + (label ? ' role="img" aria-label="' + esc(label) + '"' : ' aria-hidden="true"') + '>' + body + '</svg>';
  }
  // Lockups. inline: A://TERNIT  FREEWEIGHTS on one line. stack: wordmark above the name.
  function lockup(kind, h, cls) {
    var a = wordmark(0, 0);
    if (kind === 'stack') { var n = dots('FREEWEIGHTS', 0, 7); return svg(Math.max(a.cols, n.cols), 12, a.body + n.body, h, 'Allternit Freeweights', cls || 'lockup stack'); }
    if (kind === 'name') { var o = dots('FREEWEIGHTS'); return svg(o.cols, 5, o.body, h, 'Freeweights', cls || 'lockup name'); }
    var m = dots('FREEWEIGHTS', a.cols + 4, 0); return svg(a.cols + 4 + m.cols, 5, a.body + m.body, h, 'Allternit Freeweights', cls || 'lockup inline');
  }
  function word(text, h, cls, label) { var d = dots(text); return svg(d.cols, 5, d.body, h, label, cls); }

  var esc = function (s) { return String(s == null ? '' : s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'); };
  function bytes(n) {
    if (n == null || isNaN(n)) return 'size unknown';
    var u = ['B', 'KB', 'MB', 'GB', 'TB'], i = 0, v = Number(n);
    while (v >= 1000 && i < u.length - 1) { v /= 1000; i++; }
    return (v >= 100 || i === 0 ? Math.round(v).toLocaleString('en-US') : v >= 10 ? v.toFixed(1) : v.toFixed(2)) + ' ' + u[i];
  }
  function sizeParts(n) { var s = bytes(n).split(' '); return { v: s[0], u: s[1] || '' }; }
  function params(n) { return !n ? '' : n >= 1e12 ? +(n / 1e12).toFixed(2) + 'T' : n >= 1e9 ? +(n / 1e9).toFixed(1) + 'B' : +(n / 1e6).toFixed(0) + 'M'; }
  var LC = { copyleft: 'copyleft', permissive: 'permissive', custom: 'custom terms', 'non-commercial': 'non-commercial', gated: 'gated', territory: 'territory-limited', unreviewed: 'license not reviewed' };
  function src(r) { return r.nc ? (r.how || 'From the publisher') : r.src || 'From source'; }
  function srcBadge(r) { var cls = r.nc || /source|publisher/i.test(src(r)) ? ' src' : /seed/i.test(src(r)) ? ' seed' : ''; return '<span class="b' + cls + '"><i class="d"></i>' + esc(src(r)) + '</span>'; }
  function lcTag(r) { return '<span class="lc lc-' + esc(r.lc || 'unknown') + '">' + esc(LC[r.lc] || r.lc || 'see card') + '</span>'; }
  function task(r) { var t = (r.task || [])[0]; return t ? t.replace(/-/g, ' ') : r.sec === 'alt.freeweights.datasets' ? 'dataset' : ''; }
  function date(t) { var d = new Date(t); if (isNaN(d)) return ''; return d.toUTCString().slice(0, 16); }
  function mono(r, h, cls) { return word(r.mono || 'FW', h, cls || 'mono-svg', null); }

  // A. Spec sheet
  function cardA(r) {
    var spec = [['Params', params(r.params) || 'not stated'], ['Size', bytes(r.size)], ['Format', (r.fmt || []).join(', ') || 'see files'], ['License', esc(r.lic)]];
    return '<a class="fwc fwc-a" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '">' +
      '<div class="a-top"><span class="a-mono">' + mono(r, 22) + '</span><div class="a-t"><div class="a-sec mono">' + esc(r.sec) + '</div><div class="a-name">' + esc(r.n) + '</div><div class="a-id mono">' + esc(r.id) + '</div></div></div>' +
      '<dl class="a-spec">' + spec.map(function (s) { return '<div><dt>' + s[0] + '</dt><dd' + (s[0] === 'Format' ? ' class="mono"' : '') + '>' + (s[0] === 'License' ? s[1] : esc(s[1])) + '</dd></div>'; }).join('') + '</dl>' +
      '<div class="a-foot">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + (task(r) ? '<span class="a-task">' + esc(task(r)) + '</span>' : '') + '</div></a>';
  }
  // B. Newsgroup post
  function cardB(r) {
    return '<a class="fwc fwc-b" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '">' +
      '<div class="b-head mono"><div><b>From:</b> ' + esc(r.org) + ' &lt;' + esc(r.host || 'huggingface.co') + '/' + esc(r.org) + '&gt;</div><div><b>Newsgroups:</b> ' + esc(r.sec) + '</div><div><b>Subject:</b> <span class="b-subj">' + esc(r.n) + '</span> (' + esc(bytes(r.size)) + ')</div><div><b>Date:</b> ' + esc(date(r.added)) + '</div></div>' +
      '<p class="b-body">' + esc(r.sum) + '</p>' +
      '<div class="b-sig mono">-- <br>' + esc(r.lic) + ' · ' + esc(LC[r.lc] || r.lc || '') + ' · ' + esc(src(r).toLowerCase()) + (r.rt ? ' · read terms first' : '') + '</div></a>';
  }
  // C. Element tile
  function cardC(r) {
    var s = sizeParts(r.size);
    return '<a class="fwc fwc-c" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '">' +
      '<div class="c-row mono"><span>' + esc(params(r.params) || '&mdash;').replace('&amp;mdash;', '&mdash;') + '</span><span>' + esc(LC[r.lc] || r.lc || '') + '</span></div>' +
      '<div class="c-mono">' + mono(r, 40) + '</div>' +
      '<div class="c-size"><b class="mono">' + esc(s.v) + '</b><span class="mono">' + esc(s.u) + '</span></div>' +
      '<div class="c-name">' + esc(r.n) + '</div>' +
      '<div class="c-facts mono"><span>' + esc((r.fmt || []).join(' ') || 'files') + '</span><span>' + esc(task(r) || r.fam) + '</span></div></a>';
  }
  // D. Terminal
  function cardD(r) {
    var lines = r.nc
      ? ['<span class="p">$</span> fw info ' + esc(r.id), '<span class="c"># link only: ' + esc(r.lic) + (r.lc ? ' (' + esc(LC[r.lc] || r.lc) + ')' : '') + '</span>', '<span class="c"># from the publisher, never hosted here</span>', '<span class="w">[publisher]</span> ' + esc(r.host || 'huggingface.co')]
      : ['<span class="p">$</span> fw get ' + esc(r.id), '<span class="c"># manifest sha256 ' + esc(r.sha || 'pending') + '&hellip;</span>', '<span class="c"># ' + esc(bytes(r.size)) + ' · ' + esc(r.files || 0) + ' files · ' + esc((r.fmt || []).join(', ')) + '</span>', '<span class="c"># license ' + esc(r.lic) + ' (' + esc(r.lc || 'see card') + ')' + (r.rt ? ', read terms first' : '') + '</span>', '<span class="ok">[' + esc(src(r).toLowerCase().replace(/^from /, '')) + ']</span> ' + esc(r.host || 'huggingface.co') + (r.commit ? ' @ ' + esc(r.commit.slice(0, 7)) : '')];
    return '<a class="fwc fwc-d mono" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '"><div class="d-bar"><i></i><i></i><i></i><span>' + esc(r.sec) + '</span></div><pre>' + lines.join('\n') + '</pre></a>';
  }
  var CARDS = { a: cardA, b: cardB, c: cardC, d: cardD };
  function card(style, r) { return (CARDS[style] || cardA)(r); }
  // Deck: one model in focus.
  function deck(r) {
    var facts = [['Size', bytes(r.size)], ['Params', params(r.params) || 'not stated'], ['Formats', (r.fmt || []).join(', ') || 'see files'], ['Task', task(r) || 'not tagged'], ['License', r.lic + ' (' + (LC[r.lc] || r.lc || 'see card') + ')'], ['Files', r.nc ? 'link only' : (r.files || 0) + ' with sha256'], ['Publisher', r.org], ['Added', date(r.added)]];
    return '<article class="deck-card"><div class="dk-top"><span class="dk-mono">' + mono(r, 52) + '</span><div><div class="mono dk-sec">' + esc(r.sec) + '</div><h2 class="dk-name">' + esc(r.n) + '</h2><div class="mono dk-id">' + esc(r.id) + '</div></div></div>' +
      '<p class="dk-sum">' + esc(r.sum) + '</p><dl class="dk-facts">' + facts.map(function (f) { return '<div><dt>' + f[0] + '</dt><dd>' + esc(f[1]) + '</dd></div>'; }).join('') + '</dl>' +
      '<div class="dk-foot"><div class="badges">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + '</div><a class="btn pri" href="' + esc(r.url) + '">' + (r.nc ? 'License and source' : 'Open model page') + '</a></div></article>';
  }

  // ---------- One card, four faces ----------
  // IDENTITY (element tile), FACTS (spec sheet), RECORD (newsgroup header), ACTION (terminal).
  // Every view composes these from the same row, so the faces read as one card.
  function logo(r, px, cls) { return r.logo ? '<img class="org' + (cls ? ' ' + cls : '') + '" src="' + esc(r.logo) + '" alt="" width="' + px + '" height="' + px + '" loading="lazy" decoding="async">' : ''; }
  function shortDate(t) { var d = new Date(t); return isNaN(d) ? '' : d.toISOString().slice(0, 10); }
  function repo(r) { return String(r.id).split('/').slice(1).join('/'); }
  function lcName(r) { return LC[r.lc] || r.lc || 'see card'; }
  // Kinds: model and dataset share the size figure; a tool shows its latest release there.
  function faceIdentity(r) {
    if (r.kind === 'tool') {
      var tag = r.tag || 'no release';
      return '<span class="fwf fwf-id">' +
        '<span class="id-row mono"><span>' + esc(r.tkind || 'tool') + '</span>' + logo(r, 24, 'id-org') + '</span>' +
        '<span class="id-mono">' + mono(r, 40) + '</span>' +
        '<span class="id-size tag' + (tag.length > 8 ? ' long' : tag.length > 5 ? ' mid' : '') + '"><b class="mono">' + esc(tag) + '</b><span class="mono">release</span></span>' +
        '<span class="id-name">' + esc(r.n) + '</span>' +
        '<span class="id-facts mono"><span>' + esc(r.lang || 'tool') + '</span><span>' + esc(r.lic || '') + '</span></span></span>';
    }
    var s = sizeParts(r.size);
    return '<span class="fwf fwf-id">' +
      '<span class="id-row mono"><span>' + esc(params(r.params) ? params(r.params) + ' params' : (r.fam || '')) + '</span>' + logo(r, 24, 'id-org') + '</span>' +
      '<span class="id-mono">' + mono(r, 40) + '</span>' +
      '<span class="id-size"><b class="mono">' + esc(s.v) + '</b><span class="mono">' + esc(s.u) + '</span></span>' +
      '<span class="id-name">' + esc(r.n) + '</span>' +
      '<span class="id-facts mono"><span>' + esc((r.fmt || []).join(' ') || 'files') + '</span><span>' + esc(lcName(r)) + '</span></span></span>';
  }
  function faceFacts(r) {
    if (r.kind === 'tool') {
      var tr = [['License', r.lic || 'see project'], ['Language', r.lang || 'not stated'], ['Kind', r.tkind || 'tool'], ['Release', r.relDate || 'none'], ['Platforms', (r.platforms || []).join(', ') || 'see project']];
      return '<span class="fwf fwf-facts"><span class="ff-h mono">Facts</span><span class="ff-dl">' +
        tr.map(function (x) { return '<span class="ff-r' + (x[0] === 'Platforms' ? ' wide' : '') + '"><span class="ff-k">' + x[0] + '</span><span class="ff-v">' + esc(x[1]) + '</span></span>'; }).join('') +
        '</span><span class="ff-b"><span class="b src"><i class="d"></i>Links to the project</span>' + lcTag(r) + '</span></span>';
    }
    if (r.kind === 'dataset') {
      var dr = [['Rows', r.rowsN ? Number(r.rowsN).toLocaleString('en-US') : 'not stated'], ['Size', bytes(r.size)], ['Format', (r.fmt || []).join(', ') || 'see files'], ['License', r.lic]];
      return '<span class="fwf fwf-facts"><span class="ff-h mono">Facts</span><span class="ff-dl">' +
        dr.map(function (x) { return '<span class="ff-r"><span class="ff-k">' + x[0] + '</span><span class="ff-v' + (x[0] === 'Format' ? ' mono' : '') + '">' + esc(x[1]) + '</span></span>'; }).join('') +
        '</span><span class="ff-b">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + '</span></span>';
    }
    var rows = [['Params', params(r.params) || 'not stated'], ['Size', bytes(r.size)], ['Formats', (r.fmt || []).join(', ') || 'see files'], ['Task', task(r) || 'not tagged'], ['License', r.lic]];
    return '<span class="fwf fwf-facts"><span class="ff-h mono">Facts</span><span class="ff-dl">' +
      rows.map(function (x) { return '<span class="ff-r"><span class="ff-k">' + x[0] + '</span><span class="ff-v' + (x[0] === 'Formats' ? ' mono' : '') + '">' + esc(x[1]) + '</span></span>'; }).join('') +
      '</span><span class="ff-b">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + '</span></span>';
  }
  // mode: 'full' (all header lines + summary), 'strip' (From + Subject, under a tile), 'head' (lines, no summary)
  function faceRecord(r, mode, opt) {
    opt = opt || {};
    var subj = opt.h1 ? '<h1 class="rc-subj rc-h1">' + esc(r.n) + '</h1>' : opt.link === false ? '<span class="rc-subj">' + esc(r.n) + '</span>' : '<a class="rc-subj" href="' + esc(r.url) + '">' + esc(r.n) + '</a>';
    var from = '<span class="rc-l"><b>From:</b> ' + logo(r, 18, 'rc-org') + esc(r.org) + ' <span class="rc-dim">&lt;' + esc(r.host || 'huggingface.co') + '/' + esc(r.org) + '&gt;</span></span>';
    if (mode === 'strip') return '<span class="fwf fwf-rec strip mono">' + from + '<span class="rc-l"><b>Subject:</b> ' + subj + '</span></span>';
    var lines = [from, '<span class="rc-l"><b>Newsgroups:</b> ' + esc(r.sec) + '</span>', '<span class="rc-l"><b>Subject:</b> ' + subj + '</span>'];
    if (r.kind === 'tool') lines.push('<span class="rc-l"><b>Release:</b> ' + esc(r.tag ? r.tag + (r.relDate ? ' (' + r.relDate + ')' : '') : 'none') + '</span>');
    else lines.push('<span class="rc-l"><b>Date:</b> ' + esc(shortDate(r.added) || 'unknown') + '</span>');
    if (r.commit) lines.push('<span class="rc-l"><b>Commit:</b> ' + esc(r.commit.slice(0, 7)) + '</span>');
    if (r.kind === 'tool') lines.push('<span class="rc-l"><b>License:</b> ' + esc(r.lic || 'see project') + '</span>');
    if (r.sig) lines.push('<span class="rc-l"><b>Signed:</b> ' + esc(r.sig) + ' <span class="rc-ok" aria-label="signature present">&#10003;</span></span>');
    return '<div class="fwf fwf-rec mono">' + lines.join('') + (mode === 'head' ? '' : '<p class="rc-sum">' + esc(r.sum) + '</p>') + '</div>';
  }
  function faceAction(r) {
    if (r.kind === 'tool') {
      var T = (r.install && r.install.length ? r.install.map(function (l) { return /^#/.test(l) ? '<span class="c">' + esc(l) + '</span>' : '<span class="p">$</span> ' + esc(l); }) : ['<span class="c"># install: see the project\'s instructions</span>', '<span class="u">' + esc(r.installDocs || r.licUrl || '') + '</span>'])
        .concat(r.use ? ['<span class="c"># use with Freeweights</span>', (/^#/.test(r.use) ? '<span class="c">' + esc(r.use) + '</span>' : '<span class="p">$</span> ' + esc(r.use))] : []);
      return '<div class="fwf fwf-act mono"><div class="act-bar" aria-hidden="true"><i></i><i></i><i></i><span>install</span></div><pre>' + T.join('\n') + '</pre></div>';
    }
    if (r.kind === 'dataset' && !r.nc) {
      var D = (r.rt ? ['<span class="w"># custom terms: read the license before you download</span>'] : []).concat([
        '<span class="p">&gt;&gt;&gt;</span> from datasets import load_dataset',
        '<span class="p">&gt;&gt;&gt;</span> ds = load_dataset("' + esc(r.id) + '")',
        '<span class="c"># with HF_ENDPOINT=https://freeweights.allternit.com &middot; manifest sha256 ' + esc(r.sha || 'pending') + '&hellip;</span>']);
      return '<div class="fwf fwf-act mono"><div class="act-bar" aria-hidden="true"><i></i><i></i><i></i><span>python</span></div><pre>' + D.join('\n') + '</pre></div>';
    }
    var L = r.nc
      ? ['<span class="p">$</span> fw info ' + esc(r.id), '<span class="w">link only &mdash; read the license:</span>', '<span class="u">' + esc(r.licUrl || ('https://' + (r.host || 'huggingface.co') + '/' + r.id)) + '</span>']
      : (r.rt ? ['<span class="w"># custom terms: read the license before you download</span>'] : []).concat([
        '<span class="p">$</span> fw get ' + esc(r.id),
        '<span class="c"># manifest sha256 ' + esc(r.sha || 'pending') + '&hellip; &middot; ' + esc(bytes(r.size)) + ' &middot; ' + esc(r.files || 0) + ' files</span>',
        '<span class="p">$</span> fw verify ./' + esc(repo(r)) + ' ' + esc(r.id)]);
    return '<div class="fwf fwf-act mono"><div class="act-bar" aria-hidden="true"><i></i><i></i><i></i><span>' + (r.nc ? 'link only' : 'fw') + '</span></div><pre>' + L.join('\n') + '</pre></div>';
  }
  // The combined card: RECORD header on top, IDENTITY + FACTS side by side, ACTION footer.
  function combined(r, opt) {
    opt = opt || {};
    return '<article class="fwk' + (opt.cls ? ' ' + opt.cls : '') + '" data-id="' + esc(r.id) + '">' +
      '<div class="fwk-rec">' + faceRecord(r, 'full', { h1: opt.h1 }) + '</div>' +
      '<div class="fwk-mid"><div class="fwk-id">' + faceIdentity(r) + '</div><div class="fwk-facts">' + faceFacts(r) + '</div></div>' +
      '<div class="fwk-act">' + faceAction(r) + '</div>' +
      (opt.open ? '<div class="fwk-foot"><a class="btn pri sm" href="' + esc(r.url) + '">' + (r.nc ? 'License and source' : 'Open model page') + '</a></div>' : '') +
      '</article>';
  }
  // Grid / home / related tile: IDENTITY front, FACTS back (flip), RECORD strip below.
  function tile(r) {
    return '<div class="fwt" data-id="' + esc(r.id) + '">' +
      '<button type="button" class="flip" aria-pressed="false" aria-label="' + esc(r.n) + ': show facts">' +
      '<span class="flip-in"><span class="flip-f">' + faceIdentity(r) + '</span><span class="flip-b">' + faceFacts(r) + '</span></span>' +
      '<span class="flip-hint mono" aria-hidden="true">flip</span></button>' +
      faceRecord(r, 'strip') + '</div>';
  }
  var api = { FONT: FONT, dots: dots, wordmark: wordmark, svg: svg, lockup: lockup, word: word, card: card, deck: deck, bytes: bytes, params: params, esc: esc, STYLES: ['a', 'b', 'c', 'd'],
    logo: logo, identity: faceIdentity, facts: faceFacts, record: faceRecord, action: faceAction, combined: combined, tile: tile };
  g.FWCards = api;
})(typeof window !== 'undefined' ? window : globalThis);

const ROWS=[{"kind": "model", "id": "lmstudio-community/Qwen3.5-4B-GGUF", "n": "Qwen3.5-4B-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 11329544112, "params": 4200000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-14T21:41:57Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 2.7, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 2.5, "e": "MLX"}]}, {"kind": "model", "id": "lmstudio-community/Qwen3.5-2B-GGUF", "n": "Qwen3.5-2B-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 5510586672, "params": 2300000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 1.3, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 1.2, "e": "MLX"}]}, {"kind": "model", "id": "google/gemma-4-E2B-it-qat-q4_0-gguf", "n": "gemma-4-E2B-it-qat-q4_0-gguf", "org": "google", "sec": "alt.freeweights.gemma", "fam": "gemma", "mono": "GM", "url": "#", "size": 4336380945, "params": 5100000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-14T21:41:57Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "https://freeweights.allternit.com/assets/orgs/google.png", "sig": "", "v": [{"f": "GGUF", "q": "Q4_0 QAT", "s": 3.2, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 3.0, "e": "MLX"}]}, {"kind": "model", "id": "ggml-org/SmolLM3-3B-GGUF", "n": "SmolLM3-3B-GGUF", "org": "ggml-org", "sec": "alt.freeweights.smollm", "fam": "smollm", "mono": "SM", "url": "#", "size": 11349231863, "params": 3100000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-14T21:41:57Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 1.9, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 1.8, "e": "MLX"}]}, {"kind": "model", "id": "lmstudio-community/Phi-4-mini-instruct-GGUF", "n": "Phi-4-mini-instruct-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.phi", "fam": "phi", "mono": "PH", "url": "#", "size": 11981766167, "params": 3800000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "MIT", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "https://freeweights.allternit.com/assets/orgs/microsoft.png", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 2.5, "e": "llama.cpp"}]}, {"kind": "model", "id": "Qwen/Qwen3-1.7B-GGUF", "n": "Qwen3-1.7B-GGUF", "org": "Qwen", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 1834445939, "params": 1700000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 1.1, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 1.0, "e": "MLX"}, {"f": "ONNX", "q": "q4f16", "s": 1.2, "e": "WebGPU"}]}, {"kind": "model", "id": "Qwen/Qwen3-0.6B-GGUF", "n": "Qwen3-0.6B-GGUF", "org": "Qwen", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 639466546, "params": 600000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 0.4, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 0.35, "e": "MLX"}, {"f": "ONNX", "q": "q4f16", "s": 0.45, "e": "WebGPU"}]}, {"kind": "model", "id": "lmstudio-community/gemma-4-E4B-it-QAT-GGUF", "n": "gemma-4-E4B-it-QAT-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.gemma", "fam": "gemma", "mono": "GM", "url": "#", "size": 6146496828, "params": 8000000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "https://freeweights.allternit.com/assets/orgs/google.png", "sig": "", "v": [{"f": "GGUF", "q": "Q4_0 QAT", "s": 5.0, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 4.8, "e": "MLX"}]}, {"kind": "model", "id": "lmstudio-community/Qwen3.5-27B-GGUF", "n": "Qwen3.5-27B-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 68149711960, "params": 27000000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 16.5, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 15.6, "e": "MLX"}]}];
const DEV={"mac": {"name": "Joe's MacBook Pro", "hw": "Apple M4 Pro \u00b7 48 GB unified memory", "os": "macOS 26", "kind": "unified", "apple": true, "budget": 31, "disk": "412 GB free", "eng": [["llama.cpp", "Metal", "b7180", "Ready"], ["MLX", "Metal", "0.30.1", "Ready"]], "ship": "Engines ship inside Allternit Desktop and update with it."}, "win": {"name": "Studio PC", "hw": "NVIDIA RTX 4070 12 GB \u00b7 32 GB RAM \u00b7 Ryzen 7", "os": "Windows 11", "kind": "discrete", "vram": 12, "ram": 32, "disk": "690 GB free", "eng": [["llama.cpp", "CUDA 13", "b7180", "Ready"], ["llama.cpp", "Vulkan", "b7180", "Installed, not in use"], ["llama.cpp", "CPU (AVX2)", "b7180", "Fallback"]], "ship": "The CUDA build is picked because an NVIDIA GPU is present. Vulkan stays as the fallback."}, "winarm": {"name": "Surface Laptop", "hw": "Snapdragon X Elite \u00b7 32 GB \u00b7 Adreno X1", "os": "Windows 11 on Arm", "kind": "unified", "budget": 19, "disk": "301 GB free", "eng": [["llama.cpp", "CPU (Arm, KleidiAI)", "b7180", "Ready"], ["llama.cpp", "Vulkan (Adreno)", "b7180", "Ready"], ["NPU (Hexagon)", "QNN", "preview", "Off: turn on in Advanced"]], "ship": "The NPU path is off by default. CPU and GPU are the baseline."}, "linux": {"name": "Workstation", "hw": "AMD Radeon RX 7900 XTX 24 GB \u00b7 64 GB RAM", "os": "Ubuntu 24.04", "kind": "discrete", "vram": 24, "ram": 64, "disk": "1.6 TB free", "eng": [["llama.cpp", "ROCm 7 (HIP)", "b7180", "Ready"], ["llama.cpp", "Vulkan", "b7180", "Installed, not in use"], ["llama.cpp", "CPU", "b7180", "Fallback"]], "ship": "ROCm is picked for RDNA3. Vulkan covers GPUs that ROCm does not support."}, "cpu": {"name": "Home server", "hw": "8 vCPU \u00b7 16 GB RAM \u00b7 no GPU", "os": "Debian 13", "kind": "cpu", "ram": 16, "disk": "88 GB free", "eng": [["llama.cpp", "CPU (AVX2)", "b7180", "Ready"]], "ship": "No GPU found. Models run on the CPU, so small models only."}, "web": {"name": "This browser tab", "hw": "Chrome 142 \u00b7 WebGPU \u00b7 2 GB model limit", "os": "ai.allternit.com", "kind": "browser", "budget": 2, "disk": "Browser storage 41 GB", "eng": [["WebGPU", "transformers.js (ONNX)", "4.3", "Ready"]], "ship": "Nothing to install. Weights are cached in the browser. Bigger models run in Desktop or the phone app."}, "iphone": {"name": "Joe's iPhone", "hw": "iPhone 17 Pro \u00b7 12 GB \u00b7 6 GB per app", "os": "iOS 26", "kind": "phone", "apple": true, "budget": 4.5, "disk": "41 GB free", "eng": [["MLX", "Metal", "0.30.1", "Ready"], ["llama.cpp", "Metal", "b7180", "Ready"], ["Apple Intelligence", "built in", "iOS 26", "Ready"]], "ship": "Engines are inside the app. Apple Intelligence needs no download."}, "android": {"name": "Galaxy S25", "hw": "Snapdragon 8 Elite \u00b7 12 GB", "os": "Android 16", "kind": "phone", "budget": 5, "disk": "96 GB free", "eng": [["llama.cpp", "OpenCL (Adreno)", "b7180", "Ready"], ["llama.cpp", "CPU", "b7180", "Fallback"], ["WebGPU", "in the PWA", "Chrome 142", "Ready (small models)"], ["Gemini Nano", "ML Kit", "system", "Not on this phone"]], "ship": "The app engine runs 1 to 4B models. The PWA alone runs models under 0.5 GB."}};
const pick=(d,r)=>d.kind==='browser'?r.v.find(x=>x.e==='WebGPU'):d.apple?(r.v.find(x=>x.e==='MLX')||r.v.find(x=>x.e==='llama.cpp')):r.v.find(x=>x.e==='llama.cpp');
function fit(d,r){const v=pick(d,r);
 if(!v)return{k:'na',t:'No browser build',why:'Run it in Desktop or the phone app',v:r.v[0]};
 const need=v.s*1.2+0.4,n=need.toFixed(1);
 if(d.kind==='discrete'){if(need<=d.vram*0.9)return{k:'ok',t:'Fits',why:'Full GPU, '+n+' of '+d.vram+' GB VRAM',v};if(need<=d.vram+d.ram*0.5)return{k:'slow',t:'May be slow',why:'Part runs on the CPU',v};return{k:'no',t:"Won't fit",why:'Needs about '+n+' GB',v};}
 if(d.kind==='cpu'){if(need>d.ram*0.6)return{k:'no',t:"Won't fit",why:'Needs about '+n+' GB',v};return v.s<1.5?{k:'ok',t:'Fits',why:'CPU only',v}:{k:'slow',t:'May be slow',why:'CPU only, about 5 tok/s',v};}
 if(need<=d.budget*0.8)return{k:'ok',t:'Fits',why:n+' of '+d.budget+' GB for models',v};
 if(need<=d.budget)return{k:'slow',t:'Tight fit',why:'Close other apps first',v};
 return{k:'no',t:"Won't fit",why:'Needs '+n+' GB, '+d.budget+' GB free for models',v};}
const FC={ok:'var(--ok)',slow:'var(--warn)',no:'var(--bad)',na:'var(--ink3)'};
const ENVS=[['mac','Mac'],['win','Windows + NVIDIA'],['winarm','Windows on Arm'],['linux','Linux + AMD'],['cpu','Linux CPU only'],['web','Browser'],['iphone','iPhone'],['android','Android']];

const FW=window.FWCards;
const H=({html,cls,tag})=>React.createElement(tag||'div',{className:cls,dangerouslySetInnerHTML:{__html:html}});
const Pill=({f,small})=>(<span title={f.why} style={{display:'inline-flex',alignItems:'center',gap:5,fontSize:small?10.5:11.5,fontWeight:600,color:FC[f.k],background:'var(--fill)',borderRadius:999,padding:small?'1px 7px':'2px 9px',whiteSpace:'nowrap'}}><i style={{width:6,height:6,borderRadius:3,background:FC[f.k]}}/>{f.t}</span>);
const Btn=({children,pri,onClick,small})=>(<button onClick={onClick} style={{border:pri?'0':'1px solid var(--line2)',background:pri?'var(--btn)':'transparent',color:pri?'var(--btnink)':'var(--ink)',borderRadius:9,padding:small?'4px 10px':'6px 12px',fontSize:small?11.5:12.5,fontWeight:600,cursor:'pointer',whiteSpace:'nowrap',fontFamily:'inherit'}}>{children}</button>);
function EnvSwitch({env,setEnv,only}){const list=only?ENVS.filter(e=>only.includes(e[0])):ENVS;return(<div style={{display:'flex',flexWrap:'wrap',gap:6,alignItems:'center',padding:'8px 10px',border:'1px dashed var(--line2)',borderRadius:10,marginBottom:12,fontSize:12}}><span className="mono" style={{color:'var(--ink3)',marginRight:4}}>Mockup only, pretend this runs on:</span>{list.map(([k,l])=>(<button key={k} onClick={()=>setEnv(k)} style={{border:0,borderRadius:999,padding:'3px 10px',fontSize:12,cursor:'pointer',fontFamily:'inherit',background:env===k?'var(--btn)':'var(--fill)',color:env===k?'var(--btnink)':'var(--ink2)'}}>{l}</button>))}</div>);}
const TABS=['Catalog','This device','Playground','Train','Cloud'];
function Chrome({tab,children,d}){return(<div style={{background:'var(--bg)',color:'var(--ink)',fontFamily:'var(--font-allternit-sans)',padding:'22px 24px',fontSize:14}}>
<div style={{fontFamily:'var(--font-allternit-serif)',fontSize:28,fontWeight:500,letterSpacing:'-.01em'}}>Freeweights Studio</div>
<div style={{color:'var(--ink2)',fontSize:13.5,marginTop:2}}>Find open weights in Freeweights, then download and run them on this device, your other devices or the cloud.</div>
<div style={{display:'flex',gap:6,marginTop:14,flexWrap:'wrap'}}>{TABS.map(t=>(<span key={t} style={{border:'1px solid '+(t===tab?'var(--ink)':'var(--line)'),background:t===tab?'var(--fill2)':'transparent',borderRadius:10,padding:'5px 12px',fontSize:12.5,fontWeight:600}}>{t}</span>))}</div>
<div style={{display:'flex',alignItems:'center',gap:10,marginTop:14,padding:'10px 12px',border:'1px solid var(--line)',borderRadius:12,flexWrap:'wrap'}}><span style={{width:8,height:8,borderRadius:4,background:'var(--ok)'}}/><b style={{fontSize:13}}>{d.name}</b><span className="mono" style={{fontSize:11.5,color:'var(--ink2)'}}>{d.hw} · {d.os}</span><span style={{flex:1}}/><span className="mono" style={{fontSize:11.5,color:'var(--ink3)'}}>{d.eng.filter(e=>/Ready/.test(e[3])).map(e=>e[0]+' '+e[1]).join(' + ')}</span></div>
<div style={{marginTop:16}}>{children}</div></div>);}

const HAVE={mac:['Qwen3.5-4B-GGUF'],win:['Qwen3.5-4B-GGUF'],linux:['Qwen3.5-27B-GGUF'],iphone:['Qwen3.5-2B-GGUF'],android:['Qwen3.5-2B-GGUF'],web:['Qwen3-0.6B-GGUF']};
function Action({d,f,r,env}){const have=(HAVE[env]||[]).includes(r.n);
 if(have)return <Btn pri small>Chat</Btn>;
 if(f.k==='no'||f.k==='na')return <Btn small>{d.kind==='phone'||d.kind==='browser'?'Run on my computer':'Send to a bigger device'}</Btn>;
 return <Btn pri small>Run here</Btn>;}
function Variant({f,r,env}){const have=(HAVE[env]||[]).includes(r.n);return(<span className="mono" style={{fontSize:11,color:'var(--ink2)'}}>{f.v.f} {f.v.q} · {f.v.s} GB · {f.v.e}{have?' · downloaded':''}</span>);}
function ListView({d,env,sel,setSel,rows}){return(<div className="view-list">
<div className="list-h" style={{gridTemplateColumns:'minmax(0,1fr) 70px 60px 104px 200px'}}><span>Subject</span><span className="r">File</span><span className="r">Params</span><span>License</span><span>On {d.kind==='browser'?'this browser':'this device'}</span></div>
{rows.map(r=>{const f=fit(d,r);return(<div key={r.id} className="msg" onClick={()=>setSel(r.id)} style={{gridTemplateColumns:'minmax(0,1fr) 70px 60px 104px 200px',cursor:'pointer',background:sel===r.id?'var(--fill)':undefined}}>
<span className="s"><span className="msg-org msg-mono" dangerouslySetInnerHTML={{__html:r.logo?'<img class="org" src="'+r.logo+'" width="22" height="22">':FW.word(r.mono,9,'mono-svg')}}/><span className="s-t"><span className="s-n">{r.n}</span><small><span className="hw hw-host">Hosted (signed)</span> <b>From:</b> {r.org} <b>in</b> {r.sec}</small></span></span>
<span className="z mono">{f.v.s} GB</span><span className="pp mono">{FW.params(r.params)}</span><span className="lcc"><span className={'lc lc-'+r.lc}>{r.lc==='permissive'?'Permissive':r.lc}</span></span>
<span style={{display:'flex',alignItems:'center',gap:8,justifyContent:'space-between'}}><Pill f={f} small/><Action d={d} f={f} r={r} env={env}/></span></div>);})}</div>);}
function GridView({d,env,rows}){return(<div className="cards tiles">{rows.map(r=>{const f=fit(d,r);return(<div key={r.id} style={{display:'flex',flexDirection:'column',gap:8}}>
<H html={FW.tile({...r,size:f.v.s*1e9})}/>
<div style={{border:'1px solid var(--line)',borderRadius:12,padding:'8px 10px',display:'grid',gap:6}}><div style={{display:'flex',justifyContent:'space-between',alignItems:'center',gap:6}}><Pill f={f} small/><Action d={d} f={f} r={r} env={env}/></div><Variant f={f} r={r} env={env}/></div></div>);})}</div>);}
function RunPanel({d,r,env}){const f=fit(d,r);const [vi,setVi]=React.useState(-1);const opts=r.v.filter(v=>d.kind==='browser'?v.e==='WebGPU':d.apple?true:v.e!=='MLX'&&v.e!=='WebGPU');
 return(<div style={{borderTop:'1px solid var(--line)',padding:'16px 20px',display:'grid',gap:10}}>
<div className="mono" style={{fontSize:11,color:'var(--ink3)',textTransform:'uppercase',letterSpacing:'.08em'}}>Run on {d.name}</div>
<div style={{display:'flex',gap:8,alignItems:'center',flexWrap:'wrap'}}><Pill f={f}/><span style={{fontSize:12.5,color:'var(--ink2)'}}>{f.why}</span></div>
{opts.length?opts.map((v,i)=>{const on=vi===-1?v===f.v:vi===i;return(<label key={i} onClick={()=>setVi(i)} style={{display:'flex',alignItems:'center',gap:10,border:'1px solid '+(on?'var(--ink)':'var(--line)'),borderRadius:10,padding:'8px 12px',cursor:'pointer'}}><span style={{width:14,height:14,borderRadius:7,border:'2px solid var(--ink)',background:on?'var(--ink)':'transparent'}}/><span style={{flex:1,fontSize:13}}><b>{v.f} {v.q}</b> <span style={{color:'var(--ink2)'}}>with {v.e}</span>{v===f.v?<span className="mono" style={{fontSize:10.5,color:'var(--ok)',marginLeft:8}}>best for this device</span>:null}</span><span className="mono" style={{fontSize:12}}>{v.s} GB</span></label>);}):<div style={{fontSize:13,color:'var(--ink2)'}}>No build of this model runs in a browser tab.</div>}
<div style={{display:'flex',gap:8,flexWrap:'wrap'}}><Action d={d} f={f} r={r} env={env}/><Btn small>Send to my iPhone</Btn><Btn small>Open on Freeweights</Btn></div>
<div style={{fontSize:11.5,color:'var(--ink3)'}}>Files are checked against the signed Freeweights manifest (sha256) before the model loads.</div></div>);}
function DeckView({d,env}){const [i,setI]=React.useState(0);const r0=ROWS[i];const r={...r0,size:fit(d,r0).v.s*1e9};return(<div className="view-deck" style={{maxWidth:760}}>
<div className="deck-nav"><button className="iconbtn" onClick={()=>setI((i+ROWS.length-1)%ROWS.length)}>‹</button><span className="mono deck-count">{i+1} of {ROWS.length}</span><button className="iconbtn" onClick={()=>setI((i+1)%ROWS.length)}>›</button></div>
<article className="fwk"><H cls="fwk-rec" html={FW.record(r,'full')}/><div className="fwk-mid"><H cls="fwk-id" html={FW.identity(r)}/><H cls="fwk-facts" html={FW.facts(r)}/></div><RunPanel d={d} r={r0} env={env} key={env+i}/></article></div>);}
export default function App(){const [env,setEnv]=React.useState('mac');const [view,setView]=React.useState('grid');const [sel,setSel]=React.useState(ROWS[0].id);const [fits,setFits]=React.useState(false);const d=DEV[env];
 const shown=fits?ROWS.filter(r=>['ok','slow'].includes(fit(d,r).k)):ROWS;
 return(<div style={{background:'var(--bg)'}}><EnvSwitch env={env} setEnv={setEnv}/>
<Chrome tab="Catalog" d={d}>
<div style={{display:'grid',gridTemplateColumns:'190px minmax(0,1fr)',gap:18}}>
<aside style={{fontSize:13}}><div className="mono" style={{fontSize:11,color:'var(--ink3)',textTransform:'uppercase',letterSpacing:'.08em',marginBottom:6}}>On this device</div>
{[['Everything',false],['Fits this device',true]].map(([l,v])=>(<div key={l} onClick={()=>setFits(v)} style={{padding:'5px 8px',borderRadius:8,cursor:'pointer',background:fits===v?'var(--fill2)':'transparent',fontWeight:fits===v?600:400}}>{l}</div>))}
<div className="mono" style={{fontSize:11,color:'var(--ink3)',textTransform:'uppercase',letterSpacing:'.08em',margin:'14px 0 6px'}}>Collections</div>
{['For this device','Phone','Laptop','Workstation','Browser','GGUF','MLX'].map(c=><div key={c} style={{padding:'4px 8px',color:'var(--ink2)'}}>{c}</div>)}
<div className="mono" style={{fontSize:11,color:'var(--ink3)',textTransform:'uppercase',letterSpacing:'.08em',margin:'14px 0 6px'}}>Sections</div>
{[['qwen',405],['gemma',366],['smollm',35],['phi',19],['llama',239],['lfm',104]].map(([s,n])=><div key={s} className="mono" style={{display:'flex',justifyContent:'space-between',padding:'3px 8px',fontSize:12.5,color:'var(--ink2)'}}><span>{s}</span><span style={{color:'var(--ink3)'}}>{n}</span></div>)}</aside>
<div style={{minWidth:0}}>
<div className="toolbar" style={{display:'flex',gap:8,alignItems:'center',flexWrap:'wrap'}}><label className="q" style={{flex:1,minWidth:180}}><input type="search" placeholder="Search 8,900 models" style={{width:'100%'}}/></label>
<select className="sort" defaultValue="fit" style={{padding:'7px 10px',borderRadius:10,border:'1px solid var(--line)',background:'var(--bg)',color:'var(--ink)'}}><option value="fit">Best for this device</option><option>Best first</option><option>Newest</option><option>Size: smallest</option></select>
<div className="seg" role="group">{[['list','List'],['grid','Grid'],['deck','Deck']].map(([v,l])=><button key={v} className="segb" aria-pressed={view===v} onClick={()=>setView(v)}>{l}</button>)}</div></div>
<div className="results-h" style={{margin:'10px 0 8px',fontSize:13,color:'var(--ink2)'}}>{shown.length} models · sizes are the file you download, not the whole repo</div>
{view==='list'?<ListView d={d} env={env} sel={sel} setSel={setSel} rows={shown}/>:view==='grid'?<GridView d={d} env={env} rows={shown}/>:<DeckView d={d} env={env}/>}
</div></div></Chrome></div>);}

```


```callout ok Decided 11-15
Default catalog view: Grid.
```

## E This device and Playground

"This device" replaces the Engine and Studio tabs. It shows what the engine detected, which engines are ready, what is running, downloads, storage and models recommended for this hardware. Switch the device to see each environment's engines. Mac uses Metal + MLX. NVIDIA uses CUDA, AMD uses ROCm, and Vulkan and CPU are the fallbacks. Snapdragon adds an NPU preview, and a browser uses WebGPU.

```react css=fw.css title="Model Lab › This device: hardware, engines, running, downloads, storage" h=820
import React from 'react';
// Freeweights shared rendering: the Allternit dot lettering (same 10-unit grid, 8.5 dots,
// 1.5 radius as the A://TERNIT wordmark) and the four preview card styles (A to D).
// Loaded by site/build.mjs at build time and by the browser for grid, deck and style switching.
(function (g) {
  var FONT = {
    A: ['..#..', '.#.#.', '#####', '#...#', '#...#'], B: ['####.', '#...#', '####.', '#...#', '####.'],
    C: ['.####', '#....', '#....', '#....', '.####'], D: ['####.', '#...#', '#...#', '#...#', '####.'],
    E: ['#####', '#....', '####.', '#....', '#####'], F: ['#####', '#....', '####.', '#....', '#....'],
    G: ['.####', '#....', '#.###', '#...#', '.###.'], H: ['#...#', '#...#', '#####', '#...#', '#...#'],
    I: ['###', '.#.', '.#.', '.#.', '###'], J: ['..###', '...#.', '...#.', '#..#.', '.##..'],
    K: ['#...#', '#..#.', '###..', '#..#.', '#...#'], L: ['#....', '#....', '#....', '#....', '#####'],
    M: ['#...#', '##.##', '#.#.#', '#...#', '#...#'], N: ['#...#', '##..#', '#.#.#', '#..##', '#...#'],
    O: ['.###.', '#...#', '#...#', '#...#', '.###.'], P: ['####.', '#...#', '####.', '#....', '#....'],
    Q: ['.###.', '#...#', '#.#.#', '#..#.', '.##.#'], R: ['####.', '#...#', '####.', '#.#..', '#..##'],
    S: ['.####', '#....', '.###.', '....#', '####.'], T: ['#####', '..#..', '..#..', '..#..', '..#..'],
    U: ['#...#', '#...#', '#...#', '#...#', '.###.'], V: ['#...#', '#...#', '#...#', '.#.#.', '..#..'],
    W: ['#...#', '#...#', '#.#.#', '##.##', '#...#'], X: ['#...#', '.#.#.', '..#..', '.#.#.', '#...#'],
    Y: ['#...#', '.#.#.', '..#..', '..#..', '..#..'], Z: ['#####', '...#.', '..#..', '.#...', '#####'],
    0: ['.###.', '#..##', '#.#.#', '##..#', '.###.'], 1: ['.#.', '##.', '.#.', '.#.', '###'],
    2: ['####.', '....#', '.###.', '#....', '#####'], 3: ['####.', '....#', '.###.', '....#', '####.'],
    4: ['#..#.', '#..#.', '#####', '...#.', '...#.'], 5: ['#####', '#....', '####.', '....#', '####.'],
    6: ['.###.', '#....', '####.', '#...#', '.###.'], 7: ['#####', '....#', '...#.', '..#..', '..#..'],
    8: ['.###.', '#...#', '.###.', '#...#', '.###.'], 9: ['.###.', '#...#', '.####', '....#', '.###.'],
    '.': ['.', '.', '.', '.', '#'], ':': ['.', '#', '.', '#', '.'], '/': ['..#', '..#', '.#.', '#..', '#..'],
    '-': ['...', '...', '###', '...', '...'], ' ': ['..', '..', '..', '..', '..']
  };
  var CELL = 10;
  function rect(c, r, fill) { return '<rect x="' + (c * CELL + 0.75) + '" y="' + (r * CELL + 0.75) + '" width="8.5" height="8.5" rx="1.5"' + (fill ? ' fill="' + fill + '"' : '') + '/>'; }
  // Lay out text on the dot grid. Returns { cols, rows, body }. Letters are separated by one empty column.
  function dots(text, col0, row0) {
    var c = col0 || 0, r0 = row0 || 0, body = '', chars = String(text).toUpperCase().split('');
    chars.forEach(function (ch, i) {
      var gl = FONT[ch] || FONT[' '];
      for (var r = 0; r < 5; r++) for (var x = 0; x < gl[r].length; x++) if (gl[r][x] === '#') body += rect(c + x, r0 + r);
      c += gl[0].length + (i < chars.length - 1 ? 1 : 0);
    });
    return { cols: c - (col0 || 0), body: body };
  }
  // The A://TERNIT wordmark cells, copied from the brand SVG (o = the coral dot).
  var WM = ['..#.......#...#.#####.#####.####..#...#..###..#####', '.#.#..#...#...#...#...#.....#...#.##..#...#.....#..', '##o##....#...#....#...####..####..#.#.#...#.....#..', '#...#.#.#...#.....#...#.....#.#...#..##...#.....#..', '#...#...#...#.....#...#####.#..##.#...#..###....#..'];
  function wordmark(col0, row0) {
    var b = '';
    WM.forEach(function (line, r) { for (var x = 0; x < line.length; x++) { if (line[x] === '#') b += rect(col0 + x, row0 + r); else if (line[x] === 'o') b += rect(col0 + x, row0 + r, '#D97757'); } });
    return { cols: WM[0].length, body: b };
  }
  function svg(cols, rows, body, h, label, cls) {
    var w = Math.round((h * cols) / rows * 100) / 100;
    return '<svg class="' + (cls || 'dotw') + '" viewBox="0 0 ' + cols * CELL + ' ' + rows * CELL + '" height="' + h + '" width="' + w + '" fill="currentColor" shape-rendering="geometricPrecision"' + (label ? ' role="img" aria-label="' + esc(label) + '"' : ' aria-hidden="true"') + '>' + body + '</svg>';
  }
  // Lockups. inline: A://TERNIT  FREEWEIGHTS on one line. stack: wordmark above the name.
  function lockup(kind, h, cls) {
    var a = wordmark(0, 0);
    if (kind === 'stack') { var n = dots('FREEWEIGHTS', 0, 7); return svg(Math.max(a.cols, n.cols), 12, a.body + n.body, h, 'Allternit Freeweights', cls || 'lockup stack'); }
    if (kind === 'name') { var o = dots('FREEWEIGHTS'); return svg(o.cols, 5, o.body, h, 'Freeweights', cls || 'lockup name'); }
    var m = dots('FREEWEIGHTS', a.cols + 4, 0); return svg(a.cols + 4 + m.cols, 5, a.body + m.body, h, 'Allternit Freeweights', cls || 'lockup inline');
  }
  function word(text, h, cls, label) { var d = dots(text); return svg(d.cols, 5, d.body, h, label, cls); }

  var esc = function (s) { return String(s == null ? '' : s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'); };
  function bytes(n) {
    if (n == null || isNaN(n)) return 'size unknown';
    var u = ['B', 'KB', 'MB', 'GB', 'TB'], i = 0, v = Number(n);
    while (v >= 1000 && i < u.length - 1) { v /= 1000; i++; }
    return (v >= 100 || i === 0 ? Math.round(v).toLocaleString('en-US') : v >= 10 ? v.toFixed(1) : v.toFixed(2)) + ' ' + u[i];
  }
  function sizeParts(n) { var s = bytes(n).split(' '); return { v: s[0], u: s[1] || '' }; }
  function params(n) { return !n ? '' : n >= 1e12 ? +(n / 1e12).toFixed(2) + 'T' : n >= 1e9 ? +(n / 1e9).toFixed(1) + 'B' : +(n / 1e6).toFixed(0) + 'M'; }
  var LC = { copyleft: 'copyleft', permissive: 'permissive', custom: 'custom terms', 'non-commercial': 'non-commercial', gated: 'gated', territory: 'territory-limited', unreviewed: 'license not reviewed' };
  function src(r) { return r.nc ? (r.how || 'From the publisher') : r.src || 'From source'; }
  function srcBadge(r) { var cls = r.nc || /source|publisher/i.test(src(r)) ? ' src' : /seed/i.test(src(r)) ? ' seed' : ''; return '<span class="b' + cls + '"><i class="d"></i>' + esc(src(r)) + '</span>'; }
  function lcTag(r) { return '<span class="lc lc-' + esc(r.lc || 'unknown') + '">' + esc(LC[r.lc] || r.lc || 'see card') + '</span>'; }
  function task(r) { var t = (r.task || [])[0]; return t ? t.replace(/-/g, ' ') : r.sec === 'alt.freeweights.datasets' ? 'dataset' : ''; }
  function date(t) { var d = new Date(t); if (isNaN(d)) return ''; return d.toUTCString().slice(0, 16); }
  function mono(r, h, cls) { return word(r.mono || 'FW', h, cls || 'mono-svg', null); }

  // A. Spec sheet
  function cardA(r) {
    var spec = [['Params', params(r.params) || 'not stated'], ['Size', bytes(r.size)], ['Format', (r.fmt || []).join(', ') || 'see files'], ['License', esc(r.lic)]];
    return '<a class="fwc fwc-a" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '">' +
      '<div class="a-top"><span class="a-mono">' + mono(r, 22) + '</span><div class="a-t"><div class="a-sec mono">' + esc(r.sec) + '</div><div class="a-name">' + esc(r.n) + '</div><div class="a-id mono">' + esc(r.id) + '</div></div></div>' +
      '<dl class="a-spec">' + spec.map(function (s) { return '<div><dt>' + s[0] + '</dt><dd' + (s[0] === 'Format' ? ' class="mono"' : '') + '>' + (s[0] === 'License' ? s[1] : esc(s[1])) + '</dd></div>'; }).join('') + '</dl>' +
      '<div class="a-foot">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + (task(r) ? '<span class="a-task">' + esc(task(r)) + '</span>' : '') + '</div></a>';
  }
  // B. Newsgroup post
  function cardB(r) {
    return '<a class="fwc fwc-b" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '">' +
      '<div class="b-head mono"><div><b>From:</b> ' + esc(r.org) + ' &lt;' + esc(r.host || 'huggingface.co') + '/' + esc(r.org) + '&gt;</div><div><b>Newsgroups:</b> ' + esc(r.sec) + '</div><div><b>Subject:</b> <span class="b-subj">' + esc(r.n) + '</span> (' + esc(bytes(r.size)) + ')</div><div><b>Date:</b> ' + esc(date(r.added)) + '</div></div>' +
      '<p class="b-body">' + esc(r.sum) + '</p>' +
      '<div class="b-sig mono">-- <br>' + esc(r.lic) + ' · ' + esc(LC[r.lc] || r.lc || '') + ' · ' + esc(src(r).toLowerCase()) + (r.rt ? ' · read terms first' : '') + '</div></a>';
  }
  // C. Element tile
  function cardC(r) {
    var s = sizeParts(r.size);
    return '<a class="fwc fwc-c" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '">' +
      '<div class="c-row mono"><span>' + esc(params(r.params) || '&mdash;').replace('&amp;mdash;', '&mdash;') + '</span><span>' + esc(LC[r.lc] || r.lc || '') + '</span></div>' +
      '<div class="c-mono">' + mono(r, 40) + '</div>' +
      '<div class="c-size"><b class="mono">' + esc(s.v) + '</b><span class="mono">' + esc(s.u) + '</span></div>' +
      '<div class="c-name">' + esc(r.n) + '</div>' +
      '<div class="c-facts mono"><span>' + esc((r.fmt || []).join(' ') || 'files') + '</span><span>' + esc(task(r) || r.fam) + '</span></div></a>';
  }
  // D. Terminal
  function cardD(r) {
    var lines = r.nc
      ? ['<span class="p">$</span> fw info ' + esc(r.id), '<span class="c"># link only: ' + esc(r.lic) + (r.lc ? ' (' + esc(LC[r.lc] || r.lc) + ')' : '') + '</span>', '<span class="c"># from the publisher, never hosted here</span>', '<span class="w">[publisher]</span> ' + esc(r.host || 'huggingface.co')]
      : ['<span class="p">$</span> fw get ' + esc(r.id), '<span class="c"># manifest sha256 ' + esc(r.sha || 'pending') + '&hellip;</span>', '<span class="c"># ' + esc(bytes(r.size)) + ' · ' + esc(r.files || 0) + ' files · ' + esc((r.fmt || []).join(', ')) + '</span>', '<span class="c"># license ' + esc(r.lic) + ' (' + esc(r.lc || 'see card') + ')' + (r.rt ? ', read terms first' : '') + '</span>', '<span class="ok">[' + esc(src(r).toLowerCase().replace(/^from /, '')) + ']</span> ' + esc(r.host || 'huggingface.co') + (r.commit ? ' @ ' + esc(r.commit.slice(0, 7)) : '')];
    return '<a class="fwc fwc-d mono" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '"><div class="d-bar"><i></i><i></i><i></i><span>' + esc(r.sec) + '</span></div><pre>' + lines.join('\n') + '</pre></a>';
  }
  var CARDS = { a: cardA, b: cardB, c: cardC, d: cardD };
  function card(style, r) { return (CARDS[style] || cardA)(r); }
  // Deck: one model in focus.
  function deck(r) {
    var facts = [['Size', bytes(r.size)], ['Params', params(r.params) || 'not stated'], ['Formats', (r.fmt || []).join(', ') || 'see files'], ['Task', task(r) || 'not tagged'], ['License', r.lic + ' (' + (LC[r.lc] || r.lc || 'see card') + ')'], ['Files', r.nc ? 'link only' : (r.files || 0) + ' with sha256'], ['Publisher', r.org], ['Added', date(r.added)]];
    return '<article class="deck-card"><div class="dk-top"><span class="dk-mono">' + mono(r, 52) + '</span><div><div class="mono dk-sec">' + esc(r.sec) + '</div><h2 class="dk-name">' + esc(r.n) + '</h2><div class="mono dk-id">' + esc(r.id) + '</div></div></div>' +
      '<p class="dk-sum">' + esc(r.sum) + '</p><dl class="dk-facts">' + facts.map(function (f) { return '<div><dt>' + f[0] + '</dt><dd>' + esc(f[1]) + '</dd></div>'; }).join('') + '</dl>' +
      '<div class="dk-foot"><div class="badges">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + '</div><a class="btn pri" href="' + esc(r.url) + '">' + (r.nc ? 'License and source' : 'Open model page') + '</a></div></article>';
  }

  // ---------- One card, four faces ----------
  // IDENTITY (element tile), FACTS (spec sheet), RECORD (newsgroup header), ACTION (terminal).
  // Every view composes these from the same row, so the faces read as one card.
  function logo(r, px, cls) { return r.logo ? '<img class="org' + (cls ? ' ' + cls : '') + '" src="' + esc(r.logo) + '" alt="" width="' + px + '" height="' + px + '" loading="lazy" decoding="async">' : ''; }
  function shortDate(t) { var d = new Date(t); return isNaN(d) ? '' : d.toISOString().slice(0, 10); }
  function repo(r) { return String(r.id).split('/').slice(1).join('/'); }
  function lcName(r) { return LC[r.lc] || r.lc || 'see card'; }
  // Kinds: model and dataset share the size figure; a tool shows its latest release there.
  function faceIdentity(r) {
    if (r.kind === 'tool') {
      var tag = r.tag || 'no release';
      return '<span class="fwf fwf-id">' +
        '<span class="id-row mono"><span>' + esc(r.tkind || 'tool') + '</span>' + logo(r, 24, 'id-org') + '</span>' +
        '<span class="id-mono">' + mono(r, 40) + '</span>' +
        '<span class="id-size tag' + (tag.length > 8 ? ' long' : tag.length > 5 ? ' mid' : '') + '"><b class="mono">' + esc(tag) + '</b><span class="mono">release</span></span>' +
        '<span class="id-name">' + esc(r.n) + '</span>' +
        '<span class="id-facts mono"><span>' + esc(r.lang || 'tool') + '</span><span>' + esc(r.lic || '') + '</span></span></span>';
    }
    var s = sizeParts(r.size);
    return '<span class="fwf fwf-id">' +
      '<span class="id-row mono"><span>' + esc(params(r.params) ? params(r.params) + ' params' : (r.fam || '')) + '</span>' + logo(r, 24, 'id-org') + '</span>' +
      '<span class="id-mono">' + mono(r, 40) + '</span>' +
      '<span class="id-size"><b class="mono">' + esc(s.v) + '</b><span class="mono">' + esc(s.u) + '</span></span>' +
      '<span class="id-name">' + esc(r.n) + '</span>' +
      '<span class="id-facts mono"><span>' + esc((r.fmt || []).join(' ') || 'files') + '</span><span>' + esc(lcName(r)) + '</span></span></span>';
  }
  function faceFacts(r) {
    if (r.kind === 'tool') {
      var tr = [['License', r.lic || 'see project'], ['Language', r.lang || 'not stated'], ['Kind', r.tkind || 'tool'], ['Release', r.relDate || 'none'], ['Platforms', (r.platforms || []).join(', ') || 'see project']];
      return '<span class="fwf fwf-facts"><span class="ff-h mono">Facts</span><span class="ff-dl">' +
        tr.map(function (x) { return '<span class="ff-r' + (x[0] === 'Platforms' ? ' wide' : '') + '"><span class="ff-k">' + x[0] + '</span><span class="ff-v">' + esc(x[1]) + '</span></span>'; }).join('') +
        '</span><span class="ff-b"><span class="b src"><i class="d"></i>Links to the project</span>' + lcTag(r) + '</span></span>';
    }
    if (r.kind === 'dataset') {
      var dr = [['Rows', r.rowsN ? Number(r.rowsN).toLocaleString('en-US') : 'not stated'], ['Size', bytes(r.size)], ['Format', (r.fmt || []).join(', ') || 'see files'], ['License', r.lic]];
      return '<span class="fwf fwf-facts"><span class="ff-h mono">Facts</span><span class="ff-dl">' +
        dr.map(function (x) { return '<span class="ff-r"><span class="ff-k">' + x[0] + '</span><span class="ff-v' + (x[0] === 'Format' ? ' mono' : '') + '">' + esc(x[1]) + '</span></span>'; }).join('') +
        '</span><span class="ff-b">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + '</span></span>';
    }
    var rows = [['Params', params(r.params) || 'not stated'], ['Size', bytes(r.size)], ['Formats', (r.fmt || []).join(', ') || 'see files'], ['Task', task(r) || 'not tagged'], ['License', r.lic]];
    return '<span class="fwf fwf-facts"><span class="ff-h mono">Facts</span><span class="ff-dl">' +
      rows.map(function (x) { return '<span class="ff-r"><span class="ff-k">' + x[0] + '</span><span class="ff-v' + (x[0] === 'Formats' ? ' mono' : '') + '">' + esc(x[1]) + '</span></span>'; }).join('') +
      '</span><span class="ff-b">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + '</span></span>';
  }
  // mode: 'full' (all header lines + summary), 'strip' (From + Subject, under a tile), 'head' (lines, no summary)
  function faceRecord(r, mode, opt) {
    opt = opt || {};
    var subj = opt.h1 ? '<h1 class="rc-subj rc-h1">' + esc(r.n) + '</h1>' : opt.link === false ? '<span class="rc-subj">' + esc(r.n) + '</span>' : '<a class="rc-subj" href="' + esc(r.url) + '">' + esc(r.n) + '</a>';
    var from = '<span class="rc-l"><b>From:</b> ' + logo(r, 18, 'rc-org') + esc(r.org) + ' <span class="rc-dim">&lt;' + esc(r.host || 'huggingface.co') + '/' + esc(r.org) + '&gt;</span></span>';
    if (mode === 'strip') return '<span class="fwf fwf-rec strip mono">' + from + '<span class="rc-l"><b>Subject:</b> ' + subj + '</span></span>';
    var lines = [from, '<span class="rc-l"><b>Newsgroups:</b> ' + esc(r.sec) + '</span>', '<span class="rc-l"><b>Subject:</b> ' + subj + '</span>'];
    if (r.kind === 'tool') lines.push('<span class="rc-l"><b>Release:</b> ' + esc(r.tag ? r.tag + (r.relDate ? ' (' + r.relDate + ')' : '') : 'none') + '</span>');
    else lines.push('<span class="rc-l"><b>Date:</b> ' + esc(shortDate(r.added) || 'unknown') + '</span>');
    if (r.commit) lines.push('<span class="rc-l"><b>Commit:</b> ' + esc(r.commit.slice(0, 7)) + '</span>');
    if (r.kind === 'tool') lines.push('<span class="rc-l"><b>License:</b> ' + esc(r.lic || 'see project') + '</span>');
    if (r.sig) lines.push('<span class="rc-l"><b>Signed:</b> ' + esc(r.sig) + ' <span class="rc-ok" aria-label="signature present">&#10003;</span></span>');
    return '<div class="fwf fwf-rec mono">' + lines.join('') + (mode === 'head' ? '' : '<p class="rc-sum">' + esc(r.sum) + '</p>') + '</div>';
  }
  function faceAction(r) {
    if (r.kind === 'tool') {
      var T = (r.install && r.install.length ? r.install.map(function (l) { return /^#/.test(l) ? '<span class="c">' + esc(l) + '</span>' : '<span class="p">$</span> ' + esc(l); }) : ['<span class="c"># install: see the project\'s instructions</span>', '<span class="u">' + esc(r.installDocs || r.licUrl || '') + '</span>'])
        .concat(r.use ? ['<span class="c"># use with Freeweights</span>', (/^#/.test(r.use) ? '<span class="c">' + esc(r.use) + '</span>' : '<span class="p">$</span> ' + esc(r.use))] : []);
      return '<div class="fwf fwf-act mono"><div class="act-bar" aria-hidden="true"><i></i><i></i><i></i><span>install</span></div><pre>' + T.join('\n') + '</pre></div>';
    }
    if (r.kind === 'dataset' && !r.nc) {
      var D = (r.rt ? ['<span class="w"># custom terms: read the license before you download</span>'] : []).concat([
        '<span class="p">&gt;&gt;&gt;</span> from datasets import load_dataset',
        '<span class="p">&gt;&gt;&gt;</span> ds = load_dataset("' + esc(r.id) + '")',
        '<span class="c"># with HF_ENDPOINT=https://freeweights.allternit.com &middot; manifest sha256 ' + esc(r.sha || 'pending') + '&hellip;</span>']);
      return '<div class="fwf fwf-act mono"><div class="act-bar" aria-hidden="true"><i></i><i></i><i></i><span>python</span></div><pre>' + D.join('\n') + '</pre></div>';
    }
    var L = r.nc
      ? ['<span class="p">$</span> fw info ' + esc(r.id), '<span class="w">link only &mdash; read the license:</span>', '<span class="u">' + esc(r.licUrl || ('https://' + (r.host || 'huggingface.co') + '/' + r.id)) + '</span>']
      : (r.rt ? ['<span class="w"># custom terms: read the license before you download</span>'] : []).concat([
        '<span class="p">$</span> fw get ' + esc(r.id),
        '<span class="c"># manifest sha256 ' + esc(r.sha || 'pending') + '&hellip; &middot; ' + esc(bytes(r.size)) + ' &middot; ' + esc(r.files || 0) + ' files</span>',
        '<span class="p">$</span> fw verify ./' + esc(repo(r)) + ' ' + esc(r.id)]);
    return '<div class="fwf fwf-act mono"><div class="act-bar" aria-hidden="true"><i></i><i></i><i></i><span>' + (r.nc ? 'link only' : 'fw') + '</span></div><pre>' + L.join('\n') + '</pre></div>';
  }
  // The combined card: RECORD header on top, IDENTITY + FACTS side by side, ACTION footer.
  function combined(r, opt) {
    opt = opt || {};
    return '<article class="fwk' + (opt.cls ? ' ' + opt.cls : '') + '" data-id="' + esc(r.id) + '">' +
      '<div class="fwk-rec">' + faceRecord(r, 'full', { h1: opt.h1 }) + '</div>' +
      '<div class="fwk-mid"><div class="fwk-id">' + faceIdentity(r) + '</div><div class="fwk-facts">' + faceFacts(r) + '</div></div>' +
      '<div class="fwk-act">' + faceAction(r) + '</div>' +
      (opt.open ? '<div class="fwk-foot"><a class="btn pri sm" href="' + esc(r.url) + '">' + (r.nc ? 'License and source' : 'Open model page') + '</a></div>' : '') +
      '</article>';
  }
  // Grid / home / related tile: IDENTITY front, FACTS back (flip), RECORD strip below.
  function tile(r) {
    return '<div class="fwt" data-id="' + esc(r.id) + '">' +
      '<button type="button" class="flip" aria-pressed="false" aria-label="' + esc(r.n) + ': show facts">' +
      '<span class="flip-in"><span class="flip-f">' + faceIdentity(r) + '</span><span class="flip-b">' + faceFacts(r) + '</span></span>' +
      '<span class="flip-hint mono" aria-hidden="true">flip</span></button>' +
      faceRecord(r, 'strip') + '</div>';
  }
  var api = { FONT: FONT, dots: dots, wordmark: wordmark, svg: svg, lockup: lockup, word: word, card: card, deck: deck, bytes: bytes, params: params, esc: esc, STYLES: ['a', 'b', 'c', 'd'],
    logo: logo, identity: faceIdentity, facts: faceFacts, record: faceRecord, action: faceAction, combined: combined, tile: tile };
  g.FWCards = api;
})(typeof window !== 'undefined' ? window : globalThis);

const ROWS=[{"kind": "model", "id": "lmstudio-community/Qwen3.5-4B-GGUF", "n": "Qwen3.5-4B-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 11329544112, "params": 4200000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-14T21:41:57Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 2.7, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 2.5, "e": "MLX"}]}, {"kind": "model", "id": "lmstudio-community/Qwen3.5-2B-GGUF", "n": "Qwen3.5-2B-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 5510586672, "params": 2300000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 1.3, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 1.2, "e": "MLX"}]}, {"kind": "model", "id": "google/gemma-4-E2B-it-qat-q4_0-gguf", "n": "gemma-4-E2B-it-qat-q4_0-gguf", "org": "google", "sec": "alt.freeweights.gemma", "fam": "gemma", "mono": "GM", "url": "#", "size": 4336380945, "params": 5100000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-14T21:41:57Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "https://freeweights.allternit.com/assets/orgs/google.png", "sig": "", "v": [{"f": "GGUF", "q": "Q4_0 QAT", "s": 3.2, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 3.0, "e": "MLX"}]}, {"kind": "model", "id": "ggml-org/SmolLM3-3B-GGUF", "n": "SmolLM3-3B-GGUF", "org": "ggml-org", "sec": "alt.freeweights.smollm", "fam": "smollm", "mono": "SM", "url": "#", "size": 11349231863, "params": 3100000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-14T21:41:57Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 1.9, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 1.8, "e": "MLX"}]}, {"kind": "model", "id": "lmstudio-community/Phi-4-mini-instruct-GGUF", "n": "Phi-4-mini-instruct-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.phi", "fam": "phi", "mono": "PH", "url": "#", "size": 11981766167, "params": 3800000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "MIT", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "https://freeweights.allternit.com/assets/orgs/microsoft.png", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 2.5, "e": "llama.cpp"}]}, {"kind": "model", "id": "Qwen/Qwen3-1.7B-GGUF", "n": "Qwen3-1.7B-GGUF", "org": "Qwen", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 1834445939, "params": 1700000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 1.1, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 1.0, "e": "MLX"}, {"f": "ONNX", "q": "q4f16", "s": 1.2, "e": "WebGPU"}]}, {"kind": "model", "id": "Qwen/Qwen3-0.6B-GGUF", "n": "Qwen3-0.6B-GGUF", "org": "Qwen", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 639466546, "params": 600000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 0.4, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 0.35, "e": "MLX"}, {"f": "ONNX", "q": "q4f16", "s": 0.45, "e": "WebGPU"}]}, {"kind": "model", "id": "lmstudio-community/gemma-4-E4B-it-QAT-GGUF", "n": "gemma-4-E4B-it-QAT-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.gemma", "fam": "gemma", "mono": "GM", "url": "#", "size": 6146496828, "params": 8000000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "https://freeweights.allternit.com/assets/orgs/google.png", "sig": "", "v": [{"f": "GGUF", "q": "Q4_0 QAT", "s": 5.0, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 4.8, "e": "MLX"}]}, {"kind": "model", "id": "lmstudio-community/Qwen3.5-27B-GGUF", "n": "Qwen3.5-27B-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 68149711960, "params": 27000000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 16.5, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 15.6, "e": "MLX"}]}];
const DEV={"mac": {"name": "Joe's MacBook Pro", "hw": "Apple M4 Pro \u00b7 48 GB unified memory", "os": "macOS 26", "kind": "unified", "apple": true, "budget": 31, "disk": "412 GB free", "eng": [["llama.cpp", "Metal", "b7180", "Ready"], ["MLX", "Metal", "0.30.1", "Ready"]], "ship": "Engines ship inside Allternit Desktop and update with it."}, "win": {"name": "Studio PC", "hw": "NVIDIA RTX 4070 12 GB \u00b7 32 GB RAM \u00b7 Ryzen 7", "os": "Windows 11", "kind": "discrete", "vram": 12, "ram": 32, "disk": "690 GB free", "eng": [["llama.cpp", "CUDA 13", "b7180", "Ready"], ["llama.cpp", "Vulkan", "b7180", "Installed, not in use"], ["llama.cpp", "CPU (AVX2)", "b7180", "Fallback"]], "ship": "The CUDA build is picked because an NVIDIA GPU is present. Vulkan stays as the fallback."}, "winarm": {"name": "Surface Laptop", "hw": "Snapdragon X Elite \u00b7 32 GB \u00b7 Adreno X1", "os": "Windows 11 on Arm", "kind": "unified", "budget": 19, "disk": "301 GB free", "eng": [["llama.cpp", "CPU (Arm, KleidiAI)", "b7180", "Ready"], ["llama.cpp", "Vulkan (Adreno)", "b7180", "Ready"], ["NPU (Hexagon)", "QNN", "preview", "Off: turn on in Advanced"]], "ship": "The NPU path is off by default. CPU and GPU are the baseline."}, "linux": {"name": "Workstation", "hw": "AMD Radeon RX 7900 XTX 24 GB \u00b7 64 GB RAM", "os": "Ubuntu 24.04", "kind": "discrete", "vram": 24, "ram": 64, "disk": "1.6 TB free", "eng": [["llama.cpp", "ROCm 7 (HIP)", "b7180", "Ready"], ["llama.cpp", "Vulkan", "b7180", "Installed, not in use"], ["llama.cpp", "CPU", "b7180", "Fallback"]], "ship": "ROCm is picked for RDNA3. Vulkan covers GPUs that ROCm does not support."}, "cpu": {"name": "Home server", "hw": "8 vCPU \u00b7 16 GB RAM \u00b7 no GPU", "os": "Debian 13", "kind": "cpu", "ram": 16, "disk": "88 GB free", "eng": [["llama.cpp", "CPU (AVX2)", "b7180", "Ready"]], "ship": "No GPU found. Models run on the CPU, so small models only."}, "web": {"name": "This browser tab", "hw": "Chrome 142 \u00b7 WebGPU \u00b7 2 GB model limit", "os": "ai.allternit.com", "kind": "browser", "budget": 2, "disk": "Browser storage 41 GB", "eng": [["WebGPU", "transformers.js (ONNX)", "4.3", "Ready"]], "ship": "Nothing to install. Weights are cached in the browser. Bigger models run in Desktop or the phone app."}, "iphone": {"name": "Joe's iPhone", "hw": "iPhone 17 Pro \u00b7 12 GB \u00b7 6 GB per app", "os": "iOS 26", "kind": "phone", "apple": true, "budget": 4.5, "disk": "41 GB free", "eng": [["MLX", "Metal", "0.30.1", "Ready"], ["llama.cpp", "Metal", "b7180", "Ready"], ["Apple Intelligence", "built in", "iOS 26", "Ready"]], "ship": "Engines are inside the app. Apple Intelligence needs no download."}, "android": {"name": "Galaxy S25", "hw": "Snapdragon 8 Elite \u00b7 12 GB", "os": "Android 16", "kind": "phone", "budget": 5, "disk": "96 GB free", "eng": [["llama.cpp", "OpenCL (Adreno)", "b7180", "Ready"], ["llama.cpp", "CPU", "b7180", "Fallback"], ["WebGPU", "in the PWA", "Chrome 142", "Ready (small models)"], ["Gemini Nano", "ML Kit", "system", "Not on this phone"]], "ship": "The app engine runs 1 to 4B models. The PWA alone runs models under 0.5 GB."}};
const pick=(d,r)=>d.kind==='browser'?r.v.find(x=>x.e==='WebGPU'):d.apple?(r.v.find(x=>x.e==='MLX')||r.v.find(x=>x.e==='llama.cpp')):r.v.find(x=>x.e==='llama.cpp');
function fit(d,r){const v=pick(d,r);
 if(!v)return{k:'na',t:'No browser build',why:'Run it in Desktop or the phone app',v:r.v[0]};
 const need=v.s*1.2+0.4,n=need.toFixed(1);
 if(d.kind==='discrete'){if(need<=d.vram*0.9)return{k:'ok',t:'Fits',why:'Full GPU, '+n+' of '+d.vram+' GB VRAM',v};if(need<=d.vram+d.ram*0.5)return{k:'slow',t:'May be slow',why:'Part runs on the CPU',v};return{k:'no',t:"Won't fit",why:'Needs about '+n+' GB',v};}
 if(d.kind==='cpu'){if(need>d.ram*0.6)return{k:'no',t:"Won't fit",why:'Needs about '+n+' GB',v};return v.s<1.5?{k:'ok',t:'Fits',why:'CPU only',v}:{k:'slow',t:'May be slow',why:'CPU only, about 5 tok/s',v};}
 if(need<=d.budget*0.8)return{k:'ok',t:'Fits',why:n+' of '+d.budget+' GB for models',v};
 if(need<=d.budget)return{k:'slow',t:'Tight fit',why:'Close other apps first',v};
 return{k:'no',t:"Won't fit",why:'Needs '+n+' GB, '+d.budget+' GB free for models',v};}
const FC={ok:'var(--ok)',slow:'var(--warn)',no:'var(--bad)',na:'var(--ink3)'};
const ENVS=[['mac','Mac'],['win','Windows + NVIDIA'],['winarm','Windows on Arm'],['linux','Linux + AMD'],['cpu','Linux CPU only'],['web','Browser'],['iphone','iPhone'],['android','Android']];

const FW=window.FWCards;
const H=({html,cls,tag})=>React.createElement(tag||'div',{className:cls,dangerouslySetInnerHTML:{__html:html}});
const Pill=({f,small})=>(<span title={f.why} style={{display:'inline-flex',alignItems:'center',gap:5,fontSize:small?10.5:11.5,fontWeight:600,color:FC[f.k],background:'var(--fill)',borderRadius:999,padding:small?'1px 7px':'2px 9px',whiteSpace:'nowrap'}}><i style={{width:6,height:6,borderRadius:3,background:FC[f.k]}}/>{f.t}</span>);
const Btn=({children,pri,onClick,small})=>(<button onClick={onClick} style={{border:pri?'0':'1px solid var(--line2)',background:pri?'var(--btn)':'transparent',color:pri?'var(--btnink)':'var(--ink)',borderRadius:9,padding:small?'4px 10px':'6px 12px',fontSize:small?11.5:12.5,fontWeight:600,cursor:'pointer',whiteSpace:'nowrap',fontFamily:'inherit'}}>{children}</button>);
function EnvSwitch({env,setEnv,only}){const list=only?ENVS.filter(e=>only.includes(e[0])):ENVS;return(<div style={{display:'flex',flexWrap:'wrap',gap:6,alignItems:'center',padding:'8px 10px',border:'1px dashed var(--line2)',borderRadius:10,marginBottom:12,fontSize:12}}><span className="mono" style={{color:'var(--ink3)',marginRight:4}}>Mockup only, pretend this runs on:</span>{list.map(([k,l])=>(<button key={k} onClick={()=>setEnv(k)} style={{border:0,borderRadius:999,padding:'3px 10px',fontSize:12,cursor:'pointer',fontFamily:'inherit',background:env===k?'var(--btn)':'var(--fill)',color:env===k?'var(--btnink)':'var(--ink2)'}}>{l}</button>))}</div>);}
const TABS=['Catalog','This device','Playground','Train','Cloud'];
function Chrome({tab,children,d}){return(<div style={{background:'var(--bg)',color:'var(--ink)',fontFamily:'var(--font-allternit-sans)',padding:'22px 24px',fontSize:14}}>
<div style={{fontFamily:'var(--font-allternit-serif)',fontSize:28,fontWeight:500,letterSpacing:'-.01em'}}>Freeweights Studio</div>
<div style={{color:'var(--ink2)',fontSize:13.5,marginTop:2}}>Find open weights in Freeweights, then download and run them on this device, your other devices or the cloud.</div>
<div style={{display:'flex',gap:6,marginTop:14,flexWrap:'wrap'}}>{TABS.map(t=>(<span key={t} style={{border:'1px solid '+(t===tab?'var(--ink)':'var(--line)'),background:t===tab?'var(--fill2)':'transparent',borderRadius:10,padding:'5px 12px',fontSize:12.5,fontWeight:600}}>{t}</span>))}</div>
<div style={{display:'flex',alignItems:'center',gap:10,marginTop:14,padding:'10px 12px',border:'1px solid var(--line)',borderRadius:12,flexWrap:'wrap'}}><span style={{width:8,height:8,borderRadius:4,background:'var(--ok)'}}/><b style={{fontSize:13}}>{d.name}</b><span className="mono" style={{fontSize:11.5,color:'var(--ink2)'}}>{d.hw} · {d.os}</span><span style={{flex:1}}/><span className="mono" style={{fontSize:11.5,color:'var(--ink3)'}}>{d.eng.filter(e=>/Ready/.test(e[3])).map(e=>e[0]+' '+e[1]).join(' + ')}</span></div>
<div style={{marginTop:16}}>{children}</div></div>);}

const RUN={mac:['Qwen3.5-4B-GGUF',3.4,46,'2,140 / 8,192'],win:['Qwen3.5-4B-GGUF',3.6,88,'1,020 / 8,192'],winarm:['Qwen3.5-2B-GGUF',1.9,24,'610 / 8,192'],linux:['Qwen3.5-27B-GGUF',19.2,31,'4,800 / 16,384'],cpu:['Qwen3-0.6B-GGUF',0.8,14,'300 / 4,096'],web:['Qwen3-0.6B-GGUF',0.7,38,'220 / 4,096'],iphone:['Qwen3.5-2B-GGUF',1.7,58,'900 / 4,096'],android:['Qwen3.5-2B-GGUF',1.8,21,'700 / 4,096']};
const Sec=({t,children,right})=>(<section style={{border:'1px solid var(--line)',borderRadius:14,padding:'14px 16px',display:'grid',gap:10}}><div style={{display:'flex',justifyContent:'space-between',alignItems:'center'}}><b style={{fontSize:13.5}}>{t}</b>{right}</div>{children}</section>);
const Bar=({v,max})=>(<div style={{height:6,borderRadius:3,background:'var(--fill2)'}}><div style={{width:Math.min(100,v/max*100)+'%',height:6,borderRadius:3,background:'var(--ink)'}}/></div>);
function stCol(s){return /Ready/.test(s)?'var(--ok)':/Off|Not on/.test(s)?'var(--ink3)':/Fallback|not in use/.test(s)?'var(--ink2)':'var(--warn)';}
export default function App(){const [env,setEnv]=React.useState('mac');const d=DEV[env];const rr=RUN[env];const r=ROWS.find(x=>x.n===rr[0]);const f=fit(d,r);
 const mem=d.kind==='discrete'?d.vram:d.kind==='cpu'?d.ram:d.budget;const rec=ROWS.filter(x=>fit(d,x).k==='ok').slice(0,3);
 const [api,setApi]=React.useState(false);
 return(<div style={{background:'var(--bg)'}}><EnvSwitch env={env} setEnv={setEnv}/>
<Chrome tab="This device" d={d}>
<div style={{display:'grid',gridTemplateColumns:'repeat(auto-fit,minmax(300px,1fr))',gap:14}}>
<Sec t="Hardware"><div className="mono" style={{fontSize:12,display:'grid',gridTemplateColumns:'110px 1fr',rowGap:4,color:'var(--ink2)'}}><span>System</span><span style={{color:'var(--ink)'}}>{d.os}</span><span>Hardware</span><span style={{color:'var(--ink)'}}>{d.hw}</span><span>Disk</span><span style={{color:'var(--ink)'}}>{d.disk}</span><span>For models</span><span style={{color:'var(--ink)'}}>{mem} GB {d.kind==='discrete'?'VRAM (full speed), plus RAM for overflow':d.kind==='cpu'?'RAM, CPU speed':d.kind==='browser'?'per tab (WebGPU limit)':'after the system and other apps'}</span></div>
<div style={{fontSize:12,color:'var(--ink3)'}}>Detected by the engine{d.kind==='browser'?' from WebGPU adapter limits':d.kind==='phone'?' from the app\u2019s real free memory':''}. Every fit label in the catalog uses this number.</div></Sec>
<Sec t="Engines" right={<span className="mono" style={{fontSize:11,color:'var(--ink3)'}}>{d.kind==='browser'?'loaded by the page':d.kind==='phone'?'inside the app':'inside Allternit Desktop'}</span>}>
{d.eng.map((e,i)=>(<div key={i} style={{display:'flex',alignItems:'center',gap:10,fontSize:13}}><span style={{width:7,height:7,borderRadius:4,background:stCol(e[3])}}/><span style={{flex:1}}><b>{e[0]}</b> <span style={{color:'var(--ink2)'}}>{e[1]}</span></span><span className="mono" style={{fontSize:11,color:'var(--ink3)'}}>{e[2]}</span><span style={{fontSize:12,color:stCol(e[3]),minWidth:96,textAlign:'right'}}>{e[3]}</span></div>))}
<div style={{fontSize:12,color:'var(--ink3)'}}>{d.ship}</div>
{d.kind!=='browser'&&d.kind!=='phone'?<div style={{display:'flex',gap:8}}><Btn small>Check for engine updates</Btn><Btn small>Advanced</Btn></div>:null}</Sec>
<Sec t="Running now" right={<Btn small>Unload</Btn>}>
<div style={{display:'flex',justifyContent:'space-between',alignItems:'center',gap:8}}><div><b style={{fontSize:13.5}}>{r.n}</b><div className="mono" style={{fontSize:11.5,color:'var(--ink2)'}}>{f.v.f} {f.v.q} · {f.v.e}</div></div><Btn pri small>Open in Playground</Btn></div>
<div className="mono" style={{fontSize:11.5,color:'var(--ink2)',display:'grid',gridTemplateColumns:'1fr 1fr 1fr',gap:6}}><span><b style={{color:'var(--ink)',fontSize:15}}>{rr[2]}</b> tok/s</span><span><b style={{color:'var(--ink)',fontSize:15}}>{rr[1]}</b> GB used</span><span><b style={{color:'var(--ink)',fontSize:15}}>{rr[3].split(' / ')[0]}</b> / {rr[3].split(' / ')[1]} ctx</span></div>
<Bar v={rr[1]} max={mem}/>
<div style={{fontSize:12,color:'var(--ink3)'}}>Unloads after 5 minutes without use{d.kind==='phone'?' or when the app goes to the background':''}.</div>
{d.kind==='browser'?null:<label style={{display:'flex',gap:8,alignItems:'center',fontSize:12.5,cursor:'pointer'}} onClick={()=>setApi(!api)}><span style={{width:30,height:18,borderRadius:9,background:api?'var(--ok)':'var(--fill2)',position:'relative'}}><i style={{position:'absolute',top:2,left:api?14:2,width:14,height:14,borderRadius:7,background:'#fff'}}/></span>{d.kind==='phone'?'Let my other devices use this model (through my Allternit account)':'OpenAI-compatible API on localhost:3015 (this computer only)'}</label>}</Sec>
<Sec t="Downloads"><div style={{display:'grid',gap:6}}><div style={{display:'flex',justifyContent:'space-between',fontSize:13}}><span>{(rec[1]||rec[0]||ROWS[1]).n}</span><span className="mono" style={{fontSize:11.5,color:'var(--ink2)'}}>63% · {d.kind==='phone'?'continues in the background':'resumes after a restart'}</span></div><Bar v={63} max={100}/><div style={{display:'flex',gap:8}}><Btn small>Pause</Btn><Btn small>Cancel</Btn></div></div>
<div style={{fontSize:12,color:'var(--ink3)'}}>After the download: sha256 check against the signed Freeweights manifest. A file that fails is deleted and never loaded.</div></Sec>
<Sec t="Storage" right={d.kind==='browser'||d.kind==='phone'?null:<Btn small>Change folder</Btn>}><div style={{display:'grid',gap:6,fontSize:13}}>{[r,...rec.filter(x=>x!==r).slice(0,1)].map(x=>{const fx=fit(d,x);return(<div key={x.id} style={{display:'flex',justifyContent:'space-between',alignItems:'center'}}><span>{x.n}</span><span style={{display:'flex',gap:10,alignItems:'center'}}><span className="mono" style={{fontSize:11.5,color:'var(--ink2)'}}>{fx.v.s} GB</span><Btn small>Delete</Btn></span></div>);})}</div><div className="mono" style={{fontSize:11.5,color:'var(--ink3)'}}>{d.kind==='browser'?'Kept in this browser (OPFS). Clearing site data removes it.':d.kind==='phone'?'Kept in the app, not in iCloud or Google backups.':d.kind==='discrete'&&d.os.startsWith('Windows')?'C:\\Users\\joe\\AppData\\Local\\Allternit\\models':d.os.startsWith('macOS')?'~/Library/Application Support/Allternit/models':'~/.local/share/allternit/models'}</div></Sec>
<Sec t={'Recommended for '+d.name}>{rec.map(x=>{const fx=fit(d,x);return(<div key={x.id} style={{display:'flex',alignItems:'center',justifyContent:'space-between',gap:8}}><div style={{minWidth:0}}><div style={{fontSize:13,fontWeight:600}}>{x.n}</div><div className="mono" style={{fontSize:11,color:'var(--ink2)'}}>{fx.v.f} {fx.v.q} · {fx.v.s} GB · {x.lic}</div></div><Btn pri small>Get</Btn></div>);})}
{d.kind==='phone'&&env==='iphone'?<div style={{display:'flex',alignItems:'center',justifyContent:'space-between'}}><div><div style={{fontSize:13,fontWeight:600}}>Apple Intelligence</div><div className="mono" style={{fontSize:11,color:'var(--ink2)'}}>built in · no download · 4k context</div></div><Btn small>Use</Btn></div>:null}</Sec>
</div></Chrome></div>);}

```


```react css=fw.css title="Model Lab › Playground: picker grouped by device, stats on each reply" h=560
import React from 'react';
// Freeweights shared rendering: the Allternit dot lettering (same 10-unit grid, 8.5 dots,
// 1.5 radius as the A://TERNIT wordmark) and the four preview card styles (A to D).
// Loaded by site/build.mjs at build time and by the browser for grid, deck and style switching.
(function (g) {
  var FONT = {
    A: ['..#..', '.#.#.', '#####', '#...#', '#...#'], B: ['####.', '#...#', '####.', '#...#', '####.'],
    C: ['.####', '#....', '#....', '#....', '.####'], D: ['####.', '#...#', '#...#', '#...#', '####.'],
    E: ['#####', '#....', '####.', '#....', '#####'], F: ['#####', '#....', '####.', '#....', '#....'],
    G: ['.####', '#....', '#.###', '#...#', '.###.'], H: ['#...#', '#...#', '#####', '#...#', '#...#'],
    I: ['###', '.#.', '.#.', '.#.', '###'], J: ['..###', '...#.', '...#.', '#..#.', '.##..'],
    K: ['#...#', '#..#.', '###..', '#..#.', '#...#'], L: ['#....', '#....', '#....', '#....', '#####'],
    M: ['#...#', '##.##', '#.#.#', '#...#', '#...#'], N: ['#...#', '##..#', '#.#.#', '#..##', '#...#'],
    O: ['.###.', '#...#', '#...#', '#...#', '.###.'], P: ['####.', '#...#', '####.', '#....', '#....'],
    Q: ['.###.', '#...#', '#.#.#', '#..#.', '.##.#'], R: ['####.', '#...#', '####.', '#.#..', '#..##'],
    S: ['.####', '#....', '.###.', '....#', '####.'], T: ['#####', '..#..', '..#..', '..#..', '..#..'],
    U: ['#...#', '#...#', '#...#', '#...#', '.###.'], V: ['#...#', '#...#', '#...#', '.#.#.', '..#..'],
    W: ['#...#', '#...#', '#.#.#', '##.##', '#...#'], X: ['#...#', '.#.#.', '..#..', '.#.#.', '#...#'],
    Y: ['#...#', '.#.#.', '..#..', '..#..', '..#..'], Z: ['#####', '...#.', '..#..', '.#...', '#####'],
    0: ['.###.', '#..##', '#.#.#', '##..#', '.###.'], 1: ['.#.', '##.', '.#.', '.#.', '###'],
    2: ['####.', '....#', '.###.', '#....', '#####'], 3: ['####.', '....#', '.###.', '....#', '####.'],
    4: ['#..#.', '#..#.', '#####', '...#.', '...#.'], 5: ['#####', '#....', '####.', '....#', '####.'],
    6: ['.###.', '#....', '####.', '#...#', '.###.'], 7: ['#####', '....#', '...#.', '..#..', '..#..'],
    8: ['.###.', '#...#', '.###.', '#...#', '.###.'], 9: ['.###.', '#...#', '.####', '....#', '.###.'],
    '.': ['.', '.', '.', '.', '#'], ':': ['.', '#', '.', '#', '.'], '/': ['..#', '..#', '.#.', '#..', '#..'],
    '-': ['...', '...', '###', '...', '...'], ' ': ['..', '..', '..', '..', '..']
  };
  var CELL = 10;
  function rect(c, r, fill) { return '<rect x="' + (c * CELL + 0.75) + '" y="' + (r * CELL + 0.75) + '" width="8.5" height="8.5" rx="1.5"' + (fill ? ' fill="' + fill + '"' : '') + '/>'; }
  // Lay out text on the dot grid. Returns { cols, rows, body }. Letters are separated by one empty column.
  function dots(text, col0, row0) {
    var c = col0 || 0, r0 = row0 || 0, body = '', chars = String(text).toUpperCase().split('');
    chars.forEach(function (ch, i) {
      var gl = FONT[ch] || FONT[' '];
      for (var r = 0; r < 5; r++) for (var x = 0; x < gl[r].length; x++) if (gl[r][x] === '#') body += rect(c + x, r0 + r);
      c += gl[0].length + (i < chars.length - 1 ? 1 : 0);
    });
    return { cols: c - (col0 || 0), body: body };
  }
  // The A://TERNIT wordmark cells, copied from the brand SVG (o = the coral dot).
  var WM = ['..#.......#...#.#####.#####.####..#...#..###..#####', '.#.#..#...#...#...#...#.....#...#.##..#...#.....#..', '##o##....#...#....#...####..####..#.#.#...#.....#..', '#...#.#.#...#.....#...#.....#.#...#..##...#.....#..', '#...#...#...#.....#...#####.#..##.#...#..###....#..'];
  function wordmark(col0, row0) {
    var b = '';
    WM.forEach(function (line, r) { for (var x = 0; x < line.length; x++) { if (line[x] === '#') b += rect(col0 + x, row0 + r); else if (line[x] === 'o') b += rect(col0 + x, row0 + r, '#D97757'); } });
    return { cols: WM[0].length, body: b };
  }
  function svg(cols, rows, body, h, label, cls) {
    var w = Math.round((h * cols) / rows * 100) / 100;
    return '<svg class="' + (cls || 'dotw') + '" viewBox="0 0 ' + cols * CELL + ' ' + rows * CELL + '" height="' + h + '" width="' + w + '" fill="currentColor" shape-rendering="geometricPrecision"' + (label ? ' role="img" aria-label="' + esc(label) + '"' : ' aria-hidden="true"') + '>' + body + '</svg>';
  }
  // Lockups. inline: A://TERNIT  FREEWEIGHTS on one line. stack: wordmark above the name.
  function lockup(kind, h, cls) {
    var a = wordmark(0, 0);
    if (kind === 'stack') { var n = dots('FREEWEIGHTS', 0, 7); return svg(Math.max(a.cols, n.cols), 12, a.body + n.body, h, 'Allternit Freeweights', cls || 'lockup stack'); }
    if (kind === 'name') { var o = dots('FREEWEIGHTS'); return svg(o.cols, 5, o.body, h, 'Freeweights', cls || 'lockup name'); }
    var m = dots('FREEWEIGHTS', a.cols + 4, 0); return svg(a.cols + 4 + m.cols, 5, a.body + m.body, h, 'Allternit Freeweights', cls || 'lockup inline');
  }
  function word(text, h, cls, label) { var d = dots(text); return svg(d.cols, 5, d.body, h, label, cls); }

  var esc = function (s) { return String(s == null ? '' : s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'); };
  function bytes(n) {
    if (n == null || isNaN(n)) return 'size unknown';
    var u = ['B', 'KB', 'MB', 'GB', 'TB'], i = 0, v = Number(n);
    while (v >= 1000 && i < u.length - 1) { v /= 1000; i++; }
    return (v >= 100 || i === 0 ? Math.round(v).toLocaleString('en-US') : v >= 10 ? v.toFixed(1) : v.toFixed(2)) + ' ' + u[i];
  }
  function sizeParts(n) { var s = bytes(n).split(' '); return { v: s[0], u: s[1] || '' }; }
  function params(n) { return !n ? '' : n >= 1e12 ? +(n / 1e12).toFixed(2) + 'T' : n >= 1e9 ? +(n / 1e9).toFixed(1) + 'B' : +(n / 1e6).toFixed(0) + 'M'; }
  var LC = { copyleft: 'copyleft', permissive: 'permissive', custom: 'custom terms', 'non-commercial': 'non-commercial', gated: 'gated', territory: 'territory-limited', unreviewed: 'license not reviewed' };
  function src(r) { return r.nc ? (r.how || 'From the publisher') : r.src || 'From source'; }
  function srcBadge(r) { var cls = r.nc || /source|publisher/i.test(src(r)) ? ' src' : /seed/i.test(src(r)) ? ' seed' : ''; return '<span class="b' + cls + '"><i class="d"></i>' + esc(src(r)) + '</span>'; }
  function lcTag(r) { return '<span class="lc lc-' + esc(r.lc || 'unknown') + '">' + esc(LC[r.lc] || r.lc || 'see card') + '</span>'; }
  function task(r) { var t = (r.task || [])[0]; return t ? t.replace(/-/g, ' ') : r.sec === 'alt.freeweights.datasets' ? 'dataset' : ''; }
  function date(t) { var d = new Date(t); if (isNaN(d)) return ''; return d.toUTCString().slice(0, 16); }
  function mono(r, h, cls) { return word(r.mono || 'FW', h, cls || 'mono-svg', null); }

  // A. Spec sheet
  function cardA(r) {
    var spec = [['Params', params(r.params) || 'not stated'], ['Size', bytes(r.size)], ['Format', (r.fmt || []).join(', ') || 'see files'], ['License', esc(r.lic)]];
    return '<a class="fwc fwc-a" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '">' +
      '<div class="a-top"><span class="a-mono">' + mono(r, 22) + '</span><div class="a-t"><div class="a-sec mono">' + esc(r.sec) + '</div><div class="a-name">' + esc(r.n) + '</div><div class="a-id mono">' + esc(r.id) + '</div></div></div>' +
      '<dl class="a-spec">' + spec.map(function (s) { return '<div><dt>' + s[0] + '</dt><dd' + (s[0] === 'Format' ? ' class="mono"' : '') + '>' + (s[0] === 'License' ? s[1] : esc(s[1])) + '</dd></div>'; }).join('') + '</dl>' +
      '<div class="a-foot">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + (task(r) ? '<span class="a-task">' + esc(task(r)) + '</span>' : '') + '</div></a>';
  }
  // B. Newsgroup post
  function cardB(r) {
    return '<a class="fwc fwc-b" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '">' +
      '<div class="b-head mono"><div><b>From:</b> ' + esc(r.org) + ' &lt;' + esc(r.host || 'huggingface.co') + '/' + esc(r.org) + '&gt;</div><div><b>Newsgroups:</b> ' + esc(r.sec) + '</div><div><b>Subject:</b> <span class="b-subj">' + esc(r.n) + '</span> (' + esc(bytes(r.size)) + ')</div><div><b>Date:</b> ' + esc(date(r.added)) + '</div></div>' +
      '<p class="b-body">' + esc(r.sum) + '</p>' +
      '<div class="b-sig mono">-- <br>' + esc(r.lic) + ' · ' + esc(LC[r.lc] || r.lc || '') + ' · ' + esc(src(r).toLowerCase()) + (r.rt ? ' · read terms first' : '') + '</div></a>';
  }
  // C. Element tile
  function cardC(r) {
    var s = sizeParts(r.size);
    return '<a class="fwc fwc-c" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '">' +
      '<div class="c-row mono"><span>' + esc(params(r.params) || '&mdash;').replace('&amp;mdash;', '&mdash;') + '</span><span>' + esc(LC[r.lc] || r.lc || '') + '</span></div>' +
      '<div class="c-mono">' + mono(r, 40) + '</div>' +
      '<div class="c-size"><b class="mono">' + esc(s.v) + '</b><span class="mono">' + esc(s.u) + '</span></div>' +
      '<div class="c-name">' + esc(r.n) + '</div>' +
      '<div class="c-facts mono"><span>' + esc((r.fmt || []).join(' ') || 'files') + '</span><span>' + esc(task(r) || r.fam) + '</span></div></a>';
  }
  // D. Terminal
  function cardD(r) {
    var lines = r.nc
      ? ['<span class="p">$</span> fw info ' + esc(r.id), '<span class="c"># link only: ' + esc(r.lic) + (r.lc ? ' (' + esc(LC[r.lc] || r.lc) + ')' : '') + '</span>', '<span class="c"># from the publisher, never hosted here</span>', '<span class="w">[publisher]</span> ' + esc(r.host || 'huggingface.co')]
      : ['<span class="p">$</span> fw get ' + esc(r.id), '<span class="c"># manifest sha256 ' + esc(r.sha || 'pending') + '&hellip;</span>', '<span class="c"># ' + esc(bytes(r.size)) + ' · ' + esc(r.files || 0) + ' files · ' + esc((r.fmt || []).join(', ')) + '</span>', '<span class="c"># license ' + esc(r.lic) + ' (' + esc(r.lc || 'see card') + ')' + (r.rt ? ', read terms first' : '') + '</span>', '<span class="ok">[' + esc(src(r).toLowerCase().replace(/^from /, '')) + ']</span> ' + esc(r.host || 'huggingface.co') + (r.commit ? ' @ ' + esc(r.commit.slice(0, 7)) : '')];
    return '<a class="fwc fwc-d mono" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '"><div class="d-bar"><i></i><i></i><i></i><span>' + esc(r.sec) + '</span></div><pre>' + lines.join('\n') + '</pre></a>';
  }
  var CARDS = { a: cardA, b: cardB, c: cardC, d: cardD };
  function card(style, r) { return (CARDS[style] || cardA)(r); }
  // Deck: one model in focus.
  function deck(r) {
    var facts = [['Size', bytes(r.size)], ['Params', params(r.params) || 'not stated'], ['Formats', (r.fmt || []).join(', ') || 'see files'], ['Task', task(r) || 'not tagged'], ['License', r.lic + ' (' + (LC[r.lc] || r.lc || 'see card') + ')'], ['Files', r.nc ? 'link only' : (r.files || 0) + ' with sha256'], ['Publisher', r.org], ['Added', date(r.added)]];
    return '<article class="deck-card"><div class="dk-top"><span class="dk-mono">' + mono(r, 52) + '</span><div><div class="mono dk-sec">' + esc(r.sec) + '</div><h2 class="dk-name">' + esc(r.n) + '</h2><div class="mono dk-id">' + esc(r.id) + '</div></div></div>' +
      '<p class="dk-sum">' + esc(r.sum) + '</p><dl class="dk-facts">' + facts.map(function (f) { return '<div><dt>' + f[0] + '</dt><dd>' + esc(f[1]) + '</dd></div>'; }).join('') + '</dl>' +
      '<div class="dk-foot"><div class="badges">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + '</div><a class="btn pri" href="' + esc(r.url) + '">' + (r.nc ? 'License and source' : 'Open model page') + '</a></div></article>';
  }

  // ---------- One card, four faces ----------
  // IDENTITY (element tile), FACTS (spec sheet), RECORD (newsgroup header), ACTION (terminal).
  // Every view composes these from the same row, so the faces read as one card.
  function logo(r, px, cls) { return r.logo ? '<img class="org' + (cls ? ' ' + cls : '') + '" src="' + esc(r.logo) + '" alt="" width="' + px + '" height="' + px + '" loading="lazy" decoding="async">' : ''; }
  function shortDate(t) { var d = new Date(t); return isNaN(d) ? '' : d.toISOString().slice(0, 10); }
  function repo(r) { return String(r.id).split('/').slice(1).join('/'); }
  function lcName(r) { return LC[r.lc] || r.lc || 'see card'; }
  // Kinds: model and dataset share the size figure; a tool shows its latest release there.
  function faceIdentity(r) {
    if (r.kind === 'tool') {
      var tag = r.tag || 'no release';
      return '<span class="fwf fwf-id">' +
        '<span class="id-row mono"><span>' + esc(r.tkind || 'tool') + '</span>' + logo(r, 24, 'id-org') + '</span>' +
        '<span class="id-mono">' + mono(r, 40) + '</span>' +
        '<span class="id-size tag' + (tag.length > 8 ? ' long' : tag.length > 5 ? ' mid' : '') + '"><b class="mono">' + esc(tag) + '</b><span class="mono">release</span></span>' +
        '<span class="id-name">' + esc(r.n) + '</span>' +
        '<span class="id-facts mono"><span>' + esc(r.lang || 'tool') + '</span><span>' + esc(r.lic || '') + '</span></span></span>';
    }
    var s = sizeParts(r.size);
    return '<span class="fwf fwf-id">' +
      '<span class="id-row mono"><span>' + esc(params(r.params) ? params(r.params) + ' params' : (r.fam || '')) + '</span>' + logo(r, 24, 'id-org') + '</span>' +
      '<span class="id-mono">' + mono(r, 40) + '</span>' +
      '<span class="id-size"><b class="mono">' + esc(s.v) + '</b><span class="mono">' + esc(s.u) + '</span></span>' +
      '<span class="id-name">' + esc(r.n) + '</span>' +
      '<span class="id-facts mono"><span>' + esc((r.fmt || []).join(' ') || 'files') + '</span><span>' + esc(lcName(r)) + '</span></span></span>';
  }
  function faceFacts(r) {
    if (r.kind === 'tool') {
      var tr = [['License', r.lic || 'see project'], ['Language', r.lang || 'not stated'], ['Kind', r.tkind || 'tool'], ['Release', r.relDate || 'none'], ['Platforms', (r.platforms || []).join(', ') || 'see project']];
      return '<span class="fwf fwf-facts"><span class="ff-h mono">Facts</span><span class="ff-dl">' +
        tr.map(function (x) { return '<span class="ff-r' + (x[0] === 'Platforms' ? ' wide' : '') + '"><span class="ff-k">' + x[0] + '</span><span class="ff-v">' + esc(x[1]) + '</span></span>'; }).join('') +
        '</span><span class="ff-b"><span class="b src"><i class="d"></i>Links to the project</span>' + lcTag(r) + '</span></span>';
    }
    if (r.kind === 'dataset') {
      var dr = [['Rows', r.rowsN ? Number(r.rowsN).toLocaleString('en-US') : 'not stated'], ['Size', bytes(r.size)], ['Format', (r.fmt || []).join(', ') || 'see files'], ['License', r.lic]];
      return '<span class="fwf fwf-facts"><span class="ff-h mono">Facts</span><span class="ff-dl">' +
        dr.map(function (x) { return '<span class="ff-r"><span class="ff-k">' + x[0] + '</span><span class="ff-v' + (x[0] === 'Format' ? ' mono' : '') + '">' + esc(x[1]) + '</span></span>'; }).join('') +
        '</span><span class="ff-b">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + '</span></span>';
    }
    var rows = [['Params', params(r.params) || 'not stated'], ['Size', bytes(r.size)], ['Formats', (r.fmt || []).join(', ') || 'see files'], ['Task', task(r) || 'not tagged'], ['License', r.lic]];
    return '<span class="fwf fwf-facts"><span class="ff-h mono">Facts</span><span class="ff-dl">' +
      rows.map(function (x) { return '<span class="ff-r"><span class="ff-k">' + x[0] + '</span><span class="ff-v' + (x[0] === 'Formats' ? ' mono' : '') + '">' + esc(x[1]) + '</span></span>'; }).join('') +
      '</span><span class="ff-b">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + '</span></span>';
  }
  // mode: 'full' (all header lines + summary), 'strip' (From + Subject, under a tile), 'head' (lines, no summary)
  function faceRecord(r, mode, opt) {
    opt = opt || {};
    var subj = opt.h1 ? '<h1 class="rc-subj rc-h1">' + esc(r.n) + '</h1>' : opt.link === false ? '<span class="rc-subj">' + esc(r.n) + '</span>' : '<a class="rc-subj" href="' + esc(r.url) + '">' + esc(r.n) + '</a>';
    var from = '<span class="rc-l"><b>From:</b> ' + logo(r, 18, 'rc-org') + esc(r.org) + ' <span class="rc-dim">&lt;' + esc(r.host || 'huggingface.co') + '/' + esc(r.org) + '&gt;</span></span>';
    if (mode === 'strip') return '<span class="fwf fwf-rec strip mono">' + from + '<span class="rc-l"><b>Subject:</b> ' + subj + '</span></span>';
    var lines = [from, '<span class="rc-l"><b>Newsgroups:</b> ' + esc(r.sec) + '</span>', '<span class="rc-l"><b>Subject:</b> ' + subj + '</span>'];
    if (r.kind === 'tool') lines.push('<span class="rc-l"><b>Release:</b> ' + esc(r.tag ? r.tag + (r.relDate ? ' (' + r.relDate + ')' : '') : 'none') + '</span>');
    else lines.push('<span class="rc-l"><b>Date:</b> ' + esc(shortDate(r.added) || 'unknown') + '</span>');
    if (r.commit) lines.push('<span class="rc-l"><b>Commit:</b> ' + esc(r.commit.slice(0, 7)) + '</span>');
    if (r.kind === 'tool') lines.push('<span class="rc-l"><b>License:</b> ' + esc(r.lic || 'see project') + '</span>');
    if (r.sig) lines.push('<span class="rc-l"><b>Signed:</b> ' + esc(r.sig) + ' <span class="rc-ok" aria-label="signature present">&#10003;</span></span>');
    return '<div class="fwf fwf-rec mono">' + lines.join('') + (mode === 'head' ? '' : '<p class="rc-sum">' + esc(r.sum) + '</p>') + '</div>';
  }
  function faceAction(r) {
    if (r.kind === 'tool') {
      var T = (r.install && r.install.length ? r.install.map(function (l) { return /^#/.test(l) ? '<span class="c">' + esc(l) + '</span>' : '<span class="p">$</span> ' + esc(l); }) : ['<span class="c"># install: see the project\'s instructions</span>', '<span class="u">' + esc(r.installDocs || r.licUrl || '') + '</span>'])
        .concat(r.use ? ['<span class="c"># use with Freeweights</span>', (/^#/.test(r.use) ? '<span class="c">' + esc(r.use) + '</span>' : '<span class="p">$</span> ' + esc(r.use))] : []);
      return '<div class="fwf fwf-act mono"><div class="act-bar" aria-hidden="true"><i></i><i></i><i></i><span>install</span></div><pre>' + T.join('\n') + '</pre></div>';
    }
    if (r.kind === 'dataset' && !r.nc) {
      var D = (r.rt ? ['<span class="w"># custom terms: read the license before you download</span>'] : []).concat([
        '<span class="p">&gt;&gt;&gt;</span> from datasets import load_dataset',
        '<span class="p">&gt;&gt;&gt;</span> ds = load_dataset("' + esc(r.id) + '")',
        '<span class="c"># with HF_ENDPOINT=https://freeweights.allternit.com &middot; manifest sha256 ' + esc(r.sha || 'pending') + '&hellip;</span>']);
      return '<div class="fwf fwf-act mono"><div class="act-bar" aria-hidden="true"><i></i><i></i><i></i><span>python</span></div><pre>' + D.join('\n') + '</pre></div>';
    }
    var L = r.nc
      ? ['<span class="p">$</span> fw info ' + esc(r.id), '<span class="w">link only &mdash; read the license:</span>', '<span class="u">' + esc(r.licUrl || ('https://' + (r.host || 'huggingface.co') + '/' + r.id)) + '</span>']
      : (r.rt ? ['<span class="w"># custom terms: read the license before you download</span>'] : []).concat([
        '<span class="p">$</span> fw get ' + esc(r.id),
        '<span class="c"># manifest sha256 ' + esc(r.sha || 'pending') + '&hellip; &middot; ' + esc(bytes(r.size)) + ' &middot; ' + esc(r.files || 0) + ' files</span>',
        '<span class="p">$</span> fw verify ./' + esc(repo(r)) + ' ' + esc(r.id)]);
    return '<div class="fwf fwf-act mono"><div class="act-bar" aria-hidden="true"><i></i><i></i><i></i><span>' + (r.nc ? 'link only' : 'fw') + '</span></div><pre>' + L.join('\n') + '</pre></div>';
  }
  // The combined card: RECORD header on top, IDENTITY + FACTS side by side, ACTION footer.
  function combined(r, opt) {
    opt = opt || {};
    return '<article class="fwk' + (opt.cls ? ' ' + opt.cls : '') + '" data-id="' + esc(r.id) + '">' +
      '<div class="fwk-rec">' + faceRecord(r, 'full', { h1: opt.h1 }) + '</div>' +
      '<div class="fwk-mid"><div class="fwk-id">' + faceIdentity(r) + '</div><div class="fwk-facts">' + faceFacts(r) + '</div></div>' +
      '<div class="fwk-act">' + faceAction(r) + '</div>' +
      (opt.open ? '<div class="fwk-foot"><a class="btn pri sm" href="' + esc(r.url) + '">' + (r.nc ? 'License and source' : 'Open model page') + '</a></div>' : '') +
      '</article>';
  }
  // Grid / home / related tile: IDENTITY front, FACTS back (flip), RECORD strip below.
  function tile(r) {
    return '<div class="fwt" data-id="' + esc(r.id) + '">' +
      '<button type="button" class="flip" aria-pressed="false" aria-label="' + esc(r.n) + ': show facts">' +
      '<span class="flip-in"><span class="flip-f">' + faceIdentity(r) + '</span><span class="flip-b">' + faceFacts(r) + '</span></span>' +
      '<span class="flip-hint mono" aria-hidden="true">flip</span></button>' +
      faceRecord(r, 'strip') + '</div>';
  }
  var api = { FONT: FONT, dots: dots, wordmark: wordmark, svg: svg, lockup: lockup, word: word, card: card, deck: deck, bytes: bytes, params: params, esc: esc, STYLES: ['a', 'b', 'c', 'd'],
    logo: logo, identity: faceIdentity, facts: faceFacts, record: faceRecord, action: faceAction, combined: combined, tile: tile };
  g.FWCards = api;
})(typeof window !== 'undefined' ? window : globalThis);

const ROWS=[{"kind": "model", "id": "lmstudio-community/Qwen3.5-4B-GGUF", "n": "Qwen3.5-4B-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 11329544112, "params": 4200000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-14T21:41:57Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 2.7, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 2.5, "e": "MLX"}]}, {"kind": "model", "id": "lmstudio-community/Qwen3.5-2B-GGUF", "n": "Qwen3.5-2B-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 5510586672, "params": 2300000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 1.3, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 1.2, "e": "MLX"}]}, {"kind": "model", "id": "google/gemma-4-E2B-it-qat-q4_0-gguf", "n": "gemma-4-E2B-it-qat-q4_0-gguf", "org": "google", "sec": "alt.freeweights.gemma", "fam": "gemma", "mono": "GM", "url": "#", "size": 4336380945, "params": 5100000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-14T21:41:57Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "https://freeweights.allternit.com/assets/orgs/google.png", "sig": "", "v": [{"f": "GGUF", "q": "Q4_0 QAT", "s": 3.2, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 3.0, "e": "MLX"}]}, {"kind": "model", "id": "ggml-org/SmolLM3-3B-GGUF", "n": "SmolLM3-3B-GGUF", "org": "ggml-org", "sec": "alt.freeweights.smollm", "fam": "smollm", "mono": "SM", "url": "#", "size": 11349231863, "params": 3100000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-14T21:41:57Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 1.9, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 1.8, "e": "MLX"}]}, {"kind": "model", "id": "lmstudio-community/Phi-4-mini-instruct-GGUF", "n": "Phi-4-mini-instruct-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.phi", "fam": "phi", "mono": "PH", "url": "#", "size": 11981766167, "params": 3800000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "MIT", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "https://freeweights.allternit.com/assets/orgs/microsoft.png", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 2.5, "e": "llama.cpp"}]}, {"kind": "model", "id": "Qwen/Qwen3-1.7B-GGUF", "n": "Qwen3-1.7B-GGUF", "org": "Qwen", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 1834445939, "params": 1700000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 1.1, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 1.0, "e": "MLX"}, {"f": "ONNX", "q": "q4f16", "s": 1.2, "e": "WebGPU"}]}, {"kind": "model", "id": "Qwen/Qwen3-0.6B-GGUF", "n": "Qwen3-0.6B-GGUF", "org": "Qwen", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 639466546, "params": 600000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 0.4, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 0.35, "e": "MLX"}, {"f": "ONNX", "q": "q4f16", "s": 0.45, "e": "WebGPU"}]}, {"kind": "model", "id": "lmstudio-community/gemma-4-E4B-it-QAT-GGUF", "n": "gemma-4-E4B-it-QAT-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.gemma", "fam": "gemma", "mono": "GM", "url": "#", "size": 6146496828, "params": 8000000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "https://freeweights.allternit.com/assets/orgs/google.png", "sig": "", "v": [{"f": "GGUF", "q": "Q4_0 QAT", "s": 5.0, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 4.8, "e": "MLX"}]}, {"kind": "model", "id": "lmstudio-community/Qwen3.5-27B-GGUF", "n": "Qwen3.5-27B-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 68149711960, "params": 27000000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 16.5, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 15.6, "e": "MLX"}]}];
const DEV={"mac": {"name": "Joe's MacBook Pro", "hw": "Apple M4 Pro \u00b7 48 GB unified memory", "os": "macOS 26", "kind": "unified", "apple": true, "budget": 31, "disk": "412 GB free", "eng": [["llama.cpp", "Metal", "b7180", "Ready"], ["MLX", "Metal", "0.30.1", "Ready"]], "ship": "Engines ship inside Allternit Desktop and update with it."}, "win": {"name": "Studio PC", "hw": "NVIDIA RTX 4070 12 GB \u00b7 32 GB RAM \u00b7 Ryzen 7", "os": "Windows 11", "kind": "discrete", "vram": 12, "ram": 32, "disk": "690 GB free", "eng": [["llama.cpp", "CUDA 13", "b7180", "Ready"], ["llama.cpp", "Vulkan", "b7180", "Installed, not in use"], ["llama.cpp", "CPU (AVX2)", "b7180", "Fallback"]], "ship": "The CUDA build is picked because an NVIDIA GPU is present. Vulkan stays as the fallback."}, "winarm": {"name": "Surface Laptop", "hw": "Snapdragon X Elite \u00b7 32 GB \u00b7 Adreno X1", "os": "Windows 11 on Arm", "kind": "unified", "budget": 19, "disk": "301 GB free", "eng": [["llama.cpp", "CPU (Arm, KleidiAI)", "b7180", "Ready"], ["llama.cpp", "Vulkan (Adreno)", "b7180", "Ready"], ["NPU (Hexagon)", "QNN", "preview", "Off: turn on in Advanced"]], "ship": "The NPU path is off by default. CPU and GPU are the baseline."}, "linux": {"name": "Workstation", "hw": "AMD Radeon RX 7900 XTX 24 GB \u00b7 64 GB RAM", "os": "Ubuntu 24.04", "kind": "discrete", "vram": 24, "ram": 64, "disk": "1.6 TB free", "eng": [["llama.cpp", "ROCm 7 (HIP)", "b7180", "Ready"], ["llama.cpp", "Vulkan", "b7180", "Installed, not in use"], ["llama.cpp", "CPU", "b7180", "Fallback"]], "ship": "ROCm is picked for RDNA3. Vulkan covers GPUs that ROCm does not support."}, "cpu": {"name": "Home server", "hw": "8 vCPU \u00b7 16 GB RAM \u00b7 no GPU", "os": "Debian 13", "kind": "cpu", "ram": 16, "disk": "88 GB free", "eng": [["llama.cpp", "CPU (AVX2)", "b7180", "Ready"]], "ship": "No GPU found. Models run on the CPU, so small models only."}, "web": {"name": "This browser tab", "hw": "Chrome 142 \u00b7 WebGPU \u00b7 2 GB model limit", "os": "ai.allternit.com", "kind": "browser", "budget": 2, "disk": "Browser storage 41 GB", "eng": [["WebGPU", "transformers.js (ONNX)", "4.3", "Ready"]], "ship": "Nothing to install. Weights are cached in the browser. Bigger models run in Desktop or the phone app."}, "iphone": {"name": "Joe's iPhone", "hw": "iPhone 17 Pro \u00b7 12 GB \u00b7 6 GB per app", "os": "iOS 26", "kind": "phone", "apple": true, "budget": 4.5, "disk": "41 GB free", "eng": [["MLX", "Metal", "0.30.1", "Ready"], ["llama.cpp", "Metal", "b7180", "Ready"], ["Apple Intelligence", "built in", "iOS 26", "Ready"]], "ship": "Engines are inside the app. Apple Intelligence needs no download."}, "android": {"name": "Galaxy S25", "hw": "Snapdragon 8 Elite \u00b7 12 GB", "os": "Android 16", "kind": "phone", "budget": 5, "disk": "96 GB free", "eng": [["llama.cpp", "OpenCL (Adreno)", "b7180", "Ready"], ["llama.cpp", "CPU", "b7180", "Fallback"], ["WebGPU", "in the PWA", "Chrome 142", "Ready (small models)"], ["Gemini Nano", "ML Kit", "system", "Not on this phone"]], "ship": "The app engine runs 1 to 4B models. The PWA alone runs models under 0.5 GB."}};
const pick=(d,r)=>d.kind==='browser'?r.v.find(x=>x.e==='WebGPU'):d.apple?(r.v.find(x=>x.e==='MLX')||r.v.find(x=>x.e==='llama.cpp')):r.v.find(x=>x.e==='llama.cpp');
function fit(d,r){const v=pick(d,r);
 if(!v)return{k:'na',t:'No browser build',why:'Run it in Desktop or the phone app',v:r.v[0]};
 const need=v.s*1.2+0.4,n=need.toFixed(1);
 if(d.kind==='discrete'){if(need<=d.vram*0.9)return{k:'ok',t:'Fits',why:'Full GPU, '+n+' of '+d.vram+' GB VRAM',v};if(need<=d.vram+d.ram*0.5)return{k:'slow',t:'May be slow',why:'Part runs on the CPU',v};return{k:'no',t:"Won't fit",why:'Needs about '+n+' GB',v};}
 if(d.kind==='cpu'){if(need>d.ram*0.6)return{k:'no',t:"Won't fit",why:'Needs about '+n+' GB',v};return v.s<1.5?{k:'ok',t:'Fits',why:'CPU only',v}:{k:'slow',t:'May be slow',why:'CPU only, about 5 tok/s',v};}
 if(need<=d.budget*0.8)return{k:'ok',t:'Fits',why:n+' of '+d.budget+' GB for models',v};
 if(need<=d.budget)return{k:'slow',t:'Tight fit',why:'Close other apps first',v};
 return{k:'no',t:"Won't fit",why:'Needs '+n+' GB, '+d.budget+' GB free for models',v};}
const FC={ok:'var(--ok)',slow:'var(--warn)',no:'var(--bad)',na:'var(--ink3)'};
const ENVS=[['mac','Mac'],['win','Windows + NVIDIA'],['winarm','Windows on Arm'],['linux','Linux + AMD'],['cpu','Linux CPU only'],['web','Browser'],['iphone','iPhone'],['android','Android']];

const FW=window.FWCards;
const H=({html,cls,tag})=>React.createElement(tag||'div',{className:cls,dangerouslySetInnerHTML:{__html:html}});
const Pill=({f,small})=>(<span title={f.why} style={{display:'inline-flex',alignItems:'center',gap:5,fontSize:small?10.5:11.5,fontWeight:600,color:FC[f.k],background:'var(--fill)',borderRadius:999,padding:small?'1px 7px':'2px 9px',whiteSpace:'nowrap'}}><i style={{width:6,height:6,borderRadius:3,background:FC[f.k]}}/>{f.t}</span>);
const Btn=({children,pri,onClick,small})=>(<button onClick={onClick} style={{border:pri?'0':'1px solid var(--line2)',background:pri?'var(--btn)':'transparent',color:pri?'var(--btnink)':'var(--ink)',borderRadius:9,padding:small?'4px 10px':'6px 12px',fontSize:small?11.5:12.5,fontWeight:600,cursor:'pointer',whiteSpace:'nowrap',fontFamily:'inherit'}}>{children}</button>);
function EnvSwitch({env,setEnv,only}){const list=only?ENVS.filter(e=>only.includes(e[0])):ENVS;return(<div style={{display:'flex',flexWrap:'wrap',gap:6,alignItems:'center',padding:'8px 10px',border:'1px dashed var(--line2)',borderRadius:10,marginBottom:12,fontSize:12}}><span className="mono" style={{color:'var(--ink3)',marginRight:4}}>Mockup only, pretend this runs on:</span>{list.map(([k,l])=>(<button key={k} onClick={()=>setEnv(k)} style={{border:0,borderRadius:999,padding:'3px 10px',fontSize:12,cursor:'pointer',fontFamily:'inherit',background:env===k?'var(--btn)':'var(--fill)',color:env===k?'var(--btnink)':'var(--ink2)'}}>{l}</button>))}</div>);}
const TABS=['Catalog','This device','Playground','Train','Cloud'];
function Chrome({tab,children,d}){return(<div style={{background:'var(--bg)',color:'var(--ink)',fontFamily:'var(--font-allternit-sans)',padding:'22px 24px',fontSize:14}}>
<div style={{fontFamily:'var(--font-allternit-serif)',fontSize:28,fontWeight:500,letterSpacing:'-.01em'}}>Freeweights Studio</div>
<div style={{color:'var(--ink2)',fontSize:13.5,marginTop:2}}>Find open weights in Freeweights, then download and run them on this device, your other devices or the cloud.</div>
<div style={{display:'flex',gap:6,marginTop:14,flexWrap:'wrap'}}>{TABS.map(t=>(<span key={t} style={{border:'1px solid '+(t===tab?'var(--ink)':'var(--line)'),background:t===tab?'var(--fill2)':'transparent',borderRadius:10,padding:'5px 12px',fontSize:12.5,fontWeight:600}}>{t}</span>))}</div>
<div style={{display:'flex',alignItems:'center',gap:10,marginTop:14,padding:'10px 12px',border:'1px solid var(--line)',borderRadius:12,flexWrap:'wrap'}}><span style={{width:8,height:8,borderRadius:4,background:'var(--ok)'}}/><b style={{fontSize:13}}>{d.name}</b><span className="mono" style={{fontSize:11.5,color:'var(--ink2)'}}>{d.hw} · {d.os}</span><span style={{flex:1}}/><span className="mono" style={{fontSize:11.5,color:'var(--ink3)'}}>{d.eng.filter(e=>/Ready/.test(e[3])).map(e=>e[0]+' '+e[1]).join(' + ')}</span></div>
<div style={{marginTop:16}}>{children}</div></div>);}

export default function App(){const [env,setEnv]=React.useState('win');const [open,setOpen]=React.useState(true);const d=DEV[env];const r=env==='web'?ROWS[6]:ROWS[0];const f=fit(d,r);
 const groups=[['On '+d.name+' · works offline',[[r.n,f.v.f+' '+f.v.q+' · '+f.v.e,true],[ROWS[6].n,'downloaded']]],['My other devices',[["Joe's MacBook Pro · Qwen3.5 27B",'through my Allternit account'],["Joe's iPhone · Qwen3.5 2B",'when the app is open']]],['Cloud',[['Allternit Cloud models','metered']]]];
 return(<div style={{background:'var(--bg)'}}><EnvSwitch env={env} setEnv={setEnv} only={['mac','win','linux','web']}/>
<Chrome tab="Playground" d={d}>
<div style={{border:'1px solid var(--line)',borderRadius:14,overflow:'hidden',maxWidth:820}}>
<div style={{display:'flex',alignItems:'center',gap:10,padding:'10px 14px',borderBottom:'1px solid var(--line)',position:'relative'}}>
<button onClick={()=>setOpen(!open)} style={{display:'flex',alignItems:'center',gap:8,border:'1px solid var(--line2)',borderRadius:10,padding:'5px 10px',background:'transparent',color:'var(--ink)',fontFamily:'inherit',fontSize:13,cursor:'pointer'}}><b>{r.n}</b><span style={{fontSize:11,fontWeight:600,color:'var(--ok)',background:'var(--fill)',borderRadius:999,padding:'1px 8px'}}>On this device</span>▾</button>
<span className="mono" style={{fontSize:11.5,color:'var(--ink3)'}}>{f.v.f} {f.v.q} · {f.v.e} · temp 0.7 · ctx 8,192</span>
{open?<div style={{position:'absolute',top:46,left:14,width:380,background:'var(--bg)',border:'1px solid var(--line2)',borderRadius:12,boxShadow:'0 12px 30px rgba(0,0,0,.12)',padding:8,zIndex:2}}>{groups.map(([g,items])=>(<div key={g} style={{marginBottom:6}}><div className="mono" style={{fontSize:10.5,color:'var(--ink3)',textTransform:'uppercase',letterSpacing:'.08em',padding:'6px 8px 2px'}}>{g}</div>{items.map(([n,s,on])=>(<div key={n} style={{display:'flex',justifyContent:'space-between',padding:'6px 8px',borderRadius:8,background:on?'var(--fill)':'transparent'}}><div><div style={{fontSize:13}}>{n}</div><div className="mono" style={{fontSize:11,color:'var(--ink3)'}}>{s}</div></div>{on?'✓':''}</div>))}</div>))}</div>:null}
</div>
<div style={{padding:'18px 16px',display:'grid',gap:14,minHeight:300}}>
<div style={{justifySelf:'end',background:'var(--fill2)',borderRadius:14,padding:'8px 12px',maxWidth:'70%',fontSize:13.5}}>Summarize this contract clause in two lines.</div>
<div style={{maxWidth:'80%',fontSize:13.5,lineHeight:1.55}}>The supplier must fix defects within 30 days at no cost. If it misses that window, you can end the contract and get a pro-rated refund.
<div className="mono" style={{display:'flex',gap:12,flexWrap:'wrap',marginTop:8,fontSize:11,color:'var(--ink3)'}}><span>{env==='win'?88:env==='linux'?95:env==='web'?38:46} tok/s</span><span>first token 0.3 s</span><span>{env==='web'?'0.7':'3.4'} GB</span><span>212 / 8,192 ctx</span><span>{f.v.e}</span><span style={{color:'var(--ok)'}}>offline</span></div></div>
</div>
<div style={{borderTop:'1px solid var(--line)',padding:'10px 14px',color:'var(--ink3)',fontSize:13}}>Message {r.n}…</div></div>
</Chrome></div>);}

```


## F iPhone and Android

iPhone is native SwiftUI in the Models tab, with no new rail tab. The tiles use the same Freeweights identity face, ported to SwiftUI. Screens: Catalog, a model's run sheet, On this iPhone, and Chat with the model picker.

```react css=fw.css title="iPhone: Models › Catalog, run sheet, On this iPhone, Chat" h=740
import React from 'react';
// Freeweights shared rendering: the Allternit dot lettering (same 10-unit grid, 8.5 dots,
// 1.5 radius as the A://TERNIT wordmark) and the four preview card styles (A to D).
// Loaded by site/build.mjs at build time and by the browser for grid, deck and style switching.
(function (g) {
  var FONT = {
    A: ['..#..', '.#.#.', '#####', '#...#', '#...#'], B: ['####.', '#...#', '####.', '#...#', '####.'],
    C: ['.####', '#....', '#....', '#....', '.####'], D: ['####.', '#...#', '#...#', '#...#', '####.'],
    E: ['#####', '#....', '####.', '#....', '#####'], F: ['#####', '#....', '####.', '#....', '#....'],
    G: ['.####', '#....', '#.###', '#...#', '.###.'], H: ['#...#', '#...#', '#####', '#...#', '#...#'],
    I: ['###', '.#.', '.#.', '.#.', '###'], J: ['..###', '...#.', '...#.', '#..#.', '.##..'],
    K: ['#...#', '#..#.', '###..', '#..#.', '#...#'], L: ['#....', '#....', '#....', '#....', '#####'],
    M: ['#...#', '##.##', '#.#.#', '#...#', '#...#'], N: ['#...#', '##..#', '#.#.#', '#..##', '#...#'],
    O: ['.###.', '#...#', '#...#', '#...#', '.###.'], P: ['####.', '#...#', '####.', '#....', '#....'],
    Q: ['.###.', '#...#', '#.#.#', '#..#.', '.##.#'], R: ['####.', '#...#', '####.', '#.#..', '#..##'],
    S: ['.####', '#....', '.###.', '....#', '####.'], T: ['#####', '..#..', '..#..', '..#..', '..#..'],
    U: ['#...#', '#...#', '#...#', '#...#', '.###.'], V: ['#...#', '#...#', '#...#', '.#.#.', '..#..'],
    W: ['#...#', '#...#', '#.#.#', '##.##', '#...#'], X: ['#...#', '.#.#.', '..#..', '.#.#.', '#...#'],
    Y: ['#...#', '.#.#.', '..#..', '..#..', '..#..'], Z: ['#####', '...#.', '..#..', '.#...', '#####'],
    0: ['.###.', '#..##', '#.#.#', '##..#', '.###.'], 1: ['.#.', '##.', '.#.', '.#.', '###'],
    2: ['####.', '....#', '.###.', '#....', '#####'], 3: ['####.', '....#', '.###.', '....#', '####.'],
    4: ['#..#.', '#..#.', '#####', '...#.', '...#.'], 5: ['#####', '#....', '####.', '....#', '####.'],
    6: ['.###.', '#....', '####.', '#...#', '.###.'], 7: ['#####', '....#', '...#.', '..#..', '..#..'],
    8: ['.###.', '#...#', '.###.', '#...#', '.###.'], 9: ['.###.', '#...#', '.####', '....#', '.###.'],
    '.': ['.', '.', '.', '.', '#'], ':': ['.', '#', '.', '#', '.'], '/': ['..#', '..#', '.#.', '#..', '#..'],
    '-': ['...', '...', '###', '...', '...'], ' ': ['..', '..', '..', '..', '..']
  };
  var CELL = 10;
  function rect(c, r, fill) { return '<rect x="' + (c * CELL + 0.75) + '" y="' + (r * CELL + 0.75) + '" width="8.5" height="8.5" rx="1.5"' + (fill ? ' fill="' + fill + '"' : '') + '/>'; }
  // Lay out text on the dot grid. Returns { cols, rows, body }. Letters are separated by one empty column.
  function dots(text, col0, row0) {
    var c = col0 || 0, r0 = row0 || 0, body = '', chars = String(text).toUpperCase().split('');
    chars.forEach(function (ch, i) {
      var gl = FONT[ch] || FONT[' '];
      for (var r = 0; r < 5; r++) for (var x = 0; x < gl[r].length; x++) if (gl[r][x] === '#') body += rect(c + x, r0 + r);
      c += gl[0].length + (i < chars.length - 1 ? 1 : 0);
    });
    return { cols: c - (col0 || 0), body: body };
  }
  // The A://TERNIT wordmark cells, copied from the brand SVG (o = the coral dot).
  var WM = ['..#.......#...#.#####.#####.####..#...#..###..#####', '.#.#..#...#...#...#...#.....#...#.##..#...#.....#..', '##o##....#...#....#...####..####..#.#.#...#.....#..', '#...#.#.#...#.....#...#.....#.#...#..##...#.....#..', '#...#...#...#.....#...#####.#..##.#...#..###....#..'];
  function wordmark(col0, row0) {
    var b = '';
    WM.forEach(function (line, r) { for (var x = 0; x < line.length; x++) { if (line[x] === '#') b += rect(col0 + x, row0 + r); else if (line[x] === 'o') b += rect(col0 + x, row0 + r, '#D97757'); } });
    return { cols: WM[0].length, body: b };
  }
  function svg(cols, rows, body, h, label, cls) {
    var w = Math.round((h * cols) / rows * 100) / 100;
    return '<svg class="' + (cls || 'dotw') + '" viewBox="0 0 ' + cols * CELL + ' ' + rows * CELL + '" height="' + h + '" width="' + w + '" fill="currentColor" shape-rendering="geometricPrecision"' + (label ? ' role="img" aria-label="' + esc(label) + '"' : ' aria-hidden="true"') + '>' + body + '</svg>';
  }
  // Lockups. inline: A://TERNIT  FREEWEIGHTS on one line. stack: wordmark above the name.
  function lockup(kind, h, cls) {
    var a = wordmark(0, 0);
    if (kind === 'stack') { var n = dots('FREEWEIGHTS', 0, 7); return svg(Math.max(a.cols, n.cols), 12, a.body + n.body, h, 'Allternit Freeweights', cls || 'lockup stack'); }
    if (kind === 'name') { var o = dots('FREEWEIGHTS'); return svg(o.cols, 5, o.body, h, 'Freeweights', cls || 'lockup name'); }
    var m = dots('FREEWEIGHTS', a.cols + 4, 0); return svg(a.cols + 4 + m.cols, 5, a.body + m.body, h, 'Allternit Freeweights', cls || 'lockup inline');
  }
  function word(text, h, cls, label) { var d = dots(text); return svg(d.cols, 5, d.body, h, label, cls); }

  var esc = function (s) { return String(s == null ? '' : s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'); };
  function bytes(n) {
    if (n == null || isNaN(n)) return 'size unknown';
    var u = ['B', 'KB', 'MB', 'GB', 'TB'], i = 0, v = Number(n);
    while (v >= 1000 && i < u.length - 1) { v /= 1000; i++; }
    return (v >= 100 || i === 0 ? Math.round(v).toLocaleString('en-US') : v >= 10 ? v.toFixed(1) : v.toFixed(2)) + ' ' + u[i];
  }
  function sizeParts(n) { var s = bytes(n).split(' '); return { v: s[0], u: s[1] || '' }; }
  function params(n) { return !n ? '' : n >= 1e12 ? +(n / 1e12).toFixed(2) + 'T' : n >= 1e9 ? +(n / 1e9).toFixed(1) + 'B' : +(n / 1e6).toFixed(0) + 'M'; }
  var LC = { copyleft: 'copyleft', permissive: 'permissive', custom: 'custom terms', 'non-commercial': 'non-commercial', gated: 'gated', territory: 'territory-limited', unreviewed: 'license not reviewed' };
  function src(r) { return r.nc ? (r.how || 'From the publisher') : r.src || 'From source'; }
  function srcBadge(r) { var cls = r.nc || /source|publisher/i.test(src(r)) ? ' src' : /seed/i.test(src(r)) ? ' seed' : ''; return '<span class="b' + cls + '"><i class="d"></i>' + esc(src(r)) + '</span>'; }
  function lcTag(r) { return '<span class="lc lc-' + esc(r.lc || 'unknown') + '">' + esc(LC[r.lc] || r.lc || 'see card') + '</span>'; }
  function task(r) { var t = (r.task || [])[0]; return t ? t.replace(/-/g, ' ') : r.sec === 'alt.freeweights.datasets' ? 'dataset' : ''; }
  function date(t) { var d = new Date(t); if (isNaN(d)) return ''; return d.toUTCString().slice(0, 16); }
  function mono(r, h, cls) { return word(r.mono || 'FW', h, cls || 'mono-svg', null); }

  // A. Spec sheet
  function cardA(r) {
    var spec = [['Params', params(r.params) || 'not stated'], ['Size', bytes(r.size)], ['Format', (r.fmt || []).join(', ') || 'see files'], ['License', esc(r.lic)]];
    return '<a class="fwc fwc-a" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '">' +
      '<div class="a-top"><span class="a-mono">' + mono(r, 22) + '</span><div class="a-t"><div class="a-sec mono">' + esc(r.sec) + '</div><div class="a-name">' + esc(r.n) + '</div><div class="a-id mono">' + esc(r.id) + '</div></div></div>' +
      '<dl class="a-spec">' + spec.map(function (s) { return '<div><dt>' + s[0] + '</dt><dd' + (s[0] === 'Format' ? ' class="mono"' : '') + '>' + (s[0] === 'License' ? s[1] : esc(s[1])) + '</dd></div>'; }).join('') + '</dl>' +
      '<div class="a-foot">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + (task(r) ? '<span class="a-task">' + esc(task(r)) + '</span>' : '') + '</div></a>';
  }
  // B. Newsgroup post
  function cardB(r) {
    return '<a class="fwc fwc-b" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '">' +
      '<div class="b-head mono"><div><b>From:</b> ' + esc(r.org) + ' &lt;' + esc(r.host || 'huggingface.co') + '/' + esc(r.org) + '&gt;</div><div><b>Newsgroups:</b> ' + esc(r.sec) + '</div><div><b>Subject:</b> <span class="b-subj">' + esc(r.n) + '</span> (' + esc(bytes(r.size)) + ')</div><div><b>Date:</b> ' + esc(date(r.added)) + '</div></div>' +
      '<p class="b-body">' + esc(r.sum) + '</p>' +
      '<div class="b-sig mono">-- <br>' + esc(r.lic) + ' · ' + esc(LC[r.lc] || r.lc || '') + ' · ' + esc(src(r).toLowerCase()) + (r.rt ? ' · read terms first' : '') + '</div></a>';
  }
  // C. Element tile
  function cardC(r) {
    var s = sizeParts(r.size);
    return '<a class="fwc fwc-c" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '">' +
      '<div class="c-row mono"><span>' + esc(params(r.params) || '&mdash;').replace('&amp;mdash;', '&mdash;') + '</span><span>' + esc(LC[r.lc] || r.lc || '') + '</span></div>' +
      '<div class="c-mono">' + mono(r, 40) + '</div>' +
      '<div class="c-size"><b class="mono">' + esc(s.v) + '</b><span class="mono">' + esc(s.u) + '</span></div>' +
      '<div class="c-name">' + esc(r.n) + '</div>' +
      '<div class="c-facts mono"><span>' + esc((r.fmt || []).join(' ') || 'files') + '</span><span>' + esc(task(r) || r.fam) + '</span></div></a>';
  }
  // D. Terminal
  function cardD(r) {
    var lines = r.nc
      ? ['<span class="p">$</span> fw info ' + esc(r.id), '<span class="c"># link only: ' + esc(r.lic) + (r.lc ? ' (' + esc(LC[r.lc] || r.lc) + ')' : '') + '</span>', '<span class="c"># from the publisher, never hosted here</span>', '<span class="w">[publisher]</span> ' + esc(r.host || 'huggingface.co')]
      : ['<span class="p">$</span> fw get ' + esc(r.id), '<span class="c"># manifest sha256 ' + esc(r.sha || 'pending') + '&hellip;</span>', '<span class="c"># ' + esc(bytes(r.size)) + ' · ' + esc(r.files || 0) + ' files · ' + esc((r.fmt || []).join(', ')) + '</span>', '<span class="c"># license ' + esc(r.lic) + ' (' + esc(r.lc || 'see card') + ')' + (r.rt ? ', read terms first' : '') + '</span>', '<span class="ok">[' + esc(src(r).toLowerCase().replace(/^from /, '')) + ']</span> ' + esc(r.host || 'huggingface.co') + (r.commit ? ' @ ' + esc(r.commit.slice(0, 7)) : '')];
    return '<a class="fwc fwc-d mono" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '"><div class="d-bar"><i></i><i></i><i></i><span>' + esc(r.sec) + '</span></div><pre>' + lines.join('\n') + '</pre></a>';
  }
  var CARDS = { a: cardA, b: cardB, c: cardC, d: cardD };
  function card(style, r) { return (CARDS[style] || cardA)(r); }
  // Deck: one model in focus.
  function deck(r) {
    var facts = [['Size', bytes(r.size)], ['Params', params(r.params) || 'not stated'], ['Formats', (r.fmt || []).join(', ') || 'see files'], ['Task', task(r) || 'not tagged'], ['License', r.lic + ' (' + (LC[r.lc] || r.lc || 'see card') + ')'], ['Files', r.nc ? 'link only' : (r.files || 0) + ' with sha256'], ['Publisher', r.org], ['Added', date(r.added)]];
    return '<article class="deck-card"><div class="dk-top"><span class="dk-mono">' + mono(r, 52) + '</span><div><div class="mono dk-sec">' + esc(r.sec) + '</div><h2 class="dk-name">' + esc(r.n) + '</h2><div class="mono dk-id">' + esc(r.id) + '</div></div></div>' +
      '<p class="dk-sum">' + esc(r.sum) + '</p><dl class="dk-facts">' + facts.map(function (f) { return '<div><dt>' + f[0] + '</dt><dd>' + esc(f[1]) + '</dd></div>'; }).join('') + '</dl>' +
      '<div class="dk-foot"><div class="badges">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + '</div><a class="btn pri" href="' + esc(r.url) + '">' + (r.nc ? 'License and source' : 'Open model page') + '</a></div></article>';
  }

  // ---------- One card, four faces ----------
  // IDENTITY (element tile), FACTS (spec sheet), RECORD (newsgroup header), ACTION (terminal).
  // Every view composes these from the same row, so the faces read as one card.
  function logo(r, px, cls) { return r.logo ? '<img class="org' + (cls ? ' ' + cls : '') + '" src="' + esc(r.logo) + '" alt="" width="' + px + '" height="' + px + '" loading="lazy" decoding="async">' : ''; }
  function shortDate(t) { var d = new Date(t); return isNaN(d) ? '' : d.toISOString().slice(0, 10); }
  function repo(r) { return String(r.id).split('/').slice(1).join('/'); }
  function lcName(r) { return LC[r.lc] || r.lc || 'see card'; }
  // Kinds: model and dataset share the size figure; a tool shows its latest release there.
  function faceIdentity(r) {
    if (r.kind === 'tool') {
      var tag = r.tag || 'no release';
      return '<span class="fwf fwf-id">' +
        '<span class="id-row mono"><span>' + esc(r.tkind || 'tool') + '</span>' + logo(r, 24, 'id-org') + '</span>' +
        '<span class="id-mono">' + mono(r, 40) + '</span>' +
        '<span class="id-size tag' + (tag.length > 8 ? ' long' : tag.length > 5 ? ' mid' : '') + '"><b class="mono">' + esc(tag) + '</b><span class="mono">release</span></span>' +
        '<span class="id-name">' + esc(r.n) + '</span>' +
        '<span class="id-facts mono"><span>' + esc(r.lang || 'tool') + '</span><span>' + esc(r.lic || '') + '</span></span></span>';
    }
    var s = sizeParts(r.size);
    return '<span class="fwf fwf-id">' +
      '<span class="id-row mono"><span>' + esc(params(r.params) ? params(r.params) + ' params' : (r.fam || '')) + '</span>' + logo(r, 24, 'id-org') + '</span>' +
      '<span class="id-mono">' + mono(r, 40) + '</span>' +
      '<span class="id-size"><b class="mono">' + esc(s.v) + '</b><span class="mono">' + esc(s.u) + '</span></span>' +
      '<span class="id-name">' + esc(r.n) + '</span>' +
      '<span class="id-facts mono"><span>' + esc((r.fmt || []).join(' ') || 'files') + '</span><span>' + esc(lcName(r)) + '</span></span></span>';
  }
  function faceFacts(r) {
    if (r.kind === 'tool') {
      var tr = [['License', r.lic || 'see project'], ['Language', r.lang || 'not stated'], ['Kind', r.tkind || 'tool'], ['Release', r.relDate || 'none'], ['Platforms', (r.platforms || []).join(', ') || 'see project']];
      return '<span class="fwf fwf-facts"><span class="ff-h mono">Facts</span><span class="ff-dl">' +
        tr.map(function (x) { return '<span class="ff-r' + (x[0] === 'Platforms' ? ' wide' : '') + '"><span class="ff-k">' + x[0] + '</span><span class="ff-v">' + esc(x[1]) + '</span></span>'; }).join('') +
        '</span><span class="ff-b"><span class="b src"><i class="d"></i>Links to the project</span>' + lcTag(r) + '</span></span>';
    }
    if (r.kind === 'dataset') {
      var dr = [['Rows', r.rowsN ? Number(r.rowsN).toLocaleString('en-US') : 'not stated'], ['Size', bytes(r.size)], ['Format', (r.fmt || []).join(', ') || 'see files'], ['License', r.lic]];
      return '<span class="fwf fwf-facts"><span class="ff-h mono">Facts</span><span class="ff-dl">' +
        dr.map(function (x) { return '<span class="ff-r"><span class="ff-k">' + x[0] + '</span><span class="ff-v' + (x[0] === 'Format' ? ' mono' : '') + '">' + esc(x[1]) + '</span></span>'; }).join('') +
        '</span><span class="ff-b">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + '</span></span>';
    }
    var rows = [['Params', params(r.params) || 'not stated'], ['Size', bytes(r.size)], ['Formats', (r.fmt || []).join(', ') || 'see files'], ['Task', task(r) || 'not tagged'], ['License', r.lic]];
    return '<span class="fwf fwf-facts"><span class="ff-h mono">Facts</span><span class="ff-dl">' +
      rows.map(function (x) { return '<span class="ff-r"><span class="ff-k">' + x[0] + '</span><span class="ff-v' + (x[0] === 'Formats' ? ' mono' : '') + '">' + esc(x[1]) + '</span></span>'; }).join('') +
      '</span><span class="ff-b">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + '</span></span>';
  }
  // mode: 'full' (all header lines + summary), 'strip' (From + Subject, under a tile), 'head' (lines, no summary)
  function faceRecord(r, mode, opt) {
    opt = opt || {};
    var subj = opt.h1 ? '<h1 class="rc-subj rc-h1">' + esc(r.n) + '</h1>' : opt.link === false ? '<span class="rc-subj">' + esc(r.n) + '</span>' : '<a class="rc-subj" href="' + esc(r.url) + '">' + esc(r.n) + '</a>';
    var from = '<span class="rc-l"><b>From:</b> ' + logo(r, 18, 'rc-org') + esc(r.org) + ' <span class="rc-dim">&lt;' + esc(r.host || 'huggingface.co') + '/' + esc(r.org) + '&gt;</span></span>';
    if (mode === 'strip') return '<span class="fwf fwf-rec strip mono">' + from + '<span class="rc-l"><b>Subject:</b> ' + subj + '</span></span>';
    var lines = [from, '<span class="rc-l"><b>Newsgroups:</b> ' + esc(r.sec) + '</span>', '<span class="rc-l"><b>Subject:</b> ' + subj + '</span>'];
    if (r.kind === 'tool') lines.push('<span class="rc-l"><b>Release:</b> ' + esc(r.tag ? r.tag + (r.relDate ? ' (' + r.relDate + ')' : '') : 'none') + '</span>');
    else lines.push('<span class="rc-l"><b>Date:</b> ' + esc(shortDate(r.added) || 'unknown') + '</span>');
    if (r.commit) lines.push('<span class="rc-l"><b>Commit:</b> ' + esc(r.commit.slice(0, 7)) + '</span>');
    if (r.kind === 'tool') lines.push('<span class="rc-l"><b>License:</b> ' + esc(r.lic || 'see project') + '</span>');
    if (r.sig) lines.push('<span class="rc-l"><b>Signed:</b> ' + esc(r.sig) + ' <span class="rc-ok" aria-label="signature present">&#10003;</span></span>');
    return '<div class="fwf fwf-rec mono">' + lines.join('') + (mode === 'head' ? '' : '<p class="rc-sum">' + esc(r.sum) + '</p>') + '</div>';
  }
  function faceAction(r) {
    if (r.kind === 'tool') {
      var T = (r.install && r.install.length ? r.install.map(function (l) { return /^#/.test(l) ? '<span class="c">' + esc(l) + '</span>' : '<span class="p">$</span> ' + esc(l); }) : ['<span class="c"># install: see the project\'s instructions</span>', '<span class="u">' + esc(r.installDocs || r.licUrl || '') + '</span>'])
        .concat(r.use ? ['<span class="c"># use with Freeweights</span>', (/^#/.test(r.use) ? '<span class="c">' + esc(r.use) + '</span>' : '<span class="p">$</span> ' + esc(r.use))] : []);
      return '<div class="fwf fwf-act mono"><div class="act-bar" aria-hidden="true"><i></i><i></i><i></i><span>install</span></div><pre>' + T.join('\n') + '</pre></div>';
    }
    if (r.kind === 'dataset' && !r.nc) {
      var D = (r.rt ? ['<span class="w"># custom terms: read the license before you download</span>'] : []).concat([
        '<span class="p">&gt;&gt;&gt;</span> from datasets import load_dataset',
        '<span class="p">&gt;&gt;&gt;</span> ds = load_dataset("' + esc(r.id) + '")',
        '<span class="c"># with HF_ENDPOINT=https://freeweights.allternit.com &middot; manifest sha256 ' + esc(r.sha || 'pending') + '&hellip;</span>']);
      return '<div class="fwf fwf-act mono"><div class="act-bar" aria-hidden="true"><i></i><i></i><i></i><span>python</span></div><pre>' + D.join('\n') + '</pre></div>';
    }
    var L = r.nc
      ? ['<span class="p">$</span> fw info ' + esc(r.id), '<span class="w">link only &mdash; read the license:</span>', '<span class="u">' + esc(r.licUrl || ('https://' + (r.host || 'huggingface.co') + '/' + r.id)) + '</span>']
      : (r.rt ? ['<span class="w"># custom terms: read the license before you download</span>'] : []).concat([
        '<span class="p">$</span> fw get ' + esc(r.id),
        '<span class="c"># manifest sha256 ' + esc(r.sha || 'pending') + '&hellip; &middot; ' + esc(bytes(r.size)) + ' &middot; ' + esc(r.files || 0) + ' files</span>',
        '<span class="p">$</span> fw verify ./' + esc(repo(r)) + ' ' + esc(r.id)]);
    return '<div class="fwf fwf-act mono"><div class="act-bar" aria-hidden="true"><i></i><i></i><i></i><span>' + (r.nc ? 'link only' : 'fw') + '</span></div><pre>' + L.join('\n') + '</pre></div>';
  }
  // The combined card: RECORD header on top, IDENTITY + FACTS side by side, ACTION footer.
  function combined(r, opt) {
    opt = opt || {};
    return '<article class="fwk' + (opt.cls ? ' ' + opt.cls : '') + '" data-id="' + esc(r.id) + '">' +
      '<div class="fwk-rec">' + faceRecord(r, 'full', { h1: opt.h1 }) + '</div>' +
      '<div class="fwk-mid"><div class="fwk-id">' + faceIdentity(r) + '</div><div class="fwk-facts">' + faceFacts(r) + '</div></div>' +
      '<div class="fwk-act">' + faceAction(r) + '</div>' +
      (opt.open ? '<div class="fwk-foot"><a class="btn pri sm" href="' + esc(r.url) + '">' + (r.nc ? 'License and source' : 'Open model page') + '</a></div>' : '') +
      '</article>';
  }
  // Grid / home / related tile: IDENTITY front, FACTS back (flip), RECORD strip below.
  function tile(r) {
    return '<div class="fwt" data-id="' + esc(r.id) + '">' +
      '<button type="button" class="flip" aria-pressed="false" aria-label="' + esc(r.n) + ': show facts">' +
      '<span class="flip-in"><span class="flip-f">' + faceIdentity(r) + '</span><span class="flip-b">' + faceFacts(r) + '</span></span>' +
      '<span class="flip-hint mono" aria-hidden="true">flip</span></button>' +
      faceRecord(r, 'strip') + '</div>';
  }
  var api = { FONT: FONT, dots: dots, wordmark: wordmark, svg: svg, lockup: lockup, word: word, card: card, deck: deck, bytes: bytes, params: params, esc: esc, STYLES: ['a', 'b', 'c', 'd'],
    logo: logo, identity: faceIdentity, facts: faceFacts, record: faceRecord, action: faceAction, combined: combined, tile: tile };
  g.FWCards = api;
})(typeof window !== 'undefined' ? window : globalThis);

const ROWS=[{"kind": "model", "id": "lmstudio-community/Qwen3.5-4B-GGUF", "n": "Qwen3.5-4B-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 11329544112, "params": 4200000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-14T21:41:57Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 2.7, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 2.5, "e": "MLX"}]}, {"kind": "model", "id": "lmstudio-community/Qwen3.5-2B-GGUF", "n": "Qwen3.5-2B-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 5510586672, "params": 2300000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 1.3, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 1.2, "e": "MLX"}]}, {"kind": "model", "id": "google/gemma-4-E2B-it-qat-q4_0-gguf", "n": "gemma-4-E2B-it-qat-q4_0-gguf", "org": "google", "sec": "alt.freeweights.gemma", "fam": "gemma", "mono": "GM", "url": "#", "size": 4336380945, "params": 5100000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-14T21:41:57Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "https://freeweights.allternit.com/assets/orgs/google.png", "sig": "", "v": [{"f": "GGUF", "q": "Q4_0 QAT", "s": 3.2, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 3.0, "e": "MLX"}]}, {"kind": "model", "id": "ggml-org/SmolLM3-3B-GGUF", "n": "SmolLM3-3B-GGUF", "org": "ggml-org", "sec": "alt.freeweights.smollm", "fam": "smollm", "mono": "SM", "url": "#", "size": 11349231863, "params": 3100000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-14T21:41:57Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 1.9, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 1.8, "e": "MLX"}]}, {"kind": "model", "id": "lmstudio-community/Phi-4-mini-instruct-GGUF", "n": "Phi-4-mini-instruct-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.phi", "fam": "phi", "mono": "PH", "url": "#", "size": 11981766167, "params": 3800000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "MIT", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "https://freeweights.allternit.com/assets/orgs/microsoft.png", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 2.5, "e": "llama.cpp"}]}, {"kind": "model", "id": "Qwen/Qwen3-1.7B-GGUF", "n": "Qwen3-1.7B-GGUF", "org": "Qwen", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 1834445939, "params": 1700000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 1.1, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 1.0, "e": "MLX"}, {"f": "ONNX", "q": "q4f16", "s": 1.2, "e": "WebGPU"}]}, {"kind": "model", "id": "Qwen/Qwen3-0.6B-GGUF", "n": "Qwen3-0.6B-GGUF", "org": "Qwen", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 639466546, "params": 600000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 0.4, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 0.35, "e": "MLX"}, {"f": "ONNX", "q": "q4f16", "s": 0.45, "e": "WebGPU"}]}, {"kind": "model", "id": "lmstudio-community/gemma-4-E4B-it-QAT-GGUF", "n": "gemma-4-E4B-it-QAT-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.gemma", "fam": "gemma", "mono": "GM", "url": "#", "size": 6146496828, "params": 8000000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "https://freeweights.allternit.com/assets/orgs/google.png", "sig": "", "v": [{"f": "GGUF", "q": "Q4_0 QAT", "s": 5.0, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 4.8, "e": "MLX"}]}, {"kind": "model", "id": "lmstudio-community/Qwen3.5-27B-GGUF", "n": "Qwen3.5-27B-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 68149711960, "params": 27000000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 16.5, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 15.6, "e": "MLX"}]}];
const DEV={"mac": {"name": "Joe's MacBook Pro", "hw": "Apple M4 Pro \u00b7 48 GB unified memory", "os": "macOS 26", "kind": "unified", "apple": true, "budget": 31, "disk": "412 GB free", "eng": [["llama.cpp", "Metal", "b7180", "Ready"], ["MLX", "Metal", "0.30.1", "Ready"]], "ship": "Engines ship inside Allternit Desktop and update with it."}, "win": {"name": "Studio PC", "hw": "NVIDIA RTX 4070 12 GB \u00b7 32 GB RAM \u00b7 Ryzen 7", "os": "Windows 11", "kind": "discrete", "vram": 12, "ram": 32, "disk": "690 GB free", "eng": [["llama.cpp", "CUDA 13", "b7180", "Ready"], ["llama.cpp", "Vulkan", "b7180", "Installed, not in use"], ["llama.cpp", "CPU (AVX2)", "b7180", "Fallback"]], "ship": "The CUDA build is picked because an NVIDIA GPU is present. Vulkan stays as the fallback."}, "winarm": {"name": "Surface Laptop", "hw": "Snapdragon X Elite \u00b7 32 GB \u00b7 Adreno X1", "os": "Windows 11 on Arm", "kind": "unified", "budget": 19, "disk": "301 GB free", "eng": [["llama.cpp", "CPU (Arm, KleidiAI)", "b7180", "Ready"], ["llama.cpp", "Vulkan (Adreno)", "b7180", "Ready"], ["NPU (Hexagon)", "QNN", "preview", "Off: turn on in Advanced"]], "ship": "The NPU path is off by default. CPU and GPU are the baseline."}, "linux": {"name": "Workstation", "hw": "AMD Radeon RX 7900 XTX 24 GB \u00b7 64 GB RAM", "os": "Ubuntu 24.04", "kind": "discrete", "vram": 24, "ram": 64, "disk": "1.6 TB free", "eng": [["llama.cpp", "ROCm 7 (HIP)", "b7180", "Ready"], ["llama.cpp", "Vulkan", "b7180", "Installed, not in use"], ["llama.cpp", "CPU", "b7180", "Fallback"]], "ship": "ROCm is picked for RDNA3. Vulkan covers GPUs that ROCm does not support."}, "cpu": {"name": "Home server", "hw": "8 vCPU \u00b7 16 GB RAM \u00b7 no GPU", "os": "Debian 13", "kind": "cpu", "ram": 16, "disk": "88 GB free", "eng": [["llama.cpp", "CPU (AVX2)", "b7180", "Ready"]], "ship": "No GPU found. Models run on the CPU, so small models only."}, "web": {"name": "This browser tab", "hw": "Chrome 142 \u00b7 WebGPU \u00b7 2 GB model limit", "os": "ai.allternit.com", "kind": "browser", "budget": 2, "disk": "Browser storage 41 GB", "eng": [["WebGPU", "transformers.js (ONNX)", "4.3", "Ready"]], "ship": "Nothing to install. Weights are cached in the browser. Bigger models run in Desktop or the phone app."}, "iphone": {"name": "Joe's iPhone", "hw": "iPhone 17 Pro \u00b7 12 GB \u00b7 6 GB per app", "os": "iOS 26", "kind": "phone", "apple": true, "budget": 4.5, "disk": "41 GB free", "eng": [["MLX", "Metal", "0.30.1", "Ready"], ["llama.cpp", "Metal", "b7180", "Ready"], ["Apple Intelligence", "built in", "iOS 26", "Ready"]], "ship": "Engines are inside the app. Apple Intelligence needs no download."}, "android": {"name": "Galaxy S25", "hw": "Snapdragon 8 Elite \u00b7 12 GB", "os": "Android 16", "kind": "phone", "budget": 5, "disk": "96 GB free", "eng": [["llama.cpp", "OpenCL (Adreno)", "b7180", "Ready"], ["llama.cpp", "CPU", "b7180", "Fallback"], ["WebGPU", "in the PWA", "Chrome 142", "Ready (small models)"], ["Gemini Nano", "ML Kit", "system", "Not on this phone"]], "ship": "The app engine runs 1 to 4B models. The PWA alone runs models under 0.5 GB."}};
const pick=(d,r)=>d.kind==='browser'?r.v.find(x=>x.e==='WebGPU'):d.apple?(r.v.find(x=>x.e==='MLX')||r.v.find(x=>x.e==='llama.cpp')):r.v.find(x=>x.e==='llama.cpp');
function fit(d,r){const v=pick(d,r);
 if(!v)return{k:'na',t:'No browser build',why:'Run it in Desktop or the phone app',v:r.v[0]};
 const need=v.s*1.2+0.4,n=need.toFixed(1);
 if(d.kind==='discrete'){if(need<=d.vram*0.9)return{k:'ok',t:'Fits',why:'Full GPU, '+n+' of '+d.vram+' GB VRAM',v};if(need<=d.vram+d.ram*0.5)return{k:'slow',t:'May be slow',why:'Part runs on the CPU',v};return{k:'no',t:"Won't fit",why:'Needs about '+n+' GB',v};}
 if(d.kind==='cpu'){if(need>d.ram*0.6)return{k:'no',t:"Won't fit",why:'Needs about '+n+' GB',v};return v.s<1.5?{k:'ok',t:'Fits',why:'CPU only',v}:{k:'slow',t:'May be slow',why:'CPU only, about 5 tok/s',v};}
 if(need<=d.budget*0.8)return{k:'ok',t:'Fits',why:n+' of '+d.budget+' GB for models',v};
 if(need<=d.budget)return{k:'slow',t:'Tight fit',why:'Close other apps first',v};
 return{k:'no',t:"Won't fit",why:'Needs '+n+' GB, '+d.budget+' GB free for models',v};}
const FC={ok:'var(--ok)',slow:'var(--warn)',no:'var(--bad)',na:'var(--ink3)'};
const ENVS=[['mac','Mac'],['win','Windows + NVIDIA'],['winarm','Windows on Arm'],['linux','Linux + AMD'],['cpu','Linux CPU only'],['web','Browser'],['iphone','iPhone'],['android','Android']];

const FW=window.FWCards;
const H=({html,cls,tag})=>React.createElement(tag||'div',{className:cls,dangerouslySetInnerHTML:{__html:html}});
const Pill=({f,small})=>(<span title={f.why} style={{display:'inline-flex',alignItems:'center',gap:5,fontSize:small?10.5:11.5,fontWeight:600,color:FC[f.k],background:'var(--fill)',borderRadius:999,padding:small?'1px 7px':'2px 9px',whiteSpace:'nowrap'}}><i style={{width:6,height:6,borderRadius:3,background:FC[f.k]}}/>{f.t}</span>);
const Btn=({children,pri,onClick,small})=>(<button onClick={onClick} style={{border:pri?'0':'1px solid var(--line2)',background:pri?'var(--btn)':'transparent',color:pri?'var(--btnink)':'var(--ink)',borderRadius:9,padding:small?'4px 10px':'6px 12px',fontSize:small?11.5:12.5,fontWeight:600,cursor:'pointer',whiteSpace:'nowrap',fontFamily:'inherit'}}>{children}</button>);
function EnvSwitch({env,setEnv,only}){const list=only?ENVS.filter(e=>only.includes(e[0])):ENVS;return(<div style={{display:'flex',flexWrap:'wrap',gap:6,alignItems:'center',padding:'8px 10px',border:'1px dashed var(--line2)',borderRadius:10,marginBottom:12,fontSize:12}}><span className="mono" style={{color:'var(--ink3)',marginRight:4}}>Mockup only, pretend this runs on:</span>{list.map(([k,l])=>(<button key={k} onClick={()=>setEnv(k)} style={{border:0,borderRadius:999,padding:'3px 10px',fontSize:12,cursor:'pointer',fontFamily:'inherit',background:env===k?'var(--btn)':'var(--fill)',color:env===k?'var(--btnink)':'var(--ink2)'}}>{l}</button>))}</div>);}
const TABS=['Catalog','This device','Playground','Train','Cloud'];
function Chrome({tab,children,d}){return(<div style={{background:'var(--bg)',color:'var(--ink)',fontFamily:'var(--font-allternit-sans)',padding:'22px 24px',fontSize:14}}>
<div style={{fontFamily:'var(--font-allternit-serif)',fontSize:28,fontWeight:500,letterSpacing:'-.01em'}}>Freeweights Studio</div>
<div style={{color:'var(--ink2)',fontSize:13.5,marginTop:2}}>Find open weights in Freeweights, then download and run them on this device, your other devices or the cloud.</div>
<div style={{display:'flex',gap:6,marginTop:14,flexWrap:'wrap'}}>{TABS.map(t=>(<span key={t} style={{border:'1px solid '+(t===tab?'var(--ink)':'var(--line)'),background:t===tab?'var(--fill2)':'transparent',borderRadius:10,padding:'5px 12px',fontSize:12.5,fontWeight:600}}>{t}</span>))}</div>
<div style={{display:'flex',alignItems:'center',gap:10,marginTop:14,padding:'10px 12px',border:'1px solid var(--line)',borderRadius:12,flexWrap:'wrap'}}><span style={{width:8,height:8,borderRadius:4,background:'var(--ok)'}}/><b style={{fontSize:13}}>{d.name}</b><span className="mono" style={{fontSize:11.5,color:'var(--ink2)'}}>{d.hw} · {d.os}</span><span style={{flex:1}}/><span className="mono" style={{fontSize:11.5,color:'var(--ink3)'}}>{d.eng.filter(e=>/Ready/.test(e[3])).map(e=>e[0]+' '+e[1]).join(' + ')}</span></div>
<div style={{marginTop:16}}>{children}</div></div>);}

const I={bg:'#F2F2F7',panel:'#FFFFFF',t1:'#1F1E1D',t2:'#6B6A68',sep:'#E5E5EA'};
const d=DEV.iphone;
function Phone({children,title,back}){return(<div style={{width:340,height:700,borderRadius:46,border:'9px solid #111',background:I.bg,overflow:'hidden',fontFamily:'-apple-system,system-ui',color:I.t1,flex:'none',display:'flex',flexDirection:'column'}}>
<div style={{height:40,fontSize:13,fontWeight:600,display:'flex',justifyContent:'space-between',padding:'14px 26px 0'}}><span>9:41</span><span>●●● ▮</span></div>
{title?<div style={{display:'flex',alignItems:'center',padding:'4px 16px 8px'}}><span style={{width:60,color:'#007AFF',fontSize:15}}>{back||''}</span><span style={{flex:1,textAlign:'center',fontSize:17,fontWeight:600}}>{title}</span><span style={{width:60}}/></div>:null}
<div style={{flex:1,overflow:'hidden'}}>{children}</div></div>);}
const Grp=({h,children,f})=>(<div style={{margin:'0 14px 14px'}}>{h?<div style={{fontSize:12,color:I.t2,textTransform:'uppercase',padding:'0 14px 5px'}}>{h}</div>:null}<div style={{background:I.panel,borderRadius:12,overflow:'hidden'}}>{children}</div>{f?<div style={{fontSize:12,color:I.t2,padding:'5px 14px 0'}}>{f}</div>:null}</div>);
const Row=({l,s,r,first})=>(<div style={{display:'flex',alignItems:'center',gap:10,padding:'10px 14px',borderTop:first?'none':'0.5px solid '+I.sep}}><div style={{flex:1,minWidth:0}}><div style={{fontSize:15}}>{l}</div>{s?<div style={{fontSize:12,color:I.t2}}>{s}</div>:null}</div>{r}</div>);
const Dot=({c})=><i style={{display:'inline-block',width:7,height:7,borderRadius:4,background:c,marginRight:5}}/>;
const Seg=({items,on})=>(<div style={{margin:'0 14px 12px',background:'rgba(118,118,128,.12)',borderRadius:9,padding:2,display:'flex'}}>{items.map(x=><span key={x} style={{flex:1,textAlign:'center',fontSize:13,fontWeight:600,padding:'5px 0',borderRadius:7,background:x===on?'#fff':'transparent',boxShadow:x===on?'0 1px 3px rgba(0,0,0,.12)':'none'}}>{x}</span>)}</div>);
const Btn2=({children,pri})=><div style={{background:pri?I.t1:'transparent',color:pri?'#fff':I.t1,border:pri?'0':'1px solid #D1D1D6',borderRadius:12,textAlign:'center',padding:'11px 0',fontWeight:600,fontSize:15,marginTop:8}}>{children}</div>;
const ok='#34C759',warn='#FF9F0A',bad='#FF3B30',CC={ok,slow:warn,no:bad,na:I.t2};
export default function App(){const r=ROWS[1];const f=fit(d,r);const run=ROWS[1];
return(<div className="fwscope" style={{display:'flex',gap:22,flexWrap:'wrap',justifyContent:'center',padding:12,background:'transparent'}}>
<Phone title="Models"><Seg items={['Catalog','On this iPhone']} on="Catalog"/>
<div style={{display:'flex',gap:8,margin:'0 14px 10px'}}><div style={{flex:1,background:'rgba(118,118,128,.12)',borderRadius:10,padding:'7px 10px',fontSize:14,color:I.t2}}>Search Freeweights</div><div style={{display:'flex',background:'rgba(118,118,128,.12)',borderRadius:10,padding:2}}>{['☰','▦','▯'].map((x,i)=><span key={x} style={{padding:'4px 9px',borderRadius:8,background:i===1?'#fff':'transparent',fontSize:13}}>{x}</span>)}</div></div>
<div style={{display:'flex',gap:6,margin:'0 14px 10px',fontSize:12}}><span style={{background:I.t1,color:'#fff',borderRadius:999,padding:'3px 10px'}}>Fits this iPhone</span><span style={{background:'rgba(118,118,128,.12)',borderRadius:999,padding:'3px 10px'}}>Everything</span><span style={{background:'rgba(118,118,128,.12)',borderRadius:999,padding:'3px 10px'}}>Phone collection</span></div>
<style>{'.iptile .fwf-id{padding:8px 9px 9px}.iptile .id-size b{font-size:24px}.iptile .id-name{font-size:12px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.iptile .id-facts{font-size:9.5px}.iptile .id-mono svg{height:16px;width:auto}'}</style><div style={{display:'grid',gridTemplateColumns:'repeat(2,minmax(0,1fr))',gap:10,margin:'0 14px'}}>{ROWS.slice(0,4).map(x=>{const fx=fit(d,x);return(<div key={x.id} className="iptile" style={{display:'grid',gap:5,minWidth:0,overflow:'hidden'}}><H html={FW.identity({...x,size:fx.v.s*1e9})}/><div style={{fontSize:11,color:I.t2}}><Dot c={CC[fx.k]}/>{fx.t} · {fx.v.s} GB</div></div>);})}</div></Phone>
<Phone title={r.n} back="‹ Models"><div style={{margin:'0 14px 12px',background:'#fff',borderRadius:14,overflow:'hidden'}}><H html={FW.identity({...r,size:f.v.s*1e9})}/></div>
<Grp h="Run on this iPhone" f="Downloads keep going in the background, on any network. You get a notification when it is ready.">
<Row first l={<span><b>MLX 4-bit</b> · 1.2 GB</span>} s="Faster on iPhone · best for this device" r={<span style={{color:'#007AFF'}}>✓</span>}/>
<Row l={<span>GGUF Q4_K_M · 1.3 GB</span>} s="llama.cpp"/>
<div style={{padding:'4px 14px 14px'}}><div style={{fontSize:12,color:I.t2,margin:'6px 0'}}><Dot c={CC[f.k]}/>{f.t}: {f.why}</div><Btn2 pri>Download to iPhone</Btn2><Btn2>Send to my computer</Btn2></div></Grp></Phone>
<Phone title="Models"><Seg items={['Catalog','On this iPhone']} on="On this iPhone"/>
<Grp h="Running now"><Row first l={run.n} s="MLX 4-bit · 58 tok/s · unloads when the app is in the background" r={<span style={{color:'#007AFF',fontSize:15}}>Stop</span>}/><div style={{padding:'0 14px 12px'}}><div style={{height:6,borderRadius:3,background:I.sep}}><div style={{width:'38%',height:6,borderRadius:3,background:I.t1}}/></div><div style={{fontSize:11,color:I.t2,marginTop:4}}>1.7 GB of 4.5 GB this app can use for models</div></div></Grp>
<Grp h="Engines"><Row first l={<span><Dot c={ok}/>MLX</span>} s="Metal · fastest for MLX models"/><Row l={<span><Dot c={ok}/>llama.cpp</span>} s="Metal · runs every GGUF model"/><Row l={<span><Dot c={ok}/>Apple Intelligence</span>} s="Built in · no download · 4k context" r={<span style={{color:'#007AFF',fontSize:15}}>Use</span>}/></Grp>
<Grp h="Downloads"><Row first l="Gemma 4 E2B · MLX" s="63% · 1.9 of 3.0 GB · background" r={<span style={{color:'#007AFF',fontSize:15}}>Pause</span>}/></Grp>
<Grp h="Storage" f="Models stay on this iPhone and are not backed up to iCloud."><Row first l="Qwen3.5 2B" s="1.2 GB" r={<span style={{color:bad,fontSize:15}}>Delete</span>}/></Grp></Phone>
<Phone title="Chat"><div style={{margin:'0 14px',display:'flex',justifyContent:'center'}}><span style={{background:'#fff',borderRadius:999,padding:'5px 12px',fontSize:13,fontWeight:600}}>{run.n} · <span style={{color:ok}}>On this iPhone</span> ▾</span></div>
<div style={{padding:'18px 14px',display:'grid',gap:12,fontSize:15}}><div style={{justifySelf:'end',background:'#E9E9EB',borderRadius:18,padding:'8px 12px',maxWidth:'78%'}}>What should I pack for a 3-day hike?</div><div style={{maxWidth:'90%',lineHeight:1.45}}>Water filter, 2 liters of water, layers for rain and cold, a headlamp, first aid kit, food for 3 days plus one extra, and a paper map.<div style={{fontSize:11,color:I.t2,marginTop:6,fontFamily:'ui-monospace,Menlo'}}>58 tok/s · first token 0.2 s · 1.7 GB · offline</div></div></div>
<div style={{margin:'0 14px'}}><div style={{fontSize:12,color:I.t2,textTransform:'uppercase',padding:'10px 4px 5px'}}>Model picker</div><div style={{background:'#fff',borderRadius:12}}><Row first l={<span>{run.n} <span style={{fontSize:11,color:ok}}>offline</span></span>} s="On this iPhone" r="✓"/><Row l="Apple Intelligence" s="On this iPhone · built in"/><Row l="Qwen3.5 27B" s="Joe's MacBook Pro"/><Row l="Allternit Cloud" s="Cloud"/></div></div></Phone>
</div>);}

```


```callout ok Decided 11-15
iPhone Models tab: two segments, Catalog and On this iPhone.
```

Android runs the PWA's Model Lab phone layout. The app adds the native engine. Pick "This browser" in the first screen to see the PWA limit: models under 0.5 GB only. The third screen shows the notifications Android needs for a long download and a running model.

```react css=fw.css title="Android: Model Lab (phone layout), This phone, system notifications" h=740
import React from 'react';
// Freeweights shared rendering: the Allternit dot lettering (same 10-unit grid, 8.5 dots,
// 1.5 radius as the A://TERNIT wordmark) and the four preview card styles (A to D).
// Loaded by site/build.mjs at build time and by the browser for grid, deck and style switching.
(function (g) {
  var FONT = {
    A: ['..#..', '.#.#.', '#####', '#...#', '#...#'], B: ['####.', '#...#', '####.', '#...#', '####.'],
    C: ['.####', '#....', '#....', '#....', '.####'], D: ['####.', '#...#', '#...#', '#...#', '####.'],
    E: ['#####', '#....', '####.', '#....', '#####'], F: ['#####', '#....', '####.', '#....', '#....'],
    G: ['.####', '#....', '#.###', '#...#', '.###.'], H: ['#...#', '#...#', '#####', '#...#', '#...#'],
    I: ['###', '.#.', '.#.', '.#.', '###'], J: ['..###', '...#.', '...#.', '#..#.', '.##..'],
    K: ['#...#', '#..#.', '###..', '#..#.', '#...#'], L: ['#....', '#....', '#....', '#....', '#####'],
    M: ['#...#', '##.##', '#.#.#', '#...#', '#...#'], N: ['#...#', '##..#', '#.#.#', '#..##', '#...#'],
    O: ['.###.', '#...#', '#...#', '#...#', '.###.'], P: ['####.', '#...#', '####.', '#....', '#....'],
    Q: ['.###.', '#...#', '#.#.#', '#..#.', '.##.#'], R: ['####.', '#...#', '####.', '#.#..', '#..##'],
    S: ['.####', '#....', '.###.', '....#', '####.'], T: ['#####', '..#..', '..#..', '..#..', '..#..'],
    U: ['#...#', '#...#', '#...#', '#...#', '.###.'], V: ['#...#', '#...#', '#...#', '.#.#.', '..#..'],
    W: ['#...#', '#...#', '#.#.#', '##.##', '#...#'], X: ['#...#', '.#.#.', '..#..', '.#.#.', '#...#'],
    Y: ['#...#', '.#.#.', '..#..', '..#..', '..#..'], Z: ['#####', '...#.', '..#..', '.#...', '#####'],
    0: ['.###.', '#..##', '#.#.#', '##..#', '.###.'], 1: ['.#.', '##.', '.#.', '.#.', '###'],
    2: ['####.', '....#', '.###.', '#....', '#####'], 3: ['####.', '....#', '.###.', '....#', '####.'],
    4: ['#..#.', '#..#.', '#####', '...#.', '...#.'], 5: ['#####', '#....', '####.', '....#', '####.'],
    6: ['.###.', '#....', '####.', '#...#', '.###.'], 7: ['#####', '....#', '...#.', '..#..', '..#..'],
    8: ['.###.', '#...#', '.###.', '#...#', '.###.'], 9: ['.###.', '#...#', '.####', '....#', '.###.'],
    '.': ['.', '.', '.', '.', '#'], ':': ['.', '#', '.', '#', '.'], '/': ['..#', '..#', '.#.', '#..', '#..'],
    '-': ['...', '...', '###', '...', '...'], ' ': ['..', '..', '..', '..', '..']
  };
  var CELL = 10;
  function rect(c, r, fill) { return '<rect x="' + (c * CELL + 0.75) + '" y="' + (r * CELL + 0.75) + '" width="8.5" height="8.5" rx="1.5"' + (fill ? ' fill="' + fill + '"' : '') + '/>'; }
  // Lay out text on the dot grid. Returns { cols, rows, body }. Letters are separated by one empty column.
  function dots(text, col0, row0) {
    var c = col0 || 0, r0 = row0 || 0, body = '', chars = String(text).toUpperCase().split('');
    chars.forEach(function (ch, i) {
      var gl = FONT[ch] || FONT[' '];
      for (var r = 0; r < 5; r++) for (var x = 0; x < gl[r].length; x++) if (gl[r][x] === '#') body += rect(c + x, r0 + r);
      c += gl[0].length + (i < chars.length - 1 ? 1 : 0);
    });
    return { cols: c - (col0 || 0), body: body };
  }
  // The A://TERNIT wordmark cells, copied from the brand SVG (o = the coral dot).
  var WM = ['..#.......#...#.#####.#####.####..#...#..###..#####', '.#.#..#...#...#...#...#.....#...#.##..#...#.....#..', '##o##....#...#....#...####..####..#.#.#...#.....#..', '#...#.#.#...#.....#...#.....#.#...#..##...#.....#..', '#...#...#...#.....#...#####.#..##.#...#..###....#..'];
  function wordmark(col0, row0) {
    var b = '';
    WM.forEach(function (line, r) { for (var x = 0; x < line.length; x++) { if (line[x] === '#') b += rect(col0 + x, row0 + r); else if (line[x] === 'o') b += rect(col0 + x, row0 + r, '#D97757'); } });
    return { cols: WM[0].length, body: b };
  }
  function svg(cols, rows, body, h, label, cls) {
    var w = Math.round((h * cols) / rows * 100) / 100;
    return '<svg class="' + (cls || 'dotw') + '" viewBox="0 0 ' + cols * CELL + ' ' + rows * CELL + '" height="' + h + '" width="' + w + '" fill="currentColor" shape-rendering="geometricPrecision"' + (label ? ' role="img" aria-label="' + esc(label) + '"' : ' aria-hidden="true"') + '>' + body + '</svg>';
  }
  // Lockups. inline: A://TERNIT  FREEWEIGHTS on one line. stack: wordmark above the name.
  function lockup(kind, h, cls) {
    var a = wordmark(0, 0);
    if (kind === 'stack') { var n = dots('FREEWEIGHTS', 0, 7); return svg(Math.max(a.cols, n.cols), 12, a.body + n.body, h, 'Allternit Freeweights', cls || 'lockup stack'); }
    if (kind === 'name') { var o = dots('FREEWEIGHTS'); return svg(o.cols, 5, o.body, h, 'Freeweights', cls || 'lockup name'); }
    var m = dots('FREEWEIGHTS', a.cols + 4, 0); return svg(a.cols + 4 + m.cols, 5, a.body + m.body, h, 'Allternit Freeweights', cls || 'lockup inline');
  }
  function word(text, h, cls, label) { var d = dots(text); return svg(d.cols, 5, d.body, h, label, cls); }

  var esc = function (s) { return String(s == null ? '' : s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'); };
  function bytes(n) {
    if (n == null || isNaN(n)) return 'size unknown';
    var u = ['B', 'KB', 'MB', 'GB', 'TB'], i = 0, v = Number(n);
    while (v >= 1000 && i < u.length - 1) { v /= 1000; i++; }
    return (v >= 100 || i === 0 ? Math.round(v).toLocaleString('en-US') : v >= 10 ? v.toFixed(1) : v.toFixed(2)) + ' ' + u[i];
  }
  function sizeParts(n) { var s = bytes(n).split(' '); return { v: s[0], u: s[1] || '' }; }
  function params(n) { return !n ? '' : n >= 1e12 ? +(n / 1e12).toFixed(2) + 'T' : n >= 1e9 ? +(n / 1e9).toFixed(1) + 'B' : +(n / 1e6).toFixed(0) + 'M'; }
  var LC = { copyleft: 'copyleft', permissive: 'permissive', custom: 'custom terms', 'non-commercial': 'non-commercial', gated: 'gated', territory: 'territory-limited', unreviewed: 'license not reviewed' };
  function src(r) { return r.nc ? (r.how || 'From the publisher') : r.src || 'From source'; }
  function srcBadge(r) { var cls = r.nc || /source|publisher/i.test(src(r)) ? ' src' : /seed/i.test(src(r)) ? ' seed' : ''; return '<span class="b' + cls + '"><i class="d"></i>' + esc(src(r)) + '</span>'; }
  function lcTag(r) { return '<span class="lc lc-' + esc(r.lc || 'unknown') + '">' + esc(LC[r.lc] || r.lc || 'see card') + '</span>'; }
  function task(r) { var t = (r.task || [])[0]; return t ? t.replace(/-/g, ' ') : r.sec === 'alt.freeweights.datasets' ? 'dataset' : ''; }
  function date(t) { var d = new Date(t); if (isNaN(d)) return ''; return d.toUTCString().slice(0, 16); }
  function mono(r, h, cls) { return word(r.mono || 'FW', h, cls || 'mono-svg', null); }

  // A. Spec sheet
  function cardA(r) {
    var spec = [['Params', params(r.params) || 'not stated'], ['Size', bytes(r.size)], ['Format', (r.fmt || []).join(', ') || 'see files'], ['License', esc(r.lic)]];
    return '<a class="fwc fwc-a" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '">' +
      '<div class="a-top"><span class="a-mono">' + mono(r, 22) + '</span><div class="a-t"><div class="a-sec mono">' + esc(r.sec) + '</div><div class="a-name">' + esc(r.n) + '</div><div class="a-id mono">' + esc(r.id) + '</div></div></div>' +
      '<dl class="a-spec">' + spec.map(function (s) { return '<div><dt>' + s[0] + '</dt><dd' + (s[0] === 'Format' ? ' class="mono"' : '') + '>' + (s[0] === 'License' ? s[1] : esc(s[1])) + '</dd></div>'; }).join('') + '</dl>' +
      '<div class="a-foot">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + (task(r) ? '<span class="a-task">' + esc(task(r)) + '</span>' : '') + '</div></a>';
  }
  // B. Newsgroup post
  function cardB(r) {
    return '<a class="fwc fwc-b" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '">' +
      '<div class="b-head mono"><div><b>From:</b> ' + esc(r.org) + ' &lt;' + esc(r.host || 'huggingface.co') + '/' + esc(r.org) + '&gt;</div><div><b>Newsgroups:</b> ' + esc(r.sec) + '</div><div><b>Subject:</b> <span class="b-subj">' + esc(r.n) + '</span> (' + esc(bytes(r.size)) + ')</div><div><b>Date:</b> ' + esc(date(r.added)) + '</div></div>' +
      '<p class="b-body">' + esc(r.sum) + '</p>' +
      '<div class="b-sig mono">-- <br>' + esc(r.lic) + ' · ' + esc(LC[r.lc] || r.lc || '') + ' · ' + esc(src(r).toLowerCase()) + (r.rt ? ' · read terms first' : '') + '</div></a>';
  }
  // C. Element tile
  function cardC(r) {
    var s = sizeParts(r.size);
    return '<a class="fwc fwc-c" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '">' +
      '<div class="c-row mono"><span>' + esc(params(r.params) || '&mdash;').replace('&amp;mdash;', '&mdash;') + '</span><span>' + esc(LC[r.lc] || r.lc || '') + '</span></div>' +
      '<div class="c-mono">' + mono(r, 40) + '</div>' +
      '<div class="c-size"><b class="mono">' + esc(s.v) + '</b><span class="mono">' + esc(s.u) + '</span></div>' +
      '<div class="c-name">' + esc(r.n) + '</div>' +
      '<div class="c-facts mono"><span>' + esc((r.fmt || []).join(' ') || 'files') + '</span><span>' + esc(task(r) || r.fam) + '</span></div></a>';
  }
  // D. Terminal
  function cardD(r) {
    var lines = r.nc
      ? ['<span class="p">$</span> fw info ' + esc(r.id), '<span class="c"># link only: ' + esc(r.lic) + (r.lc ? ' (' + esc(LC[r.lc] || r.lc) + ')' : '') + '</span>', '<span class="c"># from the publisher, never hosted here</span>', '<span class="w">[publisher]</span> ' + esc(r.host || 'huggingface.co')]
      : ['<span class="p">$</span> fw get ' + esc(r.id), '<span class="c"># manifest sha256 ' + esc(r.sha || 'pending') + '&hellip;</span>', '<span class="c"># ' + esc(bytes(r.size)) + ' · ' + esc(r.files || 0) + ' files · ' + esc((r.fmt || []).join(', ')) + '</span>', '<span class="c"># license ' + esc(r.lic) + ' (' + esc(r.lc || 'see card') + ')' + (r.rt ? ', read terms first' : '') + '</span>', '<span class="ok">[' + esc(src(r).toLowerCase().replace(/^from /, '')) + ']</span> ' + esc(r.host || 'huggingface.co') + (r.commit ? ' @ ' + esc(r.commit.slice(0, 7)) : '')];
    return '<a class="fwc fwc-d mono" href="' + esc(r.url) + '" data-id="' + esc(r.id) + '"><div class="d-bar"><i></i><i></i><i></i><span>' + esc(r.sec) + '</span></div><pre>' + lines.join('\n') + '</pre></a>';
  }
  var CARDS = { a: cardA, b: cardB, c: cardC, d: cardD };
  function card(style, r) { return (CARDS[style] || cardA)(r); }
  // Deck: one model in focus.
  function deck(r) {
    var facts = [['Size', bytes(r.size)], ['Params', params(r.params) || 'not stated'], ['Formats', (r.fmt || []).join(', ') || 'see files'], ['Task', task(r) || 'not tagged'], ['License', r.lic + ' (' + (LC[r.lc] || r.lc || 'see card') + ')'], ['Files', r.nc ? 'link only' : (r.files || 0) + ' with sha256'], ['Publisher', r.org], ['Added', date(r.added)]];
    return '<article class="deck-card"><div class="dk-top"><span class="dk-mono">' + mono(r, 52) + '</span><div><div class="mono dk-sec">' + esc(r.sec) + '</div><h2 class="dk-name">' + esc(r.n) + '</h2><div class="mono dk-id">' + esc(r.id) + '</div></div></div>' +
      '<p class="dk-sum">' + esc(r.sum) + '</p><dl class="dk-facts">' + facts.map(function (f) { return '<div><dt>' + f[0] + '</dt><dd>' + esc(f[1]) + '</dd></div>'; }).join('') + '</dl>' +
      '<div class="dk-foot"><div class="badges">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + '</div><a class="btn pri" href="' + esc(r.url) + '">' + (r.nc ? 'License and source' : 'Open model page') + '</a></div></article>';
  }

  // ---------- One card, four faces ----------
  // IDENTITY (element tile), FACTS (spec sheet), RECORD (newsgroup header), ACTION (terminal).
  // Every view composes these from the same row, so the faces read as one card.
  function logo(r, px, cls) { return r.logo ? '<img class="org' + (cls ? ' ' + cls : '') + '" src="' + esc(r.logo) + '" alt="" width="' + px + '" height="' + px + '" loading="lazy" decoding="async">' : ''; }
  function shortDate(t) { var d = new Date(t); return isNaN(d) ? '' : d.toISOString().slice(0, 10); }
  function repo(r) { return String(r.id).split('/').slice(1).join('/'); }
  function lcName(r) { return LC[r.lc] || r.lc || 'see card'; }
  // Kinds: model and dataset share the size figure; a tool shows its latest release there.
  function faceIdentity(r) {
    if (r.kind === 'tool') {
      var tag = r.tag || 'no release';
      return '<span class="fwf fwf-id">' +
        '<span class="id-row mono"><span>' + esc(r.tkind || 'tool') + '</span>' + logo(r, 24, 'id-org') + '</span>' +
        '<span class="id-mono">' + mono(r, 40) + '</span>' +
        '<span class="id-size tag' + (tag.length > 8 ? ' long' : tag.length > 5 ? ' mid' : '') + '"><b class="mono">' + esc(tag) + '</b><span class="mono">release</span></span>' +
        '<span class="id-name">' + esc(r.n) + '</span>' +
        '<span class="id-facts mono"><span>' + esc(r.lang || 'tool') + '</span><span>' + esc(r.lic || '') + '</span></span></span>';
    }
    var s = sizeParts(r.size);
    return '<span class="fwf fwf-id">' +
      '<span class="id-row mono"><span>' + esc(params(r.params) ? params(r.params) + ' params' : (r.fam || '')) + '</span>' + logo(r, 24, 'id-org') + '</span>' +
      '<span class="id-mono">' + mono(r, 40) + '</span>' +
      '<span class="id-size"><b class="mono">' + esc(s.v) + '</b><span class="mono">' + esc(s.u) + '</span></span>' +
      '<span class="id-name">' + esc(r.n) + '</span>' +
      '<span class="id-facts mono"><span>' + esc((r.fmt || []).join(' ') || 'files') + '</span><span>' + esc(lcName(r)) + '</span></span></span>';
  }
  function faceFacts(r) {
    if (r.kind === 'tool') {
      var tr = [['License', r.lic || 'see project'], ['Language', r.lang || 'not stated'], ['Kind', r.tkind || 'tool'], ['Release', r.relDate || 'none'], ['Platforms', (r.platforms || []).join(', ') || 'see project']];
      return '<span class="fwf fwf-facts"><span class="ff-h mono">Facts</span><span class="ff-dl">' +
        tr.map(function (x) { return '<span class="ff-r' + (x[0] === 'Platforms' ? ' wide' : '') + '"><span class="ff-k">' + x[0] + '</span><span class="ff-v">' + esc(x[1]) + '</span></span>'; }).join('') +
        '</span><span class="ff-b"><span class="b src"><i class="d"></i>Links to the project</span>' + lcTag(r) + '</span></span>';
    }
    if (r.kind === 'dataset') {
      var dr = [['Rows', r.rowsN ? Number(r.rowsN).toLocaleString('en-US') : 'not stated'], ['Size', bytes(r.size)], ['Format', (r.fmt || []).join(', ') || 'see files'], ['License', r.lic]];
      return '<span class="fwf fwf-facts"><span class="ff-h mono">Facts</span><span class="ff-dl">' +
        dr.map(function (x) { return '<span class="ff-r"><span class="ff-k">' + x[0] + '</span><span class="ff-v' + (x[0] === 'Format' ? ' mono' : '') + '">' + esc(x[1]) + '</span></span>'; }).join('') +
        '</span><span class="ff-b">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + '</span></span>';
    }
    var rows = [['Params', params(r.params) || 'not stated'], ['Size', bytes(r.size)], ['Formats', (r.fmt || []).join(', ') || 'see files'], ['Task', task(r) || 'not tagged'], ['License', r.lic]];
    return '<span class="fwf fwf-facts"><span class="ff-h mono">Facts</span><span class="ff-dl">' +
      rows.map(function (x) { return '<span class="ff-r"><span class="ff-k">' + x[0] + '</span><span class="ff-v' + (x[0] === 'Formats' ? ' mono' : '') + '">' + esc(x[1]) + '</span></span>'; }).join('') +
      '</span><span class="ff-b">' + srcBadge(r) + lcTag(r) + (r.rt ? '<span class="lc lc-terms">read terms</span>' : '') + '</span></span>';
  }
  // mode: 'full' (all header lines + summary), 'strip' (From + Subject, under a tile), 'head' (lines, no summary)
  function faceRecord(r, mode, opt) {
    opt = opt || {};
    var subj = opt.h1 ? '<h1 class="rc-subj rc-h1">' + esc(r.n) + '</h1>' : opt.link === false ? '<span class="rc-subj">' + esc(r.n) + '</span>' : '<a class="rc-subj" href="' + esc(r.url) + '">' + esc(r.n) + '</a>';
    var from = '<span class="rc-l"><b>From:</b> ' + logo(r, 18, 'rc-org') + esc(r.org) + ' <span class="rc-dim">&lt;' + esc(r.host || 'huggingface.co') + '/' + esc(r.org) + '&gt;</span></span>';
    if (mode === 'strip') return '<span class="fwf fwf-rec strip mono">' + from + '<span class="rc-l"><b>Subject:</b> ' + subj + '</span></span>';
    var lines = [from, '<span class="rc-l"><b>Newsgroups:</b> ' + esc(r.sec) + '</span>', '<span class="rc-l"><b>Subject:</b> ' + subj + '</span>'];
    if (r.kind === 'tool') lines.push('<span class="rc-l"><b>Release:</b> ' + esc(r.tag ? r.tag + (r.relDate ? ' (' + r.relDate + ')' : '') : 'none') + '</span>');
    else lines.push('<span class="rc-l"><b>Date:</b> ' + esc(shortDate(r.added) || 'unknown') + '</span>');
    if (r.commit) lines.push('<span class="rc-l"><b>Commit:</b> ' + esc(r.commit.slice(0, 7)) + '</span>');
    if (r.kind === 'tool') lines.push('<span class="rc-l"><b>License:</b> ' + esc(r.lic || 'see project') + '</span>');
    if (r.sig) lines.push('<span class="rc-l"><b>Signed:</b> ' + esc(r.sig) + ' <span class="rc-ok" aria-label="signature present">&#10003;</span></span>');
    return '<div class="fwf fwf-rec mono">' + lines.join('') + (mode === 'head' ? '' : '<p class="rc-sum">' + esc(r.sum) + '</p>') + '</div>';
  }
  function faceAction(r) {
    if (r.kind === 'tool') {
      var T = (r.install && r.install.length ? r.install.map(function (l) { return /^#/.test(l) ? '<span class="c">' + esc(l) + '</span>' : '<span class="p">$</span> ' + esc(l); }) : ['<span class="c"># install: see the project\'s instructions</span>', '<span class="u">' + esc(r.installDocs || r.licUrl || '') + '</span>'])
        .concat(r.use ? ['<span class="c"># use with Freeweights</span>', (/^#/.test(r.use) ? '<span class="c">' + esc(r.use) + '</span>' : '<span class="p">$</span> ' + esc(r.use))] : []);
      return '<div class="fwf fwf-act mono"><div class="act-bar" aria-hidden="true"><i></i><i></i><i></i><span>install</span></div><pre>' + T.join('\n') + '</pre></div>';
    }
    if (r.kind === 'dataset' && !r.nc) {
      var D = (r.rt ? ['<span class="w"># custom terms: read the license before you download</span>'] : []).concat([
        '<span class="p">&gt;&gt;&gt;</span> from datasets import load_dataset',
        '<span class="p">&gt;&gt;&gt;</span> ds = load_dataset("' + esc(r.id) + '")',
        '<span class="c"># with HF_ENDPOINT=https://freeweights.allternit.com &middot; manifest sha256 ' + esc(r.sha || 'pending') + '&hellip;</span>']);
      return '<div class="fwf fwf-act mono"><div class="act-bar" aria-hidden="true"><i></i><i></i><i></i><span>python</span></div><pre>' + D.join('\n') + '</pre></div>';
    }
    var L = r.nc
      ? ['<span class="p">$</span> fw info ' + esc(r.id), '<span class="w">link only &mdash; read the license:</span>', '<span class="u">' + esc(r.licUrl || ('https://' + (r.host || 'huggingface.co') + '/' + r.id)) + '</span>']
      : (r.rt ? ['<span class="w"># custom terms: read the license before you download</span>'] : []).concat([
        '<span class="p">$</span> fw get ' + esc(r.id),
        '<span class="c"># manifest sha256 ' + esc(r.sha || 'pending') + '&hellip; &middot; ' + esc(bytes(r.size)) + ' &middot; ' + esc(r.files || 0) + ' files</span>',
        '<span class="p">$</span> fw verify ./' + esc(repo(r)) + ' ' + esc(r.id)]);
    return '<div class="fwf fwf-act mono"><div class="act-bar" aria-hidden="true"><i></i><i></i><i></i><span>' + (r.nc ? 'link only' : 'fw') + '</span></div><pre>' + L.join('\n') + '</pre></div>';
  }
  // The combined card: RECORD header on top, IDENTITY + FACTS side by side, ACTION footer.
  function combined(r, opt) {
    opt = opt || {};
    return '<article class="fwk' + (opt.cls ? ' ' + opt.cls : '') + '" data-id="' + esc(r.id) + '">' +
      '<div class="fwk-rec">' + faceRecord(r, 'full', { h1: opt.h1 }) + '</div>' +
      '<div class="fwk-mid"><div class="fwk-id">' + faceIdentity(r) + '</div><div class="fwk-facts">' + faceFacts(r) + '</div></div>' +
      '<div class="fwk-act">' + faceAction(r) + '</div>' +
      (opt.open ? '<div class="fwk-foot"><a class="btn pri sm" href="' + esc(r.url) + '">' + (r.nc ? 'License and source' : 'Open model page') + '</a></div>' : '') +
      '</article>';
  }
  // Grid / home / related tile: IDENTITY front, FACTS back (flip), RECORD strip below.
  function tile(r) {
    return '<div class="fwt" data-id="' + esc(r.id) + '">' +
      '<button type="button" class="flip" aria-pressed="false" aria-label="' + esc(r.n) + ': show facts">' +
      '<span class="flip-in"><span class="flip-f">' + faceIdentity(r) + '</span><span class="flip-b">' + faceFacts(r) + '</span></span>' +
      '<span class="flip-hint mono" aria-hidden="true">flip</span></button>' +
      faceRecord(r, 'strip') + '</div>';
  }
  var api = { FONT: FONT, dots: dots, wordmark: wordmark, svg: svg, lockup: lockup, word: word, card: card, deck: deck, bytes: bytes, params: params, esc: esc, STYLES: ['a', 'b', 'c', 'd'],
    logo: logo, identity: faceIdentity, facts: faceFacts, record: faceRecord, action: faceAction, combined: combined, tile: tile };
  g.FWCards = api;
})(typeof window !== 'undefined' ? window : globalThis);

const ROWS=[{"kind": "model", "id": "lmstudio-community/Qwen3.5-4B-GGUF", "n": "Qwen3.5-4B-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 11329544112, "params": 4200000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-14T21:41:57Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 2.7, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 2.5, "e": "MLX"}]}, {"kind": "model", "id": "lmstudio-community/Qwen3.5-2B-GGUF", "n": "Qwen3.5-2B-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 5510586672, "params": 2300000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 1.3, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 1.2, "e": "MLX"}]}, {"kind": "model", "id": "google/gemma-4-E2B-it-qat-q4_0-gguf", "n": "gemma-4-E2B-it-qat-q4_0-gguf", "org": "google", "sec": "alt.freeweights.gemma", "fam": "gemma", "mono": "GM", "url": "#", "size": 4336380945, "params": 5100000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-14T21:41:57Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "https://freeweights.allternit.com/assets/orgs/google.png", "sig": "", "v": [{"f": "GGUF", "q": "Q4_0 QAT", "s": 3.2, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 3.0, "e": "MLX"}]}, {"kind": "model", "id": "ggml-org/SmolLM3-3B-GGUF", "n": "SmolLM3-3B-GGUF", "org": "ggml-org", "sec": "alt.freeweights.smollm", "fam": "smollm", "mono": "SM", "url": "#", "size": 11349231863, "params": 3100000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-14T21:41:57Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 1.9, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 1.8, "e": "MLX"}]}, {"kind": "model", "id": "lmstudio-community/Phi-4-mini-instruct-GGUF", "n": "Phi-4-mini-instruct-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.phi", "fam": "phi", "mono": "PH", "url": "#", "size": 11981766167, "params": 3800000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "MIT", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "https://freeweights.allternit.com/assets/orgs/microsoft.png", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 2.5, "e": "llama.cpp"}]}, {"kind": "model", "id": "Qwen/Qwen3-1.7B-GGUF", "n": "Qwen3-1.7B-GGUF", "org": "Qwen", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 1834445939, "params": 1700000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 1.1, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 1.0, "e": "MLX"}, {"f": "ONNX", "q": "q4f16", "s": 1.2, "e": "WebGPU"}]}, {"kind": "model", "id": "Qwen/Qwen3-0.6B-GGUF", "n": "Qwen3-0.6B-GGUF", "org": "Qwen", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 639466546, "params": 600000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 0.4, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 0.35, "e": "MLX"}, {"f": "ONNX", "q": "q4f16", "s": 0.45, "e": "WebGPU"}]}, {"kind": "model", "id": "lmstudio-community/gemma-4-E4B-it-QAT-GGUF", "n": "gemma-4-E4B-it-QAT-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.gemma", "fam": "gemma", "mono": "GM", "url": "#", "size": 6146496828, "params": 8000000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "https://freeweights.allternit.com/assets/orgs/google.png", "sig": "", "v": [{"f": "GGUF", "q": "Q4_0 QAT", "s": 5.0, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 4.8, "e": "MLX"}]}, {"kind": "model", "id": "lmstudio-community/Qwen3.5-27B-GGUF", "n": "Qwen3.5-27B-GGUF", "org": "lmstudio-community", "sec": "alt.freeweights.qwen", "fam": "qwen", "mono": "QW", "url": "#", "size": 68149711960, "params": 27000000000.0, "fmt": ["gguf"], "task": ["text-generation"], "lc": "permissive", "lic": "Apache-2.0", "rt": false, "src": "From source", "how": "Hosted (signed)", "nc": false, "added": "2026-11-15T05:34:43Z", "sha": "pending", "files": 0, "host": "huggingface.co", "commit": "", "logo": "", "sig": "", "v": [{"f": "GGUF", "q": "Q4_K_M", "s": 16.5, "e": "llama.cpp"}, {"f": "MLX", "q": "4-bit", "s": 15.6, "e": "MLX"}]}];
const DEV={"mac": {"name": "Joe's MacBook Pro", "hw": "Apple M4 Pro \u00b7 48 GB unified memory", "os": "macOS 26", "kind": "unified", "apple": true, "budget": 31, "disk": "412 GB free", "eng": [["llama.cpp", "Metal", "b7180", "Ready"], ["MLX", "Metal", "0.30.1", "Ready"]], "ship": "Engines ship inside Allternit Desktop and update with it."}, "win": {"name": "Studio PC", "hw": "NVIDIA RTX 4070 12 GB \u00b7 32 GB RAM \u00b7 Ryzen 7", "os": "Windows 11", "kind": "discrete", "vram": 12, "ram": 32, "disk": "690 GB free", "eng": [["llama.cpp", "CUDA 13", "b7180", "Ready"], ["llama.cpp", "Vulkan", "b7180", "Installed, not in use"], ["llama.cpp", "CPU (AVX2)", "b7180", "Fallback"]], "ship": "The CUDA build is picked because an NVIDIA GPU is present. Vulkan stays as the fallback."}, "winarm": {"name": "Surface Laptop", "hw": "Snapdragon X Elite \u00b7 32 GB \u00b7 Adreno X1", "os": "Windows 11 on Arm", "kind": "unified", "budget": 19, "disk": "301 GB free", "eng": [["llama.cpp", "CPU (Arm, KleidiAI)", "b7180", "Ready"], ["llama.cpp", "Vulkan (Adreno)", "b7180", "Ready"], ["NPU (Hexagon)", "QNN", "preview", "Off: turn on in Advanced"]], "ship": "The NPU path is off by default. CPU and GPU are the baseline."}, "linux": {"name": "Workstation", "hw": "AMD Radeon RX 7900 XTX 24 GB \u00b7 64 GB RAM", "os": "Ubuntu 24.04", "kind": "discrete", "vram": 24, "ram": 64, "disk": "1.6 TB free", "eng": [["llama.cpp", "ROCm 7 (HIP)", "b7180", "Ready"], ["llama.cpp", "Vulkan", "b7180", "Installed, not in use"], ["llama.cpp", "CPU", "b7180", "Fallback"]], "ship": "ROCm is picked for RDNA3. Vulkan covers GPUs that ROCm does not support."}, "cpu": {"name": "Home server", "hw": "8 vCPU \u00b7 16 GB RAM \u00b7 no GPU", "os": "Debian 13", "kind": "cpu", "ram": 16, "disk": "88 GB free", "eng": [["llama.cpp", "CPU (AVX2)", "b7180", "Ready"]], "ship": "No GPU found. Models run on the CPU, so small models only."}, "web": {"name": "This browser tab", "hw": "Chrome 142 \u00b7 WebGPU \u00b7 2 GB model limit", "os": "ai.allternit.com", "kind": "browser", "budget": 2, "disk": "Browser storage 41 GB", "eng": [["WebGPU", "transformers.js (ONNX)", "4.3", "Ready"]], "ship": "Nothing to install. Weights are cached in the browser. Bigger models run in Desktop or the phone app."}, "iphone": {"name": "Joe's iPhone", "hw": "iPhone 17 Pro \u00b7 12 GB \u00b7 6 GB per app", "os": "iOS 26", "kind": "phone", "apple": true, "budget": 4.5, "disk": "41 GB free", "eng": [["MLX", "Metal", "0.30.1", "Ready"], ["llama.cpp", "Metal", "b7180", "Ready"], ["Apple Intelligence", "built in", "iOS 26", "Ready"]], "ship": "Engines are inside the app. Apple Intelligence needs no download."}, "android": {"name": "Galaxy S25", "hw": "Snapdragon 8 Elite \u00b7 12 GB", "os": "Android 16", "kind": "phone", "budget": 5, "disk": "96 GB free", "eng": [["llama.cpp", "OpenCL (Adreno)", "b7180", "Ready"], ["llama.cpp", "CPU", "b7180", "Fallback"], ["WebGPU", "in the PWA", "Chrome 142", "Ready (small models)"], ["Gemini Nano", "ML Kit", "system", "Not on this phone"]], "ship": "The app engine runs 1 to 4B models. The PWA alone runs models under 0.5 GB."}};
const pick=(d,r)=>d.kind==='browser'?r.v.find(x=>x.e==='WebGPU'):d.apple?(r.v.find(x=>x.e==='MLX')||r.v.find(x=>x.e==='llama.cpp')):r.v.find(x=>x.e==='llama.cpp');
function fit(d,r){const v=pick(d,r);
 if(!v)return{k:'na',t:'No browser build',why:'Run it in Desktop or the phone app',v:r.v[0]};
 const need=v.s*1.2+0.4,n=need.toFixed(1);
 if(d.kind==='discrete'){if(need<=d.vram*0.9)return{k:'ok',t:'Fits',why:'Full GPU, '+n+' of '+d.vram+' GB VRAM',v};if(need<=d.vram+d.ram*0.5)return{k:'slow',t:'May be slow',why:'Part runs on the CPU',v};return{k:'no',t:"Won't fit",why:'Needs about '+n+' GB',v};}
 if(d.kind==='cpu'){if(need>d.ram*0.6)return{k:'no',t:"Won't fit",why:'Needs about '+n+' GB',v};return v.s<1.5?{k:'ok',t:'Fits',why:'CPU only',v}:{k:'slow',t:'May be slow',why:'CPU only, about 5 tok/s',v};}
 if(need<=d.budget*0.8)return{k:'ok',t:'Fits',why:n+' of '+d.budget+' GB for models',v};
 if(need<=d.budget)return{k:'slow',t:'Tight fit',why:'Close other apps first',v};
 return{k:'no',t:"Won't fit",why:'Needs '+n+' GB, '+d.budget+' GB free for models',v};}
const FC={ok:'var(--ok)',slow:'var(--warn)',no:'var(--bad)',na:'var(--ink3)'};
const ENVS=[['mac','Mac'],['win','Windows + NVIDIA'],['winarm','Windows on Arm'],['linux','Linux + AMD'],['cpu','Linux CPU only'],['web','Browser'],['iphone','iPhone'],['android','Android']];

const FW=window.FWCards;
const H=({html,cls,tag})=>React.createElement(tag||'div',{className:cls,dangerouslySetInnerHTML:{__html:html}});
const Pill=({f,small})=>(<span title={f.why} style={{display:'inline-flex',alignItems:'center',gap:5,fontSize:small?10.5:11.5,fontWeight:600,color:FC[f.k],background:'var(--fill)',borderRadius:999,padding:small?'1px 7px':'2px 9px',whiteSpace:'nowrap'}}><i style={{width:6,height:6,borderRadius:3,background:FC[f.k]}}/>{f.t}</span>);
const Btn=({children,pri,onClick,small})=>(<button onClick={onClick} style={{border:pri?'0':'1px solid var(--line2)',background:pri?'var(--btn)':'transparent',color:pri?'var(--btnink)':'var(--ink)',borderRadius:9,padding:small?'4px 10px':'6px 12px',fontSize:small?11.5:12.5,fontWeight:600,cursor:'pointer',whiteSpace:'nowrap',fontFamily:'inherit'}}>{children}</button>);
function EnvSwitch({env,setEnv,only}){const list=only?ENVS.filter(e=>only.includes(e[0])):ENVS;return(<div style={{display:'flex',flexWrap:'wrap',gap:6,alignItems:'center',padding:'8px 10px',border:'1px dashed var(--line2)',borderRadius:10,marginBottom:12,fontSize:12}}><span className="mono" style={{color:'var(--ink3)',marginRight:4}}>Mockup only, pretend this runs on:</span>{list.map(([k,l])=>(<button key={k} onClick={()=>setEnv(k)} style={{border:0,borderRadius:999,padding:'3px 10px',fontSize:12,cursor:'pointer',fontFamily:'inherit',background:env===k?'var(--btn)':'var(--fill)',color:env===k?'var(--btnink)':'var(--ink2)'}}>{l}</button>))}</div>);}
const TABS=['Catalog','This device','Playground','Train','Cloud'];
function Chrome({tab,children,d}){return(<div style={{background:'var(--bg)',color:'var(--ink)',fontFamily:'var(--font-allternit-sans)',padding:'22px 24px',fontSize:14}}>
<div style={{fontFamily:'var(--font-allternit-serif)',fontSize:28,fontWeight:500,letterSpacing:'-.01em'}}>Freeweights Studio</div>
<div style={{color:'var(--ink2)',fontSize:13.5,marginTop:2}}>Find open weights in Freeweights, then download and run them on this device, your other devices or the cloud.</div>
<div style={{display:'flex',gap:6,marginTop:14,flexWrap:'wrap'}}>{TABS.map(t=>(<span key={t} style={{border:'1px solid '+(t===tab?'var(--ink)':'var(--line)'),background:t===tab?'var(--fill2)':'transparent',borderRadius:10,padding:'5px 12px',fontSize:12.5,fontWeight:600}}>{t}</span>))}</div>
<div style={{display:'flex',alignItems:'center',gap:10,marginTop:14,padding:'10px 12px',border:'1px solid var(--line)',borderRadius:12,flexWrap:'wrap'}}><span style={{width:8,height:8,borderRadius:4,background:'var(--ok)'}}/><b style={{fontSize:13}}>{d.name}</b><span className="mono" style={{fontSize:11.5,color:'var(--ink2)'}}>{d.hw} · {d.os}</span><span style={{flex:1}}/><span className="mono" style={{fontSize:11.5,color:'var(--ink3)'}}>{d.eng.filter(e=>/Ready/.test(e[3])).map(e=>e[0]+' '+e[1]).join(' + ')}</span></div>
<div style={{marginTop:16}}>{children}</div></div>);}

const d=DEV.android;const dw=DEV.web;
function Droid({children,dark}){return(<div style={{width:340,height:700,borderRadius:34,border:'9px solid #1b1b1b',background:dark?'#1c1c1e':'var(--bg)',overflow:'hidden',flex:'none',display:'flex',flexDirection:'column',fontFamily:'var(--font-allternit-sans)',color:dark?'#eee':'var(--ink)'}}>
<div style={{height:30,fontSize:12,display:'flex',justifyContent:'space-between',padding:'8px 20px 0'}}><span>9:41</span><span>▾ ▮ 82%</span></div><div style={{flex:1,overflow:'hidden'}}>{children}</div><div style={{height:22,display:'flex',justifyContent:'center'}}><i style={{width:110,height:4,borderRadius:2,background:dark?'#888':'#bbb',marginTop:8}}/></div></div>);}
const PH=({t})=>(<div style={{display:'flex',alignItems:'center',gap:12,padding:'6px 14px 10px'}}><span style={{fontSize:20}}>☰</span><b style={{fontFamily:'var(--font-allternit-serif)',fontSize:22,fontWeight:500}}>{t}</b></div>);
const Chip=({on,children})=><span style={{flex:'none',fontSize:12.5,fontWeight:600,border:'1px solid '+(on?'var(--ink)':'var(--line)'),background:on?'var(--fill2)':'transparent',borderRadius:10,padding:'5px 11px'}}>{children}</span>;
export default function App(){const [eng,setEng]=React.useState('app');const dev=eng==='app'?d:{...dw,budget:0.45,name:'Chrome on this phone'};
return(<div style={{display:'flex',gap:22,flexWrap:'wrap',justifyContent:'center',padding:12}}>
<Droid><PH t="Freeweights Studio"/><div style={{display:'flex',gap:6,padding:'0 14px 10px',overflow:'hidden'}}><Chip on>Catalog</Chip><Chip>This phone</Chip><Chip>Playground</Chip></div>
<div style={{margin:'0 14px 10px',border:'1px solid var(--line)',borderRadius:12,padding:'8px 10px',fontSize:12.5}}><div className="mono" style={{fontSize:10.5,color:'var(--ink3)',textTransform:'uppercase',letterSpacing:'.08em',marginBottom:5}}>Run with</div><div style={{display:'flex',gap:6}}>{[['app','App engine · up to 4B'],['web','This browser · under 0.5 GB']].map(([k,l])=><button key={k} onClick={()=>setEng(k)} style={{flex:1,border:0,borderRadius:8,padding:'6px 4px',fontSize:11.5,fontWeight:600,fontFamily:'inherit',cursor:'pointer',background:eng===k?'var(--btn)':'var(--fill)',color:eng===k?'var(--btnink)':'var(--ink2)'}}>{l}</button>)}</div></div>
<div style={{display:'flex',gap:8,margin:'0 14px 8px'}}><div style={{flex:1,border:'1px solid var(--line)',borderRadius:10,padding:'7px 10px',fontSize:13,color:'var(--ink3)'}}>Search models</div><div className="seg"><button className="segb" aria-pressed="true">☰</button><button className="segb">▦</button><button className="segb">▯</button></div></div>
<div style={{padding:'0 10px'}}>{ROWS.slice(0,7).map(r=>{const f=fit(dev,r);return(<div key={r.id} className="msg" style={{gridTemplateColumns:'minmax(0,1fr) auto',padding:'8px 6px'}}><span className="s"><span className="msg-org msg-mono" dangerouslySetInnerHTML={{__html:r.logo?'<img class="org" src="'+r.logo+'" width="22" height="22">':FW.word(r.mono,9,'mono-svg')}}/><span className="s-t"><span className="s-n">{r.n}</span><small>{f.v.s} GB · {r.lic}</small></span></span><span style={{display:'grid',justifyItems:'end',gap:3}}><Pill f={f} small/>{f.k==='ok'||f.k==='slow'?<Btn pri small>Get</Btn>:null}</span></div>);})}</div></Droid>
<Droid><PH t="Freeweights Studio"/><div style={{display:'flex',gap:6,padding:'0 14px 10px'}}><Chip>Catalog</Chip><Chip on>This phone</Chip><Chip>Playground</Chip></div>
<div style={{padding:'0 14px',display:'grid',gap:10,fontSize:13}}>
<div style={{border:'1px solid var(--line)',borderRadius:12,padding:'10px 12px'}}><b>{d.name}</b><div className="mono" style={{fontSize:11,color:'var(--ink2)'}}>{d.hw} · {d.os}</div><div className="mono" style={{fontSize:11,color:'var(--ink3)',marginTop:4}}>5 GB free for models right now</div></div>
<div style={{border:'1px solid var(--line)',borderRadius:12,padding:'10px 12px',display:'grid',gap:8}}><b>Engines</b>{[['App engine','llama.cpp · OpenCL (Adreno)','Ready','var(--ok)'],['Browser engine','WebGPU in Chrome · small models','Ready','var(--ok)'],['Gemini Nano','Android system model','Not on this phone','var(--ink3)']].map(([a,b,c,col])=>(<div key={a} style={{display:'flex',alignItems:'center',gap:8}}><i style={{width:7,height:7,borderRadius:4,background:col}}/><div style={{flex:1}}><div style={{fontWeight:600}}>{a}</div><div className="mono" style={{fontSize:10.5,color:'var(--ink2)'}}>{b}</div></div><span style={{fontSize:11.5,color:col}}>{c}</span></div>))}<div style={{fontSize:11.5,color:'var(--ink3)'}}>The app engine ships inside the Allternit app from Google Play. In Chrome without the app, only the browser engine runs.</div></div>
<div style={{border:'1px solid var(--line)',borderRadius:12,padding:'10px 12px',display:'grid',gap:6}}><div style={{display:'flex',justifyContent:'space-between'}}><b>Running now</b><Btn small>Stop</Btn></div><div>Qwen3.5-2B-GGUF <span className="mono" style={{fontSize:11,color:'var(--ink2)'}}>Q4_K_M · App engine</span></div><div className="mono" style={{fontSize:11,color:'var(--ink2)'}}>21 tok/s · 1.8 GB · 700 / 4,096 ctx</div><div style={{height:6,borderRadius:3,background:'var(--fill2)'}}><div style={{width:'36%',height:6,borderRadius:3,background:'var(--ink)'}}/></div></div>
<div style={{border:'1px solid var(--line)',borderRadius:12,padding:'10px 12px'}}><div style={{display:'flex',justifyContent:'space-between'}}><b>Downloads</b><span className="mono" style={{fontSize:11,color:'var(--ink2)'}}>any network</span></div><div style={{marginTop:6}}>SmolLM3-3B-GGUF · 41%</div><div style={{height:6,borderRadius:3,background:'var(--fill2)',marginTop:4}}><div style={{width:'41%',height:6,borderRadius:3,background:'var(--ink)'}}/></div></div></div></Droid>
<Droid dark><div style={{padding:'10px 14px',fontSize:13,color:'#aaa'}}>Mon, Nov 16</div>
{[['Allternit · Downloading','SmolLM3-3B-GGUF · 41% · 0.8 of 1.9 GB','Pause','Cancel'],['Allternit · Model running','Qwen3.5-2B-GGUF · 1.8 GB · stops after 5 min idle','Stop','Open']].map(([t,s,a,b])=>(<div key={t} style={{margin:'0 10px 10px',background:'#2c2c2e',borderRadius:18,padding:'12px 14px'}}><div style={{display:'flex',gap:8,alignItems:'center',fontSize:12,color:'#bbb'}}><span style={{width:18,height:18,borderRadius:9,background:'#fff',display:'inline-block'}}/>{t}</div><div style={{fontSize:13.5,marginTop:6,color:'#eee'}}>{s}</div>{t.includes('Download')?<div style={{height:4,borderRadius:2,background:'#444',marginTop:8}}><div style={{width:'41%',height:4,borderRadius:2,background:'#8ab4f8'}}/></div>:null}<div style={{display:'flex',gap:18,marginTop:10,fontSize:13,color:'#8ab4f8',fontWeight:600}}><span>{a}</span><span>{b}</span></div></div>))}
<div style={{margin:'14px 10px',fontSize:12,color:'#999',lineHeight:1.5}}>Android shows these while the app engine works: a download in a WorkManager job, and a running model in a foreground service, so the system does not stop it.</div></Droid>
</div>);}

```


## G How Freeweights plugs in

```flow LR
Freeweights catalog -> device.json overlay: per-quant files, size, RAM estimate, engine, best_for
device.json overlay -> Worker API /api/models/:id/device
Worker API /api/models/:id/device -> Model Lab Catalog (desktop, web, PWA)
Worker API /api/models/:id/device -> iOS Models tab
Worker API /api/models/:id/device -> gizzi models run
fw-cards.js faces -> Model Lab Catalog (desktop, web, PWA)
fw-cards.js faces -> iOS Models tab
Model Lab Catalog (desktop, web, PWA) -> Download + sha256 check against the signed manifest
iOS Models tab -> Download + sha256 check against the signed manifest
Download + sha256 check against the signed manifest -> Engine on this device
```

```table
| Piece | Owner | What changes |
|---|---|---|
| Card faces `fw-cards.js` + `fw.css` | Freeweights site (HF-parity lane) | Model Lab and iOS reuse them read-only. When device.json lands, the site faces show the smallest quant ("from X GB"), so the app and the site match |
| `catalog/overlay/device.json` | This lane | New: per-quant files, size, RAM estimate, format, engine, best device classes |
| Worker `routes/device.ts` | This lane | New route; one line in the router table; `model-info.ts` untouched |
| "On device / fits" facet, "Run at home" on model pages | HF-parity lane | Reads device.json |
| Collections phone / laptop / workstation / browser | This lane | `site/pages/collections.mjs` |
| `?view=model-lab&model=<id>` deep link | HF-parity lane adds it; this lane keeps it working | Opens Catalog with the model selected |
| Fit formula | This lane | One function in every client: file size × 1.2 + context, against the device's model budget |
```

## H Surface Matrix and what happens next

```table
| Surface | Status | Work |
|---|---|---|
| Desktop (Mac, Windows, Linux) | BUILD | Model Lab overhaul; llama.cpp builds per backend (Metal, CUDA, ROCm, Vulkan, CPU) and MLX inside the runtime package; hardware endpoint; idle unload; local API |
| ai.allternit.com web + phone layout | BUILD | Same Model Lab; WebGPU engine (transformers.js) in `browser-webgpu.ts` with adapter-limit budget |
| m.allternit.com PWA | BUILD | Same as web, phone sizing, 0.5 GB limit on iPhone Safari |
| iOS | BUILD | llama.cpp xcframework + MLX Swift + Foundation Models; Models tab segments; background downloads; memory entitlement check |
| Android | BUILD | Native llama.cpp engine (OpenCL/CPU) in the app, bridged to the PWA; WorkManager downloads; foreground service while running |
| Gizzi Code | BUILD | `gizzi models run <id>` starts the model on the local engine |
| Backend | BUILD | local-engine `/hardware` route, MLX launch, bundled llama-server builds |
| Freeweights | BUILD | device.json overlay, `routes/device.ts`, device collections |
| Docs | BUILD | Model Lab guide, per-OS engine notes, phone guides |
```

Next: your picks on this page go to the planner (Fable) with this matrix. The plan lands the contract first (device.json + hardware endpoint + fit function), then one lane per row. iOS engine and memory work go to builder-hard (Opus).

Not checked yet: App Store access to `increased-memory-limit` (first TestFlight build), and Firefox WebGPU status (feature-detect at runtime).
