/**
 * Allternit Desktop — Startup Window (onboarding welcome + Clerk auth + loading)
 *
 * First launch / signed-out launches show a welcome step, then load the native
 * Clerk auth renderer for sign-in. Returning signed-in launches go straight to
 * the loading step.
 *
 * Design: splash option A — A://TERNIT wordmark, thin coral progress bar,
 * single status line, and a bottom "Details" toggle for the live service list.
 */

import { app, BrowserWindow } from 'electron';
import log from 'electron-log';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { openExternalAllowlisted } from './security.js';

const __dirname = dirname(fileURLToPath(import.meta.url));

export type StartupInitialStep = 'welcome' | 'loading';

export interface StartupWindowOptions {
  initialStep: StartupInitialStep;
}

const BRAND_NAME = 'Allternit';
const TAGLINE = 'Your AI platform, right on your desktop';
const TERMS_URL = 'https://allternit.com/terms';
const PRIVACY_URL = 'https://allternit.com/privacy';

// A://TERNIT wordmark, ink (dark-on-light) variant. Canonical source:
// surfaces/ai.allternit.com/public/brand/a-protocol/a-ternit-wordmark.svg
const WORDMARK_SVG = `<svg class="wordmark" width="510" height="50" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 510 50" shape-rendering="geometricPrecision"><rect x="20.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#D97757"/><rect x="20.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="10.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="30.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="0.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="10.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="30.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="40.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="0.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="40.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="0.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="40.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="60.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="60.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="80.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="80.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="90.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="100.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="100.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="120.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="120.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="130.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="140.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="140.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="160.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="170.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="180.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="190.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="200.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="180.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="180.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="180.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="180.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="220.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="230.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="240.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="250.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="260.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="220.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="220.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="230.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="240.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="250.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="220.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="220.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="230.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="240.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="250.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="260.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="280.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="290.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="300.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="310.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="280.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="320.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="280.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="290.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="300.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="310.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="280.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="300.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="280.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="310.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="320.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="340.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="380.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="340.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="350.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="380.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="340.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="360.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="380.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="340.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="370.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="380.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="340.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="380.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="410.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="420.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="430.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="420.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="420.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="420.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="410.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="420.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="430.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="460.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="470.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="480.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="490.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="500.75" y="0.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="480.75" y="10.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="480.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="480.75" y="30.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/><rect x="480.75" y="40.75" width="8.5" height="8.5" rx="1.5" fill="#141413"/></svg>`;

function buildStartupHtml(initialStep: StartupInitialStep): string {
  return `<!DOCTYPE html>
<html>
<head>
  <meta charset="UTF-8">
  <style>
    :root {
      --bg: #faf9f7;
      --panel: #fffefc;
      --border: #e1e5eb;
      --text: #1a1916;
      --muted: #74716b;
      --soft: #989590;
      --coral: #D97757;
      --up: #1f7a3a;
      --down: #9c2a25;
      --btn-primary-bg: #1a1916;
      --btn-primary-fg: #faf9f7;
    }
    * { margin: 0; padding: 0; box-sizing: border-box; }
    html, body { height: 100%; -webkit-app-region: no-drag; }
    body {
      font-family: 'Allternit Sans', Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
      background: var(--bg);
      color: var(--text);
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
      user-select: none;
      position: relative;
    }
    /* Drag only a title-bar strip. Putting drag on body (a flex container)
       makes Chromium swallow clicks on child buttons even with no-drag. */
    .drag-bar {
      -webkit-app-region: drag;
      position: fixed;
      top: 0;
      left: 0;
      right: 0;
      height: 52px;
      z-index: 1;
    }
    button, a, .btn {
      -webkit-app-region: no-drag;
      pointer-events: auto;
      position: relative;
      z-index: 2;
    }
    .step { display: none; flex-direction: column; align-items: center; width: 100%; -webkit-app-region: no-drag; }
    .step.active { display: flex; }
    .wordmark { width: 180px; height: auto; margin-bottom: 48px; }

    .tagline {
      font-size: 15px;
      color: var(--muted);
      margin-bottom: 56px;
      text-align: center;
    }
    .btn {
      width: 100%;
      max-width: 340px;
      padding: 12px 24px;
      border-radius: 10px;
      font-size: 14px;
      font-weight: 600;
      cursor: pointer;
      transition: all 0.15s ease;
      display: flex;
      align-items: center;
      justify-content: center;
      gap: 10px;
      font-family: inherit;
    }
    .btn-primary {
      background: var(--btn-primary-bg);
      color: var(--btn-primary-fg);
      border: 1px solid var(--btn-primary-bg);
    }
    .btn-primary:hover { opacity: 0.85; }
    .btn:disabled, .btn:disabled:hover { cursor: not-allowed; opacity: 0.5; }
    .footer-legal {
      position: absolute;
      bottom: 28px;
      left: 40px;
      right: 40px;
      font-size: 12px;
      color: var(--soft);
      text-align: center;
      line-height: 1.5;
    }
    .footer-legal a { color: var(--muted); text-decoration: none; }
    .footer-legal a:hover { color: var(--coral); }

    /* ---- progress ---- */
    .progress-wrap {
      display: flex;
      align-items: center;
      gap: 10px;
      width: 240px;
      margin-bottom: 14px;
    }
    .progress-track {
      flex: 1;
      height: 2px;
      background: var(--border);
      border-radius: 2px;
      overflow: hidden;
    }
    .progress-fill {
      height: 100%;
      background: var(--coral);
      border-radius: 2px;
      width: 0%;
      transition: width 0.3s ease;
    }
    .progress-pct {
      font-size: 11px;
      color: var(--soft);
      font-variant-numeric: tabular-nums;
      min-width: 30px;
      text-align: right;
    }
    .status {
      font-size: 13px;
      color: var(--muted);
      min-height: 20px;
      text-align: center;
    }

    /* ---- details toggle ---- */
    .details-btn {
      position: absolute;
      bottom: 24px;
      left: 0; right: 0;
      margin: 0 auto;
      width: fit-content;
      background: none;
      border: none;
      font-size: 11px;
      color: var(--soft);
      cursor: pointer;
      font-family: inherit;
      letter-spacing: 0.3px;
    }
    .details-btn:hover { color: var(--muted); }
    .details {
      margin-top: 36px;
      width: 240px;
      font-size: 11.5px;
    }
    .details.hidden { display: none; }
    .detail-row {
      display: flex;
      align-items: center;
      justify-content: space-between;
      padding: 5px 0;
      color: var(--muted);
    }
    .detail-row + .detail-row { border-top: 1px solid rgba(0,0,0,0.04); }
    .dot {
      width: 6px; height: 6px;
      border-radius: 50%;
      display: inline-block;
      margin-right: 8px;
      vertical-align: 1px;
      background: #c9c5bf;
    }
    .dot.up { background: var(--up); }
    .dot.down { background: var(--down); }
    .dot.wait { background: #d8d4cd; }
    .detail-state { color: var(--soft); font-variant-numeric: tabular-nums; text-align: right; }

    /* ---- folder grants ---- */
    .folders-title {
      font-size: 20px;
      font-weight: 600;
      color: var(--text);
      margin-bottom: 10px;
    }
    .folders-hint {
      font-size: 13px;
      color: var(--muted);
      max-width: 380px;
      text-align: center;
      margin-bottom: 14px;
      line-height: 1.5;
    }
    .folder-list { width: 380px; max-height: 180px; overflow-y: auto; margin-bottom: 10px; }
    .folder-empty {
      color: var(--soft);
      font-size: 12px;
      text-align: center;
      padding: 14px 0;
      border: 1px dashed var(--border);
      border-radius: 8px;
    }
    .folder-row {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 8px;
      padding: 6px 10px;
      border: 1px solid rgba(0,0,0,0.08);
      border-radius: 8px;
      margin-bottom: 6px;
      background: var(--panel);
    }
    .folder-path {
      font-size: 12px;
      color: var(--text);
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
      direction: rtl;
    }
    .folder-remove { border: none; background: none; color: var(--soft); cursor: pointer; font-size: 11px; font-family: inherit; }
    .folder-remove:hover { color: var(--down); }
    .folder-actions { display: flex; gap: 10px; margin-top: 14px; }
    .folder-actions .btn { border: 1px solid var(--border); background: var(--panel); color: var(--text); }
    .folder-actions .btn-primary { background: var(--btn-primary-bg); color: var(--btn-primary-fg); border-color: var(--btn-primary-bg); }

    .version {
      position: absolute;
      bottom: 12px;
      right: 16px;
      font-size: 11px;
      color: var(--soft);
    }
  </style>
</head>
<body>
  <div class="drag-bar"></div>

  <div class="step ${initialStep === 'welcome' ? 'active' : ''}" id="step-welcome">
    ${WORDMARK_SVG}
    <div class="tagline">${TAGLINE}</div>
    <button type="button" class="btn btn-primary" id="btn-get-started">Get started</button>
    <div class="footer-legal">
      By continuing, you agree to the
      <a href="${TERMS_URL}" target="_blank" rel="noreferrer">Terms of Service</a>
      and
      <a href="${PRIVACY_URL}" target="_blank" rel="noreferrer">Privacy Policy</a>.
    </div>
  </div>

  <div class="step ${initialStep === 'loading' ? 'active' : ''}" id="step-loading">
    ${WORDMARK_SVG}
    <div class="progress-wrap">
      <div class="progress-track"><div class="progress-fill" id="progress-fill"></div></div>
      <div class="progress-pct" id="progress-pct"></div>
    </div>
    <div class="status" id="status">Starting…</div>
    <div class="details hidden" id="details">
      <div class="detail-row" data-key="api"><span><span class="dot wait"></span>Allternit API</span><span class="detail-state">Starting…</span></div>
      <div class="detail-row" data-key="gateway"><span><span class="dot wait"></span>Gateway</span><span class="detail-state">Starting…</span></div>
      <div class="detail-row" data-key="gizzi"><span><span class="dot wait"></span>Gizzi Runtime</span><span class="detail-state">Starting…</span></div>
      <div class="detail-row" data-key="fabricWorker"><span><span class="dot wait"></span>Fabric Worker</span><span class="detail-state">Waiting…</span></div>
      <div class="detail-row" data-key="office"><span><span class="dot wait"></span>Office Engine</span><span class="detail-state">Waiting…</span></div>
      <div class="detail-row" data-key="platform"><span><span class="dot wait"></span>Platform</span><span class="detail-state">Waiting…</span></div>
    </div>
    <button class="details-btn" type="button" id="btn-details">Details</button>
  </div>

  <div class="step" id="step-folders">
    ${WORDMARK_SVG}
    <div class="folders-title">Grant workspace folders</div>
    <div class="folders-hint">
      Allternit Cowork only works in folders you grant. Pick one or more — you can change this later in Settings.
    </div>
    <div class="folder-list" id="folder-list"></div>
    <button type="button" class="btn" id="btn-add-folder">＋ Add folder</button>
    <div class="folder-actions">
      <button type="button" class="btn" id="btn-skip-folders">Skip for now</button>
      <button type="button" class="btn btn-primary" id="btn-save-folders" disabled>Save &amp; continue</button>
    </div>
  </div>

  <div class="version">v${app.getVersion()}</div>

  <script>
    (function () {
      var api = window.startup;
      var btn = document.getElementById('btn-get-started');
      if (btn) {
        btn.addEventListener('click', function (event) {
          event.preventDefault();
          event.stopPropagation();
          if (api && typeof api.startLogin === 'function') {
            api.startLogin();
          } else {
            console.error('startup preload is missing; Get started cannot start login');
          }
        });
      }
      if (!api) {
        console.error('window.startup is undefined');
        return;
      }

      // Details toggle (option A): shows/hides the live service list.
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
          var dot = row.querySelector('.dot');
          var label = row.querySelector('.detail-state');
          if (dot) {
            dot.className = 'dot ' + (state.status === 'up' ? 'up' : state.status === 'down' ? 'down' : 'wait');
          }
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
        if (node) {
          node.textContent = 'Ready';
          node.style.color = 'var(--text)';
        }
      });
      api.onError(function (message) {
        var node = document.getElementById('status');
        if (node) {
          node.textContent = 'Error: ' + message;
          node.style.color = 'var(--down)';
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
              return '<div class="folder-row"><span class="folder-path" title="' + f.replace(/"/g, '&quot;') + '">' + f + '</span>' +
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
            saveFolders.textContent = 'Save & continue';
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
      api.onFoldersShow(function () {
        document.getElementById('step-loading').classList.remove('active');
        document.getElementById('step-folders').classList.add('active');
        renderFolders();
      });
      api.onFoldersHide(function () {
        document.getElementById('step-folders').classList.remove('active');
        document.getElementById('step-loading').classList.add('active');
      });
    })();
  </script>
</body>
</html>`;
}

export function createStartupWindow(options: StartupWindowOptions): BrowserWindow {
  const window = new BrowserWindow({
    width: 560,
    height: 640,
    resizable: false,
    alwaysOnTop: true,
    titleBarStyle: 'hiddenInset',
    backgroundColor: '#faf9f7',
    show: false,
    webPreferences: {
      preload: join(__dirname, '../preload/startup.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });

  window.once('ready-to-show', () => {
    window.show();
  });

  window.webContents.on('console-message', (event) => {
    log.info(`[Startup] ${event.message} (${event.sourceId}:${event.lineNumber})`);
  });

  window.webContents.setWindowOpenHandler(({ url }) => {
    void openExternalAllowlisted(url);
    return { action: 'deny' };
  });

  window.loadURL(`data:text/html;charset=utf-8,${encodeURIComponent(buildStartupHtml(options.initialStep))}`);
  return window;
}

/**
 * Loads the native Clerk auth renderer into the startup window.
 * In packaged builds this is dist/renderer/auth/index.html; in development
 * Vite serves it from src/renderer/auth/index.html.
 */
