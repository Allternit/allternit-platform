import { BrowserWindow, screen } from 'electron';

export interface PillBounds { x: number; y: number; width: number; height: number }
export interface PillState {
  botName: string;
  step?: number;
  /** Target window bounds in screen points. Main looks up the Safari window when absent. */
  bounds?: PillBounds;
}
export type PillAction = 'takeover' | 'stop';

const PILL_SCHEME = 'allternit-pill:';
const PILL_WIDTH = 440;
const PILL_HEIGHT = 34;

const esc = (s: string) => s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c] as string));

/** The pill's own page: white + neutral fill, graphite in dark mode, buttons are plain links main intercepts. */
export function pillHtml(state: PillState): string {
  const step = state.step && state.step > 0 ? ` · step ${Math.floor(state.step)}` : '';
  return `<!doctype html><meta charset="utf-8"><meta name="color-scheme" content="light dark"><style>
:root{--bg:#fff;--fg:#1a1a1a;--fill:#f0f0f0;--line:#d8d8d8;--dot:#1a1a1a}
@media(prefers-color-scheme:dark){:root{--bg:#232326;--fg:#f2f2f2;--fill:#34343a;--line:#45454c;--dot:#f2f2f2}}
html,body{margin:0;background:transparent;font:12px -apple-system,BlinkMacSystemFont,system-ui,sans-serif;overflow:hidden;-webkit-user-select:none}
.pill{box-sizing:border-box;height:${PILL_HEIGHT - 4}px;margin:2px;display:flex;align-items:center;gap:8px;padding:0 6px 0 12px;border:1px solid var(--line);border-radius:999px;background:var(--bg);color:var(--fg);box-shadow:0 1px 6px rgba(0,0,0,.18)}
.dot{width:7px;height:7px;border-radius:50%;background:var(--dot);animation:p 1.4s ease-in-out infinite}
@keyframes p{50%{opacity:.35}}@media(prefers-reduced-motion:reduce){.dot{animation:none}}
.t{flex:1;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
a{color:var(--fg);text-decoration:none;background:var(--fill);border-radius:999px;padding:3px 10px;font-weight:500;cursor:pointer}
a:hover{border-color:var(--line);filter:brightness(.96)}
</style><div class="pill" role="status"><span class="dot"></span><span class="t">${esc(state.botName)} is working${step}</span><a href="${PILL_SCHEME}//takeover">Take over</a><a href="${PILL_SCHEME}//stop">Stop</a></div>`;
}

/** Centered on the target window's top edge, kept on the display that holds it. */
export function pillPlacement(target: PillBounds, work: PillBounds): PillBounds {
  const width = Math.min(PILL_WIDTH, Math.max(260, target.width - 24));
  const x = Math.round(Math.min(Math.max(target.x + (target.width - width) / 2, work.x), work.x + work.width - width));
  const y = Math.round(Math.max(target.y + 4, work.y));
  return { x, y, width, height: PILL_HEIGHT };
}

/**
 * The "<bot> is working · step N · Take over · Stop" pill pinned to the top
 * edge of the agent's target window. Unlike the cursor badge window it takes
 * clicks (never focus). Clicks come back as `onAction`; main does not decide
 * what they mean, the cursor page owns the lease and the run.
 */
export function createAgentPillWindow(onAction: (action: PillAction) => void) {
  let win: BrowserWindow | null = null;
  let lastHtml = '';

  function ensure(): BrowserWindow {
    if (win && !win.isDestroyed()) return win;
    const next = new BrowserWindow({
      width: PILL_WIDTH, height: PILL_HEIGHT, title: 'Allternit Agent Status',
      frame: false, transparent: true, backgroundColor: '#00000000', hasShadow: false,
      resizable: false, movable: false, minimizable: false, maximizable: false, fullscreenable: false,
      focusable: false, skipTaskbar: true, alwaysOnTop: true, show: false,
      type: process.platform === 'darwin' ? 'panel' : undefined,
      webPreferences: { sandbox: true, contextIsolation: true, nodeIntegration: false },
    });
    next.setAlwaysOnTop(true, 'screen-saver');
    next.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true, skipTransformProcessType: true });
    next.setContentProtection(true);
    next.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
    next.webContents.on('will-navigate', (event, url) => {
      event.preventDefault();
      if (!url.startsWith(`${PILL_SCHEME}//`)) return;
      const action = url.slice(`${PILL_SCHEME}//`.length).replace(/\/$/, '');
      if (action === 'takeover' || action === 'stop') onAction(action);
    });
    next.on('closed', () => { if (win === next) { win = null; lastHtml = ''; } });
    win = next;
    return next;
  }

  return {
    show(state: PillState, bounds: PillBounds) {
      const w = ensure();
      const display = screen.getDisplayMatching(bounds);
      w.setBounds(pillPlacement(bounds, display.workArea));
      const html = pillHtml(state);
      if (html !== lastHtml) {
        lastHtml = html;
        void w.loadURL(`data:text/html;charset=utf-8,${encodeURIComponent(html)}`).catch(() => { lastHtml = ''; });
      }
      if (!w.isVisible()) w.showInactive();
    },
    hide() { if (win && !win.isDestroyed()) win.hide(); },
    window: () => win,
    destroy() { if (win && !win.isDestroyed()) win.destroy(); win = null; },
  };
}
