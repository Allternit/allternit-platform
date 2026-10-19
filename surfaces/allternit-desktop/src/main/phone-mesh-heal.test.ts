import { describe, expect, it } from 'vitest';
import type { LinkedPhone } from './phone-link-health.js';
import { createMeshHealer, MESH_HEAL_COOLDOWN_MS, phoneOffMeshRoutes, type MeshHealDeps, type OffMeshRoute } from './phone-mesh-heal.js';
import type { AdbCall } from './phone-tools.js';

const ANDROID: LinkedPhone = { deviceId: 'dev-a', platform: 'android', name: 'moto', lanHost: '192.168.1.5:8200', meshHost: '100.64.0.16:8200', storeKey: 'k' };

function setup(over: Partial<MeshHealDeps> = {}, answers: Record<string, string> = { '192.168.1.5:8200': 'dev-a' }) {
  let t = 0;
  const logs: string[] = [];
  const calls: { host: string; method: string; params: Record<string, unknown> }[] = [];
  const closed: string[] = [];
  const deps: MeshHealDeps = {
    routes: (phone) => [async (): Promise<OffMeshRoute | null> => (phone.lanHost ? { host: phone.lanHost, via: 'lan', close: () => void closed.push(phone.lanHost!) } : null)],
    fetchWhoami: async (host) => (answers[host] ? { device_id: answers[host] } : null),
    call: async (_phone, host, method, params) => {
      calls.push({ host, method, params });
      return { mesh_state: 'running', mesh_host: '100.64.0.16:8200' };
    },
    log: (l) => logs.push(l),
    now: () => t,
    ...over,
  };
  return { heal: createMeshHealer(deps), logs, calls, closed, advance: (ms: number) => (t += ms) };
}

describe('createMeshHealer', () => {
  it('restarts the mesh over the LAN and logs one line', async () => {
    const { heal, logs, calls, closed } = setup();
    expect(await heal(ANDROID, 'live-screen mesh route failed')).toEqual({ meshState: 'running', meshHost: '100.64.0.16:8200' });
    expect(calls).toEqual([{ host: '192.168.1.5:8200', method: 'mesh_restart', params: { wait: 20 } }]);
    expect(closed).toEqual(['192.168.1.5:8200']);
    expect(logs).toEqual(['[PhoneLink] mesh heal moto: live-screen mesh route failed; mesh_restart over lan -> running 100.64.0.16:8200']);
  });

  it('tries at most once per 2 minutes per phone, failures included', async () => {
    const { heal, calls, advance } = setup({}, {});
    expect(await heal(ANDROID, 'x')).toBeNull();
    expect(await heal(ANDROID, 'x')).toBeNull();
    advance(MESH_HEAL_COOLDOWN_MS - 1);
    expect(await heal(ANDROID, 'x')).toBeNull();
    expect(calls).toHaveLength(0);
    const ok = setup();
    await ok.heal(ANDROID, 'x');
    await ok.heal(ANDROID, 'x');
    expect(ok.calls).toHaveLength(1);
    ok.advance(MESH_HEAL_COOLDOWN_MS);
    await ok.heal(ANDROID, 'x');
    expect(ok.calls).toHaveLength(2);
    // Another phone has its own budget.
    await ok.heal({ ...ANDROID, deviceId: 'dev-b', lanHost: '192.168.1.6:8200' }, 'x');
  });

  it('shares one attempt between concurrent callers', async () => {
    const { heal, calls } = setup();
    const [a, b] = await Promise.all([heal(ANDROID, 'x'), heal(ANDROID, 'y')]);
    expect(a).toEqual(b);
    expect(calls).toHaveLength(1);
  });

  it('skips a route that answers as another phone, and says when nothing answered', async () => {
    const { heal, calls, logs, closed } = setup({}, { '192.168.1.5:8200': 'someone-else' });
    expect(await heal(ANDROID, 'mesh route failed')).toBeNull();
    expect(calls).toHaveLength(0);
    expect(closed).toEqual(['192.168.1.5:8200']);
    expect(logs).toEqual(["[PhoneLink] mesh heal moto: mesh route failed; no LAN or cable route answered, can't restart its mesh"]);
  });

  it('reports an older helper without mesh_restart and gives up', async () => {
    const { heal, logs } = setup({ call: async () => Promise.reject(new Error('Method not found')) });
    expect(await heal(ANDROID, 'x')).toBeNull();
    expect(logs[0]).toMatch(/mesh_restart over lan failed: Method not found/);
  });

  it('saves a changed mesh address', async () => {
    const saved: string[] = [];
    const { heal } = setup({
      call: async () => ({ mesh_state: 'running', mesh_host: '100.64.0.40:8200' }),
      rememberMeshHost: (_p, h) => saved.push(h),
    });
    expect((await heal(ANDROID, 'x'))?.meshHost).toBe('100.64.0.40:8200');
    expect(saved).toEqual(['100.64.0.40:8200']);
  });
});

describe('phoneOffMeshRoutes', () => {
  function fakeAdb(port = '43210') {
    const runs: string[][] = [];
    const adb = (async (args: string[]) => {
      runs.push(args);
      return { stdout: args[0] === 'forward' && args[1] === 'tcp:0' ? `${port}\n` : '', stderr: '', code: 0 };
    }) as unknown as AdbCall;
    return { adb, runs };
  }

  it('LAN first, then an adb forward over the cable that is removed on close', async () => {
    const { adb, runs } = fakeAdb();
    const routes = phoneOffMeshRoutes({ androidSerial: () => 'ZY22', iosUdid: () => null, adb: async () => adb })(ANDROID);
    expect(routes).toHaveLength(2);
    expect(await routes[0]()).toEqual({ host: '192.168.1.5:8200', via: 'lan' });
    const cable = await routes[1]();
    expect(cable).toMatchObject({ host: '127.0.0.1:43210', via: 'cable' });
    expect(runs).toEqual([['forward', 'tcp:0', 'tcp:8200']]);
    await cable!.close!();
    expect(runs[1]).toEqual(['forward', '--remove', 'tcp:43210']);
  });

  it('never starts adb: no running adb means no cable route', async () => {
    const routes = phoneOffMeshRoutes({ androidSerial: () => 'ZY22', iosUdid: () => null, adb: async () => null })({ ...ANDROID, lanHost: undefined });
    expect(routes).toHaveLength(1);
    expect(await routes[0]()).toBeNull();
  });

  it('an iPhone goes over a usbmux forward', async () => {
    let closed = false;
    const routes = phoneOffMeshRoutes({
      androidSerial: () => null,
      iosUdid: () => 'udid-1',
      adb: async () => null,
      usbForward: async (udid, port) => {
        expect([udid, port]).toEqual(['udid-1', 8200]);
        return { port: 51000, close: async () => void (closed = true) };
      },
    })({ ...ANDROID, platform: 'ios', lanHost: undefined });
    const r = await routes[0]();
    expect(r).toMatchObject({ host: '127.0.0.1:51000', via: 'cable' });
    await r!.close!();
    expect(closed).toBe(true);
  });
});
