import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('electron-log', () => ({ default: { info: () => {}, warn: () => {}, error: () => {} } }));

import { onWebRequest, __resetWebRequestHubForTests } from './web-request-hub.js';

function fakeSession() {
  const installed: Record<string, (...args: any[]) => void> = {};
  const webRequest = new Proxy({}, {
    get: (_t, event: string) => vi.fn((listener) => { installed[event] = listener; }),
  });
  return { session: { webRequest } as any, installed };
}

beforeEach(() => __resetWebRequestHubForTests());

describe('web-request-hub', () => {
  it('runs every handler instead of letting the last one replace the rest', () => {
    const { session, installed } = fakeSession();
    const seen: string[] = [];
    onWebRequest(session, 'onBeforeSendHeaders', (details, cb) => {
      seen.push('auth');
      cb({ requestHeaders: { ...details.requestHeaders, Authorization: 'Bearer t' } });
    });
    onWebRequest(session, 'onBeforeSendHeaders', (details, cb) => {
      seen.push(`capture:${details.requestHeaders.Authorization}`);
      cb({});
    });
    const done = vi.fn();
    installed.onBeforeSendHeaders({ url: 'https://x.test/', requestHeaders: {} }, done);
    expect(seen).toEqual(['auth', 'capture:Bearer t']);
    expect(done).toHaveBeenCalledWith({ requestHeaders: { Authorization: 'Bearer t' } });
  });

  it('stops the chain on a redirect and honours url filters', () => {
    const { session, installed } = fakeSession();
    const later = vi.fn((_d, cb) => cb({}));
    onWebRequest(session, 'onBeforeRequest', (_d, cb) => cb({ redirectURL: 'allternit-api://x' }), { urls: ['*://app.test/api/*'] });
    onWebRequest(session, 'onBeforeRequest', later);
    const done = vi.fn();
    installed.onBeforeRequest({ url: 'https://app.test/api/me' }, done);
    expect(done).toHaveBeenCalledWith({ redirectURL: 'allternit-api://x' });
    expect(later).not.toHaveBeenCalled();
    installed.onBeforeRequest({ url: 'https://other.test/' }, done);
    expect(later).toHaveBeenCalledTimes(1);
  });

  it('removing one handler leaves the others in place', () => {
    const { session, installed } = fakeSession();
    const keep = vi.fn();
    const remove = onWebRequest(session, 'onCompleted', vi.fn());
    onWebRequest(session, 'onCompleted', keep);
    remove();
    installed.onCompleted({ url: 'https://x.test/' });
    expect(keep).toHaveBeenCalledTimes(1);
  });
});
