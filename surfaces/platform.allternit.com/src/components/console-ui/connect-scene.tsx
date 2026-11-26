/* Copied from reference/ConnectScene.reference.tsx (handoffs/ui-polish-2026-11-24). Geometry, CSS and timings are
 * the reference's; recopy, never redraw. Console changes: the CSS is injected once into <head> (id-guarded)
 * instead of a <style> per instance, the component is exported as ConnectScene, and it takes `paper` and `className`. */
/**
 * ConnectScene.reference.tsx — PLAN-v2 step 2.14 (web), 2.15 (iOS), 2.16 (Android). Source of truth.
 * Scene F of SCOPE-B1: "Gizzi asleep -> connecting -> cables + check", a small line scene inside the connect sheet.
 * Native copies these paths, timings, easings and still frames exactly.
 *
 * DRIVEN ONLY BY REAL STATE. The scene never advances on a timer of its own. Mapping from the real connection
 * status (web ConnStatus.state in connect-kit/types.ts; native: the same relay/pair status):
 *     'off' | 'needs'  -> 'asleep'      (nothing is happening)
 *     'connecting'     -> 'connecting'  (a login / pair / poll is in flight)
 *     'on'             -> 'connected'   (the row is really on, server confirmed)
 * A failure goes back to 'asleep' (never shows a check it did not earn). No fake progress.
 *
 * GEOMETRY: viewBox 0 0 120 72, render width = the `width` prop (default 240 px / 240 dp; empty states pass round(size * 1.67)), max 100% of the container, height = width * 0.6. Stroke 1.5, round caps,
 * currentColor (foreground), one colour. Paper fill = screen background (--ea-bg).
 *   Gizzi slot: left, box x 6..42, y 14..62 (36 x 48). Web: <GizziMascot size={44}/> centred in it; native: pixel-pet sprite 44.
 *       asleep: still frame, whole slot opacity .55, emotion steady, plus "z" marks (below). connecting: emotion curious, opacity 1.
 *       connected: emotion pleased, opacity 1. (There is no 'asleep' emotion: asleep = dim + still + z marks.)
 *   Device (the thing being connected: app/computer/vendor): rounded rect x78 y18 w36 h30 rx4 (paper), screen rect x82 y22 w28 h18 rx2,
 *       stand M96 48 V54 M88 54 H104, power/port socket circle cx 78 cy 33 r 2.6 (paper) on the LEFT edge where cables land.
 *   Cables (two, each pathLength=1): top  M42 30 C 56 30 60 26 75.4 33
 *                                    bottom M42 44 C 56 44 60 40 75.4 33     (both end at the socket)
 *   z marks (asleep only): "z" strokes M12 10 h5 l-5 6 h5 at translate (0,0) and a smaller one scaled .7 at (10,-6); text-free paths.
 *   Check badge (connected): circle cx 96 cy 33 r 9 (paper, sits over the screen) + check M91.5 33 l3 3.2 l6.5 -6.8.
 *
 * TIMELINE (state transitions; each uses the listed duration + easing, restarts from the current value on interrupt):
 *   EASE_OUT = cubic-bezier(.16,1,.3,1)   EASE_IN_OUT = cubic-bezier(.65,0,.35,1)   LINEAR
 *   asleep (rest, loops):
 *     z marks: 2.4 s loop, each z translate(0,0)->(5,-8), opacity 0 -> 1 (at 30%) -> 0 (100%), second z delayed 1.2 s. EASE_IN_OUT.
 *     cables: drawn as dashed stubs, stroke-dasharray .06 .94 (barely visible dots at both ends only), opacity .35.
 *     Gizzi slot opacity .55.
 *   asleep -> connecting (0.40 s): Gizzi opacity .55 -> 1, EASE_OUT; z marks fade out 0.16 s; cables opacity .35 -> 1.
 *   connecting (loops while status is connecting):
 *     cables: stroke-dasharray .22 .78, stroke-dashoffset 1 -> 0 over 1.2 s LINEAR infinite (a pulse travelling Gizzi -> device);
 *             bottom cable delayed 0.6 s so the pulses alternate.
 *     socket ring: circle cx78 cy33 r 2.6 -> scale 1 -> 1.8, opacity .6 -> 0, 1.2 s EASE_OUT infinite.
 *   connecting -> connected (0.55 s total):
 *     cables: dasharray 1 0, dashoffset snaps to 0, solid (no pulse) 0.25 s EASE_OUT
 *     check badge: scale .6 -> 1, opacity 0 -> 1, 0.30 s EASE_OUT starting at 0.25 s; device screen gets one 0.4 s opacity .5 -> 1.
 *     Gizzi emotion -> pleased.
 *   connected (rest): fully still. No loops.
 *   connected -> asleep (reconnect needed): reverse in 0.25 s EASE_IN_OUT: badge out, cables back to stubs.
 *
 * REDUCED MOTION: no loops and no travelling pulse. Each state is its still frame: asleep = dim Gizzi + z marks static (no float)
 *   + dotted stubs; connecting = Gizzi awake, cables dashed static (dasharray .22 .78 offset 0) and the text status line says
 *   "Connecting..." (the sheet already has role="status"); connected = solid cables + badge shown. State changes switch instantly.
 *
 * SCOPE (v2): the path/rect/circle and svg rules are :is(.cs ...):not(foreignObject *) so the Gizzi mascot inside the foreignObject keeps its own colours.
 *
 * COLOURS: strokes currentColor (--arc-foreground; graphite flips automatically). Badge circle stroke is currentColor too, no green:
 *   the sheet's own status line carries the colour. One solid accent is allowed on the check only: var(--status-success).
 */
import React, { useInsertionEffect } from 'react';

/** Injects `css` into <head> once per id, shared by every instance (decision 4: one style, not one per render). */
export function useHeadStyle(id: string, css: string): void {
  useInsertionEffect(() => {
    if (typeof document === 'undefined' || document.getElementById(id)) return;
    const el = document.createElement('style');
    el.id = id;
    el.textContent = css;
    document.head.appendChild(el);
  }, [id, css]);
}

export type ConnectSceneState = 'asleep' | 'connecting' | 'connected';

export const CONNECT_SCENE_VIEWBOX = { w: 120, h: 72 };

export const CONNECT_SCENE_CSS = `
.cs{color:var(--arc-foreground,currentColor);--ea-bg:var(--arc-background,#fff);--cs-out:cubic-bezier(.16,1,.3,1);--cs-io:cubic-bezier(.65,0,.35,1)}
:is(.cs svg):not(foreignObject *){overflow:visible;display:block;width:100%;height:auto}
:is(.cs path,.cs rect,.cs circle):not(foreignObject *){stroke:currentColor;stroke-width:1.5;stroke-linecap:round;stroke-linejoin:round;fill:none}
.cs .cs-paper{fill:var(--ea-bg)}
.cs .cs-gizzi{opacity:.55;transition:opacity .4s var(--cs-out)}
.cs .cs-cable{stroke-dasharray:.06 .94;stroke-dashoffset:0;opacity:.35;transition:opacity .4s var(--cs-out)}
.cs .cs-z{opacity:0}
.cs .cs-badge{opacity:0;transform:scale(.6);transform-box:fill-box;transform-origin:center;transition:opacity .3s var(--cs-out) .25s,transform .3s var(--cs-out) .25s}
.cs .cs-badge path.cs-tick{stroke:var(--status-success,currentColor)}
.cs .cs-ring{opacity:0;transform-box:fill-box;transform-origin:center}
.cs[data-state="asleep"] .cs-z{opacity:.6}
.cs[data-state="connecting"] .cs-gizzi,.cs[data-state="connected"] .cs-gizzi{opacity:1}
.cs[data-state="connecting"] .cs-cable{opacity:1;stroke-dasharray:.22 .78}
.cs[data-state="connected"] .cs-cable{opacity:1;stroke-dasharray:1 0;transition:opacity .25s var(--cs-out)}
.cs[data-state="connected"] .cs-badge{opacity:1;transform:scale(1)}
@media (prefers-reduced-motion:no-preference){
.cs[data-state="asleep"] .cs-z{animation:cs-z 2.4s infinite var(--cs-io)}
.cs[data-state="asleep"] .cs-z2{animation-delay:1.2s}
.cs[data-state="connecting"] .cs-cable{animation:cs-run 1.2s infinite linear}
.cs[data-state="connecting"] .cs-cable2{animation-delay:.6s}
.cs[data-state="connecting"] .cs-ring{animation:cs-ring 1.2s infinite var(--cs-out)}
.cs[data-state="connected"] .cs-screen{animation:cs-screen .4s var(--cs-out)}
}
@media (prefers-reduced-motion:reduce){.cs .cs-gizzi,.cs .cs-cable,.cs .cs-badge{transition:none}}
@keyframes cs-z{0%{transform:translate(0,0);opacity:0}30%{opacity:1}100%{transform:translate(5px,-8px);opacity:0}}
@keyframes cs-run{from{stroke-dashoffset:1}to{stroke-dashoffset:0}}
@keyframes cs-ring{from{opacity:.6;transform:scale(1)}to{opacity:0;transform:scale(1.8)}}
@keyframes cs-screen{from{opacity:.5}to{opacity:1}}
`;

/** `gizzi` is the slot: empty-art passes the lazy <GizziMascot/>; without it a plain box keeps the layout. */
export function ConnectScene({
  state,
  gizzi,
  width = 240,
  paper,
  className,
}: {
  state: ConnectSceneState;
  gizzi?: React.ReactNode;
  width?: number;
  /** Fill for the solid parts when the scene sits on a filled card (sets --ea-bg). */
  paper?: string;
  className?: string;
}) {
  useHeadStyle('console-connect-scene-css', CONNECT_SCENE_CSS);
  const style = { width, maxWidth: '100%', ...(paper ? { '--ea-bg': paper } : {}) } as React.CSSProperties;
  return (
    <div className={className ? `cs ${className}` : 'cs'} data-state={state} style={style} aria-hidden="true">
      <svg viewBox="0 0 120 72">
        <g className="cs-gizzi">
          <foreignObject x="6" y="14" width="36" height="48">{gizzi ?? <div style={{ width: 36, height: 48, border: '1px dashed currentColor' }} />}</foreignObject>
        </g>
        <path className="cs-z cs-z1" d="M12 10 h5 l-5 6 h5" />
        <path className="cs-z cs-z2" d="M20 4 h3.5 l-3.5 4 h3.5" />
        <path className="cs-cable cs-cable1" pathLength={1} d="M42 30 C 56 30 60 26 75.4 33" />
        <path className="cs-cable cs-cable2" pathLength={1} d="M42 44 C 56 44 60 40 75.4 33" />
        <g className="cs-device">
          <rect x="78" y="18" width="36" height="30" rx="4" className="cs-paper" />
          <rect x="82" y="22" width="28" height="18" rx="2" className="cs-screen" />
          <path d="M96 48 V54 M88 54 H104" />
          <circle cx="78" cy="33" r="2.6" className="cs-paper" />
          <circle cx="78" cy="33" r="2.6" className="cs-ring" />
        </g>
        <g className="cs-badge">
          <circle cx="96" cy="33" r="9" className="cs-paper" />
          <path className="cs-tick" d="M91.5 33 l3 3.2 l6.5 -6.8" />
        </g>
      </svg>
    </div>
  );
}
