/**
 * GizziMascot for the console. Copied mechanically from
 * handoffs/ui-polish-2026-11-24/empty-art-v2/gallery-full.tsx lines 1-1431 (the self-contained
 * copy of allternit-ai's ai-elements/GizziMascot used by the approved empty-art gallery).
 * Only change: the exports. Do not edit the drawing here; recopy it instead.
 * Loaded with React.lazy from empty-art.tsx so framer-motion stays out of the main console chunk.
 */
import React, { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { AnimatePresence, motion } from 'framer-motion';

const cn = (...a: any[]) => a.filter(Boolean).join(' ');
const useReducedMotion = () => { const [r, setR] = useState(false); useEffect(() => { const m = window.matchMedia('(prefers-reduced-motion: reduce)'); setR(m.matches); const f = () => setR(m.matches); m.addEventListener('change', f); return () => m.removeEventListener('change', f); }, []); return r; };
const useAvatarEyes = (): any => null;

export type GizziEmotion =
  | 'alert'
  | 'curious'
  | 'focused'
  | 'steady'
  | 'pleased'
  | 'skeptical'
  | 'mischief'
  | 'proud';

type GizziAttentionState = 'tracking' | 'locked-on' | 'startled';

export interface GizziAttention {
  state: GizziAttentionState;
  target?: {
    x: number;
    y: number;
  };
}

type GizziLocomotionStyle = 'chat' | 'cowork' | 'bot' | 'code' | 'browser';
type GizziLocomotionPhase = 'idle' | 'walk-in' | 'walk-out' | 'crawl';

export interface GizziLocomotion {
  style: GizziLocomotionStyle;
  phase: GizziLocomotionPhase;
  direction?: 'forward' | 'reverse';
}

function isReverseLocomotion(locomotion: GizziLocomotion | null | undefined) {
  if (!locomotion) {
    return false;
  }

  return locomotion.phase === 'walk-out' || locomotion.direction === 'reverse';
}

export interface GizziMascotProps {
  size?: number;
  className?: string;
  label?: string;
  testId?: string;
  emotion?: GizziEmotion;
  attention?: GizziAttention | null;
  locomotion?: GizziLocomotion | null;
}

type EyePreset =
  | 'square'
  | 'wide'
  | 'curious'
  | 'narrow'
  | 'pleased'
  | 'skeptical'
  | 'mischief'
  | 'proud'
  | 'dizzy';

interface EmotionProfile {
  expressions: string[];
  hoverExpressions: string[];
  tapExpressions: string[];
  bodyAnimation: string;
  beaconAnimation: string;
  handAnimation?: string;
  eyePreset: EyePreset;
  tapEyePreset: EyePreset;
  typeDelay: number;
  holdDelay: number;
}

function resolveLocomotionFamily(style: GizziLocomotionStyle) {
  switch (style) {
    case 'cowork':
    case 'bot':
    case 'browser':
      return 'cowork';
    case 'chat':
    case 'code':
    default:
      return 'chat';
  }
}

function getLocomotionBodyAnimation(locomotion: GizziLocomotion | null | undefined) {
  if (!locomotion || locomotion.phase === 'idle') {
    return undefined;
  }

  const family = resolveLocomotionFamily(locomotion.style);
  // Use stable body animation (minimal vertical movement) for skim motion
  const animationName = family === 'cowork' ? 'gizzi-cowork-march-body' : 'gizzi-chat-crawl-body';
  const durationMs = family === 'cowork' ? 620 : 680;
  const reverse = isReverseLocomotion(locomotion);

  return {
    animation: `${animationName} ${durationMs}ms linear 0ms infinite ${reverse ? 'reverse' : 'normal'}`,
  };
}

function getLocomotionHandAnimation(
  locomotion: GizziLocomotion | null | undefined,
  side: 'left' | 'right',
) {
  if (!locomotion || locomotion.phase === 'idle') {
    return undefined;
  }

  const family = resolveLocomotionFamily(locomotion.style);
  const animationName = family === 'cowork' ? 'gizzi-cowork-hand-swing' : 'gizzi-chat-hand-swing';
  const durationMs = family === 'cowork' ? 620 : 680;
  const reverse = isReverseLocomotion(locomotion);

  const direction = reverse
    ? side === 'left' ? 'reverse' : 'alternate-reverse'
    : side === 'left' ? 'normal' : 'alternate';
  return {
    animation: `${animationName} ${durationMs}ms cubic-bezier(0.3, 0.9, 0.36, 1) 0ms infinite ${direction}`,
  };
}

function getLocomotionLegAnimation(
  locomotion: GizziLocomotion | null | undefined,
  index: number,
) {
  if (!locomotion || locomotion.phase === 'idle') {
    return undefined;
  }

  const family = resolveLocomotionFamily(locomotion.style);
  const reverse = isReverseLocomotion(locomotion);
  
  // Different animations for different phases
  if (locomotion.phase === 'walk-in' || locomotion.phase === 'walk-out') {
    // Walking has more pronounced leg movement
    const animationName = family === 'cowork'
      ? index % 2 === 0
        ? 'gizzi-cowork-walk-step-a'
        : 'gizzi-cowork-walk-step-b'
      : index % 2 === 0
        ? 'gizzi-chat-walk-step-a'
        : 'gizzi-chat-walk-step-b';
    const durationMs = family === 'cowork' ? 480 : 520;
    
    return {
      animation: `${animationName} ${durationMs}ms linear 0ms infinite ${reverse ? 'reverse' : 'normal'}`,
    };
  }

  // Crawl/skim has lateral leg motion for surface movement - faster for responsiveness
  const animationName = family === 'cowork'
    ? index % 2 === 0
      ? 'gizzi-cowork-skim-step-a'
      : 'gizzi-cowork-skim-step-b'
    : index % 2 === 0
      ? 'gizzi-chat-skim-step-a'
      : 'gizzi-chat-skim-step-b';
  const durationMs = family === 'cowork' ? 400 : 450;

  return {
    animation: `${animationName} ${durationMs}ms linear 0ms infinite ${reverse ? 'reverse' : 'normal'}`,
  };
}

const gizziStyles = `
@keyframes gizzi-hs-mouth { 0%,100% { transform: scaleY(1); } 33% { transform: scaleY(0.6); } 66% { transform: scaleY(1.1); } }
.gizzi-hs { --gizzi-hs: #2B2E35; --gizzi-hs-hi: #4A4F59; }
.dark .gizzi-hs, [data-theme="dark"] .gizzi-hs { --gizzi-hs: #5C626D; --gizzi-hs-hi: #9AA1AD; }
@keyframes gizzi-float {
  0%, 100% {
    transform: translateY(0px);
  }
  50% {
    transform: translateY(-2px);
  }
}

@keyframes gizzi-alert-hop {
  0%, 100% {
    transform: translateY(0px);
  }
  20% {
    transform: translateY(-4px);
  }
  40% {
    transform: translateY(-1px);
  }
}

@keyframes gizzi-curious-tilt {
  0%, 100% {
    transform: rotate(0deg) translateY(0px);
  }
  35% {
    transform: rotate(-3deg) translateY(-1px);
  }
  68% {
    transform: rotate(2deg) translateY(-2px);
  }
}

@keyframes gizzi-focused-breathe {
  0%, 100% {
    transform: scale(1);
  }
  50% {
    transform: scale(1.015);
  }
}

@keyframes gizzi-steady-hold {
  0%, 100% {
    transform: translateY(0px);
  }
  50% {
    transform: translateY(-1px);
  }
}

@keyframes gizzi-pleased-bob {
  0%, 100% {
    transform: translateY(0px) rotate(0deg);
  }
  40% {
    transform: translateY(-3px) rotate(-1deg);
  }
  70% {
    transform: translateY(-1px) rotate(1deg);
  }
}

@keyframes gizzi-skeptical-lean {
  0%, 100% {
    transform: rotate(0deg);
  }
  50% {
    transform: rotate(-2.5deg) translateX(-1px);
  }
}

@keyframes gizzi-mischief-sway {
  0%, 100% {
    transform: rotate(0deg) translateY(0px);
  }
  30% {
    transform: rotate(2deg) translateY(-1px);
  }
  60% {
    transform: rotate(-2deg) translateY(-2px);
  }
}

@keyframes gizzi-proud-lift {
  0%, 100% {
    transform: translateY(0px);
  }
  50% {
    transform: translateY(-3px) scale(1.01);
  }
}

@keyframes gizzi-beacon-soft {
  0%, 100% {
    opacity: 0.55;
    transform: scale(0.94);
  }
  50% {
    opacity: 0.92;
    transform: scale(1.04);
  }
}

@keyframes gizzi-beacon-alert {
  0%, 100% {
    opacity: 0.5;
    transform: scale(0.9);
  }
  50% {
    opacity: 1;
    transform: scale(1.12);
  }
}

@keyframes gizzi-hand-wave {
  0%, 100% {
    transform: translateX(0px);
  }
  50% {
    transform: translateX(-1px);
  }
}

@keyframes gizzi-hand-flick {
  0%, 100% {
    transform: translateX(0px);
  }
  50% {
    transform: translateX(-2px) translateY(-1px);
  }
}

@keyframes gizzi-dizzy-orbit {
  0% {
    transform: rotate(0deg) translateY(-16px) rotate(0deg);
    opacity: 0.2;
  }
  20% {
    opacity: 1;
  }
  100% {
    transform: rotate(360deg) translateY(-16px) rotate(-360deg);
    opacity: 0.2;
  }
}

@keyframes gizzi-startled-burst {
  0% {
    opacity: 0;
    transform: translateY(6px) scale(0.72) rotate(-10deg);
  }
  22% {
    opacity: 1;
    transform: translateY(-3px) scale(1.12) rotate(3deg);
  }
  52% {
    opacity: 0.96;
    transform: translateY(-1px) scale(1) rotate(-2deg);
  }
  100% {
    opacity: 0.18;
    transform: translateY(1px) scale(0.92) rotate(0deg);
  }
}

@keyframes gizzi-startled-shockwave {
  0% {
    opacity: 0;
    transform: scale(0.7);
  }
  28% {
    opacity: 0.48;
    transform: scale(1.02);
  }
  100% {
    opacity: 0;
    transform: scale(1.28);
  }
}

@keyframes gizzi-locked-frame-pulse {
  0%, 100% {
    opacity: 0.5;
    transform: scale(0.97);
  }
  50% {
    opacity: 1;
    transform: scale(1.04);
  }
}

@keyframes gizzi-locked-scan {
  0% {
    opacity: 0;
    transform: translateY(-10px);
  }
  18% {
    opacity: 0.86;
  }
  82% {
    opacity: 0.72;
  }
  100% {
    opacity: 0;
    transform: translateY(10px);
  }
}

@keyframes gizzi-chat-crawl-body {
  0%, 100% {
    transform: translate(0px, 0px) rotate(0.5deg);
  }
  25% {
    transform: translate(0.5px, 0px) rotate(-0.3deg);
  }
  50% {
    transform: translate(0px, 0px) rotate(0.5deg);
  }
  75% {
    transform: translate(-0.5px, 0px) rotate(-0.3deg);
  }
}

@keyframes gizzi-cowork-march-body {
  0%, 100% {
    transform: translate(0px, 0px) rotate(0.3deg);
  }
  25% {
    transform: translate(0.3px, 0px) rotate(-0.2deg);
  }
  50% {
    transform: translate(0px, 0px) rotate(0.3deg);
  }
  75% {
    transform: translate(-0.3px, 0px) rotate(-0.2deg);
  }
}

/* SKIM animations - realistic walking/skimming motion on surface */
@keyframes gizzi-chat-skim-step-a {
  0% {
    transform: translateX(-1px) translateY(0px) rotate(-8deg);
  }
  25% {
    transform: translateX(0.5px) translateY(-2px) rotate(0deg);
  }
  50% {
    transform: translateX(2px) translateY(0px) rotate(8deg);
  }
  75% {
    transform: translateX(1px) translateY(-1px) rotate(4deg);
  }
  100% {
    transform: translateX(-1px) translateY(0px) rotate(-8deg);
  }
}

@keyframes gizzi-chat-skim-step-b {
  0% {
    transform: translateX(2px) translateY(0px) rotate(8deg);
  }
  25% {
    transform: translateX(1px) translateY(-1px) rotate(4deg);
  }
  50% {
    transform: translateX(-1px) translateY(0px) rotate(-8deg);
  }
  75% {
    transform: translateX(0.5px) translateY(-2px) rotate(0deg);
  }
  100% {
    transform: translateX(2px) translateY(0px) rotate(8deg);
  }
}

@keyframes gizzi-cowork-skim-step-a {
  0% {
    transform: translateX(-0.5px) translateY(0px) rotate(-6deg);
  }
  25% {
    transform: translateX(0.5px) translateY(-1.5px) rotate(0deg);
  }
  50% {
    transform: translateX(1.5px) translateY(0px) rotate(6deg);
  }
  75% {
    transform: translateX(0.8px) translateY(-1px) rotate(3deg);
  }
  100% {
    transform: translateX(-0.5px) translateY(0px) rotate(-6deg);
  }
}

@keyframes gizzi-cowork-skim-step-b {
  0% {
    transform: translateX(1.5px) translateY(0px) rotate(6deg);
  }
  25% {
    transform: translateX(0.8px) translateY(-1px) rotate(3deg);
  }
  50% {
    transform: translateX(-0.5px) translateY(0px) rotate(-6deg);
  }
  75% {
    transform: translateX(0.5px) translateY(-1.5px) rotate(0deg);
  }
  100% {
    transform: translateX(1.5px) translateY(0px) rotate(6deg);
  }
}

/* WALK animations - more pronounced leg movement for walking on/off surfaces */
@keyframes gizzi-chat-walk-step-a {
  0%, 100% {
    transform: translateX(0px) translateY(0px) rotate(8deg);
  }
  25% {
    transform: translateX(2px) translateY(-1px) rotate(4deg);
  }
  50% {
    transform: translateX(3px) translateY(0px) rotate(0deg);
  }
  75% {
    transform: translateX(1.5px) translateY(-0.5px) rotate(5deg);
  }
}

@keyframes gizzi-chat-walk-step-b {
  0%, 100% {
    transform: translateX(0px) translateY(0px) rotate(-8deg);
  }
  25% {
    transform: translateX(1.5px) translateY(-0.5px) rotate(-5deg);
  }
  50% {
    transform: translateX(3px) translateY(0px) rotate(0deg);
  }
  75% {
    transform: translateX(2px) translateY(-1px) rotate(-4deg);
  }
}

@keyframes gizzi-cowork-walk-step-a {
  0%, 100% {
    transform: translateX(0px) translateY(0px) rotate(6deg);
  }
  25% {
    transform: translateX(1.5px) translateY(-0.5px) rotate(3deg);
  }
  50% {
    transform: translateX(2.5px) translateY(0px) rotate(0deg);
  }
  75% {
    transform: translateX(1px) translateY(-0.5px) rotate(4deg);
  }
}

@keyframes gizzi-cowork-walk-step-b {
  0%, 100% {
    transform: translateX(0px) translateY(0px) rotate(-6deg);
  }
  25% {
    transform: translateX(1px) translateY(-0.5px) rotate(-4deg);
  }
  50% {
    transform: translateX(2.5px) translateY(0px) rotate(0deg);
  }
  75% {
    transform: translateX(1.5px) translateY(-0.5px) rotate(-3deg);
  }
}

/* Legacy bounce animations (kept for backward compatibility) */
@keyframes gizzi-chat-step-a {
  0%, 100% {
    transform: translate(0px, 0px) rotate(14deg) scaleY(1);
  }
  18% {
    transform: translate(-0.4px, 0.8px) rotate(8deg) scaleY(0.94);
  }
  42% {
    transform: translate(1px, -1.8px) rotate(-14deg) scaleY(1.04);
  }
  58% {
    transform: translate(1.4px, -2.8px) rotate(-18deg) scaleY(1.07);
  }
  76% {
    transform: translate(0.2px, -0.6px) rotate(-4deg) scaleY(1.02);
  }
  88% {
    transform: translate(-0.5px, 0.25px) rotate(8deg) scaleY(0.98);
  }
}

@keyframes gizzi-chat-step-b {
  0%, 100% {
    transform: translate(0px, 0px) rotate(-14deg) scaleY(1);
  }
  18% {
    transform: translate(0.9px, -2.2px) rotate(15deg) scaleY(1.04);
  }
  36% {
    transform: translate(1.2px, -3px) rotate(18deg) scaleY(1.08);
  }
  56% {
    transform: translate(-0.25px, -0.35px) rotate(3deg) scaleY(1.01);
  }
  76% {
    transform: translate(-0.9px, 0.8px) rotate(-10deg) scaleY(0.95);
  }
  88% {
    transform: translate(-0.35px, 0.2px) rotate(-4deg) scaleY(0.99);
  }
}

@keyframes gizzi-cowork-step-a {
  0%, 100% {
    transform: translate(0px, 0px) rotate(8deg) scaleY(1);
  }
  20% {
    transform: translate(-0.2px, 0.45px) rotate(5deg) scaleY(0.97);
  }
  42% {
    transform: translate(0.75px, -1.4px) rotate(-9deg) scaleY(1.03);
  }
  60% {
    transform: translate(0.95px, -2px) rotate(-11deg) scaleY(1.05);
  }
  78% {
    transform: translate(0.1px, -0.3px) rotate(-2deg) scaleY(1.01);
  }
  90% {
    transform: translate(-0.2px, 0.15px) rotate(5deg) scaleY(0.99);
  }
}

@keyframes gizzi-cowork-step-b {
  0%, 100% {
    transform: translate(0px, 0px) rotate(-8deg) scaleY(1);
  }
  18% {
    transform: translate(0.7px, -1.45px) rotate(10deg) scaleY(1.03);
  }
  38% {
    transform: translate(0.9px, -2.05px) rotate(12deg) scaleY(1.05);
  }
  58% {
    transform: translate(0px, -0.25px) rotate(1deg) scaleY(1.01);
  }
  78% {
    transform: translate(-0.55px, 0.4px) rotate(-6deg) scaleY(0.97);
  }
  90% {
    transform: translate(-0.15px, 0.1px) rotate(-2deg) scaleY(0.99);
  }
}

@keyframes gizzi-chat-hand-swing {
  0%, 100% {
    transform: translateY(0px) rotate(0deg);
  }
  25% {
    transform: translateY(-0.5px) rotate(-7deg);
  }
  50% {
    transform: translateY(0.5px) rotate(6deg);
  }
  75% {
    transform: translateY(-0.25px) rotate(-3deg);
  }
}

@keyframes gizzi-cowork-hand-swing {
  0%, 100% {
    transform: translateY(0px) rotate(0deg);
  }
  30% {
    transform: translateY(-0.4px) rotate(-3.5deg);
  }
  60% {
    transform: translateY(0.4px) rotate(3deg);
  }
}
[data-paused="true"] svg, [data-paused="true"] svg * {
  animation-play-state: paused !important;
}
`;

const DIZZY_EXPRESSIONS = [':o', ':@@', ':x', ':o'] as const;
const STARTLED_EXPRESSIONS = [':O', ':O!', ':O!!'] as const;
const LOCKED_ON_EXPRESSIONS = [':||', '://', '://>'] as const;
const STARTLED_DURATION_MS = 560;
const LOCKED_ON_DELAY_MS = 640;
const DIZZY_MAX_TRAIL_AGE_MS = 30000;
const DIZZY_MIN_SAMPLES = 5;
const DIZZY_MIN_TURN_RADIANS = Math.PI * 0.8;
const DIZZY_MIN_AVERAGE_RADIUS = 0.17;
const DIZZY_MAX_TRAIL_POINTS = 24;

const EMOTION_PROFILES: Record<GizziEmotion, EmotionProfile> = {
  alert: {
    expressions: ['://', '://>', '://'],
    hoverExpressions: ['://?', '://!', '://?'],
    tapExpressions: ['://!', '://!!', '://!'],
    bodyAnimation: 'gizzi-alert-hop 2.6s ease-in-out infinite',
    beaconAnimation: 'gizzi-beacon-alert 1.4s ease-in-out infinite',
    handAnimation: 'gizzi-hand-flick 1.6s ease-in-out infinite',
    eyePreset: 'wide',
    tapEyePreset: 'wide',
    typeDelay: 210,
    holdDelay: 1040,
  },
  curious: {
    expressions: [':__', ':/?', ':__'],
    hoverExpressions: [':/?', ':/?_', ':/?'],
    tapExpressions: [':o', ':o?', ':o'],
    bodyAnimation: 'gizzi-curious-tilt 4.6s ease-in-out infinite',
    beaconAnimation: 'gizzi-beacon-soft 2.2s ease-in-out infinite',
    handAnimation: 'gizzi-hand-wave 3.8s ease-in-out infinite',
    eyePreset: 'curious',
    tapEyePreset: 'wide',
    typeDelay: 240,
    holdDelay: 1180,
  },
  focused: {
    expressions: [':__', ':||', ':__'],
    hoverExpressions: [':||', ':|_', ':||'],
    tapExpressions: [':|!', ':||', ':|!'],
    bodyAnimation: 'gizzi-focused-breathe 4.4s ease-in-out infinite',
    beaconAnimation: 'gizzi-beacon-soft 2.5s ease-in-out infinite',
    eyePreset: 'narrow',
    tapEyePreset: 'square',
    typeDelay: 250,
    holdDelay: 1280,
  },
  steady: {
    expressions: [':__', ':__', ':__'],
    hoverExpressions: [':_:', ':__', ':_:'],
    tapExpressions: [':o', ':__', ':o'],
    bodyAnimation: 'gizzi-steady-hold 5.8s ease-in-out infinite',
    beaconAnimation: 'gizzi-beacon-soft 2.8s ease-in-out infinite',
    eyePreset: 'square',
    tapEyePreset: 'wide',
    typeDelay: 320,
    holdDelay: 1360,
  },
  pleased: {
    expressions: [':)', ':))', ':)'],
    hoverExpressions: [':D', ':))', ':D'],
    tapExpressions: [':D', ':D!', ':D'],
    bodyAnimation: 'gizzi-pleased-bob 3.8s ease-in-out infinite',
    beaconAnimation: 'gizzi-beacon-soft 2s ease-in-out infinite',
    handAnimation: 'gizzi-hand-wave 2.8s ease-in-out infinite',
    eyePreset: 'pleased',
    tapEyePreset: 'wide',
    typeDelay: 250,
    holdDelay: 1100,
  },
  skeptical: {
    expressions: [':/', ':/_', ':/'],
    hoverExpressions: [':/?', ':/-', ':/?'],
    tapExpressions: [':/', ':/!', ':/'],
    bodyAnimation: 'gizzi-skeptical-lean 4.4s ease-in-out infinite',
    beaconAnimation: 'gizzi-beacon-soft 2.4s ease-in-out infinite',
    eyePreset: 'skeptical',
    tapEyePreset: 'curious',
    typeDelay: 260,
    holdDelay: 1240,
  },
  mischief: {
    expressions: [':)', ':>>', ':~'],
    hoverExpressions: [':~', ':>>', ':~'],
    tapExpressions: [':>', ':>>', ':>'],
    bodyAnimation: 'gizzi-mischief-sway 4.2s ease-in-out infinite',
    beaconAnimation: 'gizzi-beacon-soft 1.9s ease-in-out infinite',
    handAnimation: 'gizzi-hand-flick 2.2s ease-in-out infinite',
    eyePreset: 'mischief',
    tapEyePreset: 'mischief',
    typeDelay: 220,
    holdDelay: 1060,
  },
  proud: {
    expressions: [':)', ':_)', ':)'],
    hoverExpressions: [':_)', ':D', ':_)'],
    tapExpressions: [':D', ':_)', ':D'],
    bodyAnimation: 'gizzi-proud-lift 4.8s ease-in-out infinite',
    beaconAnimation: 'gizzi-beacon-soft 2.1s ease-in-out infinite',
    eyePreset: 'proud',
    tapEyePreset: 'proud',
    typeDelay: 250,
    holdDelay: 1200,
  },
};

function renderEye(side: 'left' | 'right', preset: EyePreset) {
  const x = side === 'left' ? 33 : 59;

  switch (preset) {
    case 'wide':
      return <rect x={x} y="36" width="8" height="10" rx="2" fill="#111318" />;
    case 'curious':
      return side === 'left' ? (
        <rect x={x} y="36" width="8" height="8" rx="2" fill="#111318" />
      ) : (
        <path d={`M${x} 45L${x + 4} 36L${x + 8} 45`} stroke="#111318" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" />
      );
    case 'narrow':
      return <rect x={x} y="40" width="8" height="4" rx="2" fill="#111318" />;
    case 'pleased':
      return (
        <path
          d={`M${x} 43C${x + 2} 39 ${x + 6} 39 ${x + 8} 43`}
          stroke="#111318"
          strokeWidth="3"
          strokeLinecap="round"
          fill="none"
        />
      );
    case 'skeptical':
      return side === 'left' ? (
        <path d={`M${x} 40H${x + 8}`} stroke="#111318" strokeWidth="3" strokeLinecap="round" />
      ) : (
        <path d={`M${x} 44L${x + 8} 40`} stroke="#111318" strokeWidth="3" strokeLinecap="round" />
      );
    case 'mischief':
      return side === 'left' ? (
        <path d={`M${x} 42L${x + 8} 38`} stroke="#111318" strokeWidth="3" strokeLinecap="round" />
      ) : (
        <path d={`M${x} 38L${x + 8} 42`} stroke="#111318" strokeWidth="3" strokeLinecap="round" />
      );
    case 'proud':
      return (
        <path
          d={`M${x} 44C${x + 2} 41 ${x + 6} 41 ${x + 8} 44`}
          stroke="#111318"
          strokeWidth="3"
          strokeLinecap="round"
          fill="none"
        />
      );
    case 'dizzy':
      return (
        <path
          d={`M${x} 37L${x + 8} 45M${x + 8} 37L${x} 45`}
          stroke="#111318"
          strokeWidth="3"
          strokeLinecap="round"
        />
      );
    case 'square':
    default:
      return <rect x={x} y="37" width="8" height="8" rx="2" fill="#111318" />;
  }
}

export function GizziMascot({
  size = 92,
  className,
  label = 'Gizzi mascot',
  testId = 'gizzi-mascot',
  emotion = 'steady',
  attention = null,
  locomotion = null,
}: GizziMascotProps) {
  const profile = useMemo(() => EMOTION_PROFILES[emotion], [emotion]);
  const [isHovered, setIsHovered] = useState(false);
  const [isPressed, setIsPressed] = useState(false);
  const [isTapped, setIsTapped] = useState(false);
  const [isDizzy, setIsDizzy] = useState(false);
  const [isStartled, setIsStartled] = useState(false);
  const [isLockedOn, setIsLockedOn] = useState(false);
  const [pointerOffset, setPointerOffset] = useState({ x: 0, y: 0 });
  const [expressionIndex, setExpressionIndex] = useState(0);
  const pointerTrailRef = useRef<Array<{ x: number; y: number; t: number }>>([]);
  const rootRef = useRef<HTMLDivElement>(null);
  const reducedMotion = useReducedMotion();
  const [offScreen, setOffScreen] = useState(false);
  useEffect(() => {
    const el = rootRef.current;
    if (!el || typeof IntersectionObserver === 'undefined') return;
    const observer = new IntersectionObserver(([entry]) => setOffScreen(!entry.isIntersecting));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  const externalAttentionState = attention?.state ?? null;
  const externalTarget = attention?.target
    ? {
        x: Math.max(-1, Math.min(1, attention.target.x)),
        y: Math.max(-1, Math.min(1, attention.target.y)),
      }
    : null;
  const isExternallyTracking =
    externalAttentionState === 'tracking' ||
    externalAttentionState === 'locked-on' ||
    externalAttentionState === 'startled';
  const effectivePointerOffset = isHovered
    ? pointerOffset
    : externalTarget ?? { x: 0, y: 0 };
  const effectiveHovered = isHovered || isExternallyTracking;
  const effectiveStartled = isStartled || externalAttentionState === 'startled';
  const effectiveLockedOn = isLockedOn || externalAttentionState === 'locked-on';
  const activeExpressions = isDizzy
    ? DIZZY_EXPRESSIONS
    : isTapped
    ? profile.tapExpressions
    : effectiveStartled
    ? STARTLED_EXPRESSIONS
    : effectiveLockedOn
    ? LOCKED_ON_EXPRESSIONS
    : effectiveHovered
      ? profile.hoverExpressions
      : profile.expressions;
  const [visibleLength, setVisibleLength] = useState(activeExpressions[0]?.length ?? 0);

  const [prevActiveExpressions, setPrevActiveExpressions] = useState(activeExpressions);
  if (activeExpressions !== prevActiveExpressions) {
    setPrevActiveExpressions(activeExpressions);
    setExpressionIndex(0);
    setVisibleLength(activeExpressions[0]?.length ?? 0);
  }

  useEffect(() => {    if (!isTapped) {
      return;
    }

    const timeoutId = window.setTimeout(() => {
      setIsTapped(false);
    }, 340);

    return () => window.clearTimeout(timeoutId);
  }, [isTapped]);

  useEffect(() => {
    if (!isDizzy) {
      return;
    }

    const timeoutId = window.setTimeout(() => {
      setIsDizzy(false);
    }, 1400);

    return () => window.clearTimeout(timeoutId);
  }, [isDizzy]);

  useEffect(() => {
    if (!isStartled) {
      return;
    }

    const timeoutId = window.setTimeout(() => {
      setIsStartled(false);
    }, STARTLED_DURATION_MS);

    return () => window.clearTimeout(timeoutId);
  }, [isStartled]);

  useEffect(() => {
    const isCenteredHover =
      isHovered &&
      !isDizzy &&
      !isTapped &&
      Math.abs(pointerOffset.x) < 0.26 &&
      Math.abs(pointerOffset.y) < 0.24;

    if (!isCenteredHover) {
      setIsLockedOn(false);
      return;
    }

    const timeoutId = window.setTimeout(() => {
      setIsLockedOn(true);
    }, LOCKED_ON_DELAY_MS);

    return () => window.clearTimeout(timeoutId);
  }, [isDizzy, isHovered, isTapped, pointerOffset.x, pointerOffset.y]);

  useEffect(() => {
    const expression = activeExpressions[expressionIndex] ?? activeExpressions[0] ?? ':__';
    const isTyping = visibleLength < expression.length;
    const nextDelay = isTyping
      ? (isTapped ? Math.max(profile.typeDelay - 70, 120) : profile.typeDelay)
      : isDizzy
        ? 220
        : isTapped
        ? 260
        : effectiveHovered
          ? Math.max(profile.holdDelay - 320, 640)
          : profile.holdDelay;

    const timeoutId = window.setTimeout(() => {
      if (isTyping) {
        setVisibleLength((current) => Math.min(current + 1, expression.length));
        return;
      }

      const nextIndex = (expressionIndex + 1) % activeExpressions.length;
      setExpressionIndex(nextIndex);
      setVisibleLength(activeExpressions[nextIndex]?.length ? 1 : 0);
    }, nextDelay);

    return () => window.clearTimeout(timeoutId);
  }, [activeExpressions, effectiveHovered, expressionIndex, isDizzy, isTapped, profile, visibleLength]);

  const fullExpression = activeExpressions[expressionIndex] ?? ':__';
  const mouthText = fullExpression.slice(0, visibleLength);
  const activeEyePreset = isDizzy
    ? 'dizzy'
    : isTapped
    ? profile.tapEyePreset
    : effectiveStartled
    ? 'wide'
    : effectiveLockedOn
    ? 'narrow'
    : profile.eyePreset;
  const pointerStrength = isHovered ? 1 : isExternallyTracking ? 0.72 : 0.35;
  const bodyTranslateX = effectivePointerOffset.x * (effectiveHovered ? 3.2 * pointerStrength : 1.2);
  const bodyTranslateY =
    effectivePointerOffset.y * (effectiveHovered ? 2.4 * pointerStrength : 0.8) +
    (isPressed ? 2.8 : 0) -
    (isTapped ? 4.8 : 0) +
    (isDizzy ? 1.4 : 0) -
    (effectiveStartled ? 5.6 : 0) -
    (effectiveLockedOn ? 2.2 : 0);
  const bodyRotate =
    effectivePointerOffset.x * (effectiveHovered ? 9 * pointerStrength : 3.5) +
    (isTapped ? (effectivePointerOffset.x >= 0 ? 8.5 : -8.5) : 0) +
    (isDizzy ? 10 : 0) +
    (effectiveStartled ? (effectivePointerOffset.x >= 0 ? -8 : 8) : 0) +
    (effectiveLockedOn ? effectivePointerOffset.x * 2.2 : 0);
  const presenceEyes = useAvatarEyes();
  const claimHeadset = presenceEyes?.claimHeadset;
  useLayoutEffect(() => {
    claimHeadset?.(true);
    return () => claimHeadset?.(false);
  }, [claimHeadset]);
  const headsetPhase = presenceEyes?.headset ?? 'off';
  const wearing = headsetPhase !== 'off';
  const hsTalking = headsetPhase === 'talking';
  const hsAnimate = presenceEyes?.animateIn !== false && !reducedMotion;
  const [hsLanding, setHsLanding] = useState(false);
  const [hsLookUp, setHsLookUp] = useState(false);
  useEffect(() => {
    if (!wearing || !hsAnimate) return;
    setHsLookUp(true);
    const a = setTimeout(() => setHsLookUp(false), 420);
    const b = setTimeout(() => setHsLanding(true), 260);
    const c = setTimeout(() => setHsLanding(false), 360);
    return () => { clearTimeout(a); clearTimeout(b); clearTimeout(c); };
  }, [wearing, hsAnimate]);
  const hsSpring = hsAnimate ? { type: 'spring' as const, stiffness: 520, damping: 30 } : { duration: 0 };
  const eyeTranslateX = (presenceEyes?.lookX ?? 0) + effectivePointerOffset.x * 2.4 * (isHovered ? 1 : isExternallyTracking ? 0.82 : 0);
  const eyeTranslateY = effectivePointerOffset.y * 1.8 * (isHovered ? 1 : isExternallyTracking ? 0.82 : 0) - (hsLookUp ? 2 : 0);
  const hoverScale = isPressed ? 0.88 : isTapped ? 1.18 : effectiveStartled ? 1.13 : effectiveLockedOn ? 1.07 : effectiveHovered ? 1.05 : 1;
  const beaconScale = isPressed ? 0.82 : isTapped ? 1.34 : effectiveStartled ? 1.34 : effectiveLockedOn ? 1.18 : isDizzy ? 1.16 : 1;
  const locomotionActive = Boolean(locomotion && locomotion.phase !== 'idle');
  const locomotionBodyStyle = getLocomotionBodyAnimation(locomotion);
  const leftHandLocomotionStyle = getLocomotionHandAnimation(locomotion, 'left');
  const rightHandLocomotionStyle = getLocomotionHandAnimation(locomotion, 'right');
  const legOrigins = [28, 40, 56, 68] as const;
  const legLocomotionStyles = legOrigins.map((_, index) => getLocomotionLegAnimation(locomotion, index));

  const handlePointerMove = (event: React.MouseEvent<HTMLDivElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    if (!rect.width || !rect.height) {
      return;
    }

    const normalizedX = ((event.clientX - rect.left) / rect.width - 0.5) * 2;
    const normalizedY = ((event.clientY - rect.top) / rect.height - 0.5) * 2;

    setPointerOffset({
      x: Math.max(-1, Math.min(1, normalizedX)),
      y: Math.max(-1, Math.min(1, normalizedY)),
    });

    const now = Date.now();
    const nextTrail = [
      ...pointerTrailRef.current,
      { x: normalizedX, y: normalizedY, t: now },
    ]
      .filter((sample) => now - sample.t <= DIZZY_MAX_TRAIL_AGE_MS)
      .slice(-DIZZY_MAX_TRAIL_POINTS);

    pointerTrailRef.current = nextTrail;

    if (!isDizzy && nextTrail.length >= DIZZY_MIN_SAMPLES) {
      let totalAngle = 0;
      let totalRadius = 0;

      for (let index = 1; index < nextTrail.length; index += 1) {
        const previous = nextTrail[index - 1];
        const current = nextTrail[index];
        const previousAngle = Math.atan2(previous.y, previous.x);
        const currentAngle = Math.atan2(current.y, current.x);
        let delta = currentAngle - previousAngle;

        if (delta > Math.PI) {
          delta -= Math.PI * 2;
        } else if (delta < -Math.PI) {
          delta += Math.PI * 2;
        }

        totalAngle += delta;
        totalRadius += Math.hypot(current.x, current.y);
      }

      const averageRadius = totalRadius / (nextTrail.length - 1);
      if (
        Math.abs(totalAngle) > DIZZY_MIN_TURN_RADIANS &&
        averageRadius > DIZZY_MIN_AVERAGE_RADIUS
      ) {
        setIsDizzy(true);
        pointerTrailRef.current = [];
      }
    }
  };

  return (
    <div
      ref={rootRef}
      className={cn('relative inline-flex items-center justify-center', className)}
      style={{ width: size, height: size, cursor: 'pointer' }}
      data-paused={offScreen || reducedMotion ? 'true' : undefined}
      role="img"
      aria-label={label}
      data-testid={testId}
      data-hovered={isHovered ? 'true' : 'false'}
      data-pressed={isPressed ? 'true' : 'false'}
      data-tapped={isTapped ? 'true' : 'false'}
      data-dizzy={isDizzy ? 'true' : 'false'}
      data-startled={effectiveStartled ? 'true' : 'false'}
      data-locked-on={effectiveLockedOn ? 'true' : 'false'}
      data-external-attention={externalAttentionState ?? 'none'}
      data-locomotion={locomotion?.style ?? 'none'}
      data-locomotion-phase={locomotion?.phase ?? 'idle'}
      data-locomotion-direction={locomotion?.direction ?? 'forward'}
      onMouseEnter={() => {
        setIsHovered(true);
        setIsStartled(true);
      }}
      onMouseLeave={() => {
        setIsHovered(false);
        setIsPressed(false);
        setIsStartled(false);
        setIsLockedOn(false);
        setPointerOffset({ x: 0, y: 0 });
        pointerTrailRef.current = [];
      }}
      onMouseMove={handlePointerMove}
      onMouseDown={() => setIsPressed(true)}
      onMouseUp={() => setIsPressed(false)}
      onClick={() => setIsTapped(true)}
    >
      <style>{gizziStyles}</style>
      <svg
        viewBox="0 0 96 96"
        width={size}
        height={size}
        className="gizzi-hs"
        style={{ overflow: wearing ? 'visible' : undefined, animation: locomotionActive ? undefined : 'gizzi-float 6.2s ease-in-out infinite' }}
        aria-hidden="true"
        shapeRendering="geometricPrecision"
      >
        <ellipse cx="48" cy="86" rx="21" ry="4" fill="rgba(9, 11, 14, 0.14)" />

        {isDizzy ? (
          <g data-testid="gizzi-dizzy-stars">
            {[0, 120, 240].map((angle, index) => (
              <g
                key={angle}
                style={{
                  transformOrigin: '48px 28px',
                  animation: `gizzi-dizzy-orbit ${1.1 + index * 0.2}s linear infinite`,
                  animationDelay: `${index * 80}ms`,
                }}
              >
                <g transform={`rotate(${angle} 48 28)`}>
                  <path
                    d="M48 12L49.4 15.6L53 17L49.4 18.4L48 22L46.6 18.4L43 17L46.6 15.6L48 12Z"
                    fill="#D97757"
                  />
                </g>
              </g>
            ))}
          </g>
        ) : null}

        {effectiveStartled ? (
          <g data-testid="gizzi-startled-marks">
            <circle
              cx="48"
              cy="44"
              r="19"
              fill="none"
              stroke="rgba(217, 119, 87, 0.4)"
              strokeWidth="2"
              style={{
                transformOrigin: '48px 44px',
                animation: 'gizzi-startled-shockwave 560ms cubic-bezier(0.22, 1, 0.36, 1)',
              }}
            />
            {[
              'M24 18L18 8',
              'M34 13L31 4',
              'M48 11V2',
              'M62 13L65 4',
              'M72 18L78 8',
            ].map((path, index) => (
              <path
                key={path}
                d={path}
                stroke="#D97757"
                strokeWidth="3.2"
                strokeLinecap="round"
                style={{
                  transformOrigin: '48px 20px',
                  animation: 'gizzi-startled-burst 560ms cubic-bezier(0.22, 1, 0.36, 1)',
                  animationDelay: `${index * 28}ms`,
                }}
              />
            ))}
          </g>
        ) : null}

        {effectiveLockedOn ? (
          <g data-testid="gizzi-locked-on-marks">
            <g
              style={{
                transformOrigin: '48px 47px',
                animation: 'gizzi-locked-frame-pulse 980ms ease-in-out infinite',
              }}
            >
              <path d="M17 31H26M17 31V42" stroke="#D97757" strokeWidth="2.4" strokeLinecap="round" />
              <path d="M79 31H70M79 31V42" stroke="#D97757" strokeWidth="2.4" strokeLinecap="round" />
              <path d="M17 63H26M17 63V52" stroke="#D97757" strokeWidth="2.4" strokeLinecap="round" />
              <path d="M79 63H70M79 63V52" stroke="#D97757" strokeWidth="2.4" strokeLinecap="round" />
              <rect
                x="26.5"
                y="28.5"
                width="43"
                height="37"
                rx="12"
                fill="none"
                stroke="rgba(217, 119, 87, 0.28)"
                strokeWidth="1.6"
                strokeDasharray="4 6"
              />
            </g>
            <rect
              x="28"
              y="38"
              width="40"
              height="2.8"
              rx="1.4"
              fill="rgba(217, 119, 87, 0.78)"
              style={{
                transformOrigin: '48px 47px',
                animation: 'gizzi-locked-scan 860ms cubic-bezier(0.33, 1, 0.68, 1) infinite',
              }}
            />
            <circle cx="48" cy="47" r="2.4" fill="#D97757" opacity="0.88" />
          </g>
        ) : null}

        <motion.g
          style={{ originX: '50%', originY: '100%' }}
          animate={wearing && hsAnimate && hsLanding ? { scaleY: 0.95, scaleX: 1.03 } : { scaleY: 1, scaleX: 1 }}
          transition={{ duration: hsAnimate ? 0.09 : 0 }}
        >
        <g
          data-testid="gizzi-body"
          style={{
            transformOrigin: '48px 48px',
            animation: locomotionBodyStyle?.animation ?? profile.bodyAnimation,
            transform: `translate(${bodyTranslateX}px, ${bodyTranslateY}px) rotate(${bodyRotate}deg) scale(${hoverScale})`,
            transition: 'transform 90ms cubic-bezier(0.22, 1, 0.36, 1)',
          }}
        >
          <rect
            x="44"
            y="8"
            width="8"
            height="5"
            rx="2"
            fill="#D97757"
            style={{
              transformOrigin: '48px 10px',
              animation: profile.beaconAnimation,
              transform: `scale(${beaconScale})`,
              transition: 'transform 120ms ease-out',
            }}
          />

          <rect x="24" y="15" width="17" height="7" rx="3" fill="#D4B08C" />
          <rect x="55" y="15" width="17" height="7" rx="3" fill="#D4B08C" />

          <path
            d="M25 23H71L78 30V56L72 63V69L64 76H32L24 69V63L18 56V30L25 23Z"
            fill="#D4B08C"
          />

          <AnimatePresence>
            {wearing && (
              <motion.g
                key="hs-cups"
                data-testid="gizzi-headset-cups"
                style={{ originX: '50%', originY: '50%' }}
                initial={hsAnimate ? { y: -20, scaleX: 1.22, opacity: 0 } : false}
                animate={{ y: 0, scaleX: 1, opacity: 1 }}
                exit={reducedMotion ? { opacity: 0 } : { y: -20, scaleX: 1.12, opacity: 0, transition: { duration: 0.28, ease: 'easeIn', delay: 0.12 } }}
                transition={hsSpring}
              >
                <rect x="11" y="26" width="10" height="16" rx="3" fill="var(--gizzi-hs)" />
                <rect x="19" y="27" width="3" height="14" rx="1" fill="#B8946E" />
                <rect x="75" y="26" width="10" height="16" rx="3" fill="var(--gizzi-hs)" />
                <rect x="74" y="27" width="3" height="14" rx="1" fill="#B8946E" />
              </motion.g>
            )}
          </AnimatePresence>

          <g
            style={{
              transformOrigin: '18px 49px',
              animation: leftHandLocomotionStyle?.animation ?? profile.handAnimation,
            }}
          >
            <path d="M18 40H14V44H10V48H14V52H18V56H22V40H18Z" fill="#D4B08C" />
          </g>
          <g
            style={{
              transformOrigin: '78px 49px',
              animation: rightHandLocomotionStyle?.animation ?? profile.handAnimation,
            }}
          >
            <path d="M78 40H82V44H86V48H82V52H78V56H74V40H78Z" fill="#D4B08C" />
          </g>

          <rect x="24" y="29" width="48" height="31" rx="9" fill="rgba(17, 19, 24, 0.16)" />

          <g
            data-testid="gizzi-eyes"
            style={{
              transform: `translate(${eyeTranslateX}px, ${eyeTranslateY}px)${presenceEyes?.blink ? ' scaleY(0.12)' : ''}`,
              transformOrigin: '48px 41px',
              transition: presenceEyes ? 'transform 120ms ease-out' : 'transform 90ms ease-out',
            }}
          >
            {renderEye('left', activeEyePreset)}
            {renderEye('right', activeEyePreset)}
          </g>

          <path
            d="M44.5 52L48 42L51.5 52H49.6L48.8 49.5H47.2L46.4 52H44.5ZM47.75 47.7H48.25L48 46.65L47.75 47.7Z"
            fill="#D97757"
          />
          <rect x="46" y="47.2" width="4" height="1.4" rx="0.7" fill="#D97757" />

          <text
            x="48"
            y="57.5"
            textAnchor="middle"
            fill="#D97757"
            fontSize="8.2"
            fontWeight="700"
            fontFamily='var(--font-mono)'
            letterSpacing="-0.4"
            data-testid="gizzi-mouth"
            style={hsTalking && !reducedMotion ? { transformBox: 'fill-box', transformOrigin: 'center', animation: 'gizzi-hs-mouth 0.42s ease-in-out infinite' } : undefined}
          >
            {mouthText}
          </text>

          <g
            style={{
              transformOrigin: '28px 74px',
              animation: legLocomotionStyles[0]?.animation,
            }}
          >
            <rect x="24" y="74" width="8" height="12" fill="#D4B08C" />
          </g>
          <g
            style={{
              transformOrigin: '40px 74px',
              animation: legLocomotionStyles[1]?.animation,
            }}
          >
            <rect x="36" y="74" width="8" height="12" fill="#D4B08C" />
          </g>
          <g
            style={{
              transformOrigin: '56px 74px',
              animation: legLocomotionStyles[2]?.animation,
            }}
          >
            <rect x="52" y="74" width="8" height="12" fill="#D4B08C" />
          </g>
          <g
            style={{
              transformOrigin: '68px 74px',
              animation: legLocomotionStyles[3]?.animation,
            }}
          >
            <rect x="64" y="74" width="8" height="12" fill="#D4B08C" />
          </g>

          <AnimatePresence>
            {wearing && (
              <motion.g
                key="hs-band"
                data-testid="gizzi-headset-band"
                initial={hsAnimate ? { y: -20, opacity: 0 } : false}
                animate={{ y: 0, opacity: 1 }}
                exit={reducedMotion ? { opacity: 0 } : { y: -20, opacity: 0, transition: { duration: 0.28, ease: 'easeIn', delay: 0.12 } }}
                transition={hsSpring}
              >
                <rect x="28" y="9" width="40" height="5" rx="2" fill="var(--gizzi-hs)" />
                <rect x="30" y="9" width="36" height="1.4" rx="0.7" fill="var(--gizzi-hs-hi)" />
                <rect x="20" y="12" width="10" height="5" rx="2" fill="var(--gizzi-hs)" />
                <rect x="66" y="12" width="10" height="5" rx="2" fill="var(--gizzi-hs)" />
                <rect x="14" y="16" width="7" height="11" rx="2" fill="var(--gizzi-hs)" />
                <rect x="75" y="16" width="7" height="11" rx="2" fill="var(--gizzi-hs)" />
                <rect x="44" y="8" width="8" height="5" rx="2" fill="#D97757" />
              </motion.g>
            )}
          </AnimatePresence>
          <AnimatePresence>
            {wearing && (
              <motion.g
                key="hs-boom"
                data-testid="gizzi-headset-boom"
                style={{ originX: '2px', originY: '1px' }}
                initial={hsAnimate ? { rotate: -95, y: -20, opacity: 0 } : false}
                animate={{ rotate: 0, y: 0, opacity: 1, transition: hsAnimate ? { y: hsSpring, opacity: { duration: 0.1 }, rotate: { type: 'spring', stiffness: 300, damping: 16, delay: 0.3 } } : { duration: 0 } }}
                exit={reducedMotion ? { opacity: 0 } : { rotate: -95, transition: { duration: 0.16, ease: 'easeIn' } }}
              >
                <rect x="17" y="40" width="4" height="14" rx="1.5" fill="var(--gizzi-hs)" />
                <rect x="18" y="52" width="17" height="3.5" rx="1.5" fill="var(--gizzi-hs)" />
                <motion.rect
                  x="33" y="50.5" width="7" height="6.5" rx="2.5"
                  fill={hsTalking ? 'var(--status-success)' : 'var(--gizzi-hs)'}
                  style={{ originX: '50%', originY: '50%' }}
                  animate={hsTalking && !reducedMotion ? { scale: [1, 1.25, 1] } : { scale: 1 }}
                  transition={{ repeat: hsTalking && !reducedMotion ? Infinity : 0, duration: 0.6 }}
                />
              </motion.g>
            )}
          </AnimatePresence>
        </g>
        </motion.g>
      </svg>
    </div>
  );
}



export default GizziMascot;
