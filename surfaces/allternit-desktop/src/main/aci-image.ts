/** Fixed ChatGPT image workflow. Never accepts page code, tokens or a download URL from IPC. */
import { app, BrowserWindow, dialog, nativeImage, session, powerMonitor, type WebContents } from 'electron';
import Store from 'electron-store';
import { createHash, randomUUID } from 'node:crypto';
import { mkdir, writeFile, readFile, stat } from 'node:fs/promises';
import path from 'node:path';

export const ACI_PRIVATE_PARTITION = 'allternit-aci-private';
const MAX_BYTES = 20 * 1024 * 1024;
const MAX_PNG_BYTES = 8 * 1024 * 1024;
export interface AciImageRequest { requestId: string; sessionId: string; prompt: string; timeoutMs?: number }
interface Attempt { sessionId: string; phase: string; conversation?: string }
interface Job { owner: WebContents; requestId: string; abort: AbortController }
let retained: BrowserWindow | undefined;
let active: Job | undefined;
let journal: Store<{ attempts: Record<string, Attempt> }> | undefined;
const attempts = () => journal ??= new Store<{ attempts: Record<string, Attempt> }>({ name: 'aci-image-attempts', defaults: { attempts: {} } });

export function validAciImageRequest(value: unknown): value is AciImageRequest {
  if (!value || typeof value !== 'object') return false;
  const r = value as AciImageRequest;
  return typeof r.requestId === 'string' && r.requestId.length > 0 && r.requestId.length <= 200
    && typeof r.sessionId === 'string' && r.sessionId.length > 0 && r.sessionId.length <= 200
    && typeof r.prompt === 'string' && r.prompt.trim().length > 0 && r.prompt.length <= 16000
    && (r.timeoutMs === undefined || (Number.isFinite(r.timeoutMs) && r.timeoutMs > 0 && r.timeoutMs <= 600000));
}

export function cancelAciImage(owner: WebContents, requestId: string): void {
  if (active?.owner === owner && active.requestId === requestId) active.abort.abort();
}
export async function teardownAciImages(): Promise<void> {
  active?.abort.abort();
  for (const child of authWindows) if (!child.isDestroyed()) child.destroy();
  authWindows.clear();
  retained?.destroy(); retained = undefined;
  if (app.isReady()) await session.fromPartition(ACI_PRIVATE_PARTITION).clearStorageData();
}

const fail = (error: string, code: string) => ({ ok: false as const, error, code });
const delay = (signal: AbortSignal) => new Promise<void>((resolve, reject) => {
  if (signal.aborted) { reject(new Error('Image generation stopped. Check ChatGPT before starting another job.')); return; }
  const stop = () => { clearTimeout(timer); reject(new Error('Image generation stopped. Check ChatGPT before starting another job.')); };
  const timer = setTimeout(() => { signal.removeEventListener('abort', stop); resolve(); }, 1000);
  signal.addEventListener('abort', stop, { once: true });
});

// Evaluated only on the fixed provider origin. No script from a request is executed.
const INSPECT = `(() => {
 const visible = e => !!(e && e.getClientRects().length);
 const text = document.body?.innerText || '';
 if (/verify you are human|checking your browser|unusual activity|captcha/i.test(text)) return {needs: 'Complete the verification in the ACI ChatGPT window.'};
 const editor = document.querySelector('#prompt-textarea[contenteditable="true"], textarea#prompt-textarea');
 const users = [...document.querySelectorAll('[data-message-author-role="user"], [data-conversation-role="user"], [data-user-message-bubble]')].filter(e => !e.parentElement?.closest('[data-message-author-role="user"], [data-conversation-role="user"], [data-user-message-bubble]'));
 const turns = [...document.querySelectorAll('[data-message-author-role="assistant"], [data-conversation-role="assistant"]')].filter(e => !e.parentElement?.closest('[data-message-author-role="assistant"], [data-conversation-role="assistant"]'));
 const candidate = turns.at(-1);
 const last = candidate && users.at(-1) && (users.at(-1).compareDocumentPosition(candidate) & Node.DOCUMENT_POSITION_FOLLOWING) ? candidate : undefined;
 const image = last && [...last.querySelectorAll('img')].find(e => /generated image/i.test(e.alt) && e.complete && e.naturalWidth >= 256);
 let preview;
 if (image && image.naturalWidth <= 4096 && image.naturalHeight <= 4096 && image.naturalWidth * image.naturalHeight <= 16000000) {
   try { const c = document.createElement('canvas'); const scale = Math.min(1, 320 / Math.max(image.naturalWidth, image.naturalHeight));
     c.width = Math.max(1, Math.round(image.naturalWidth * scale)); c.height = Math.max(1, Math.round(image.naturalHeight * scale));
     c.getContext('2d').drawImage(image, 0, 0, c.width, c.height); const data = c.toDataURL('image/jpeg', 0.6); if (data.length <= 400000) preview = data;
   } catch {}
 }
 const downloadable = last && last.querySelector('button[aria-label*="Download"],a[download]');
 const streaming = !!document.querySelector('[data-testid="stop-button"],button[aria-label="Stop streaming"],button[aria-label="Stop generating"]');
 return {ready: visible(editor), user: (users.at(-1)?.querySelector('[data-user-message-bubble]') || users.at(-1))?.textContent?.trim(), users: users.length, turns: turns.length,
   image: !streaming && downloadable && image ? image.currentSrc || image.src : undefined,
   preview, width: image?.naturalWidth, height: image?.naturalHeight};
})()`;

export async function runAciImage(owner: WebContents, input: unknown, account: () => string | undefined) {
  if (!validAciImageRequest(input)) return fail('Invalid ACI image request.', 'invalid_request');
  if (active) return fail('Another ACI image job owns this browser. Stop it first.', 'busy');
  const identity = account();
  if (!identity) return fail('Sign in to Allternit first.', 'needs_sign_in');
  const attemptKey = createHash('sha256').update(identity + '\0' + input.requestId).digest('hex');
  const previous = attempts().get('attempts')[attemptKey];
  if (previous) return fail('This image request was already attempted. Check its ChatGPT conversation; it will not be submitted again.', 'ambiguous_submission');
  const abort = new AbortController();
  active = { owner, requestId: input.requestId, abort };
  const timer = setTimeout(() => abort.abort('timeout'), input.timeoutMs ?? 600000);
  const ownerGone = () => abort.abort();
  owner.once('destroyed', ownerGone);
  const report = (stage: string, label: string, extra: Record<string, unknown> = {}) => {
    if (!owner.isDestroyed()) owner.send('aci-image:progress', { requestId: input.requestId, lane: 'picture', stage, label, ...extra });
  };
  const record = (phase: string) => {
    const saved = attempts().get('attempts');
    // Never prune submission identities: deleting one would permit a replay to send again.
    attempts().set('attempts', { ...saved, [attemptKey]: { sessionId: input.sessionId, phase, conversation: retained?.webContents.getURL() } });
  };
  const evaluate = <T = any>(script: string): Promise<T> => new Promise((resolve, reject) => {
    const stop = () => { abort.signal.removeEventListener('abort', stop); reject(new Error('Image generation stopped. Check ChatGPT before starting another job.')); };
    if (abort.signal.aborted) { stop(); return; }
    abort.signal.addEventListener('abort', stop, { once: true });
    retained!.webContents.executeJavaScript(script).then(value => {
      abort.signal.removeEventListener('abort', stop); resolve(value);
    }, error => { abort.signal.removeEventListener('abort', stop); reject(error); });
  });
  const onSuspend = () => report('queued', 'Paused while this device sleeps');
  const onResume = () => report('queued', 'Reconnecting to ChatGPT in ACI');
  powerMonitor.on('suspend', onSuspend); powerMonitor.on('resume', onResume);
  let downloadProgress: ReturnType<typeof setInterval> | undefined;
  const check = (requireProvider = true) => {
    if (abort.signal.aborted) throw new Error('Image generation stopped. Check ChatGPT before starting another job.');
    if (identity !== account()) { void teardownAciImages(); throw new Error('Account changed. Image generation stopped.'); }
    if (!retained || retained.isDestroyed()) throw new Error('Open ACI and try again. The image browser closed.');
    if (requireProvider && new URL(retained.webContents.getURL()).origin !== 'https://chatgpt.com') throw new Error('Finish signing in to ChatGPT in ACI, then start a new image request.');
  };
  try {
    report('queued', 'Confirm the image prompt');
    const parent = BrowserWindow.fromWebContents(owner);
    if (!parent || !parent.isVisible()) return fail('Open the originating chat before generating an image.', 'not_visible');
    const confirmation = await dialog.showMessageBox(parent, { type: 'question', title: 'Create an image in ChatGPT?',
      message: 'Send this exact prompt to ChatGPT in this device’s private ACI browser?', detail: input.prompt,
      signal: abort.signal, buttons: ['Cancel', 'Create image'], defaultId: 0, cancelId: 0, noLink: true });
    if (confirmation.response !== 1 || abort.signal.aborted) return fail('Image generation cancelled.', 'cancelled');
    const newBrowser = !retained || retained.isDestroyed();
    if (newBrowser) {
      retained = new BrowserWindow({ title: 'ACI · ChatGPT image', width: 1050, height: 780,
        webPreferences: { partition: ACI_PRIVATE_PARTITION, nodeIntegration: false, contextIsolation: true, sandbox: true, webSecurity: true, backgroundThrottling: false } });
      protectBrowser(retained);
      // A job survives a return to chat. Explicit Stop owns cancellation.
      retained.on('close', event => { event.preventDefault(); retained?.hide(); });
    }
    if (!retained) return fail('The local ACI browser could not open.', 'unavailable');
    retained.show();
    report('queued', 'Opening ChatGPT in ACI');
    if (newBrowser) await new Promise<void>((resolve, reject) => {
      const stop = () => { retained?.webContents.stop(); reject(new Error('Image generation stopped.')); };
      if (abort.signal.aborted) { stop(); return; }
      abort.signal.addEventListener('abort', stop, { once: true });
      retained!.loadURL('https://chatgpt.com/').then(() => {
        abort.signal.removeEventListener('abort', stop); resolve();
      }, error => { abort.signal.removeEventListener('abort', stop); reject(error); });
    });
    let page: { ready?: boolean; needs?: string; user?: string; users?: number; turns?: number; image?: string; preview?: string; width?: number; height?: number };
    for (;;) {
      check(false);
      if (new URL(retained.webContents.getURL()).origin !== 'https://chatgpt.com') {
        report('queued', 'Needs you: finish ChatGPT sign-in in ACI');
        await delay(abort.signal); continue;
      }
      page = await evaluate(INSPECT);
      if (page.needs) { report('queued', `Needs you: ${page.needs}`); retained.show(); }
      else if (page.ready) break;
      else report('queued', 'Needs you: sign in to ChatGPT in the ACI window');
      await delay(abort.signal);
    }
    // Refuse an occupied conversation or draft rather than overwrite or confuse its output.
    if (new URL(retained.webContents.getURL()).searchParams.get('temporary-chat') === 'true') return fail('Images need a regular ChatGPT chat. Turn off Temporary Chat in ACI.', 'unsupported_ui');
    if (page.users || page.turns) return fail('ChatGPT did not open an empty regular chat. Open a new chat in ACI and try again.', 'unsupported_ui');
    check();
    // Select the explicit tool; never rewrite the person's prompt to request an image.
    const mode = await evaluate(`(() => {
      const visible = e => e.getClientRects().length;
      const controls = () => [...document.querySelectorAll('button,[role="menuitem"],[role="option"]')].filter(visible);
      const label = e => (e.getAttribute('aria-label') || e.textContent || '').trim();
      const selected = controls().some(e => /^Create image$/i.test(label(e)) && (e.getAttribute('aria-pressed') === 'true' || e.getAttribute('data-state') === 'active'));
      if (selected) return 'selected';
      const create = controls().find(e => /^Create image$/i.test(label(e)));
      if (create) { create.click(); return 'clicked'; }
      const tools = controls().find(e => /^(Tools|View tools|Add files and more|Add photos and files)$/i.test(label(e)));
      if (tools) { tools.click(); return 'menu'; }
      return 'unsupported';
    })()`);
    if (mode === 'menu') {
      await delay(abort.signal); check();
      const selected = await evaluate(`(() => {
        const item = [...document.querySelectorAll('button,[role="menuitem"],[role="option"]')].find(e => e.getClientRects().length && /^Create image$/i.test((e.getAttribute('aria-label') || e.textContent || '').trim()));
        if (!item) return false; item.click(); return true;
      })()`);
      if (!selected) return fail('ChatGPT’s explicit Create image mode is unavailable. Select it in ACI, then start a new request.', 'unsupported_ui');
    } else if (mode === 'unsupported') return fail('ChatGPT’s explicit Create image mode is unavailable. Check image access in ACI.', 'unsupported_ui');
    await delay(abort.signal); check();
    const modeReady = await evaluate(`(() => [...document.querySelectorAll('button,[role="button"],[data-testid]')].some(e => e.getClientRects().length && /^(Create image|Image|Remove create image|Remove image)$/i.test((e.getAttribute('aria-label') || e.textContent || '').trim()) && e.closest('form')))()`);
    if (!modeReady) return fail('Could not verify Create image mode in the composer. No prompt was sent.', 'unsupported_ui');
    record('submission_pending'); // durable BEFORE the only action which can send
    const submitted = await evaluate(`(() => {
      const editor = document.querySelector('#prompt-textarea');
      if (!editor || (editor.textContent || editor.value || '').trim()) return false;
      const prompt = ${JSON.stringify(input.prompt)};
      editor.focus();
      if (editor.tagName === 'TEXTAREA') {
        Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(editor, prompt);
        editor.dispatchEvent(new Event('input', {bubbles: true}));
      } else { document.execCommand('insertText', false, prompt); }
      return (editor.value || editor.textContent || '') === prompt;
    })()`);
    if (!submitted) return fail('ChatGPT’s composer changed or has a draft. No prompt was sent. Check ACI.', 'unsupported_ui');
    // React enables Send after its input state commits. Never retry the click.
    await delay(abort.signal); check();
    const clicked = await evaluate(`(() => {
      const editor = document.querySelector('#prompt-textarea');
      const exact = ${JSON.stringify(input.prompt)};
      if (!editor || (editor.value || editor.textContent || '') !== exact || document.querySelector('[data-message-author-role], [data-conversation-role], [data-user-message-bubble]')) return false;
      const button = document.querySelector('button[data-testid="send-button"],button[aria-label="Send prompt"],button[aria-label="Send message"]');
      if (!button || button.disabled || !button.getClientRects().length) return false; button.click(); return true;
    })()`);
    if (!clicked) return fail('ChatGPT’s Send control is unavailable. Check ACI before trying a new request.', 'unsupported_ui');
    record('submitted');
    report('drawing', 'Waiting for ChatGPT’s image');
    for (;;) {
      await delay(abort.signal); check(false);
      if (new URL(retained!.webContents.getURL()).origin !== 'https://chatgpt.com') { report('queued', 'Needs you: finish ChatGPT sign-in in ACI'); continue; }
      page = await evaluate(INSPECT);
      if (page.needs) { report('queued', `Needs you: ${page.needs}`); retained.show(); continue; }
      if (page.user && page.user !== input.prompt) return fail('The ChatGPT conversation changed. Image generation stopped.', 'conversation_changed');
      if ((page.users ?? 0) > 1) return fail('Another prompt was sent in ACI. Check the conversation before generating again.', 'conversation_changed');
      if (page.preview && page.user === input.prompt) report('rendering', 'ChatGPT is refining the picture', { preview: page.preview });
      if (page.image && page.user === input.prompt) break;
    }
    if (!page.width || !page.height || page.width > 4096 || page.height > 4096 || page.width * page.height > 16000000) return fail('ChatGPT returned an image outside the supported dimensions.', 'invalid_image');
    report('downloading', 'Downloading the finished image');
    // Fetch in the generating webContents, with its own authenticated context; no cookies leave it.
    downloadProgress = setInterval(() => {
      if (!retained || retained.isDestroyed() || abort.signal.aborted || new URL(retained.webContents.getURL()).origin !== 'https://chatgpt.com') return;
      void retained.webContents.executeJavaScript('window.__aciImageBytes').then(count => {
        if (!count || !Number.isFinite(count.bytes) || count.bytes < 0 || count.bytes > MAX_BYTES) return;
        report('downloading', 'Downloading the finished image', { bytes: count.bytes,
          ...(Number.isFinite(count.total_bytes) && count.total_bytes > 0 && count.total_bytes <= MAX_BYTES ? { total_bytes: count.total_bytes } : {}) });
      }).catch(() => {});
    }, 1000);
    const downloaded = await evaluate(`(async () => {
      const allowed = value => { const u = new URL(value); return u.protocol === 'blob:' ? u.origin === 'https://chatgpt.com' : u.protocol === 'https:' && (u.hostname === 'chatgpt.com' || u.hostname === 'oaiusercontent.com' || u.hostname.endsWith('.oaiusercontent.com')); };
      const url = ${JSON.stringify(page.image)};
      if (!allowed(url)) throw new Error('Unsupported ChatGPT image location. Use its Download control in ACI.');
      const controller = new AbortController(); window.__aciImageDownloadAbort = controller;
      try {
        const response = await fetch(url, {credentials: 'include', signal: controller.signal});
        if (!response.ok || !allowed(response.url)) throw new Error('ChatGPT image download failed.');
        const mime = (response.headers.get('content-type') || '').split(';')[0];
        if (!['image/png','image/jpeg','image/webp'].includes(mime)) throw new Error('ChatGPT did not return supported image bytes.');
        const total = Number(response.headers.get('content-length')) || undefined;
        if (total > ${MAX_BYTES}) throw new Error('Image exceeds 20 MiB.');
        const reader = response.body.getReader(); const chunks = []; let bytes = 0;
        for (;;) { const part = await reader.read(); if (part.done) break; bytes += part.value.length;
          if (bytes > ${MAX_BYTES}) { await reader.cancel(); throw new Error('Image exceeds 20 MiB.'); }
          chunks.push(part.value); window.__aciImageBytes = {bytes, total_bytes: total};
        }
        const data = new Uint8Array(bytes); let offset = 0;
        for (const chunk of chunks) { data.set(chunk, offset); offset += chunk.length; }
        let binary = ''; for (let i = 0; i < data.length; i += 8192) binary += String.fromCharCode(...data.subarray(i,i+8192));
        return {base64: btoa(binary), bytes, mime};
      } finally { delete window.__aciImageDownloadAbort; delete window.__aciImageBytes; }
    })()`);
    clearInterval(downloadProgress); downloadProgress = undefined;
    check();
    if (!downloaded || downloaded.bytes > MAX_BYTES || downloaded.base64?.length > Math.ceil(MAX_BYTES / 3) * 4) return fail('Image download exceeded the byte limit.', 'invalid_image');
    const source = Buffer.from(downloaded.base64, 'base64');
    if (!source.length || source.length !== downloaded.bytes || source.length > MAX_BYTES) return fail('Invalid image byte count.', 'invalid_image');
    const header = imageDimensions(source, downloaded.mime);
    if (!header || !safeDimensions(header.width, header.height)) return fail('Image header or dimensions are unsupported.', 'invalid_image');
    const image = nativeImage.createFromBuffer(source);
    const size = image.getSize();
    if (image.isEmpty() || size.width < 1 || size.height < 1 || size.width > 4096 || size.height > 4096 || size.width * size.height > 16000000) return fail('The downloaded image could not be decoded within the supported dimensions.', 'invalid_image');
    if (size.width !== header.width || size.height !== header.height) return fail('Decoded dimensions do not match the image header.', 'invalid_image');
    const png = image.toPNG();
    if (png.length > MAX_PNG_BYTES) return fail('The normalized image exceeds 8 MiB.', 'invalid_image');
    report('saving', 'Saving the picture', { bytes: downloaded.bytes, total_bytes: downloaded.bytes, preview: image.resize({ width: Math.max(1, Math.round(size.width * Math.min(1, 320 / Math.max(size.width, size.height)))), height: Math.max(1, Math.round(size.height * Math.min(1, 320 / Math.max(size.width, size.height)))) }).toDataURL() });
    check();
    const artifactId = randomUUID();
    const folder = artifactFolder(identity);
    await mkdir(folder, { recursive: true, mode: 0o700 });
    await writeFile(path.join(folder, artifactId + '.png'), png, { mode: 0o600, flag: 'wx' });
    // Durable origin and exact turns exist before a renderer or optional API sync sees success.
    await writeFile(path.join(folder, artifactId + '.json'), JSON.stringify({ requestId: input.requestId, sessionId: input.sessionId,
      user: { role: 'user', content: input.prompt }, assistant: { role: 'assistant', content: '', tool: 'media_generate',
        input: { kind: 'browser-image', engine: 'chatgpt-aci', prompt: input.prompt },
        output: { ok: true, artifactId, localUrl: `allternit-api://local/aci-images/${artifactId}.png`, ...size } }, createdAt: Date.now() }), { mode: 0o600, flag: 'wx' });
    check();
    record('complete');
    return { ok: true, artifactId, localUrl: `allternit-api://local/aci-images/${artifactId}.png`, image: `data:image/png;base64,${png.toString('base64')}`, ...size };
  } catch (error) {
    if (abort.signal.reason === 'timeout') return fail('ChatGPT did not return a supported finished image before the deadline. Open the ACI ChatGPT window to check sign-in, verification, plan access or a changed image UI. This request will not be resubmitted.', 'unsupported_ui');
    return fail(error instanceof Error ? error.message : 'ACI image generation failed.', abort.signal.aborted ? 'cancelled' : 'unsupported_ui');
  } finally {
    if (abort.signal.aborted && retained && !retained.isDestroyed()) {
      void retained.webContents.executeJavaScript(`window.__aciImageDownloadAbort?.abort(); document.querySelector('[data-testid="stop-button"]')?.click()`).catch(() => {});
    }
    clearInterval(downloadProgress);
    powerMonitor.removeListener('suspend', onSuspend); powerMonitor.removeListener('resume', onResume);
    clearTimeout(timer); owner.removeListener('destroyed', ownerGone); active = undefined;
  }
}

app.on('before-quit', () => { void teardownAciImages(); });

function safeDimensions(width: number, height: number): boolean {
  return width > 0 && height > 0 && width <= 4096 && height <= 4096 && width * height <= 16000000;
}
/** Header-only bounds before any native decoder allocation. Unknown/animated WebP is refused. */
export function imageDimensions(b: Buffer, mime: string): { width: number; height: number } | undefined {
  if (mime === 'image/png' && b.length >= 33 && b.subarray(0, 8).equals(Buffer.from([137,80,78,71,13,10,26,10]))
    && b.readUInt32BE(8) === 13 && b.toString('ascii', 12, 16) === 'IHDR') {
    return { width: b.readUInt32BE(16), height: b.readUInt32BE(20) };
  }
  if (mime === 'image/jpeg' && b[0] === 255 && b[1] === 216) {
    let at = 2;
    while (at + 4 <= b.length) {
      if (b[at++] !== 255) return;
      while (b[at] === 255) at++;
      const marker = b[at++];
      if (marker === 0xda || marker === 0xd9) return;
      if (marker === 0x01 || (marker >= 0xd0 && marker <= 0xd7)) continue;
      if (at + 2 > b.length) return;
      const size = b.readUInt16BE(at);
      if (size < 2 || at + size > b.length) return;
      // Only baseline and progressive Huffman JPEGs; reject other decoder modes.
      if (marker === 0xc0 || marker === 0xc2) {
        if (size < 8 || b[at + 2] !== 8) return;
        return { width: b.readUInt16BE(at + 5), height: b.readUInt16BE(at + 3) };
      }
      at += size;
    }
  }
  if (mime === 'image/webp' && b.length >= 30 && b.toString('ascii', 0, 4) === 'RIFF'
    && b.toString('ascii', 8, 12) === 'WEBP' && b.readUInt32LE(4) + 8 === b.length) {
    // Require the simple VP8/VP8L form. VP8X can carry inconsistent canvases or animation.
    const kind = b.toString('ascii', 12, 16);
    const chunkSize = b.readUInt32LE(16);
    if (chunkSize + (chunkSize % 2) + 20 !== b.length) return;
    if (kind === 'VP8 ' && b[23] === 0x9d && b[24] === 0x01 && b[25] === 0x2a)
      return { width: b.readUInt16LE(26) & 0x3fff, height: b.readUInt16LE(28) & 0x3fff };
    if (kind === 'VP8L' && b[20] === 0x2f) {
      const bits = b.readUInt32LE(21);
      if (bits >>> 29) return;
      return { width: (bits & 0x3fff) + 1, height: ((bits >>> 14) & 0x3fff) + 1 };
    }
  }
}

const authHosts = new Set(['chatgpt.com', 'auth.openai.com', 'auth0.openai.com', 'accounts.google.com', 'appleid.apple.com', 'account.apple.com', 'login.live.com', 'login.microsoftonline.com']);
function providerNavigation(value: string): boolean {
  try { const u = new URL(value); return u.protocol === 'https:' && authHosts.has(u.hostname); } catch { return false; }
}
const authWindows = new Set<BrowserWindow>();
function protectBrowser(window: BrowserWindow): void {
  window.webContents.on('will-navigate', (event, url) => { if (!providerNavigation(url)) event.preventDefault(); });
  window.webContents.on('will-redirect', (event, url) => { if (!providerNavigation(url)) event.preventDefault(); });
  window.webContents.setWindowOpenHandler(({ url }) => providerNavigation(url) ? {
    action: 'allow', overrideBrowserWindowOptions: { show: true, parent: window,
      webPreferences: { partition: ACI_PRIVATE_PARTITION, nodeIntegration: false, contextIsolation: true, sandbox: true, webSecurity: true } },
  } : { action: 'deny' });
  window.webContents.on('did-create-window', child => {
    authWindows.add(child); child.once('closed', () => authWindows.delete(child)); protectBrowser(child);
  });
}
/** Fixed visible operation, no caller-provided navigation or code. */
export function showAciImageBrowser(): boolean {
  if (!retained || retained.isDestroyed()) return false;
  retained.show(); retained.focus(); return true;
}
function artifactFolder(identity: string): string {
  return path.join(app.getPath('userData'), 'aci-images', createHash('sha256').update(identity).digest('hex'));
}
/** Opaque ID only; never exposes or accepts a filesystem path. Current account is supplied by main. */
export async function readAciImageArtifact(id: unknown, identity: string | undefined): Promise<Buffer | undefined> {
  if (!identity || typeof id !== 'string' || !/^[a-f0-9-]{36}\.png$/.test(id)) return;
  try {
    const file = path.join(artifactFolder(identity), id);
    const info = await stat(file);
    if (!info.isFile() || info.size > MAX_PNG_BYTES) return;
    const bytes = await readFile(file);
    return bytes.length <= MAX_PNG_BYTES ? bytes : undefined;
  } catch { return; }
}
