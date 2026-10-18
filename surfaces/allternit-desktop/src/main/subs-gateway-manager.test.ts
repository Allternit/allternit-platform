import * as net from 'node:net';
import * as path from 'node:path';
import { describe, expect, it, vi } from 'vitest';

vi.mock('electron', () => ({ app: { isPackaged: false, getAppPath: () => '/nonexistent-app' } }));
vi.mock('electron-log', () => ({ default: { info: vi.fn(), warn: vi.fn(), error: vi.fn() } }));

import {
  apiEnvironmentFor,
  buildGatewayEnv,
  machineHasOwnGateway,
  pickPort,
  resolveGatewayLayout,
  SubsGatewayManager,
} from './subs-gateway-manager.js';

const layout = { root: '/r/subs-gateway', entry: '/r/subs-gateway/src/main.js', axBridge: '/r/subs-gateway/bin/ax-bridge' };

describe('subs-gateway-manager', () => {
  it('leaves a cloud computer alone: its image runs its own gateway', () => {
    expect(machineHasOwnGateway({ ALLTERNIT_LOCAL_SUBS_GATEWAY: '1' })).toBe(true);
    expect(machineHasOwnGateway({ ALLTERNIT_LOCAL_SUBS_GATEWAY: 'fallback' })).toBe(false);
    expect(machineHasOwnGateway({})).toBe(false);
  });

  it('finds the bundle by its entry file and the macOS helper when shipped', () => {
    const files = new Set([path.join('/b', 'src', 'main.js'), path.join('/b', 'bin', 'ax-bridge')]);
    const found = resolveGatewayLayout(['/a', '/b'], (p) => files.has(p));
    expect(found?.root).toBe('/b');
    expect(found?.axBridge).toBe(process.platform === 'darwin' ? path.join('/b', 'bin', 'ax-bridge') : null);
    expect(resolveGatewayLayout(['/a'], () => false)).toBeNull();
  });

  it('runs the gateway on Electron Node with file secrets, loopback TCP and own-computer mode', () => {
    const env = buildGatewayEnv({
      base: { PATH: '/bin', UNSET: undefined },
      layout,
      stateDir: '/home/u/.allternit/subscriptions',
      port: 7788,
      apiBase: 'http://127.0.0.1:8013',
      phoneGatewayFile: '/home/u/.allternit/phone-gateway.json',
    });
    expect(env).toMatchObject({
      PATH: '/bin',
      ELECTRON_RUN_AS_NODE: '1',
      SUBS_GATEWAY_KEYCHAIN: 'file',
      SUBS_GATEWAY_TCP: '1',
      SUBS_GATEWAY_TCP_HOST: '127.0.0.1',
      SUBS_GATEWAY_TCP_PORT: '7788',
      SUBS_GATEWAY_ADAPTERS_DIR: path.join('/r/subs-gateway', 'adapters'),
      SUBS_GATEWAY_MIGRATIONS_DIR: path.join('/r/subs-gateway', 'migrations'),
      SUBS_GATEWAY_BUNDLE_ROOT: '/r/subs-gateway',
      SUBS_GATEWAY_LOCAL_DESKTOP: '1',
      SUBS_GATEWAY_PHONE_GATEWAY_FILE: '/home/u/.allternit/phone-gateway.json',
      SUBS_GATEWAY_AX_BRIDGE_BIN: '/r/subs-gateway/bin/ax-bridge',
    });
    expect('UNSET' in env).toBe(false);
  });

  it('tells allternit-api to use this gateway only as the fallback', () => {
    expect(apiEnvironmentFor(7790, '/s')).toEqual({
      ALLTERNIT_LOCAL_SUBS_GATEWAY: 'fallback',
      ALLTERNIT_LOCAL_SUBS_GATEWAY_PORT: '7790',
      ALLTERNIT_LOCAL_SUBS_GATEWAY_STATE_DIR: '/s',
    });
  });

  it('takes another free port when the preferred one is in use', async () => {
    const busy = net.createServer();
    await new Promise<void>((r) => busy.listen(0, '127.0.0.1', () => r()));
    const taken = (busy.address() as net.AddressInfo).port;
    const got = await pickPort(taken);
    expect(got).not.toBe(taken);
    expect(got).toBeGreaterThan(0);
    busy.close();
  });

  it('with no bundle, gives the API no gateway env and reports degraded', async () => {
    const m = new SubsGatewayManager();
    await m.prepare();
    expect(m.getStatus()).toBe('degraded');
    expect(m.getApiEnvironment()).toEqual({});
  });
});
