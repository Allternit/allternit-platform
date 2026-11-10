/**
 * Real PDF of an HTML page, made by Chromium itself (`webContents.printToPDF`)
 * in a throwaway hidden window. Used by Desktop's "Download PDF" for design
 * and dashboard artifacts and for the document templates, so the file is vector
 * text with page breaks instead of a screenshot or the OS print dialog.
 *
 * The window gets its own in-memory session, can't navigate, open windows,
 * download, or get any permission, and closes after one print.
 */

export interface PrintPdfRequest {
  html: string;
  /** Paper size; the page's own `@page` rule wins when it sets one. */
  pageSize?: 'Letter' | 'A4';
  landscape?: boolean;
  /** Print backgrounds (default true; templates draw filled headers). */
  printBackground?: boolean;
}

export interface PrintPdfResult {
  ok: boolean;
  /** Base64 of the PDF bytes (IPC-safe). */
  pdf?: string;
  bytes?: number;
  error?: string;
}

export interface PrintWindow {
  webContents: {
    on(event: 'will-navigate', listener: (event: { preventDefault(): void }) => void): unknown;
    setWindowOpenHandler(handler: () => { action: 'deny' }): void;
    session: {
      setPermissionRequestHandler(handler: (wc: unknown, permission: string, callback: (granted: boolean) => void) => void): void;
      on(event: 'will-download', listener: (event: { preventDefault(): void }) => void): unknown;
    };
    printToPDF(options: Record<string, unknown>): Promise<Buffer>;
  };
  loadURL(url: string): Promise<void>;
  isDestroyed(): boolean;
  destroy(): void;
}

export const MAX_PRINT_HTML_BYTES = 5 * 1024 * 1024;
const PRINT_TIMEOUT_MS = 30_000;
/** Time after load for web fonts and first paints to land. */
const SETTLE_MS = 700;

export async function printHtmlToPdf(
  req: PrintPdfRequest,
  createWindow: () => PrintWindow,
  options: { timeoutMs?: number; settleMs?: number } = {},
): Promise<PrintPdfResult> {
  if (typeof req?.html !== 'string' || !req.html.trim()) return { ok: false, error: 'There is nothing to print.' };
  if (Buffer.byteLength(req.html) > MAX_PRINT_HTML_BYTES) return { ok: false, error: 'The page is too large to export (over 5 MB).' };

  let win: PrintWindow | null = null;
  const run = async (): Promise<PrintPdfResult> => {
    win = createWindow();
    win.webContents.on('will-navigate', (e) => e.preventDefault());
    win.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
    win.webContents.session.setPermissionRequestHandler((_wc, _perm, cb) => cb(false));
    win.webContents.session.on('will-download', (e) => e.preventDefault());
    await win.loadURL(`data:text/html;charset=utf-8;base64,${Buffer.from(req.html).toString('base64')}`);
    await new Promise((r) => setTimeout(r, options.settleMs ?? SETTLE_MS));
    const data = await win.webContents.printToPDF({
      pageSize: req.pageSize === 'A4' ? 'A4' : 'Letter',
      landscape: req.landscape === true,
      printBackground: req.printBackground !== false,
      preferCSSPageSize: true,
    });
    return { ok: true, pdf: data.toString('base64'), bytes: data.length };
  };

  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      run(),
      new Promise<PrintPdfResult>((resolve) => {
        timer = setTimeout(() => resolve({ ok: false, error: 'The page took too long to export (over 30s).' }), options.timeoutMs ?? PRINT_TIMEOUT_MS);
      }),
    ]);
  } catch (error) {
    return { ok: false, error: `The PDF export failed: ${error instanceof Error ? error.message : String(error)}` };
  } finally {
    if (timer) clearTimeout(timer);
    const w = win as PrintWindow | null;
    if (w && !w.isDestroyed()) w.destroy();
  }
}
