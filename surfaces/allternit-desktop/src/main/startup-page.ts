/**
 * Allternit Desktop — startup window page (pairing card, loading, folder grants)
 *
 * Pure HTML builders, no electron imports: startup-window.ts loads the page as
 * a data: URL, auth-manager.ts pushes a re-rendered pairing card on every
 * state change (pairing:render), and the tests and screenshots use the same
 * functions.
 *
 * Design: "Desktop pairing screens redesign" (approved 2026-11-18). App white,
 * graphite dark mode from prefers-color-scheme, the coral pixel is the only
 * colour. The header matches the in-app "Connect a computer" sheet.
 */

import {
  PAIRING_ACTION_LABELS,
  PAIRING_STEPS,
  pairingStepAt,
  type PairingAction,
  type PairingState,
} from './pairing-state.js';

export type StartupInitialStep = 'welcome' | 'loading';

export interface PairingComputerInfo {
  /** This computer's name, as pairing sends it. */
  computerName: string;
  /** "Linux", "macOS" or "Windows". */
  platformLabel: string;
}

export interface StartupPageOptions extends PairingComputerInfo {
  initialStep: StartupInitialStep;
  version: string;
  /** Pairing card to start on (Welcome when left out). */
  pairing?: PairingState;
}

export const TERMS_URL = 'https://allternit.com/terms';
export const PRIVACY_URL = 'https://allternit.com/privacy';
export const DEVICES_URL = 'https://platform.allternit.com/devices';
export const STATUS_URL = 'https://status.allternit.com';

/** Window background before the page paints, per system theme. */
export const STARTUP_BG_LIGHT = '#FFFFFF';
export const STARTUP_BG_DARK = '#232322';

// A://TERNIT wordmark, ink variant. Canonical source:
// surfaces/ai.allternit.com/public/brand/a-protocol/a-ternit-wordmark.svg
// The page's CSS repaints the ink rects with the theme's text colour.
const WORDMARK_SVG = `<svg class="wordmark" width="510" height="50" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 510 50" shape-rendering="geometricPrecision"><rect x="20.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#D97757"/><rect x="20.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="10.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="30.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="0.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="10.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="30.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="40.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="0.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="40.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="0.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="40.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="60.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="60.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="80.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="80.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="90.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="100.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="100.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="120.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="120.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="130.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="140.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="140.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="160.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="170.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="180.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="190.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="200.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="180.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="180.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="180.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="180.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="220.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="230.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="240.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="250.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="260.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="220.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="220.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="230.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="240.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="250.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="220.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="220.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="230.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="240.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="250.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="260.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="280.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="290.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="300.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="310.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="280.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="320.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="280.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="290.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="300.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="310.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="280.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="300.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="280.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="310.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="320.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="340.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="380.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="340.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="350.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="380.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="340.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="360.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="380.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="340.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="370.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="380.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="340.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="380.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="410.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="420.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="430.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="420.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="420.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="420.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="410.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="420.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="430.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="460.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="470.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="480.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="490.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="500.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="480.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="480.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="480.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="480.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/></svg>`;

const MARK_CELLS: Array<[number, number]> = [[20, 0], [10, 10], [30, 10], [0, 20], [10, 20], [30, 20], [40, 20], [0, 30], [40, 30], [0, 40], [40, 40]];

/** The A:// pixel mark (the wordmark's first letter) at `size` px. */
function pixelMark(size: number): string {
  const ink = MARK_CELLS
    .map(([x, y]) => `<rect x="${x + 0.75}" y="${y + 0.75}" width="8.5" height="8.5" rx="1.5" class="ink"/>`)
    .join('');
  return `<svg width="${size}" height="${size}" viewBox="0 0 50 50" aria-hidden="true">${ink}<rect x="20.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#D97757"/></svg>`;
}

const MONITOR_ICON = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></svg>';
const SWAP_ICON = '<svg class="swap" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M7 7h13l-3-3M17 17H4l3 3"/></svg>';
const CHECK = '<span class="check"><svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke-width="4"><path d="M5 12l5 5 9-10"/></svg></span>';
const SPINNER = '<span class="spinner" aria-hidden="true"></span>';
const DOT = '<span class="dot"></span>';
const SUCCESS_ICON = '<svg width="18" height="18" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="11" class="success-fill"/><path d="M7 12l3.5 3.5L17 9" stroke="#fff" stroke-width="2.4" fill="none"/></svg>';
const ERROR_ICON = '<svg class="alert-icon" width="18" height="18" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="11" class="error-fill"/><path d="M12 7v6M12 16.5v.5" stroke="#fff" stroke-width="2.4" stroke-linecap="round"/></svg>';

export function escapeHtml(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function steps(at: number): string {
  const rows = PAIRING_STEPS.map((label, i) => {
    const icon = i < at ? CHECK : i === at ? SPINNER : DOT;
    const state = i < at ? 'done' : i === at ? 'current' : 'todo';
    return `<li class="${i <= at ? 'reached' : ''}" data-step="${state}"><span class="step-icon">${icon}</span>${label}</li>`;
  }).join('');
  return `<div class="steps-box"><ol class="steps">${rows}</ol></div>`;
}

function actionButton(action: PairingAction, kind: 'primary' | 'quiet', id?: string): string {
  const idAttr = id ? ` id="${id}"` : '';
  const label = PAIRING_ACTION_LABELS[action];
  if (action === 'status' || action === 'devices') {
    const url = action === 'status' ? STATUS_URL : DEVICES_URL;
    return `<button type="button" class="btn ${kind}"${idAttr} data-action="open-url" data-url="${url}">${label}</button>`;
  }
  return `<button type="button" class="btn ${kind}"${idAttr} data-action="${action === 'retry' ? 'start-login' : 'sign-in-again'}">${label}</button>`;
}

/**
 * The pairing card for one state. The primary sign-in button keeps the id
 * `btn-get-started` (Welcome and Error): the Linux lab driver clicks it.
 */
export function renderPairingCard(state: PairingState, info: PairingComputerInfo): string {
  const name = escapeHtml(info.computerName);
  const at = pairingStepAt(state);
  const title = state.phase === 'paired' ? `${name} is connected` : 'Connect this computer';
  const header = `
    <div class="brand">${WORDMARK_SVG}</div>
    <div class="header">
      <div class="logo">${pixelMark(18)}</div>
      ${SWAP_ICON}
      <div class="logo">${MONITOR_ICON}</div>
      <div class="header-text">
        <p class="title">${title}</p>
        <p class="sub">${name} · ${escapeHtml(info.platformLabel)} · Allternit Desktop</p>
      </div>
    </div>`;

  let body = '';
  switch (state.phase) {
    case 'welcome':
      body = `
    <p class="lead">Sign in with your Allternit account. Signing in pairs this computer, so you have no code to type. Your chats, bots and projects can then run here.</p>
    <div class="mt-24">${steps(-1)}</div>
    <div class="actions">
      <button type="button" class="btn primary" id="btn-get-started" data-action="start-login">Sign in to pair</button>
      <p class="legal">By continuing you agree to the <a href="${TERMS_URL}" target="_blank" rel="noreferrer">Terms</a> and <a href="${PRIVACY_URL}" target="_blank" rel="noreferrer">Privacy Policy</a>.</p>
    </div>`;
      break;
    case 'signing-in':
    case 'approving':
      body = `
    <div class="mt-24">${steps(at)}</div>
    <p class="note" role="status">${state.phase === 'signing-in'
      ? 'Finish signing in in the window that opened.'
      : 'Allternit is checking this computer with your account. This takes a few seconds.'}</p>
    <div class="actions"><button type="button" class="btn quiet" data-action="cancel">Cancel</button></div>`;
      break;
    case 'paired':
      body = `
    <p class="connected" role="status">${SUCCESS_ICON}Connected · ${name}</p>
    <div class="mt-16">${steps(at)}</div>
    <p class="note">${state.email ? `Signed in as ${escapeHtml(state.email)}. ` : ''}Manage your computers in Settings › Computers.</p>
    <div class="actions"><button type="button" class="btn primary" data-action="open-app">Open Allternit</button></div>`;
      break;
    case 'error': {
      const error = state.error;
      body = `
    <div class="mt-24">${steps(at)}</div>
    <div class="alert" role="alert">${ERROR_ICON}<div>
      <p class="alert-title">${escapeHtml(error?.title ?? "Couldn't pair this computer")}</p>
      <p class="alert-body">${escapeHtml(error?.body ?? 'Something went wrong while pairing. Try again.')}</p>
    </div></div>
    <div class="actions">
      ${actionButton(error?.primary ?? 'retry', 'primary', 'btn-get-started')}
      ${error?.secondary ? actionButton(error.secondary, 'quiet') : ''}
    </div>`;
      break;
    }
  }
  return `<div class="pairing" data-phase="${state.phase}">${header}${body}\n  </div>`;
}

const PAGE_CSS = `
    :root {
      color-scheme: light dark;
      --bg: ${STARTUP_BG_LIGHT};
      --elev: #FFFFFF;
      --text: #1F1E1D;
      --text-2: #6B6A68;
      --text-3: #8F8D8A;
      --fill: color-mix(in srgb, var(--text) 5%, transparent);
      --fill-strong: color-mix(in srgb, var(--text) 9%, transparent);
      --success: #22c55e;
      --error: #ef4444;
    }
    @media (prefers-color-scheme: dark) {
      :root {
        --bg: ${STARTUP_BG_DARK};
        --elev: #2C2C2B;
        --text: #ECECEB;
        --text-2: #A3A29F;
        --text-3: #7C7B78;
        --success: #4ade80;
        --error: #f87171;
      }
    }
    * { margin: 0; padding: 0; box-sizing: border-box; }
    html, body { height: 100%; -webkit-app-region: no-drag; }
    body {
      font-family: 'Allternit Sans', Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
      background: var(--bg);
      color: var(--text);
      user-select: none;
      position: relative;
      overflow: hidden;
      -webkit-font-smoothing: antialiased;
    }
    /* Drag only a title-bar strip. Putting drag on body (a flex container)
       makes Chromium swallow clicks on child buttons even with no-drag. */
    .drag-bar { -webkit-app-region: drag; position: fixed; top: 0; left: 0; right: 0; height: 24px; z-index: 1; }
    button, a, input { -webkit-app-region: no-drag; position: relative; z-index: 2; }
    a { color: inherit; }
    .ink, .wordmark rect:not([fill="#D97757"]) { fill: var(--text); }

    .step { display: none; height: 100%; }
    .step.active { display: flex; }

    /* ---- pairing card ---- */
    #step-welcome { flex-direction: column; }
    .pairing { display: flex; flex-direction: column; height: 100%; padding: 28px 32px 24px; }
    .brand { display: flex; justify-content: center; margin-bottom: 28px; }
    .brand .wordmark { width: 120px; height: auto; }
    .header { display: flex; align-items: center; gap: 10px; }
    .logo {
      display: flex; align-items: center; justify-content: center; flex-shrink: 0;
      width: 30px; height: 30px; border-radius: 8px;
      border: 1px solid color-mix(in srgb, var(--text) 10%, transparent);
      background: var(--elev); color: var(--text);
    }
    .swap { color: var(--text-3); flex-shrink: 0; }
    .header-text { min-width: 0; flex: 1; }
    .title { font-size: 15px; font-weight: 600; line-height: 1.4; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .sub { font-size: 12px; line-height: 1.35; color: var(--text-2); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .lead { margin-top: 24px; font-size: 13px; line-height: 1.625; color: var(--text-2); }
    .mt-24 { margin-top: 24px; }
    .mt-16 { margin-top: 16px; }
    .steps-box { border-radius: 12px; background: var(--fill); padding: 16px; }
    .steps { list-style: none; display: flex; flex-direction: column; gap: 10px; }
    .steps li { display: flex; align-items: center; gap: 10px; font-size: 13px; line-height: 1.4; color: var(--text-3); }
    .steps li.reached { color: var(--text); }
    .step-icon { width: 16px; height: 16px; display: flex; align-items: center; justify-content: center; flex-shrink: 0; }
    .check { width: 16px; height: 16px; border-radius: 50%; background: var(--text); display: flex; align-items: center; justify-content: center; }
    .check svg { stroke: var(--elev); }
    .dot { width: 16px; height: 16px; border-radius: 50%; border: 1.5px solid color-mix(in srgb, var(--text) 20%, transparent); }
    .spinner { width: 14px; height: 14px; border-radius: 50%; border: 2px solid var(--text-3); border-top-color: var(--text); animation: spin 1s linear infinite; }
    @keyframes spin { to { transform: rotate(360deg); } }
    @media (prefers-reduced-motion: reduce) { .spinner { animation-duration: 2.4s; } }
    .note { margin-top: 16px; font-size: 12px; line-height: 1.5; color: var(--text-2); }
    .connected {
      margin-top: 24px; display: flex; align-items: center; gap: 8px;
      border-radius: 12px; background: var(--fill); padding: 12px; font-size: 14px; font-weight: 500;
    }
    .success-fill { fill: var(--success); }
    .error-fill { fill: var(--error); }
    .alert {
      margin-top: 16px; display: flex; gap: 10px; border-radius: 12px; padding: 12px;
      border: 1px solid color-mix(in srgb, var(--error) 35%, transparent);
      background: color-mix(in srgb, var(--error) 7%, transparent);
    }
    .alert-icon { margin-top: 1px; flex-shrink: 0; }
    .alert-title { font-size: 13px; font-weight: 600; line-height: 1.4; }
    .alert-body { margin-top: 2px; font-size: 12px; line-height: 1.625; color: var(--text-2); }
    .actions { margin-top: auto; display: flex; flex-direction: column; gap: 8px; padding-top: 16px; }
    .btn {
      display: inline-flex; align-items: center; justify-content: center; gap: 8px;
      height: 36px; width: 100%; border-radius: 9999px; border: 0; padding: 0 16px;
      font-family: inherit; font-size: 13px; cursor: pointer; transition: opacity 0.15s ease, background-color 0.15s ease;
    }
    .btn.primary { background: var(--text); color: var(--elev); font-weight: 500; }
    .btn.primary:hover { opacity: 0.9; }
    .btn.quiet { background: var(--fill); color: var(--text); padding: 0 14px; }
    .btn.quiet:hover { background: var(--fill-strong); }
    .btn:disabled, .btn:disabled:hover { cursor: default; opacity: 0.6; }
    .btn:focus-visible { outline: 2px solid var(--text); outline-offset: 2px; }
    .legal { font-size: 11px; line-height: 1.5; color: var(--text-3); text-align: center; }
    .legal a { text-decoration: underline; text-underline-offset: 2px; }

    /* ---- loading ---- */
    #step-loading, #step-folders { flex-direction: column; align-items: center; justify-content: center; padding: 28px 32px 24px; }
    #step-loading .wordmark, #step-folders .wordmark { width: 150px; height: auto; margin-bottom: 40px; }
    .progress-wrap { display: flex; align-items: center; gap: 10px; width: 240px; margin-bottom: 14px; }
    .progress-track { flex: 1; height: 2px; background: var(--fill-strong); border-radius: 2px; overflow: hidden; }
    .progress-fill { height: 100%; background: var(--text); border-radius: 2px; width: 0%; transition: width 0.3s ease; }
    .progress-pct { font-size: 11px; color: var(--text-3); font-variant-numeric: tabular-nums; min-width: 30px; text-align: right; }
    .status { font-size: 13px; color: var(--text-2); min-height: 20px; text-align: center; }
    .status.error { color: var(--error); }
    .details-btn {
      position: absolute; bottom: 20px; left: 0; right: 0; margin: 0 auto; width: fit-content;
      background: none; border: none; font-size: 11px; color: var(--text-3); cursor: pointer; font-family: inherit;
    }
    .details-btn:hover { color: var(--text-2); }
    .details { margin-top: 32px; width: 240px; font-size: 11.5px; }
    .details.hidden { display: none; }
    .detail-row { display: flex; align-items: center; justify-content: space-between; padding: 5px 0; color: var(--text-2); }
    .detail-row + .detail-row { border-top: 1px solid var(--fill); }
    .svc-dot { width: 6px; height: 6px; border-radius: 50%; display: inline-block; margin-right: 8px; vertical-align: 1px; background: var(--fill-strong); }
    .svc-dot.up { background: var(--success); }
    .svc-dot.down { background: var(--error); }
    .detail-state { color: var(--text-3); font-variant-numeric: tabular-nums; text-align: right; }
    .version { position: absolute; bottom: 12px; right: 16px; font-size: 11px; color: var(--text-3); }

    /* ---- folder grants ---- */
    .folders-title { font-size: 15px; font-weight: 600; margin-bottom: 8px; }
    .folders-hint { font-size: 12px; color: var(--text-2); text-align: center; margin-bottom: 14px; line-height: 1.5; }
    .folder-list { width: 100%; max-height: 180px; overflow-y: auto; margin-bottom: 10px; }
    .folder-empty { color: var(--text-3); font-size: 12px; text-align: center; padding: 14px 0; border-radius: 12px; background: var(--fill); }
    .folder-row { display: flex; align-items: center; justify-content: space-between; gap: 8px; padding: 6px 10px; border-radius: 8px; margin-bottom: 6px; background: var(--fill); }
    .folder-path { font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; direction: rtl; }
    .folder-remove { border: none; background: none; color: var(--text-3); cursor: pointer; font-size: 11px; font-family: inherit; }
    .folder-remove:hover { color: var(--error); }
    .folder-actions { display: flex; gap: 8px; margin-top: 12px; width: 100%; }
`;

const PAGE_SCRIPT = `
    (function () {
      var api = window.startup;
      var card = document.getElementById('pairing-card');

      // Pairing card buttons (rendered by main; see renderPairingCard).
      document.addEventListener('click', function (event) {
        var target = event.target && event.target.closest ? event.target.closest('[data-action]') : null;
        if (!target) return;
        event.preventDefault();
        event.stopPropagation();
        var action = target.getAttribute('data-action');
        if (action === 'open-url') { window.open(target.getAttribute('data-url'), '_blank'); return; }
        if (!api) { console.error('startup preload is missing; cannot run ' + action); return; }
        if (action === 'start-login' || action === 'sign-in-again') api.startLogin();
        else if (action === 'cancel' && typeof api.cancelLogin === 'function') api.cancelLogin();
        else if (action === 'open-app') {
          target.disabled = true;
          target.textContent = 'Starting Allternit…';
          if (typeof api.openApp === 'function') api.openApp();
        }
      });
      if (!api) {
        console.error('window.startup is undefined');
        return;
      }
      if (typeof api.onPairing === 'function') {
        api.onPairing(function (html) { if (card && typeof html === 'string') card.innerHTML = html; });
      }

      // Details toggle: shows/hides the live service list.
      var details = document.getElementById('details');
      var detailsBtn = document.getElementById('btn-details');
      if (details && detailsBtn) {
        detailsBtn.addEventListener('click', function () {
          var hidden = details.classList.toggle('hidden');
          detailsBtn.textContent = hidden ? 'Details' : 'Hide details';
        });
      }

      api.onServices(function (services) {
        var rows = document.querySelectorAll('#details .detail-row');
        for (var i = 0; i < rows.length; i++) {
          var row = rows[i];
          var state = services && services[row.getAttribute('data-key')];
          if (!state) continue;
          var dot = row.querySelector('.svc-dot');
          var label = row.querySelector('.detail-state');
          if (dot) dot.className = 'svc-dot ' + (state.status === 'up' ? 'up' : state.status === 'down' ? 'down' : 'wait');
          if (label) label.textContent = state.detail || state.status;
        }
      });
      api.onStatus(function (message) {
        var node = document.getElementById('status');
        if (node) node.textContent = message;
      });
      api.onProgress(function (percent) {
        document.getElementById('progress-fill').style.width = percent + '%';
        document.getElementById('progress-pct').textContent = percent > 0 ? percent + '%' : '';
      });
      api.onComplete(function () {
        document.getElementById('progress-fill').style.width = '100%';
        document.getElementById('progress-pct').textContent = '100%';
        var node = document.getElementById('status');
        if (node) node.textContent = 'Ready';
      });
      api.onError(function (message) {
        var node = document.getElementById('status');
        if (node) {
          node.textContent = message;
          node.classList.add('error');
        }
      });

      // Folder-grant step (consumer-packaged Cowork P1).
      var grantedFolders = [];
      function renderFolders() {
        var list = document.getElementById('folder-list');
        var save = document.getElementById('btn-save-folders');
        if (!list || !save) return;
        list.innerHTML = grantedFolders.length === 0
          ? '<div class="folder-empty">No folders granted yet.</div>'
          : grantedFolders.map(function (f, i) {
              var safe = f.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;');
              return '<div class="folder-row"><span class="folder-path" title="' + safe + '">' + safe + '</span>' +
                '<button type="button" class="folder-remove" data-index="' + i + '">Remove</button></div>';
            }).join('');
        save.disabled = grantedFolders.length === 0;
        list.querySelectorAll('.folder-remove').forEach(function (btn) {
          btn.addEventListener('click', function () {
            grantedFolders.splice(Number(btn.getAttribute('data-index')), 1);
            renderFolders();
          });
        });
      }
      var addFolder = document.getElementById('btn-add-folder');
      if (addFolder) {
        addFolder.addEventListener('click', function () {
          if (typeof api.pickFolder !== 'function') return;
          api.pickFolder().then(function (folder) {
            if (folder && grantedFolders.indexOf(folder) === -1) {
              grantedFolders.push(folder);
              renderFolders();
            }
          });
        });
      }
      var saveFolders = document.getElementById('btn-save-folders');
      if (saveFolders) {
        saveFolders.addEventListener('click', function () {
          saveFolders.disabled = true;
          saveFolders.textContent = 'Saving…';
          api.saveFolders(grantedFolders.slice()).catch(function (err) {
            saveFolders.disabled = false;
            saveFolders.textContent = 'Save and continue';
            console.error('saveFolders failed', err);
          });
        });
      }
      var skipFolders = document.getElementById('btn-skip-folders');
      if (skipFolders) {
        skipFolders.addEventListener('click', function () {
          api.saveFolders(grantedFolders.slice());
        });
      }
      function showStep(id) {
        var all = document.querySelectorAll('.step');
        for (var i = 0; i < all.length; i++) all[i].classList.toggle('active', all[i].id === id);
      }
      api.onFoldersShow(function () { showStep('step-folders'); renderFolders(); });
      api.onFoldersHide(function () { showStep('step-loading'); });
    })();
`;

export function buildStartupHtml(options: StartupPageOptions): string {
  const pairing = options.pairing ?? { phase: 'welcome' };
  return `<!DOCTYPE html>
<html>
<head>
  <meta charset="UTF-8">
  <meta name="color-scheme" content="light dark">
  <title>Allternit</title>
  <style>${PAGE_CSS}</style>
</head>
<body>
  <div class="drag-bar"></div>

  <div class="step ${options.initialStep === 'welcome' ? 'active' : ''}" id="step-welcome">
    <div id="pairing-card" style="height:100%">${renderPairingCard(pairing, options)}</div>
  </div>

  <div class="step ${options.initialStep === 'loading' ? 'active' : ''}" id="step-loading">
    ${WORDMARK_SVG}
    <div class="progress-wrap">
      <div class="progress-track"><div class="progress-fill" id="progress-fill"></div></div>
      <div class="progress-pct" id="progress-pct"></div>
    </div>
    <div class="status" id="status" role="status">Starting…</div>
    <div class="details hidden" id="details">
      <div class="detail-row" data-key="api"><span><span class="svc-dot"></span>Allternit API</span><span class="detail-state">Starting…</span></div>
      <div class="detail-row" data-key="gateway"><span><span class="svc-dot"></span>Gateway</span><span class="detail-state">Starting…</span></div>
      <div class="detail-row" data-key="gizzi"><span><span class="svc-dot"></span>Gizzi Runtime</span><span class="detail-state">Starting…</span></div>
      <div class="detail-row" data-key="fabricWorker"><span><span class="svc-dot"></span>Fabric Worker</span><span class="detail-state">Waiting…</span></div>
      <div class="detail-row" data-key="office"><span><span class="svc-dot"></span>Office Engine</span><span class="detail-state">Waiting…</span></div>
      <div class="detail-row" data-key="platform"><span><span class="svc-dot"></span>Platform</span><span class="detail-state">Waiting…</span></div>
    </div>
    <button class="details-btn" type="button" id="btn-details">Details</button>
    <div class="version">v${escapeHtml(options.version)}</div>
  </div>

  <div class="step" id="step-folders">
    ${WORDMARK_SVG}
    <div class="folders-title">Choose folders Allternit can work in</div>
    <div class="folders-hint">Allternit Cowork only works in folders you allow. You can change this later in Settings.</div>
    <div class="folder-list" id="folder-list"></div>
    <button type="button" class="btn quiet" id="btn-add-folder">Add folder</button>
    <div class="folder-actions">
      <button type="button" class="btn quiet" id="btn-skip-folders">Skip for now</button>
      <button type="button" class="btn primary" id="btn-save-folders" disabled>Save and continue</button>
    </div>
  </div>

  <script>${PAGE_SCRIPT}</script>
</body>
</html>`;
}
