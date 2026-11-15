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
