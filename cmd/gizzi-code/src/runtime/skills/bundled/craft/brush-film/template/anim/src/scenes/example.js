// example.js: "The Balloon", a 7.2 s example that shows the kit working: an acted emotion timeline, a hop, a prop held
// by a hand hook, glow, a brush wipe between shots and an iris in and out. It is one idea, not a template: for your film,
// remove this script tag from studio.html (new_film.sh does) and build your own chapters from your storyboard.
//
//   Shot A (0-3.8 s)  dusk meadow. Blot sits alone, sad. A red balloon drifts in; Blot notices (?), gets it (!), hops
//                     toward it, and a brush wipe carries us into the next shot.
//   Shot B (3.8-7.2 s) closer. Blot holds the balloon, happy, then in love; the balloon tugs and lifts Blot onto tiptoe and
//                     off the ground; the iris closes on Blot, floating.
(() => {
  const GY = 800;
  function meadowA(t, lt, dur) {
    camBegin(960 + 30 * ease(seg(lt, 0, dur)), 540 - 10 * lt, 1 + .05 * ease(seg(lt, 1.8, 3.6)));
    meadow(t, { warm: 0, groundY: 830 });
    // the balloon drifts in on a lazy S-curve from the top left and settles above the hill
    const k = ease(seg(lt, .9, 2.6)), bx = lerp(260, 1240, k) + Math.sin(lt * 1.7) * 18 * (1 - k * .6), by = lerp(-160, 300, easeOut(seg(lt, .9, 2.6))) + Math.sin(lt * 2.2) * 8;
    if (lt > .8) balloon(bx, by, 62, bx - 30 + Math.sin(lt * 2) * 25, by + 300, { sway: Math.sin(lt * 1.3) * .5 });
    const mood = emotions(lt, [[0, 'sad'], [2.05, 'curious', { lookX: .9, lookY: -.7 }], [2.55, 'surprised', { lookX: .8, lookY: -.8 }], [3.0, 'excited']]);
    const hop = jump(lt, 3.1, 3.55, 2.5), x = lerp(700, 930, ease(seg(lt, 3.05, 3.6)));
    blot(x, GY + 20, 36, { ...mood, dy: (mood.dy || 0) + hop.dy, sq: (mood.sq || 0) + hop.sq, aR: lt > 3.0 ? 1.35 : mood.aR, boilKey: 'blot' });
    const at = toScreen(700, GY + 20 - 4 * 36);
    camEnd();
    if (lt < .5) iris(...at, lerp(0, 1600, easeIn(lt / .5)));            // transition in: iris opens on Blot
    if (lt > dur - .3) brushWipe((lt - (dur - .3)) / .6, [FP.hillDk, FP.hill]);   // transition out (shot B finishes it)
  }
  function closeB(t, lt, dur) {
    const U = 32, lift = kf(lt, [[1.9, 0], [2.35, 1.7], [2.8, 1.25], [3.4, 1.45]], ease), floatY = lift * U;
    camBegin(960, 560 - floatY * .4, 1.3);
    meadow(t, { warm: ease(seg(lt, 0, 3)), groundY: 830, sunX: 1250, sunY: 300 });
    const mood = emotions(lt, [[0, 'excited'], [.7, 'happy'], [1.45, 'love'], [1.95, 'surprised', { lookY: -.8, lookX: .4 }], [2.5, 'excited']]);
    const sway = spring(lt, 1.95, 3, 9) * .6 + Math.sin(lt * 1.8) * .15;
    blot(960, GY, U, { ...mood, lift, aR: 1.3 + .1 * Math.sin(lt * 3), aL: lt > 2.5 ? 1 : mood.aL, sq: (mood.sq || 0) - (lift > .1 ? .05 : 0), boilKey: 'blot',
      holdR: (hx, hy) => { const bx = hx + 70 + sway * 60, by = hy - 300 - floatY * .2; glow(bx, by, 160, '#FFC7A0', .25); balloon(bx, by, 64, hx, hy, { sway, key: 'held' }); } });
    const at = toScreen(960, GY - 4 * U - floatY);
    camEnd();
    if (lt < .3) brushWipe(.5 + lt / .6, [FP.hillDk, FP.hill]);     // transition in: the wipe drags off
    const tIris = 2.75;                                              // transition out: iris to Blot, hold, shut
    if (lt > tIris) { const r = lt < tIris + .45 ? lerp(1500, 230, ease(seg(lt, tIris, tIris + .45))) : lt < dur - .25 ? 230 : lerp(230, 0, easeIn(seg(lt, dur - .25, dur - .02)));
      iris(...at, r); }
  }
  shots([[0, meadowA], [3.8, closeB]]);
})();
