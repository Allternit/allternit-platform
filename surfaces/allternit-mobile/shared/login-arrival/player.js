// One seekable 24-second illustration. This document never receives credentials.
const canvas = document.querySelector('canvas');
const context = canvas.getContext('2d');
const motion = matchMedia('(prefers-reduced-motion: reduce)');
let paused = document.hidden, reduced = motion.matches, visible = true;
let elapsed = 0, previous = null, lastPaint = -1, frame = 0, ready = false;
const paint = () => {
  const dpr = Math.min(devicePixelRatio || 1, 2);
  if (canvas.width !== 720 * dpr || canvas.height !== 500 * dpr) {
    canvas.width = 720 * dpr; canvas.height = 500 * dpr;
  }
  context.setTransform(dpr, 0, 0, dpr, 0, 0);
  const result = drawScene(context, reduced ? 4.6 : elapsed % DURATION, reduced);
  canvas.dataset.scene = reduced ? 'Reduced motion' : phaseAt(elapsed % DURATION);
  canvas.dataset.pieces = result.wordCount;
  canvas.dataset.bots = result.botCount;
  canvas.dataset.gizzi = result.gizziVisible;
  canvas.dataset.cycle = Math.floor(elapsed / DURATION);
};
const running = () => ready && !paused && !reduced && visible && !document.hidden;
const tick = now => {
  frame = 0;
  if (!running()) { previous = null; return; }
  if (previous !== null) elapsed += (now - previous) / 1000;
  previous = now;
  if (now - lastPaint >= 1000 / 30) { paint(); lastPaint = now; }
  frame = requestAnimationFrame(tick);
};
const sync = () => {
  previous = null;
  if (frame) cancelAnimationFrame(frame);
  frame = 0;
  if (ready) paint();
  if (running()) frame = requestAnimationFrame(tick);
};
// Only the embedding parent may change presentation state. No commands, URLs or auth data are accepted.
addEventListener('message', event => {
  if (event.source !== parent || !event.data || event.data.type !== 'allternit:arrival-state') return;
  if (typeof event.data.paused === 'boolean') paused = event.data.paused;
  if (typeof event.data.reducedMotion === 'boolean') reduced = event.data.reducedMotion || motion.matches;
  sync();
});
// Native sets this directly on its bundled page. It cannot navigate or fetch.
window.allternitArrivalState = state => {
  paused = state.paused === true;
  reduced = state.reducedMotion === true || motion.matches;
  sync();
};
addEventListener('visibilitychange', sync);
motion.addEventListener('change', () => { reduced = motion.matches; sync(); });
const observer = new IntersectionObserver(entries => { visible = entries[0].isIntersecting; sync(); });
observer.observe(canvas);
Promise.all(Object.entries(assets.bots).map(([name, src]) => new Promise(resolve => {
  const image = new Image();
  image.onload = () => { sourceImages.set(name, image); resolve(); };
  image.onerror = resolve;
  image.src = src;
}))).then(() => {
  ready = true; sync();
  if (parent !== window) parent.postMessage({type:'allternit:arrival-ready'}, '*');
});
// Deterministic seeking is available only in the local preview, never in the shipping player.
