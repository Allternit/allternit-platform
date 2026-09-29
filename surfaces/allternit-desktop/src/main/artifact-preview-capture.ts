/**
 * Take a picture of a site an agent built (a Build session's HTML output),
 * rendered for real: an offscreen, sandboxed Chromium window runs its
 * scripts and loads its CDN CSS and fonts, then we capture it. The session
 * agent looks at the picture (pane_artifact) and fixes what's wrong.
 *
 * The window gets its own in-memory session, can't navigate, open windows,
 * download, or get any permission, and closes after one capture.
 */

export interface PreviewCaptureRequest {
  html: string;
  width?: number;
  height?: number;
  /** Grow to the page's full height (capped) instead of one viewport. */
  fullPage?: boolean;
}

export interface PreviewCaptureResult {
  ok: boolean;
  /** PNG (JPEG when large) data URL. */
  image?: string;
  width?: number;
  height?: number;
  error?: string;
}

/** The slice of Electron's BrowserWindow this uses (injected for tests). */
export interface CaptureWindow {
  webContents: {
    on(event: 'will-navigate', listener: (event: { preventDefault(): void }) => void): unknown;
    setWindowOpenHandler(handler: () => { action: 'deny' }): void;
    session: {
      setPermissionRequestHandler(handler: (wc: unknown, permission: string, callback: (granted: boolean) => void) => void): void;
      on(event: 'will-download', listener: (event: { preventDefault(): void }) => void): unknown;
    };
    executeJavaScript(code: string): Promise<unknown>;
    capturePage(): Promise<{ toPNG(): Buffer; toJPEG(quality: number): Buffer; isEmpty(): boolean; getSize(): { width: number; height: number } }>;
  };
  loadURL(url: string): Promise<void>;
  setContentSize(width: number, height: number): void;
  isDestroyed(): boolean;
  destroy(): void;
}

export type CreateCaptureWindow = (size: { width: number; height: number }) => CaptureWindow;

export const MAX_HTML_BYTES = 5 * 1024 * 1024;
export const MAX_FULL_PAGE_HEIGHT = 6000;
const CAPTURE_TIMEOUT_MS = 25_000;
/** Time after load for web fonts, CDN CSS and first paints to land. */
const SETTLE_MS = 900;
/** Above this, switch to JPEG so the model can take it. */
const MAX_PNG_BYTES = 3.5 * 1024 * 1024;

const clamp = (v: unknown, min: number, max: number, fallback: number) =>
  typeof v === 'number' && Number.isFinite(v) ? Math.round(Math.min(max, Math.max(min, v))) : fallback;

export async function captureArtifactPreview(
  req: PreviewCaptureRequest,
  createWindow: CreateCaptureWindow,
  options: { timeoutMs?: number; settleMs?: number } = {},
): Promise<PreviewCaptureResult> {
  if (typeof req?.html !== 'string' || !req.html.trim()) return { ok: false, error: 'There is no HTML to preview.' };
  if (Buffer.byteLength(req.html) > MAX_HTML_BYTES) return { ok: false, error: 'The page is too large to preview (over 5 MB).' };
  const width = clamp(req.width, 320, 2560, 1280);
  const height = clamp(req.height, 240, 2560, 800);

  let win: CaptureWindow | null = null;
  const run = async (): Promise<PreviewCaptureResult> => {
    win = createWindow({ width, height });
    win.webContents.on('will-navigate', (e) => e.preventDefault());
    win.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
    win.webContents.session.setPermissionRequestHandler((_wc, _perm, cb) => cb(false));
    win.webContents.session.on('will-download', (e) => e.preventDefault());

    await win.loadURL(`data:text/html;charset=utf-8;base64,${Buffer.from(req.html).toString('base64')}`);
    await new Promise((r) => setTimeout(r, options.settleMs ?? SETTLE_MS));

    let outHeight = height;
    if (req.fullPage) {
      const full = Number(await win.webContents.executeJavaScript(
        'Math.max(document.documentElement.scrollHeight, document.body ? document.body.scrollHeight : 0)',
      ));
      outHeight = clamp(full, height, MAX_FULL_PAGE_HEIGHT, height);
      if (outHeight !== height) {
        win.setContentSize(width, outHeight);
        await new Promise((r) => setTimeout(r, 250));
      }
    }

    const image = await win.webContents.capturePage();
    if (image.isEmpty()) return { ok: false, error: 'The page rendered nothing to capture.' };
    const png = image.toPNG();
    const size = image.getSize();
    if (png.length <= MAX_PNG_BYTES) {
      return { ok: true, image: `data:image/png;base64,${png.toString('base64')}`, width: size.width, height: size.height };
    }
    const jpg = image.toJPEG(80);
    return { ok: true, image: `data:image/jpeg;base64,${jpg.toString('base64')}`, width: size.width, height: size.height };
  };

  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      run(),
      new Promise<PreviewCaptureResult>((resolve) => {
        timer = setTimeout(
          () => resolve({ ok: false, error: 'The page took too long to load (over 25s).' }),
          options.timeoutMs ?? CAPTURE_TIMEOUT_MS,
        );
      }),
    ]);
  } catch (error) {
    return { ok: false, error: `The preview failed: ${error instanceof Error ? error.message : String(error)}` };
  } finally {
    if (timer) clearTimeout(timer);
    const w = win as CaptureWindow | null;
    if (w && !w.isDestroyed()) w.destroy();
  }
}
