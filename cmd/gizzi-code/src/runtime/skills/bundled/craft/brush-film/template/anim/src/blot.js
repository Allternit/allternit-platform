// blot.js: Blot, the starter character. A round ink-blob with big eyes, noodle arms, two little feet and an ink drip on
// top, painted with p5.brush like everything else. It is a starting point: recolour it, add a hat, change its shape, or
// write your own rig in the same style (see "Write your own character" in ANIMATION_GUIDE.md).
//
//   blot(x, y, u, o) -> { head, top, handL, handR, mouth }    (x, y) = ground point between the feet; u = size unit.
//   Blot is about 8u wide and 8u tall (9.5u with the drip). Medium shot: u 22-30. Close-up: u 45-70. Wide: u 8-14.
//   The returned points are WORLD coordinates (same space you passed x, y in): use them to attach props, aim an iris or
//   hang a balloon string from a hand.
//
// Options (all optional; spread feel()/emotions() first, then your pose overrides):
//   pose    dx, dy (in u, -dy = up), sq (squash > 0, stretch < 0), rot (radians, pivots at the feet), flip,
//           aL, aR (arm angle: 0 = straight out, + = up, - = down, about +-1.5 = vertical), walk (foot phase),
//           lift (in u: the body rises but the shadow stays), noShadow
//   face    eyes: 'open' | 'wide' | 'happy' | 'closed' | 'half'; lookX, lookY (-1..1); blink 0..1; pupil (scale)
//           brow (-1 worried/sad .. +1 angry; undefined = no brows); mouth: 'smile' | 'grin' | 'o' | 'flat' | 'frown' |
//           'wavy' | 'open'; talk 0..1 (opens the mouth; drive it with talk(t, cues)); blush 0..1
//   colour  col (body hex), belly (hex), gray 0..1 (desaturate)
//   extras  emote: '!' | '?' | 'sweat' | 'heart' | 'spark' | 'zzz' with emoteK 0..1 (pop-in) ;
//           holdL / holdR: (x, y, u, sw) => {...} paints a prop at that hand, in world coordinates
//   boil    boilKey: a stable id for its boil seeds (default: call order; set it when characters come and go mid-shot)
const BLOT = { col: '#4F79C4', belly: '#86A9E0', eye: '#FFF8EC', mouth: '#3A1A2A', tongue: '#E27A92', blush: '#EE8FA0',
  emote: { '!': '#E0573F', '?': '#E8AA38', sweat: '#8EC3E6', heart: '#E2566F', spark: '#F0BE46', zzz: '#7B5CA8' } };

function blot(x, y, u, o = {}) {
  const id = o.boilKey ?? 'blot' + (++RIG_N), rs = p => boilSeed(`${id} ${p}`), sw = clamp(u / 16, .35, 2.4);
  const g = o.gray || 0, C = c => g > 0 ? mixCol(c, '#9A968F', g) : c;
  const col = C(o.col || BLOT.col), dk = mixCol(col, PAL.ink, .35), belly = C(o.belly || mixCol(col, PAL.cream, .38));
  const fx = o.flip ? -1 : 1, sq = o.sq || 0, rot = o.rot || 0, X = x + (o.dx || 0) * u, Y = y + (o.dy || 0) * u, lift = (o.lift || 0) * u;
  // local (px, y up = negative) -> world, mirroring the transforms below
  const toW = (lx, ly) => { const sx = lx * fx * (1 + sq * .5), sy = ly * (1 - sq) - lift, c = Math.cos(rot), s = Math.sin(rot); return [X + sx * c - sy * s, Y + sx * s + sy * c]; };
  const U = v => v * u, P = pts => pts.map(([a, b]) => [a * u, b * u]);

  if (!o.noShadow) { rs('shadow'); paint(ellPts(X, y + .15 * u, (3.4 - Math.min(lift / u, 3) * .4) * u, .55 * u, 22), { fill: PAL.ink, fillOp: 70, bleed: .2, tex: .3, border: .1, ink: null }); }

  push(); translate(X, Y); rotate(rot); scale(fx * (1 + sq * .5), 1 - sq); translate(0, -lift);
  const bodyC = [0, -4.1], RX = 4, RY = 3.6;
  // arms: behind the body, so the shoulder roots tuck under it
  const hands = {};
  [[-1, o.aL ?? -.7], [1, o.aR ?? -.7]].forEach(([side, a]) => {
    rs('arm' + side);
    const sh = [side * 3.3, -4.2], ca = Math.cos(a), sa = Math.sin(a), bend = .35 * side;
    const el = [sh[0] + side * 1.5 * ca, sh[1] - 1.5 * sa + .25], hd = [sh[0] + side * 2.9 * Math.cos(a + bend * .25), sh[1] - 2.9 * Math.sin(a + bend * .25 * side)];
    paint(ribbon(P([sh, el, hd]), .95 * u, .6 * u), { wash: col, ink: PAL.ink, sw: sw * .85 });
    rs('hand' + side); paint(ellPts(U(hd[0]), U(hd[1]), .5 * u, .5 * u, 14, .02 * u), { wash: col, ink: PAL.ink, sw: sw * .85 });
    hands[side] = hd;
  });
  // feet
  [-1, 1].forEach(side => {
    rs('foot' + side);
    const ph = (o.walk || 0) * Math.PI + (side > 0 ? Math.PI : 0), up = o.walk ? Math.max(0, Math.sin(ph)) * .55 : 0, fwd = o.walk ? Math.cos(ph) * .35 : 0;
    paint(ellPts(U(side * 1.6 + fwd), U(-.45 - up), .95 * u, .5 * u, 16, .02 * u), { wash: dk, ink: PAL.ink, sw: sw * .85 });
  });
  // body: one round blob, a touch wider at the bottom, with watercolour shading and a pale belly
  rs('body');
  const bp = []; for (let i = 0; i < 30; i++) { const a = i / 30 * TAU, s = Math.sin(a); bp.push([Math.cos(a) * RX * (1 + .06 * s), bodyC[1] + s * RY * (s > 0 ? .92 : 1)]); }
  paint(P(bp), { wash: col, fill: dk, fillOp: 45, bleed: .12, tex: .5, border: .5, ink: PAL.ink, sw: sw * 1.1, curv: .5 });
  rs('belly'); paint(ellPts(U(.15), U(-2.6), 2.3 * u, 1.35 * u, 20, .03 * u), { wash: belly, ink: null });
  // the ink drip on top: one ribbon curl
  rs('drip'); paint(ribbon(P([[-.3, -7.5], [.15, -8.3], [.75, -8.6], [1.05, -8.15]]), .8 * u, .25 * u), { wash: col, ink: PAL.ink, sw: sw * .8 });
  // face
  const lx = clamp(o.lookX || 0, -1, 1) * .55, ly = clamp(o.lookY || 0, -1, 1) * .45, ex = 1.35, ey = -5.05 + ly * .4;
  const eyes = o.eyes || 'open', blink = clamp(o.blink || 0), wide = eyes === 'wide' ? 1.18 : 1;
  [-1, 1].forEach(side => {
    rs('eye' + side); const cx = U(side * ex + lx * .5), cy = U(ey);
    if (eyes === 'happy') inkLine([[cx - .7 * u, cy + .25 * u], [cx, cy - .45 * u], [cx + .7 * u, cy + .25 * u]], sw * 1.2, PAL.ink, 'ink', .6);
    else if (eyes === 'closed' || blink > .85) inkLine([[cx - .7 * u, cy - .05 * u], [cx, cy + .3 * u], [cx + .7 * u, cy - .05 * u]], sw * 1.1, PAL.ink, 'ink', .6);
    else {
      const half = eyes === 'half' ? .5 : 1, ry = 1.05 * wide * (1 - blink * .85) * (half < 1 ? .62 : 1), rx = .85 * wide;
      const ecy = cy + (half < 1 ? .35 * u : 0);
      paint(ellPts(cx, ecy, rx * u, ry * u, 20, .015 * u), { wash: BLOT.eye, ink: PAL.ink, sw: sw * .9 });
      const pr = .42 * (o.pupil ?? 1) * (eyes === 'wide' ? .8 : 1), px = cx + lx * .5 * u, py = ecy + ly * .5 * u + (half < 1 ? .1 * u : 0);
      rs('pupil' + side); paint(ellPts(px, py, pr * u, Math.min(pr, ry * .8) * u, 12), { wash: PAL.ink, ink: null });
      paint(ellPts(px + .14 * u, py - .16 * u, .12 * u, .12 * u, 8), { wash: BLOT.eye, ink: null });
      if (half < 1) inkLine([[cx - rx * u, ecy - ry * .55 * u], [cx + rx * u, ecy - ry * .55 * u]], sw, PAL.ink, 'ink', 0);
    }
    if (o.brow != null) { rs('brow' + side); const b = clamp(o.brow, -1, 1), bx = cx, by = cy - 1.45 * u * wide;
      inkLine([[bx - side * .55 * u, by + b * .32 * u], [bx + side * .55 * u, by - b * .22 * u]], sw * 1.3, PAL.ink, 'ink', .3); }
    if ((o.blush || 0) > .02) { rs('blush' + side); paint(ellPts(U(side * 2.45), U(-3.75), .65 * u, .32 * u, 14), { fill: BLOT.blush, fillOp: 150 * clamp(o.blush), bleed: .25, tex: .3, ink: null }); }
  });
  rs('mouth'); const mo = blotMouth(U(lx * .4), U(-3.45 + ly * .3), u, o.mouth || 'smile', clamp(o.talk || 0), sw);
  pop();
  boilSeed('after ' + id);

  const out = { head: toW(0, U(-4.6)), top: toW(U(.6), U(-8.8)), handL: toW(U(hands[-1][0]), U(hands[-1][1])), handR: toW(U(hands[1][0]), U(hands[1][1])), mouth: toW(...mo) };
  if (o.holdL) o.holdL(...out.handL, u, sw);
  if (o.holdR) o.holdR(...out.handR, u, sw);
  if (o.emote && (o.emoteK ?? 1) > .02) emote(o.emote, out.top[0] + 1.6 * u, out.top[1] - .4 * u, u, o.emoteK ?? 1, id);
  return out;
}

function blotMouth(x, y, u, m, talk, sw) {
  const w = 1.7 * u;
  if (talk > .08 || m === 'open' || m === 'grin') {
    const op = m === 'grin' ? Math.max(.55, talk) : Math.max(talk, m === 'open' ? .6 : 0);
    const pts = [[x - w / 2, y - .1 * u], [x + w / 2, y - .1 * u], [x + w * .3, y + w * .5 * op], [x, y + w * .62 * op], [x - w * .3, y + w * .5 * op]];
    paint(pts, { wash: BLOT.mouth, ink: PAL.ink, sw: sw * .9, curv: .5 });
    if (op > .35) paint(ellPts(x + .1 * u, y + w * .42 * op, w * .22, w * .12 * op + .05 * u, 12), { wash: BLOT.tongue, ink: null });
  } else if (m === 'o') paint(ellPts(x, y + .2 * u, .36 * u, .48 * u, 14, .02 * u), { wash: BLOT.mouth, ink: PAL.ink, sw: sw * .8 });
  else if (m === 'flat') inkLine([[x - w * .32, y + .1 * u], [x + w * .32, y + .1 * u]], sw * 1.2, PAL.ink, 'ink', 0);
  else if (m === 'frown') inkLine([[x - w * .38, y + .45 * u], [x, y + .05 * u], [x + w * .38, y + .45 * u]], sw * 1.2, PAL.ink, 'ink', .6);
  else if (m === 'wavy') inkLine([[x - w * .42, y + .2 * u], [x - w * .2, y], [x, y + .25 * u], [x + w * .2, y], [x + w * .42, y + .2 * u]], sw * 1.1, PAL.ink, 'ink', .5);
  else inkLine([[x - w * .42, y], [x, y + .5 * u], [x + w * .42, y]], sw * 1.2, PAL.ink, 'ink', .6);   // smile
  return [x, y];
}

// ---------- emotes: painted marks, never letters ----------
function heartPts(cx, cy, r, n = 24) {
  const p = []; for (let i = 0; i < n; i++) { const a = i / n * TAU; p.push([cx + r * .062 * 16 * Math.pow(Math.sin(a), 3), cy - r * .062 * (13 * Math.cos(a) - 5 * Math.cos(2 * a) - 2 * Math.cos(3 * a) - Math.cos(4 * a))]); }
  return p;
}
function emote(kind, x, y, u, k = 1, key = 'emote') {
  const s = backOut(clamp(k)) * u, c = BLOT.emote[kind] || PAL.ink, sw = clamp(u / 16, .35, 2.4); if (s < .05) return;
  boilSeed(key + ' emote ' + kind); const bob = Math.sin(T * 5) * .12 * s;
  if (kind === '!') { paint(ribbon([[x, y - 2.2 * s + bob], [x + .05 * s, y - 1.1 * s + bob], [x, y - .1 * s + bob]], .9 * s, .35 * s), { wash: c, ink: PAL.ink, sw });
    paint(ellPts(x, y + .7 * s + bob, .32 * s, .32 * s, 12), { wash: c, ink: PAL.ink, sw }); }
  else if (kind === '?') { paint(ribbon(through([[x - .8 * s, y - 1.4 * s], [x - .2 * s, y - 2.2 * s], [x + .7 * s, y - 1.6 * s], [x + .2 * s, y - .7 * s], [x, y - .1 * s]].map(([a, b]) => [a, b + bob])), .6 * s, .35 * s), { wash: c, ink: PAL.ink, sw });
    paint(ellPts(x, y + .6 * s + bob, .28 * s, .28 * s, 12), { wash: c, ink: PAL.ink, sw }); }
  else if (kind === 'sweat') { const dy = (T * 1.5 % 1) * .8 * s, cx = x - 4.2 * s, cy = y + 3 * s + dy, p = [[cx, cy - 1.05 * s]];
    for (let i = 0; i <= 12; i++) { const a = -Math.PI / 6 + i / 12 * (Math.PI * 4 / 3); p.push([cx + Math.cos(a) * .5 * s, cy + Math.sin(a) * .5 * s]); }
    paint(p, { wash: c, ink: PAL.ink, sw: sw * .8, curv: .4 }); }
  else if (kind === 'heart') { [[0, 0, 1], [1.3, -1.2, .55]].forEach(([ox, oy, r], i) => { const b = Math.sin(T * 6 + i) * .08; paint(heartPts(x + ox * s, y + oy * s + bob, (1 + b) * r * s), { wash: c, ink: PAL.ink, sw }); }); }
  else if (kind === 'spark') { [[0, 0, 1.1], [1.4, .8, .55], [-1.1, .9, .45]].forEach(([ox, oy, r], i) => { const tw = .8 + .2 * Math.sin(T * 9 + i * 2); paint(starPts(x + ox * s, y + oy * s + bob, r * tw * s, .36, 4), { wash: c, ink: PAL.ink, sw: sw * .8 }); }); }
  else if (kind === 'zzz') { [0, 1, 2].forEach(i => { const ph = (T * .6 + i / 3) % 1, zs = (.35 + .3 * ph) * s, zx = x + ph * 2.4 * s, zy = y - ph * 3.4 * s; if (ph > .9) return;
    inkLine([[zx - zs, zy - zs], [zx + zs, zy - zs], [zx - zs, zy + zs], [zx + zs, zy + zs]], sw * 1.2, c, 'ink', 0); }); }
}

// ---------- emotions: a face, a colour and a way of moving, locked to the beat ----------
const _bt = t => { const b = bpOf(t); return { b, ab: Math.abs(Math.sin(b * Math.PI)), s2: Math.sin(b * TAU), hit: pulse(t) }; };
const EMO = {
  neutral:   t => { const B = _bt(t); return { eyes: 'open', mouth: 'smile', dy: -.1 * B.ab, sq: .02 * B.s2, aL: -.8, aR: -.8 }; },
  happy:     t => { const B = _bt(t); return { eyes: 'happy', mouth: 'smile', blush: .35, dy: -.35 * B.ab, sq: -.04 * B.ab, aL: -.3 + .25 * B.ab, aR: -.3 + .25 * B.ab }; },
  excited:   t => { const B = _bt(t * 2); return { eyes: 'wide', mouth: 'grin', blush: .4, dy: -.9 * B.ab, sq: .1 * (1 - B.ab) - .06, aL: 1.1 + .25 * B.s2, aR: 1.1 - .25 * B.s2 }; },
  sad:       t => ({ eyes: 'open', lookY: .7, brow: -.85, mouth: 'frown', dy: .05, sq: .07 + .015 * wob(t, .4), aL: -1.3, aR: -1.25, gray: .3, rot: .03 * wob(t, .25) }),
  surprised: t => ({ eyes: 'wide', brow: -.25, mouth: 'o', aL: .55, aR: .55, emote: '!', sq: -.05 }),
  angry:     t => { const j = (hash(Math.floor(t * 18)) - .5) * .08; return { eyes: 'open', lookY: .15, brow: 1, mouth: 'frown', col: mixCol(BLOT.col, '#D0453A', .45), dx: j, sq: .05, aL: -1.05, aR: -1.05 }; },
  worried:   t => ({ eyes: 'open', lookX: .7 * wob(t, .5), brow: -.9, mouth: 'wavy', emote: 'sweat', aL: -.2, aR: -1.2, sq: .03 * wob(t, 1.2) }),
  sleepy:    t => ({ eyes: 'closed', mouth: 'flat', emote: 'zzz', sq: .06 + .05 * wob(t, .3), aL: -1.3, aR: -1.3, rot: .05 * wob(t, .3) }),
  love:      t => { const B = _bt(t); return { eyes: 'happy', mouth: 'smile', blush: 1, emote: 'heart', rot: .06 * wob(t, .5), dy: -.2 * B.ab, aL: .1, aR: .1 }; },
  smug:      t => ({ eyes: 'half', lookX: .6, brow: .35, mouth: 'smile', aL: -1.4, aR: -.3, rot: -.04 }),
  scared:    t => { const j = (hash(Math.floor(t * 24) + 7) - .5) * .12; return { eyes: 'wide', pupil: .6, brow: -1, mouth: 'wavy', dx: j, sq: .08, aL: 1.3, aR: 1.25, gray: .2, emote: 'sweat' }; },
  idea:      t => ({ eyes: 'wide', lookY: -.6, lookX: .3, mouth: 'smile', aL: -.8, aR: 1.45, emote: 'spark', dy: -.2 }),
  curious:   t => ({ eyes: 'open', lookX: .8, brow: -.4, mouth: 'o', rot: .1, aL: -.6, aR: -.2, emote: '?' }),
};
// feel(name, t, over): an emotion, alive at time t, with overrides spread last.
function feel(name, t, over = {}) { return { ...(EMO[name] || EMO.neutral)(t), ...over }; }
// emotions(t, [[t0, name, over?], [t1, name, over?], ...]): acted mood changes. Each change gets a quick squash
// (anticipation), a blink, a stretch-and-settle take, and the new emote pops in, so faces never snap.
function emotions(t, keys) {
  let i = 0; while (i + 1 < keys.length && t >= keys[i + 1][0]) i++;
  const [t0, name, over = {}] = keys[i], f = feel(name, t, over), next = keys[i + 1];
  if (i > 0) { const tk = take(t, t0, .7); f.sq = (f.sq || 0) + tk.sq; f.dy = (f.dy || 0) + tk.dy; f.blink = Math.max(f.blink || 0, 1 - seg(t, t0, t0 + .1)); f.emoteK = seg(t, t0 + .04, t0 + .3); }
  if (next && t > next[0] - .1) { const a = seg(t, next[0] - .1, next[0]); f.sq = (f.sq || 0) + .12 * ease(a); f.blink = Math.max(f.blink || 0, a); }
  return f;
}
// talk(t, cues): mouth opening 0..1 while a line is spoken. cues = [[start, dur], ...] (e.g. from VO + DUR_VO in film.js).
function talk(t, cues) {
  for (const [a, d] of cues) if (t >= a && t < a + d) { const k = Math.min(seg(t, a, a + .06), 1 - seg(t, a + d - .08, a + d)); return k * (.25 + .75 * Math.abs(Math.sin((t - a) * 11 + hash(Math.floor((t - a) * 7)) * 2))); }
  return 0;
}
