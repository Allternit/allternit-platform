import { describe, expect, it } from 'vitest';
import { createMeshPendingRetry, MESH_RETRY_EVERY_MS, type MeshPendingPhone, type MeshRetryDeps } from './phone-mesh-retry.js';

const IPHONE: MeshPendingPhone = { deviceId: 'dev-i', platform: 'ios', name: 'iPhone', host: '10.0.0.252:8200' };
const MESH_SCHEMA = { methods: [{ name: 'config', params: [{ name: 'mesh_control_url' }, { name: 'mesh_auth_key' }] }] };

function setup(over: Partial<MeshRetryDeps> = {}, whoamis: Array<Record<string, unknown> | null> = [{ device_id: 'dev-i', mesh_state: 'off' }, { device_id: 'dev-i', mesh_state: 'running', mesh_host: '100.64.0.30:8200' }]) {
  let t = 0;
  const logs: string[] = [];
  const calls: Array<{ method: string; params: Record<string, unknown> }> = [];
  const joined: string[] = [];
  const answers = [...whoamis];
  const deps: MeshRetryDeps = {
    pending: () => [IPHONE],
    fetchSchema: async () => MESH_SCHEMA,
    whoami: async () => (answers.length > 1 ? answers.shift()! : answers[0]) as never,
    enroll: async () => ({ controlUrl: 'https://mesh.example', authKey: 'hskey-secret' }),
    call: async (_p, method, params) => void calls.push({ method, params }),
    joined: (_p, meshHost) => void joined.push(meshHost),
    log: (l) => logs.push(l),
    now: () => t,
    sleep: async (ms) => void (t += ms),
    ...over,
  };
  return { run: createMeshPendingRetry(deps), logs, calls, joined, advance: (ms: number) => (t += ms) };
}

describe('createMeshPendingRetry', () => {
  it('finishes the join the way setup does and saves the mesh address', async () => {
    const s = setup();
    await s.run();
    expect(s.calls).toEqual([{ method: 'config', params: { mesh_control_url: 'https://mesh.example', mesh_auth_key: 'hskey-secret' } }]);
    expect(s.joined).toEqual(['100.64.0.30:8200']);
    expect(s.logs).toEqual(['[PhoneLink] mesh join retry iPhone: joined at 100.64.0.30:8200']);
    expect(s.logs.join(' ')).not.toContain('hskey');
  });

  it('just records a mesh that is already running', async () => {
    const s = setup({}, [{ device_id: 'dev-i', mesh_state: 'running', mesh_host: '100.64.0.31:8200' }]);
    await s.run();
    expect(s.calls).toEqual([]);
    expect(s.joined).toEqual(['100.64.0.31:8200']);
  });

  it('stays quiet when the phone is not answering, and waits 10 min before the next try', async () => {
    const s = setup({}, [null]);
    await s.run();
    expect(s.logs).toEqual([]);
    await s.run();
    expect(s.calls).toEqual([]);
    s.advance(MESH_RETRY_EVERY_MS - 1);
    await s.run();
    expect(s.joined).toEqual([]);
  });

  it('never calls config on a helper without mesh support or another device', async () => {
    const noMesh = setup({ fetchSchema: async () => ({ methods: [{ name: 'config', params: [] }] }) });
    await noMesh.run();
    expect(noMesh.calls).toEqual([]);
    expect(noMesh.logs).toEqual(['[PhoneLink] mesh join retry iPhone: this helper has no mesh support']);
    const other = setup({}, [{ device_id: 'someone-else', mesh_state: 'off' }]);
    await other.run();
    expect(other.calls).toEqual([]);
  });

  it('gives up after the 60 s join budget', async () => {
    const s = setup({}, [{ device_id: 'dev-i', mesh_state: 'starting' }]);
    await s.run();
    expect(s.joined).toEqual([]);
    expect(s.logs).toEqual(['[PhoneLink] mesh join retry iPhone: did not report running in time']);
  });
});
