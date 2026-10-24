/**
 * Browser API Capture Manager
 *
 * Records network traffic from the default Electron session (and every
 * webview session in use, e.g. ACI browser panes) and produces a
 * HAR-style archive that can be ingested by the platform's HAR-derived API
 * service. This lets users capture API calls made by a website in the ACI
 * browser and turn them into reusable contracts.
 *
 * Implementation notes:
 * - Electron's webRequest module only allows one listener per event type on a
 *   given session, so capture registers through `web-request-hub.ts` as an
 *   observer and never replaces the app's own routing/auth/CSP listeners.
 * - Only one capture session can be active at a time. This avoids listener
 *   collisions and keeps the UX simple.
 * - Response bodies are not captured by webRequest. We capture response
 *   metadata (status, headers, content-type, content-size). Full response-body
 *   capture requires a CDP attachment and is tracked as a follow-up.
 */

import {
  app,
  session,
  webContents,
  type Session,
  type WebContents,
  type OnBeforeRequestListenerDetails,
  type OnBeforeSendHeadersListenerDetails,
  type OnHeadersReceivedListenerDetails,
  type OnCompletedListenerDetails,
} from 'electron';
import log from 'electron-log';
import { onWebRequest } from './web-request-hub.js';

interface HarHeader {
  name: string;
  value: string;
}

interface HarParam {
  name: string;
  value: string;
}

interface HarPostData {
  mimeType: string;
  text: string;
}

interface HarRequest {
  method: string;
  url: string;
  headers: HarHeader[];
  queryString: HarParam[];
  postData?: HarPostData;
}

interface HarResponse {
  status: number;
  statusText: string;
  headers: HarHeader[];
  content?: {
    mimeType?: string;
    size?: number;
    text?: string;
  };
}

interface HarEntry {
  startedDateTime: string;
  time: number;
  request: HarRequest;
  response: HarResponse;
}

interface CaptureSession {
  id: string;
  startedAt: number;
  filterUrls?: string[];
  entries: HarEntry[];
  requestStates: Map<string | number, Partial<HarEntry>>;
}

const activeSessions = new Map<string, CaptureSession>();

// Electron webRequest allows one listener per event type. These global
// listeners dispatch to the active capture session.
let globalOnBeforeRequest: ((
  details: OnBeforeRequestListenerDetails,
  callback: (response: Electron.CallbackResponse) => void,
) => void) | null = null;
let globalOnBeforeSendHeaders: ((
  details: OnBeforeSendHeadersListenerDetails,
  callback: (response: Electron.BeforeSendResponse) => void,
) => void) | null = null;
let globalOnHeadersReceived: ((
  details: OnHeadersReceivedListenerDetails,
  callback: (response: Electron.HeadersReceivedResponse) => void,
) => void) | null = null;
let globalOnCompleted: ((details: OnCompletedListenerDetails) => void) | null = null;

function parseQueryString(urlString: string): HarParam[] {
  try {
    const url = new URL(urlString);
    return Array.from(url.searchParams.entries()).map(([name, value]) => ({ name, value }));
  } catch {
    return [];
  }
}

function headersToArray(headers: Record<string, string | string[]>): HarHeader[] {
  const result: HarHeader[] = [];
  for (const [name, value] of Object.entries(headers)) {
    if (Array.isArray(value)) {
      for (const v of value) {
        result.push({ name, value: v });
      }
    } else {
      result.push({ name, value: value ?? '' });
    }
  }
  return result;
}

function getHeaderValue(headers: HarHeader[], name: string): string | undefined {
  const lower = name.toLowerCase();
  const found = headers.find((h) => h.name.toLowerCase() === lower);
  return found?.value;
}

function parseContentSize(value: string | undefined): number | undefined {
  if (!value) return undefined;
  const parsed = Number(value);
  if (Number.isNaN(parsed)) return undefined;
  return parsed;
}

const LOCAL_HOSTS = new Set(['localhost', '127.0.0.1', '[::1]', '::1']);

/** The app's own traffic (renderer → local API, custom protocols) is never part of a walkthrough. */
function isAppTraffic(url: string, origin?: string): boolean {
  for (const raw of [url, origin]) {
    if (!raw) continue;
    try {
      const parsed = new URL(raw);
      if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return true;
      if (LOCAL_HOSTS.has(parsed.hostname)) return true;
    } catch {
      return true;
    }
  }
  return false;
}

function matchesFilter(url: string, filterUrls?: string[]): boolean {
  if (!filterUrls || filterUrls.length === 0) return true;
  return filterUrls.some((pattern) => {
    // Support simple glob patterns like *://example.com/*
    // For URL filtering, wildcards should match path separators too.
    const regex = new RegExp(
      '^' +
        pattern
          .replace(/\*\*\*/g, '.*')
          .replace(/\*\*/g, '.*')
          .replace(/\*/g, '.*')
          .replace(/\?/g, '.') +
        '$'
    );
    return regex.test(url);
  });
}

function buildHar(sessionData: CaptureSession): unknown {
  const pages: unknown[] = [];
  const hosts = new Set<string>();
  for (const entry of sessionData.entries) {
    try {
      hosts.add(new URL(entry.request.url).hostname);
    } catch {}
  }

  return {
    log: {
      version: '1.2',
      creator: { name: 'Allternit Browser Capture', version: '1.1.0' },
      pages,
      entries: sessionData.entries,
      comment: `Captured by Allternit Desktop for ${Array.from(hosts).join(', ')}`,
    },
  };
}

function getDefaultSession() {
  const sess = session.defaultSession;
  if (!sess) {
    throw new Error('No default Electron session available');
  }
  return sess;
}

function dispatchToSessions<T>(handler: (session: CaptureSession, details: T) => void, details: T): void {
  for (const sessionData of activeSessions.values()) {
    handler(sessionData, details);
  }
}

const attachedSessions = new Set<Session>();
const detachers: Array<() => void> = [];

function attachSession(sess: Session): void {
  if (attachedSessions.has(sess) || !globalOnBeforeRequest) return;
  attachedSessions.add(sess);
  detachers.push(
    onWebRequest(sess, 'onBeforeRequest', globalOnBeforeRequest),
    onWebRequest(sess, 'onBeforeSendHeaders', globalOnBeforeSendHeaders!),
    onWebRequest(sess, 'onHeadersReceived', globalOnHeadersReceived!),
    onWebRequest(sess, 'onCompleted', globalOnCompleted!),
  );
}

// Webviews opened mid-recording (a new ACI pane) bring their own session.
function onWebContentsCreated(_event: unknown, wc: WebContents): void {
  attachSession(wc.session);
}

function registerGlobalListeners(): void {
  if (globalOnBeforeRequest) return;

  const sess = getDefaultSession();

  globalOnBeforeRequest = (details, callback) => {
    const origin = details.webContents && !details.webContents.isDestroyed() ? details.webContents.getURL() : undefined;
    if (isAppTraffic(details.url, origin)) {
      callback({});
      return;
    }
    dispatchToSessions((sessionData) => {
      if (!matchesFilter(details.url, sessionData.filterUrls)) return;

      const entry: Partial<HarEntry> = {
        startedDateTime: new Date(details.timestamp).toISOString(),
        request: {
          method: details.method,
          url: details.url,
          headers: [],
          queryString: parseQueryString(details.url),
        },
      };

      if (details.uploadData && details.uploadData.length > 0) {
        const parts: string[] = [];
        let mimeType: string | undefined;
        for (const data of details.uploadData) {
          if ('blobUUID' in data) {
            // Cannot read blob data via webRequest; leave empty placeholder.
            parts.push('');
          } else if (data.bytes) {
            parts.push(data.bytes.toString('utf8'));
          }
        }
        // The uploadData does not carry a mime type directly. We leave it
        // undefined here and fill it in from the Content-Type header once
        // onBeforeSendHeaders fires.
        entry.request!.postData = { mimeType: mimeType ?? 'application/octet-stream', text: parts.join('') };
      }

      sessionData.requestStates.set(details.id, entry);
    }, details);
    callback({});
  };

  globalOnBeforeSendHeaders = (details, callback) => {
    dispatchToSessions((sessionData) => {
      const state = sessionData.requestStates.get(details.id);
      if (!state || !state.request) return;

      const headers = headersToArray(details.requestHeaders);
      state.request.headers = headers;

      if (state.request.postData) {
        const contentType = getHeaderValue(headers, 'content-type');
        if (contentType) {
          state.request.postData.mimeType = contentType.split(';')[0].trim();
        }
      }
    }, details);
    callback({});
  };

  globalOnHeadersReceived = (details, callback) => {
    dispatchToSessions((sessionData) => {
      const state = sessionData.requestStates.get(details.id);
      if (!state) return;

      const headers = headersToArray(details.responseHeaders ?? {});
      const contentType = getHeaderValue(headers, 'content-type');
      const contentLength = parseContentSize(getHeaderValue(headers, 'content-length'));

      state.response = {
        status: details.statusCode,
        statusText: details.statusLine?.split(' ').slice(2).join(' ') ?? '',
        headers,
        content: {
          mimeType: contentType ? contentType.split(';')[0].trim() : undefined,
          size: contentLength,
        },
      };
    }, details);
    callback({});
  };

  globalOnCompleted = (details) => {
    dispatchToSessions((sessionData) => {
      const state = sessionData.requestStates.get(details.id);
      if (!state || !state.request) return;

      const entry: HarEntry = {
        startedDateTime: state.startedDateTime ?? new Date().toISOString(),
        time: Date.now() - new Date(state.startedDateTime ?? Date.now()).getTime(),
        request: state.request,
        response: state.response ?? {
          status: details.statusCode,
          statusText: '',
          headers: [],
        },
      };

      sessionData.entries.push(entry);
      sessionData.requestStates.delete(details.id);
    }, details);
  };

  attachSession(sess);
  for (const wc of webContents.getAllWebContents()) attachSession(wc.session);
  app.on('web-contents-created', onWebContentsCreated);

  log.info('[BrowserCapture] Global listeners registered');
}

function unregisterGlobalListeners(): void {
  if (!globalOnBeforeRequest) return;

  app.removeListener('web-contents-created', onWebContentsCreated);
  for (const remove of detachers.splice(0)) remove();
  attachedSessions.clear();

  globalOnBeforeRequest = null;
  globalOnBeforeSendHeaders = null;
  globalOnHeadersReceived = null;
  globalOnCompleted = null;

  log.info('[BrowserCapture] Global listeners unregistered');
}

export function createCaptureSession(options?: { filterUrls?: string[] }): { sessionId: string } {
  if (activeSessions.size > 0) {
    throw new Error('A capture session is already active. Stop it before starting a new one.');
  }

  const id = `capture-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  activeSessions.set(id, {
    id,
    startedAt: Date.now(),
    filterUrls: options?.filterUrls,
    entries: [],
    requestStates: new Map(),
  });

  try {
    registerGlobalListeners();
  } catch (error) {
    activeSessions.delete(id);
    throw error;
  }

  log.info('[BrowserCapture] Started session', id);
  return { sessionId: id };
}

export function stopCaptureSession(sessionId: string): { har: string } | null {
  const sessionData = activeSessions.get(sessionId);
  if (!sessionData) return null;

  activeSessions.delete(sessionId);
  if (activeSessions.size === 0) {
    unregisterGlobalListeners();
  }

  const har = JSON.stringify(buildHar(sessionData));
  log.info('[BrowserCapture] Stopped session', sessionId, 'entries:', sessionData.entries.length);
  return { har };
}

export function listCaptureSessions(): Array<{ id: string; startedAt: number; entries: number }> {
  return Array.from(activeSessions.values()).map((s) => ({
    id: s.id,
    startedAt: s.startedAt,
    entries: s.entries.length,
  }));
}

export function isCaptureAvailable(): boolean {
  return session.defaultSession != null;
}

/**
 * Test-only helper: clears all active sessions and unregisters listeners.
 * Do not call this in production code.
 */
export function __resetForTests(): void {
  activeSessions.clear();
  unregisterGlobalListeners();
}
