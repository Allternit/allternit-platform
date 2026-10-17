// shorts.js: vertical 9:16 Shorts cut from this 16:9 film, painted at full 1080×1920 (not a crop of the finished video).
// Each entry: { name, start, end, hook, view: [[t, cx, cy, z], ...] } — t in film seconds; (cx, cy) = the design-space point
// to centre (the frame shows ~607 design px across at z = 1, ~980 px at z = 0.62); z > 1 zooms in. Keep cx within ~300..1620.
// FRAMING RULE: wide/staged shots z 0.55-0.7 (default 0.62); close-ups up to ~0.85;
// never above 0.9 unless the shot is a single-character close-up. Render a 6-frame vertical --sheet before encoding.
// Render one: ./shorts.sh <name>   (or ./shorts.sh to render all). Output: exports/shorts/<film>-<name>.mp4
const SHORTS = [
  // { name: 'example', start: 10, end: 35, hook: 'The hook line, big at the top', view: [[10, 960, 540, 0.62], [35, 1100, 560, 0.7]] },
];
(() => {
  const S = SHORTS.find(s => s.name === QS.get('short')); if (!S) return;
  window.FILM_VIEW = t => { const v = kf(t, S.view.map(([tt, cx, cy, z]) => [tt, [cx, cy, z]]), ease); return { cx: v[0], cy: v[1], z: v[2] }; };
  const base = window.FILM_OVERLAY;
  window.FILM_OVERLAY = (c, t) => {
    // hook band at the top
    c.save(); const g = c.createLinearGradient(0, 0, 0, 360); g.addColorStop(0, 'rgba(20,14,10,.78)'); g.addColorStop(1, 'rgba(20,14,10,0)'); c.fillStyle = g; c.fillRect(0, 0, OW, 360);
    c.font = '800 66px "Helvetica Neue", Helvetica, Arial, sans-serif'; c.textAlign = 'center'; c.textBaseline = 'middle'; c.fillStyle = '#FDF8F3';
    wrapText(c, S.hook, OW * .86).forEach((l, i, a) => c.fillText(l, OW / 2, 150 + (i - (a.length - 1) / 2) * 78)); c.restore();
    if (base) base(c, t);
  };
})();
function wrapText(c, txt, maxw) { const out = []; let cur = ''; for (const w of String(txt).split(' ')) { const n = cur ? cur + ' ' + w : w; if (cur && c.measureText(n).width > maxw) { out.push(cur); cur = w; } else cur = n; } if (cur) out.push(cur); return out; }
