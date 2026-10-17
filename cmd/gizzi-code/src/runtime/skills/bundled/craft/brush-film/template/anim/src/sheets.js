// sheets.js: model sheets as standalone loops. Reference for you (and the model), not part of a video, so they're the one
// place labels are fine. Scrub them at studio.html?loop=emotions, ?loop=parts or ?loop=props, or render:
//   node render.mjs --loop=emotions --sheet=1 --cols=1 --w=1600 --out=out/check/emotions.jpg
(() => {
  const label = (txt, x, y, size = 22) => letter(txt, x, y, size, PAL.ink, { ink: false, alpha: .8 });
  const floor = (y) => inkLine([[60, y + 6], [W / 2, y + 4], [W - 60, y + 7]], .6, mixCol(PAL.paper, PAL.ink, .35), 'inkfine', .5);

  // Every emotion in EMO, alive. Order follows EMO.
  LOOPS.emotions = t => {
    const names = Object.keys(EMO), cols = 5, cw = W / cols, ch = H / 3;
    [0, 1, 2].forEach(r => floor(ch * r + ch - 70));
    names.forEach((name, i) => { const cx = cw * (i % cols) + cw / 2, gy = ch * Math.floor(i / cols) + ch - 70;
      blot(cx, gy, 23, feel(name, t)); label(name, cx, gy + 42); });
  };
  LOOPS.emotions.len = 4;

  // Mouths, eyes, emotes and motion helpers.
  LOOPS.parts = t => {
    const u = 15, row = (y, items) => { floor(y); items.forEach(([lab, o], i) => { const x = 110 + i * (W - 220) / (items.length - 1); blot(x, y, u, o); label(lab, x, y + 30, 18); }); };
    row(300, ['smile', 'grin', 'o', 'flat', 'frown', 'wavy', 'open'].map(m => [m, { ...feel('neutral', t), mouth: m }]));
    row(620, [['open', {}], ['wide', { eyes: 'wide' }], ['happy', { eyes: 'happy' }], ['closed', { eyes: 'closed' }], ['half', { eyes: 'half' }], ['brow +1', { brow: 1 }], ['brow -1', { brow: -1 }], ['talk', { talk: talk(t, [[0, 9]]) }]]
      .map(([l, o]) => [l, { ...feel('neutral', t), ...o }]));
    row(960, [['walk', { walk: t * 3, dy: -Math.abs(Math.sin(t * 3 * Math.PI)) * .4 }], ['jump', { ...jump(t % 1.5, .35, .95, 3) }], ['take', { ...feel('surprised', t), ...take(t % 2, .5, 1) }],
      ...['!', '?', 'sweat', 'heart', 'spark', 'zzz'].map(e => [e, { emote: e, emoteK: 1 }])].map(([l, o]) => [l, { ...feel('neutral', t), ...o }]));
  };
  LOOPS.parts.len = 4;

  LOOPS.props = t => { meadow(t); balloon(960, 360, 70, 960, 700); };
  LOOPS.props.len = 4;
})();
