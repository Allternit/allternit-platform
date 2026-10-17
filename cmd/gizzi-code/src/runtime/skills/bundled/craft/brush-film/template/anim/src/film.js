// film.js: the master timeline. Times are video seconds (an optional intro/outro is added at assembly).
// LOCK THE AUDIO FIRST: record the VO (audio/make_vo.py), copy the durations it prints into DUR_VO, place cues in VO,
// then set the shot starts in SHOT_T around them. audio/build_mix.py reads VO + MUSIC from this file.
const DUR_VO = { /* v01: 3.62, ... (printed by make_vo.py) */ };
// [start, id, subtitle]   (the subtitle is what is shown on screen; the spoken text lives in audio/vo-lines.json)
const VO = [
  // [0.25, 'v01', 'First line of the voiceover.'],
];
// bedEnd: where the music bed stops (a tape-stop when a story beat kills it; 0 or >= duration = runs to the end and fades)
// piano: optional end-card cue start (audio/end.mp3 or end.wav)
const MUSIC = { bedEnd: 0, piano: 0 };
// One start time per shot; chapters register shots with these exact times.
const SHOT_T = {
  // opening: 0.0, ..., end: 60.0,
};
// Optional black-and-white "old footage" grade: return 0..1 for time t (0 = colour). Default: never.
window.FILM_GRADE = t => 0;

// VO subtitles (screen space, crisp, bottom band). ?nocaps hides them. Keep key action above y ≈ 960.
const NOCAPS = location.search.includes('nocaps');
window.FILM_OVERLAY = (c, t) => {
  if (NOCAPS || window.LOOP) return;
  const cue = VO.find(([a, id]) => t >= a && t < a + (DUR_VO[id] || 2) + .2); if (!cue) return;
  const [a, id, txt] = cue, d = DUR_VO[id] || 2, k = Math.min(seg(t, a, a + .12), 1 - seg(t, a + d + .08, a + d + .2));
  if (k <= 0) return;
  const tall = OH > OW, fs = tall ? 54 : 40;
  c.save(); c.globalAlpha = k; c.font = `600 ${fs}px "Helvetica Neue", Helvetica, Arial, sans-serif`; c.textBaseline = 'middle'; c.textAlign = 'center';
  const lines = tall ? wrapLines(c, txt, OW * .84) : [txt], lh = fs * 1.25, y0 = (tall ? OH * .80 : OH - 74) - (lines.length - 1) * lh / 2;
  const pw = Math.max(...lines.map(l => c.measureText(l).width)) + 64, ph = lines.length * lh + 24;
  c.fillStyle = 'rgba(26,20,16,0.82)'; c.beginPath(); c.roundRect(OW / 2 - pw / 2, y0 - lh / 2 - 12, pw, ph, 30); c.fill();
  c.fillStyle = '#FDF8F3'; lines.forEach((l, i) => c.fillText(l, OW / 2, y0 + i * lh + 1)); c.restore();
};

function wrapLines(c, txt, maxw) { const out = []; let cur = ''; for (const w of String(txt).split(' ')) { const n = cur ? cur + ' ' + w : w; if (cur && c.measureText(n).width > maxw) { out.push(cur); cur = w; } else cur = n; } if (cur) out.push(cur); return out; }
