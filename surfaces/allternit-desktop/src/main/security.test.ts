import { beforeEach, describe, expect, it, vi } from 'vitest';
import { shell } from 'electron';
import {
  assertTrustedSender,
  configureSecurity,
  displayMediaHandler,
  isTrustedAppUrl,
  openExternalAllowlisted,
} from './security.js';

const APP_ORIGIN = 'https://platform.allternit.com';
const DEV_ORIGIN = 'http://localhost:3014';

function configure() {
  configureSecurity({
    isDev: false,
    getAppOrigins: () => [APP_ORIGIN, 'http://127.0.0.1:8013'],
  });
}

describe('openExternalAllowlisted', () => {
  beforeEach(() => {
    configure();
    vi.mocked(shell.openExternal).mockClear();
  });

  it('opens https, http and mailto URLs', () => {
    for (const url of ['https://allternit.com/terms', 'http://example.com', 'mailto:a@b.c']) {
      expect(openExternalAllowlisted(url)).toBe(true);
    }
    expect(shell.openExternal).toHaveBeenCalledTimes(3);
  });

  it('denies non-allowlisted schemes', () => {
    for (const url of ['file:///etc/passwd', 'javascript:alert(1)', 'allternit://hud', 'data:text/html,x']) {
      expect(openExternalAllowlisted(url)).toBe(false);
    }
    expect(shell.openExternal).not.toHaveBeenCalled();
  });

  it('denies malformed URLs', () => {
    expect(openExternalAllowlisted('not a url')).toBe(false);
    expect(shell.openExternal).not.toHaveBeenCalled();
  });
});

describe('isTrustedAppUrl / assertTrustedSender', () => {
  beforeEach(configure);

  const senderFrom = (url: string) => ({ senderFrame: { url } }) as never;

  it('trusts configured app origins and app schemes', () => {
    expect(isTrustedAppUrl(`${APP_ORIGIN}/shell`)).toBe(true);
    expect(isTrustedAppUrl('http://127.0.0.1:8013/platform')).toBe(true);
    expect(isTrustedAppUrl('allternit-api://localhost:8013/health')).toBe(true);
  });

  it('rejects unknown, file and data origins', () => {
    expect(isTrustedAppUrl('https://evil.example.com')).toBe(false);
    expect(isTrustedAppUrl('file:///Applications/Evil.app/index.html')).toBe(false);
    expect(isTrustedAppUrl('data:text/html,hi')).toBe(false);
  });

  it('assertTrustedSender passes for app frames and throws otherwise', () => {
    expect(() => assertTrustedSender(senderFrom(`${APP_ORIGIN}/hud`), 'theme:set')).not.toThrow();
    expect(() => assertTrustedSender(senderFrom('file:///tmp/x'), 'theme:set')).toThrow(/Untrusted IPC sender/);
    expect(() => assertTrustedSender(senderFrom(''), 'theme:set')).toThrow(/Untrusted IPC sender/);
  });

  it('respects dev-only origins only when configured', () => {
    configure();
    expect(isTrustedAppUrl(DEV_ORIGIN)).toBe(false);
    configureSecurity({
      isDev: true,
      getAppOrigins: () => [APP_ORIGIN, DEV_ORIGIN],
    });
    expect(isTrustedAppUrl(DEV_ORIGIN)).toBe(true);
  });
});

describe('displayMediaHandler', () => {
  beforeEach(configure);
  const sources = [
    { id: 'screen:2', display_id: '2', name: 'Side' },
    { id: 'screen:1', display_id: '1', name: 'Main' },
  ] as never[];
  const handler = displayMediaHandler({ getSources: async () => sources, primaryDisplayId: () => '1' });
  const ask = (securityOrigin: string) =>
    new Promise<unknown>((resolve) => handler({ securityOrigin, frame: null, videoRequested: true, audioRequested: false, userGesture: false }, resolve));

  it('gives app pages the primary screen', async () => {
    expect(await ask('http://127.0.0.1:8013')).toEqual({ video: sources[1] });
  });

  it('gives web pages nothing', async () => {
    expect(await ask('https://evil.example.com')).toEqual({});
  });
});
