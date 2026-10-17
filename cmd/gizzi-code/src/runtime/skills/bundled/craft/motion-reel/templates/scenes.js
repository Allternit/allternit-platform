// Starter reel for a FICTIONAL placeholder brand ("Northwind"). Replace every word, colour and
// shape with the user's real brand material before rendering anything for them.
// Times sit on a 120 BPM grid (0.5 s beats); every cut lands on a beat. 8 s, 4 scenes:
// logo assemble → kinetic type slam → product UI panel → lockup.
const BR = { bg:'#0B1020', fg:'#EEF2FF', accent:'#5B8DEF', accent2:'#7DD3C0', muted:'#8B93A7', panel:'#121A2E' };
const CX = 960, CY = 540;
// 5×5 block "N": [gx, gy, isAccent]
const LOGO = [];
for (let y = 0; y < 5; y++) { LOGO.push([0, y, 0]); LOGO.push([4, y, 0]); }
LOGO.push([1, 1, 0], [2, 2, 1], [3, 3, 0]);

window.REEL = {
  w: 1920, h: 1080, duration: 8, bpm: 120, brand: BR,
  hud: { label: 'NORTHWIND  —  PLACEHOLDER REEL', show: true },
  fonts: ['800 100px Display', '400 30px Mono', '600 30px Mono'],
  hits:    [[2.0, 20], [2.1, 10], [2.5, 8], [4.0, 14], [6.5, 12]],
  flashes: [[2.0, 0.9, BR.fg], [4.0, 0.45, BR.accent], [6.5, 0.6, BR.fg]],
  glitches:[[1.8, 2.0, 2.12, 0.8]],          // [rampStart, cut, decayEnd, strength]
  wipes:   [[4.0, BR.accent, 0.3]],          // [cut, color, halfDuration]
  scenes: [
    { start: 0, end: 2, name: 'Assemble', draw(t, lt) {
        grid(P(t, 0.1, 1.2), 0.08, t * 6, 80, BR.accent2);
        swarm(t, CX, CY - 60, 0.1, 0.9, 160, [BR.accent2, BR.accent]);
        const u = 96;
        camera(1 + 0.04 * P(t, 0, 2), () => {
          assembleBlocks(t, LOGO, CX - 2.5 * u, CY - 2.5 * u - 60, u,
            { t0: 0.45, order: 'distance', cx: 2, cy: 2, color: BR.fg, accent: BR.accent, sweep: lerp(500, 1500, P(t, 1.3, 1.8)) });
        });
        ring(CX, CY - 60, t, 1.25, 0.6, 900, BR.accent, 4);
        const lab = 'NORTHWIND  ·  PLACEHOLDER', lo = { f: 'Mono', s: 32, ls: 12, c: BR.accent2 };
        typed(lab, CX - tw(lab, lo) / 2, CY + 290, lo, P(t, 1.2, 1.65), t);
    }},
    { start: 2, end: 4, name: 'Statement', draw(t, lt) {
        // backdrop: giant stroked marquee drifting
        txt('DEPLOY  DEPLOY  DEPLOY  DEPLOY', -400 + lt * 120, 300, { s: 300, w: 800, c: BR.fg, stroke: 1, lw: 2, a: 0.08 });
        txt('REVIEW  REVIEW  REVIEW  REVIEW', -200 - lt * 120, 1010, { s: 300, w: 800, c: BR.fg, stroke: 1, lw: 2, a: 0.08 });
        camera(1 + 0.06 * P(t, 2, 4), () => {
          typed('WHAT NORTHWIND DOES', 160, 240, { f: 'Mono', s: 34, ls: 14, c: BR.accent2 }, P(t, 2.02, 2.35), t);
          slam('SHIP', 150, 520, t, 2.0, { s: 330, w: 900, c: BR.fg, ls: -8 });
          slam('FASTER', 150, 830, t, 2.5, { s: 330, w: 900, c: BR.fg, ls: -8 });
          blk(150 + tw('FASTER', { s: 330, w: 900, ls: -8 }) + 80, 782, 90 * E.outBack(P(t, 3.0, 3.3)), BR.accent, 0, 1, 30);
          X.fillStyle = BR.accent2; X.fillRect(156, 878, 1500 * E.outExpo(P(t, 3.1, 3.6)), 4);
          rise('with fewer surprises.', 156, 950, { s: 56, w: 500, c: BR.muted }, P(t, 3.3, 3.7));
        });
    }},
    { start: 4, end: 6.5, name: 'Product', draw(t, lt) {
        grid(1, 0.05, t * 10, 80, BR.accent2);
        // left: big readable line; right: a window cropped off the frame edge
        rise('One view', 130, 430, { s: 112, w: 800, c: BR.fg, ls: -3 }, P(t, 4.15, 4.5));
        rise('of every deploy.', 130, 555, { s: 112, w: 800, c: BR.fg, ls: -3 }, P(t, 4.3, 4.65));
        typed('ILLUSTRATIVE UI', 136, 660, { f: 'Mono', s: 26, ls: 10, c: BR.muted }, P(t, 4.8, 5.1), t);
        const e = E.outExpo(P(t, 4.05, 4.6)), x = lerp(2000, 1030, e) - lt * 12, y = 170 + Math.sin(lt * 2) * 6;
        camera(1 + 0.05 * P(t, 4, 6.5), () => {
          windowChrome(x, y, 1100, 760, 'NORTHWIND  ·  DEPLOYS', { fill: BR.panel, bar: '#18223A', titleCol: BR.muted });
          // chart: bars growing in stagger
          for (let k = 0; k < 11; k++) {
            const hgt = (80 + rr(k, 7) * 200) * E.outExpo(P(t, 4.5 + k * 0.04, 4.9 + k * 0.04));
            X.fillStyle = k === 9 ? BR.accent : rgba(BR.accent2, 0.55);
            X.fillRect(x + 50 + k * 70, y + 380 - hgt, 44, hgt);
          }
          // rows: status flips from running to passed
          ['api-gateway', 'billing-worker', 'web-frontend'].forEach((n, k) => {
            const ry = y + 450 + k * 88, rp = P(t, 5.0 + k * 0.08, 5.3 + k * 0.08); if (rp <= 0) return;
            X.save(); X.globalAlpha = rp; panel(x + 40, ry, 800, 70, 12, '#0F172A', 'rgba(255,255,255,0.06)');
            txt(n, x + 70, ry + 45, { f: 'Mono', s: 28, c: BR.fg });
            const done = t > 5.6 + k * 0.2;
            X.fillStyle = done ? rgba(BR.accent2, 0.2) : rgba(BR.accent, 0.2); X.beginPath(); X.roundRect(x + 600, ry + 16, 200, 38, 19); X.fill();
            txt(done ? 'PASSED' : 'RUNNING', x + 700, ry + 44, { f: 'Mono', s: 22, ls: 3, al: 'center', w: 600, c: done ? BR.accent2 : BR.accent });
            X.restore();
          });
          ring(x + 700, y + 450 + 35, t, 5.6, 0.4, 90, BR.accent2, 3);
        });
        barWipe(t, 6.5, [BR.accent, BR.fg]);
    }},
    { start: 6.5, end: 8, name: 'Lockup', draw(t, lt) {
        const u = 50;
        assembleBlocks(t, LOGO, CX - 2.5 * u - 380, CY - 2.5 * u - 40, u,
          { t0: 6.5, dur: 0.4, order: 'distance', cx: 2, cy: 2, color: BR.fg, accent: BR.accent, from: { x: CX, y: CY }, sweep: lerp(300, 1500, P(t, 7.0, 7.5)) });
        rise('NORTHWIND', CX - 220, CY + 30, { s: 150, w: 800, c: BR.fg, ls: -2 }, P(t, 6.7, 7.0));
        riseWords('Ship faster, with fewer surprises.', CX, CY + 180, { s: 50, w: 500, c: BR.muted, al: 'center' }, t, 7.0);
        rise('northwind.example', CX, CY + 250, { f: 'Mono', s: 28, ls: 8, c: BR.accent2, al: 'center' }, P(t, 7.25, 7.55));
        X.fillStyle = BR.accent; X.fillRect(CX - 170, CY + 268, 340 * E.outExpo(P(t, 7.35, 7.75)), 3);
    }},
  ],
};
