// motion-reel engine — deterministic canvas motion graphics.
// Every frame is a pure function of time t (seconds). No requestAnimationFrame state,
// no Math.random() in draw code — that's what makes frame-by-frame rendering exact.
//
// scenes.js must define:  REEL = { w, h, duration, bpm, brand, scenes:[{start, end, name, draw(t, lt)}],
//                                  hits:[[t, strength]], flashes:[[t, alpha, color]],
//                                  glitches:[[tStart, tCut, tEnd, strength]], wipes:[[tCut, color, dur]],
//                                  hud:{label, show:true, progress:true}, fadeIn:0.08, fadeOut:0.28, fonts:['800 100px Display', ...],
//                                  transparent:false /* true → alpha PNG frames: no bg, grain, vignette or flashes */ }

const REEL_ = () => window.REEL;
const W = () => REEL_().w, H = () => REEL_().h;
let C, X, B, BX, GRAIN, VIG;
const IMG = {};

// ---------- math ----------
const cl = (v, a = 0, b = 1) => Math.min(b, Math.max(a, v));
const lerp = (a, b, t) => a + (b - a) * t;
const P = (t, a, b) => cl((t - a) / (b - a));              // clamped progress: 0 before a, 1 after b
const win = (t, a, b) => t >= a && t < b;                   // use for anything that must switch OFF (glitches!)
const E = {
  linear: t => t,
  outExpo: t => t >= 1 ? 1 : 1 - Math.pow(2, -10 * t),
  inExpo: t => t <= 0 ? 0 : Math.pow(2, 10 * t - 10),
  ioExpo: t => t <= 0 ? 0 : t >= 1 ? 1 : t < .5 ? Math.pow(2, 20 * t - 10) / 2 : (2 - Math.pow(2, -20 * t + 10)) / 2,
  outCubic: t => 1 - Math.pow(1 - t, 3),
  inCubic: t => t * t * t,
  ioCubic: t => t < .5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2,
  outBack: t => { const c1 = 1.9, c3 = c1 + 1; return 1 + c3 * Math.pow(t - 1, 3) + c1 * Math.pow(t - 1, 2); },
};
let seed_ = 1337;
const RA = Array.from({ length: 8192 }, () => { seed_ ^= seed_ << 13; seed_ ^= seed_ >>> 17; seed_ ^= seed_ << 5; return (seed_ >>> 0) / 4294967296; });
const rr = (i, k = 0) => RA[Math.abs(i * 131 + k * 977 + 17) % RA.length];   // deterministic "random"
// colors: mix() takes HEX in, returns rgb() out — never nest mix(mix(...)); blend amounts instead.
const hex = h => [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16));
const mix = (a, b, t) => { const A = hex(a), B_ = hex(b); return `rgb(${A.map((v, i) => Math.round(v + (B_[i] - v) * cl(t))).join(',')})`; };
const rgba = (h, a) => { const [r, g, b] = hex(h); return `rgba(${r},${g},${b},${a})`; };
const pad = n => String(n).padStart(2, '0');

// ---------- primitives ----------
function blk(cx, cy, s, col, rot = 0, a = 1, glow = 0, radius = 0.18) {
  if (s <= 0.3 || a <= 0.003) return;
  X.save(); X.globalAlpha *= a; X.translate(cx, cy); if (rot) X.rotate(rot);
  if (glow) { X.shadowColor = col; X.shadowBlur = glow; }
  X.fillStyle = col; X.beginPath(); X.roundRect(-s / 2, -s / 2, s, s, s * radius); X.fill(); X.restore();
}
function setFont(o) { const f = o.f || 'Display'; X.font = `${o.w || 400} ${o.s || 40}px ${f}, ${f === 'Mono' ? 'ui-monospace, Menlo, "DejaVu Sans Mono", monospace' : 'system-ui, "Helvetica Neue", Arial, sans-serif'}`; X.letterSpacing = (o.ls || 0) + 'px'; }
// o: {f family, w weight, s size, c color, ls letterSpacing, al align, a alpha, stroke, lw, glow}
function txt(s, x, y, o = {}) {
  X.save(); setFont(o); X.textAlign = o.al || 'left'; X.textBaseline = o.bl || 'alphabetic';
  X.globalAlpha *= (o.a ?? 1);
  if (o.glow) { X.shadowColor = o.c || '#fff'; X.shadowBlur = o.glow; }
  if (o.stroke) { X.strokeStyle = o.c || '#fff'; X.lineWidth = o.lw || 2; X.strokeText(s, x, y); }
  else { X.fillStyle = o.c || '#fff'; X.fillText(s, x, y); }
  X.restore();
}
function tw(s, o) { X.save(); setFont(o); const w = X.measureText(s).width - (o.ls || 0); X.restore(); return w; }
// text rising out of a mask (the workhorse reveal)
function rise(s, x, y, o, p) {
  if (p <= 0) return;
  const sz = o.s || 40, w = tw(s, o) + sz, al = o.al || 'left';
  const lx = al === 'center' ? x - w / 2 : al === 'right' ? x - w : x - sz * 0.1;
  X.save(); X.beginPath(); X.rect(lx, y - sz * 1.05, w + sz * 0.2, sz * 1.35); X.clip();
  txt(s, x, y + (1 - E.outExpo(p)) * sz * 1.25, o); X.restore();
}
// word-by-word rise, centered or left
function riseWords(str, x, y, o, t, t0, stagger = 0.05, dur = 0.4, colorFor) {
  const words = str.split(' '), full = tw(str, o);
  let wx = (o.al === 'center') ? x - full / 2 : x;
  words.forEach((w, k) => { rise(w, wx, y, { ...o, al: 'left', c: colorFor ? colorFor(k, w) : o.c }, P(t, t0 + k * stagger, t0 + k * stagger + dur)); wx += tw(w + ' ', o); });
}
// typewriter with block cursor
function typed(s, x, y, o, p, t, cursorCol) {
  if (p <= 0) return;
  const n = Math.floor(s.length * cl(p)), shown = s.slice(0, n);
  txt(shown, x, y, o);
  if (p < 1.15 && (p < 1 || Math.floor(t * 8) % 2 === 0)) {
    const sz = o.s || 40; blk(x + tw(shown, o) + (n ? (o.ls || 0) : 0) + 6 + sz * 0.3, y - sz * 0.35, sz * 0.62, cursorCol || REEL_().brand.accent);
  }
}
// scale-slam with motion-blur ghosts
function slam(s, x, y, t, t0, o, from = 2.6, dur = 0.24) {
  const p = P(t, t0, t0 + dur); if (p <= 0) return;
  const w = tw(s, o), cx = x + w / 2, cy = y - (o.s || 40) * 0.35;
  for (let k = 5; k >= 0; k--) {
    const pk = P(t - k * 0.018, t0, t0 + dur); if (pk <= 0) continue;
    const sc = lerp(from, 1, E.outExpo(pk));
    X.save(); X.translate(cx, cy); X.scale(sc, sc); X.translate(-cx, -cy);
    txt(s, x, y, { ...o, a: k ? 0.1 : cl(pk * 4) }); X.restore();
  }
}
function ring(cx, cy, t, t0, dur, maxR, col, lw = 3) {
  const p = P(t, t0, t0 + dur); if (p <= 0 || p >= 1) return;
  X.save(); X.globalAlpha = (1 - p) * 0.9; X.strokeStyle = col; X.lineWidth = lw * (1 - p) + 0.5;
  X.beginPath(); X.arc(cx, cy, E.outExpo(p) * maxR, 0, Math.PI * 2); X.stroke(); X.restore();
}
function grid(p, a, drift = 0, step = 80, col) {
  if (p <= 0) return;
  const w = W(), h = H(), cx = w / 2, cy = h / 2;
  X.save(); X.strokeStyle = col || REEL_().brand.accent2 || '#888'; X.lineWidth = 1;
  const nx = Math.ceil(w / step / 2) + 1, ny = Math.ceil(h / step / 2) + 1;
  for (let k = -nx; k <= nx; k++) { const q = cl(p * 1.8 - Math.abs(k) * 0.05); if (q <= 0) continue;
    const L = h * E.outExpo(q) / 2, x = cx + k * step + (drift % step); X.globalAlpha = a * (1 - Math.abs(k) / (nx + 2));
    X.beginPath(); X.moveTo(x, cy - L); X.lineTo(x, cy + L); X.stroke(); }
  for (let k = -ny; k <= ny; k++) { const q = cl(p * 1.8 - Math.abs(k) * 0.07); if (q <= 0) continue;
    const L = w * E.outExpo(q) / 2, y = cy + k * step; X.globalAlpha = a * (1 - Math.abs(k) / (ny + 2));
    X.beginPath(); X.moveTo(cx - L, y); X.lineTo(cx + L, y); X.stroke(); }
  X.restore();
}
// streaks converging on (cx,cy) — energy before an assembly
function swarm(t, cx, cy, t0, span, n = 150, cols) {
  X.save();
  for (let i = 0; i < n; i++) {
    const ts = t0 + rr(i, 1) * span, p = P(t, ts, ts + 0.55); if (p <= 0 || p >= 1) continue;
    const ang = rr(i, 2) * Math.PI * 2, R0 = 1300, R1 = 40 + rr(i, 3) * 220;
    const at = (tt) => { const e = E.inCubic(P(tt, ts, ts + 0.55)), r = lerp(R0, R1, e), a = ang + (1 - e) * 0.9; return [cx + Math.cos(a) * r, cy + Math.sin(a) * r * 0.7]; };
    const [x, y] = at(t), [xp, yp] = at(t - 0.04);
    X.globalAlpha = Math.sin(Math.PI * p) * 0.8; X.strokeStyle = (cols || ['#fff'])[Math.floor(rr(i, 4) * (cols || ['#fff']).length)];
    X.lineWidth = 1 + rr(i, 5) * 2.5; X.beginPath(); X.moveTo(xp, yp); X.lineTo(x, y); X.stroke();
  }
  X.restore();
}
function camera(scale, fn, cx, cy, rot = 0) {
  cx = cx ?? W() / 2; cy = cy ?? H() / 2;
  X.save(); X.translate(cx, cy); X.scale(scale, scale); if (rot) X.rotate(rot); X.translate(-cx, -cy); fn(); X.restore();
}
function panel(x, y, w, h, r = 18, fill = '#151515', stroke = 'rgba(255,255,255,0.10)') {
  X.save(); X.fillStyle = fill; X.strokeStyle = stroke; X.lineWidth = 1.5; X.beginPath(); X.roundRect(x, y, w, h, r); X.fill(); X.stroke(); X.restore();
}
function windowChrome(x, y, w, h, title, o = {}) {
  panel(x, y, w, h, 16, o.fill || '#161616');
  X.fillStyle = o.bar || '#1d1d1d'; X.beginPath(); X.roundRect(x, y, w, 46, [16, 16, 0, 0]); X.fill();
  (o.dots || ['#FF5F57', '#FEBC2E', '#28C840']).forEach((c, i) => { X.fillStyle = c; X.beginPath(); X.arc(x + 24 + i * 22, y + 23, 6, 0, 7); X.fill(); });
  if (title) txt(title, x + w - 24, y + 29, { f: 'Mono', s: 15, ls: 4, al: 'right', c: o.titleCol || '#aaa', w: 600 });
}
// assemble a block logo from a grid [[gx,gy,isAccent],...]; origin = {x,y} or 'radial'
function assembleBlocks(t, blocks, ox, oy, u, o = {}) {
  const t0 = o.t0 ?? 0, dur = o.dur ?? 0.5, stag = o.stagger ?? 0.012, col = o.color || '#fff', acc = o.accent || REEL_().brand.accent;
  const from = o.from; // {x,y} → burst from a point; undefined → fly in from random far positions
  const sweepX = o.sweep ?? -1e9;
  blocks.forEach((b, i) => {
    const tx = ox + (b[0] + 0.5) * u, ty = oy + (b[1] + 0.5) * u, s = u * 0.85;
    const ts = t0 + (o.order === 'distance' ? Math.hypot(b[0] - (o.cx ?? 0), b[1] - (o.cy ?? 0)) * 0.05 : b[0] * stag) + rr(i, 1) * 0.05;
    const spec = Math.exp(-Math.pow(tx - sweepX + (ty - oy) * 0.8, 2) / (2 * 55 * 55));
    const land = 1 - P(t, ts + 0.1, ts + 0.45);
    for (let k = 3; k >= 0; k--) {
      const p = P(t - k * 0.016, ts, ts + dur); if (p <= 0) continue;
      const e = E.outExpo(p);
      let sx, sy;
      if (from) { sx = from.x; sy = from.y; } else { const a = rr(i, 2) * Math.PI * 2, r = 1000 + rr(i, 3) * 600; sx = tx + Math.cos(a) * r; sy = ty + Math.sin(a) * r; }
      const x = lerp(sx, tx, e), y = lerp(sy, ty, e) + (from ? Math.sin(Math.PI * e) * (rr(i, 2) - 0.5) * 520 : 0);
      const c = b[2] ? acc : (k ? rgba(col, 1) : mix(col, '#FFFFFF', land + spec * 0.9));
      blk(x, y, s * E.outBack(P(t - k * 0.016, ts, ts + 0.35)), c, (1 - e) * (rr(i, 3) - 0.5) * 5, k ? 0.14 : 1, k ? 0 : (b[2] ? 30 : land * 22 + spec * 20));
    }
  });
}

// ---------- global FX ----------
function shake(t) {
  let x = 0, y = 0;
  for (const [h, s] of REEL_().hits || []) { const d = t - h; if (d < 0 || d > 0.5) continue; const k = s * Math.exp(-d * 11);
    x += k * Math.sin(d * 93 + h * 7); y += k * Math.cos(d * 71 + h * 3); }
  return [x, y];
}
function flashes(t) {
  for (const [h, a, c] of REEL_().flashes || []) { const d = t - h; if (d < 0 || d > 0.4) continue;
    X.save(); X.globalAlpha = a * Math.exp(-d * 14); X.fillStyle = c; X.globalCompositeOperation = 'lighter'; X.fillRect(0, 0, W(), H()); X.restore(); }
}
function glitchAmount(t) {
  // each glitch ramps up [a → cut], decays [cut → b]; strictly windowed so it can't leak into later scenes
  let g = 0;
  for (const [a, cut, b, s] of REEL_().glitches || []) {
    if (win(t, a, cut)) g += E.inExpo(P(t, a, cut)) * s;
    if (win(t, cut, b)) g += (1 - P(t, cut, b)) * s * 0.6;
  }
  return g;
}
function sliceGlitch(k, t) {
  if (k <= 0.001) return;
  const w = W(), h = H();
  BX.clearRect(0, 0, w, h); BX.drawImage(C, 0, 0);
  X.save(); X.setTransform(1, 0, 0, 1, 0, 0);
  if (REEL_().transparent) X.clearRect(0, 0, w, h); else { X.fillStyle = REEL_().brand.bg; X.fillRect(0, 0, w, h); }
  const fr = Math.floor(t * 60); let y = 0, i = 0;
  while (y < h) { const sh = 10 + rr(fr * 3 + i, 1) * 90, dx = (rr(fr + i, 2) - 0.5) * 2 * k * 420 * (rr(i + fr, 3) > 0.45 ? 1 : 0.1);
    X.drawImage(B, 0, y, w, sh, dx, y, w, sh); y += sh; i++; }
  X.globalCompositeOperation = 'lighter'; X.globalAlpha = 0.35 * k; X.drawImage(B, 14 * k, 0); X.drawImage(B, -14 * k, 0);
  X.restore();
}
function blockWipe(t, tc, col, dur = 0.3, cs = 80) {
  if (t < tc - dur || t > tc + dur) return;
  const cols = Math.ceil(W() / cs), rows = Math.ceil(H() / cs);
  X.save(); X.fillStyle = col;
  for (let r = 0; r < rows; r++) for (let c = 0; c < cols; c++) {
    const d = ((c + r * 0.6) / (cols + rows * 0.6)) * dur * 0.55;
    const s = t < tc ? E.outCubic(P(t, tc - dur + d, tc - dur + d + dur * 0.45)) : 1 - E.inCubic(P(t, tc + d, tc + d + dur * 0.45));
    if (s <= 0) continue; const z = cs * s * 1.02;
    X.beginPath(); X.roundRect(c * cs + cs / 2 - z / 2, r * cs + cs / 2 - z / 2, z, z, z * 0.12 * (1 - s)); X.fill();
  }
  X.restore();
}
// vertical bar sweep used for "push" cuts between sibling panels
function barWipe(t, tc, cols, dur = 0.36) {
  const wp = P(t, tc - dur / 2, tc + dur / 2); if (wp <= 0 || wp >= 1) return;
  const bx = lerp(W() + 100, -300, E.ioExpo(wp));
  X.fillStyle = cols[0]; X.fillRect(bx, 0, 70, H()); if (cols[1]) { X.fillStyle = cols[1]; X.fillRect(bx + 90, 0, 16, H()); }
}
function hud(t) {
  const R = REEL_(), hd = R.hud; if (!hd || hd.show === false) return;
  const a = E.outCubic(P(t, 0.35, 0.8)) * 0.9; if (a <= 0) return;
  const w = W(), h = H(), m = 44, l = 30, fg = R.brand.fg, acc = R.brand.accent;
  X.save(); X.globalAlpha = a; X.strokeStyle = rgba(fg, 0.45); X.lineWidth = 2;
  [[m, m, 1, 1], [w - m, m, -1, 1], [m, h - m, 1, -1], [w - m, h - m, -1, -1]].forEach(([x, y, dx, dy]) => { X.beginPath(); X.moveTo(x, y + dy * l); X.lineTo(x, y); X.lineTo(x + dx * l, y); X.stroke(); });
  blk(86, 80, 12, acc);
  txt(hd.label || '', 104, 86, { f: 'Mono', s: 15, ls: 4, c: fg, a: 0.75 });
  if (Math.floor(t * 2) % 2 === 0) { X.fillStyle = acc; X.beginPath(); X.arc(w - 300, 80, 5, 0, 7); X.fill(); }
  txt(`TC 00:00:${pad(Math.floor(t))}:${pad(Math.floor(t * 30) % 30)}`, w - 86, 86, { f: 'Mono', s: 15, ls: 3, al: 'right', c: fg, a: 0.75 });
  const sc = R.scenes; let si = 0; sc.forEach((s, k) => { if (t >= s.start) si = k; });
  txt(`${pad(si + 1)} / ${pad(sc.length)}`, 86, h - 76, { f: 'Mono', s: 15, ls: 3, c: R.brand.accent2 || acc });
  txt((sc[si].name || '').toUpperCase(), 196, h - 76, { f: 'Mono', s: 15, ls: 4, c: fg, a: 0.6 });
  X.restore();
  if (hd.progress !== false) { X.save(); X.globalAlpha = a * 0.25; X.fillStyle = fg; X.fillRect(w - 386, h - 82, 300, 2);
    X.globalAlpha = a * 0.9; X.fillStyle = acc; X.fillRect(w - 386, h - 82, 300 * cl(t / R.duration), 2); X.restore(); }
}
function post(t) {
  X.save(); X.drawImage(VIG, 0, 0);
  X.globalAlpha = REEL_().grain ?? 0.07; X.globalCompositeOperation = 'overlay'; X.drawImage(GRAIN[Math.floor(t * 60) % GRAIN.length], 0, 0, W(), H());
  X.restore();
}

// ---------- frame ----------
function renderFrame(t) {
  const R = REEL_(), w = W(), h = H();
  X.setTransform(1, 0, 0, 1, 0, 0); X.globalAlpha = 1; X.globalCompositeOperation = 'source-over';
  if (R.transparent) X.clearRect(0, 0, w, h); else { X.fillStyle = R.brand.bg; X.fillRect(0, 0, w, h); }
  const [sx, sy] = shake(t);
  X.save(); X.translate(sx, sy);
  const sc = R.scenes.find(s => t >= s.start && t < s.end) || R.scenes[R.scenes.length - 1];
  sc.draw(t, t - sc.start);
  X.restore();
  sliceGlitch(glitchAmount(t), t);
  for (const [tc, col, dur] of R.wipes || []) blockWipe(t, tc, col, dur);
  if (R.overlay) R.overlay(t);
  if (!R.transparent) { flashes(t); post(t); }
  hud(t);
  const fo = P(t, R.duration - (R.fadeOut ?? 0.28), R.duration), fi = 1 - P(t, 0, R.fadeIn ?? 0.08), f = Math.max(E.inCubic(fo), fi);
  if (f > 0) {
    // transparent reels fade by erasing alpha, opaque reels fade to the background color
    X.save(); if (R.transparent) { X.globalCompositeOperation = 'destination-out'; X.fillStyle = `rgba(0,0,0,${f})`; }
    else X.fillStyle = rgba(R.brand.bg, f);
    X.fillRect(0, 0, w, h); X.restore();
  }
}

function initEngine() {
  const R = REEL_();
  C = document.getElementById('c'); C.width = R.w; C.height = R.h; X = C.getContext('2d');
  B = document.createElement('canvas'); B.width = R.w; B.height = R.h; BX = B.getContext('2d');
  GRAIN = Array.from({ length: 6 }, () => {
    const c = document.createElement('canvas'); c.width = R.w / 2; c.height = R.h / 2;
    const cx = c.getContext('2d'), id = cx.createImageData(c.width, c.height);
    for (let i = 0; i < id.data.length; i += 4) { const v = rr(i >> 2, 9) * 255; id.data[i] = id.data[i + 1] = id.data[i + 2] = v; id.data[i + 3] = 255; }
    cx.putImageData(id, 0, 0); return c;
  });
  VIG = document.createElement('canvas'); VIG.width = R.w; VIG.height = R.h;
  const v = VIG.getContext('2d'), g = v.createRadialGradient(R.w / 2, R.h / 2, R.h * 0.35, R.w / 2, R.h / 2, Math.max(R.w, R.h) * 0.6);
  g.addColorStop(0, 'rgba(0,0,0,0)'); g.addColorStop(1, `rgba(0,0,0,${R.vignette ?? 0.62})`); v.fillStyle = g; v.fillRect(0, 0, R.w, R.h);
  // images come from assets.js as data URIs (file:// images would taint the canvas and break toDataURL)
  const loads = Object.entries(window.ASSETS || {}).map(([k, src]) => new Promise(r => { const im = new Image(); im.onload = im.onerror = r; im.src = src; IMG[k] = im; }));
  const fonts = (R.fonts || ['800 100px Display', '400 30px Mono']).map(f => document.fonts.load(f));
  return Promise.all([...loads, ...fonts]).then(() => true);
}

window.addEventListener('DOMContentLoaded', () => {
  window.ready = initEngine();
  window.renderFrame = renderFrame;
  if (!location.hash.includes('render')) window.ready.then(() => {
    const st = performance.now(), D = REEL_().duration;
    const loop = () => { renderFrame(((performance.now() - st) / 1000) % D); requestAnimationFrame(loop); }; loop();
  });
});
