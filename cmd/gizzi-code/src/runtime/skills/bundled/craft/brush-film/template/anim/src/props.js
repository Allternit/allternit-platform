// props.js: shared sets and props for this film. Chapters treat it as read-only; extend it before chapters start, then
// check new pieces on a model sheet (?loop=props). Everything is painted with paint()/inkLine()/glow(), never plain p5.
// A small film palette on top of PAL (replace per film; keep 5-7 colours and a light/colour arc across the film):
const FP = { dusk: '#F2C9A8', duskTop: '#B9A6D8', hill: '#8DB872', hillDk: '#5E8F57', hillFar: '#B7CDB0', sun: '#F6D27A',
  balloon: '#E0573F', balloonDk: '#A83A2C', string: '#5A4A44' };
// Display text (title cards, genre supers): one chunky face, only where the genre needs words. window.LOAD runs at setup.
function superText(txt, x, y, size, col, o = {}) { letter(txt, x, y, size, col, { font: `${size}px "Luckiest Guy", "Permanent Marker", cursive`, stroke: o.stroke ?? PAL.ink, ...o }); }
window.LOAD = async () => { await document.fonts.load('60px "Luckiest Guy"'); };

// ---------- MEADOW: sky wash, a low sun, far and near hills with swaying grass. o.warm 0..1 shifts the light warmer ----------
function meadow(t, o = {}) {
  const warm = clamp(o.warm ?? 0), gy = o.groundY ?? 830;
  boilSeed('meadow sky'); paint(rectPts(-400, -400, W + 800, gy + 500), { wash: mixCol(FP.duskTop, FP.dusk, .35 + .4 * warm), fill: FP.dusk, fillOp: 70, bleed: .25, tex: .5, ink: null });
  boilSeed('meadow sun'); glow(o.sunX ?? 1380, o.sunY ?? 330, 380, '#FFD58A', .55 + .35 * warm);
  paint(ellPts(o.sunX ?? 1380, o.sunY ?? 330, 92, 92, 28, 1.5), { wash: mixCol(FP.sun, '#FFF1C8', warm * .5), ink: null });
  boilSeed('meadow far'); paint(through([[-400, gy - 60], [200, gy - 230], [700, gy - 150], [1200, gy - 260], [1700, gy - 140], [2300, gy - 200], [2300, gy + 200], [-400, gy + 200]], 4), { wash: FP.hillFar, fill: FP.hillDk, fillOp: 25, bleed: .2, tex: .4, ink: null });
  boilSeed('meadow near'); paint(through([[-400, gy + 10], [300, gy - 70], [960, gy - 10], [1500, gy - 80], [2300, gy - 20], [2300, gy + 600], [-400, gy + 600]], 4), { wash: FP.hill, fill: FP.hillDk, fillOp: 55, bleed: .15, tex: .6, ink: PAL.ink, sw: .9 });
  for (let i = 0; i < 26; i++) {   // grass tufts: stable positions (hash), sway on the beat
    boilSeed('tuft ' + i); const x = -200 + hash(i) * (W + 400), y = gy + 30 + hash(i + 40) * 220, h = 18 + hash(i + 80) * 26, sw = Math.sin(t * 1.6 + i) * 6;
    [-1, 0, 1].forEach(k => inkLine([[x + k * 7, y], [x + k * 9 + sw * .5, y - h * .6], [x + k * 12 + sw, y - h]], .7, FP.hillDk, 'inkfine', .5));
  }
}

// ---------- BALLOON: hangs from (sx, sy) (a hand, or a free end) and floats at (x, y). sway = extra lean ----------
function balloon(x, y, r, sx, sy, o = {}) {
  boilSeed('balloon ' + (o.key || '')); const col = o.col || FP.balloon;
  const mid = [(x + sx) / 2 + (o.sway || 0) * 40 + Math.sin(T * 2.3) * 10, (y + r + sy) / 2];
  inkLine([[x, y + r * 1.15], mid, [sx, sy]], .7, FP.string, 'inkfine', .6);
  const p = []; for (let i = 0; i < 26; i++) { const a = i / 26 * TAU, s = Math.sin(a); p.push([x + Math.cos(a) * r * (1 - .12 * Math.max(0, s)), y + s * r * 1.12]); }
  paint(p, { wash: col, fill: o.dk || FP.balloonDk, fillOp: 60, bleed: .1, tex: .5, border: .6, ink: PAL.ink, sw: 1, curv: .5 });
  paint([[x - r * .14, y + r * 1.08], [x + r * .14, y + r * 1.08], [x, y + r * 1.28]], { wash: o.dk || FP.balloonDk, ink: PAL.ink, sw: .6 });
  paint(ellPts(x - r * .38, y - r * .45, r * .16, r * .28, 12, 0, -.5), { wash: mixCol(col, PAL.cream, .7), ink: null });
}
