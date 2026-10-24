import { beforeEach, describe, expect, it, vi } from 'vitest';

const webRequestHandlers = {
  onBeforeRequest: null as ((...args: unknown[]) => void) | null,
  onBeforeSendHeaders: null as ((...args: unknown[]) => void) | null,
  onHeadersReceived: null as ((...args: unknown[]) => void) | null,
  onCompleted: null as ((...args: unknown[]) => void) | null,
};

const mockWebRequest = {
  onBeforeRequest: vi.fn((handler) => {
    webRequestHandlers.onBeforeRequest = handler;
  }),
  onBeforeSendHeaders: vi.fn((handler) => {
    webRequestHandlers.onBeforeSendHeaders = handler;
  }),
  onHeadersReceived: vi.fn((handler) => {
    webRequestHandlers.onHeadersReceived = handler;
  }),
  onCompleted: vi.fn((handler) => {
    webRequestHandlers.onCompleted = handler;
  }),
};

const mockDefaultSession = {
  webRequest: mockWebRequest,
};

vi.mock('electron', () => ({
  app: { on: vi.fn(), removeListener: vi.fn() },
  webContents: { getAllWebContents: () => [] },
  session: {
    get defaultSession() {
      return mockDefaultSession;
    },
  },
}));

vi.mock('electron-log', () => ({
  default: { info: () => {}, warn: () => {}, error: () => {} },
}));

import {
  createCaptureSession,
  stopCaptureSession,
  isCaptureAvailable,
  listCaptureSessions,
  __resetForTests,
} from './browser-capture-manager.js';
import { __resetWebRequestHubForTests } from './web-request-hub.js';

beforeEach(() => {
  __resetForTests();
  __resetWebRequestHubForTests();
  webRequestHandlers.onBeforeRequest = null;
  webRequestHandlers.onBeforeSendHeaders = null;
  webRequestHandlers.onHeadersReceived = null;
  webRequestHandlers.onCompleted = null;
  vi.clearAllMocks();
});

describe('browser-capture-manager', () => {
  it('reports availability when a default session exists', () => {
    expect(isCaptureAvailable()).toBe(true);
  });

  it('registers global webRequest listeners when starting a session', () => {
    const { sessionId } = createCaptureSession();
    expect(sessionId).toMatch(/^capture-\d+-[a-z0-9]+$/);
    expect(mockWebRequest.onBeforeRequest).toHaveBeenCalledTimes(1);
    expect(mockWebRequest.onBeforeSendHeaders).toHaveBeenCalledTimes(1);
    expect(mockWebRequest.onHeadersReceived).toHaveBeenCalledTimes(1);
    expect(mockWebRequest.onCompleted).toHaveBeenCalledTimes(1);
  });

  it('prevents concurrent capture sessions', () => {
    createCaptureSession();
    expect(() => createCaptureSession()).toThrow('A capture session is already active');
    expect(listCaptureSessions()).toHaveLength(1);
  });

  it('records a single request/response pair into the HAR', () => {
    const { sessionId } = createCaptureSession();

    const requestId = 42;
    const timestamp = Date.now();

    // Simulate request lifecycle
    webRequestHandlers.onBeforeRequest!(
      {
        id: requestId,
        url: 'https://api.example.com/v1/items?limit=10',
        method: 'GET',
        timestamp,
      },
      () => {},
    );

    webRequestHandlers.onBeforeSendHeaders!(
      {
        id: requestId,
        requestHeaders: {
          'Accept': 'application/json',
          'Authorization': 'Bearer secret-token',
        },
      },
      () => {},
    );

    webRequestHandlers.onHeadersReceived!(
      {
        id: requestId,
        statusCode: 200,
        statusLine: 'HTTP/1.1 200 OK',
        responseHeaders: {
          'content-type': ['application/json'],
          'content-length': ['256'],
        },
      },
      () => {},
    );

    webRequestHandlers.onCompleted!({
      id: requestId,
      statusCode: 200,
    });

    const result = stopCaptureSession(sessionId);
    expect(result).not.toBeNull();

    const har = JSON.parse(result!.har);
    expect(har.log.version).toBe('1.2');
    expect(har.log.entries).toHaveLength(1);

    const entry = har.log.entries[0];
    expect(entry.request.method).toBe('GET');
    expect(entry.request.url).toBe('https://api.example.com/v1/items?limit=10');
    expect(entry.request.queryString).toEqual([
      { name: 'limit', value: '10' },
    ]);
    expect(entry.request.headers).toEqual([
      { name: 'Accept', value: 'application/json' },
      { name: 'Authorization', value: 'Bearer secret-token' },
    ]);
    expect(entry.response.status).toBe(200);
    expect(entry.response.statusText).toBe('OK');
    expect(entry.response.content.mimeType).toBe('application/json');
    expect(entry.response.content.size).toBe(256);
  });

  it('never removes the session listeners the app relies on when a session stops', () => {
    const { sessionId } = createCaptureSession();
    stopCaptureSession(sessionId);

    // Electron keeps one listener per event; nulling it would drop the app's
    // own API routing/auth/CSP handlers registered through the same hub.
    expect(mockWebRequest.onBeforeRequest).not.toHaveBeenCalledWith(null);
    expect(mockWebRequest.onCompleted).not.toHaveBeenCalledWith(null);
    const callback = vi.fn();
    webRequestHandlers.onBeforeRequest!({ id: 7, url: 'https://api.example.com/x', method: 'GET', timestamp: Date.now() }, callback);
    expect(callback).toHaveBeenCalledWith({});
  });

  it('skips the app\'s own local traffic', () => {
    const { sessionId } = createCaptureSession();
    webRequestHandlers.onBeforeRequest!({ id: 9, url: 'http://127.0.0.1:8013/api/v1/me', method: 'GET', timestamp: Date.now() }, () => {});
    webRequestHandlers.onCompleted!({ id: 9, url: 'http://127.0.0.1:8013/api/v1/me', statusCode: 200 });
    const har = JSON.parse(stopCaptureSession(sessionId)!.har);
    expect(har.log.entries).toHaveLength(0);
  });

  it('filters requests by URL pattern', () => {
    createCaptureSession({ filterUrls: ['*://api.example.com/*'] });

    const matchedId = 1;
    const unmatchedId = 2;

    webRequestHandlers.onBeforeRequest!(
      { id: matchedId, url: 'https://api.example.com/v1/items', method: 'GET', timestamp: Date.now() },
      () => {},
    );
    webRequestHandlers.onBeforeRequest!(
      { id: unmatchedId, url: 'https://cdn.example.com/style.css', method: 'GET', timestamp: Date.now() },
      () => {},
    );

    webRequestHandlers.onCompleted!({ id: matchedId, statusCode: 200 });
    webRequestHandlers.onCompleted!({ id: unmatchedId, statusCode: 200 });

    const sessions = listCaptureSessions();
    expect(sessions[0].entries).toBe(1);
  });

  it('captures POST request body and content-type header', () => {
    const { sessionId } = createCaptureSession();
    const requestId = 7;

    webRequestHandlers.onBeforeRequest!(
      {
        id: requestId,
        url: 'https://api.example.com/v1/items',
        method: 'POST',
        timestamp: Date.now(),
        uploadData: [{ bytes: Buffer.from('{"name":"test"}') }],
      },
      () => {},
    );

    webRequestHandlers.onBeforeSendHeaders!(
      {
        id: requestId,
        requestHeaders: {
          'Content-Type': 'application/json; charset=utf-8',
        },
      },
      () => {},
    );

    webRequestHandlers.onHeadersReceived!(
      { id: requestId, statusCode: 201, statusLine: 'HTTP/1.1 201 Created', responseHeaders: {} },
      () => {},
    );

    webRequestHandlers.onCompleted!({ id: requestId, statusCode: 201 });

    const result = stopCaptureSession(sessionId);
    const har = JSON.parse(result!.har);
    const entry = har.log.entries[0];

    expect(entry.request.postData.text).toBe('{"name":"test"}');
    expect(entry.request.postData.mimeType).toBe('application/json');
  });

  it('returns null when stopping an unknown session', () => {
    expect(stopCaptureSession('capture-unknown')).toBeNull();
  });
});
