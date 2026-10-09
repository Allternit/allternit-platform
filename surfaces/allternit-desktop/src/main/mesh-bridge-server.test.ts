import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('electron-log', () => ({ default: { info: vi.fn(), warn: vi.fn(), error: vi.fn() } }));

import { startMeshBridgeServer } from './mesh-bridge-server.js';

let close: (() => void) | null = null;
afterEach(() => { close?.(); close = null; });

async function bridge(loopbackFor = vi.fn(async (t: string) => `127.0.0.1:4${t.length}`)) {
  const started = await startMeshBridgeServer({ secret: () => 'secret', loopbackFor });
  close = started.close;
  return { url: started.url, loopbackFor };
}

describe('mesh bridge server', () => {
  it('returns a loopback address for a mesh target with the secret', async () => {
    const { url, loopbackFor } = await bridge();
    const res = await fetch(`${url}/mesh/tcp`, {
      method: 'POST',
      headers: { 'x-allternit-desktop-access-token': 'secret', 'content-type': 'application/json' },
      body: JSON.stringify({ target: '100.64.0.7:5900' }),
    });
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ address: '127.0.0.1:415' });
    expect(loopbackFor).toHaveBeenCalledWith('100.64.0.7:5900');
  });

  it('refuses without the secret', async () => {
    const { url, loopbackFor } = await bridge();
    const res = await fetch(`${url}/mesh/tcp`, { method: 'POST', body: JSON.stringify({ target: '100.64.0.7:5900' }) });
    expect(res.status).toBe(403);
    expect(loopbackFor).not.toHaveBeenCalled();
  });

  it('reports mesh failures', async () => {
    const { url } = await bridge(vi.fn(async (_t: string): Promise<string> => { throw new Error('10.0.0.1:5900 is not a mesh address'); }));
    const res = await fetch(`${url}/mesh/tcp`, {
      method: 'POST',
      headers: { 'x-allternit-desktop-access-token': 'secret' },
      body: JSON.stringify({ target: '10.0.0.1:5900' }),
    });
    expect(res.status).toBe(502);
    expect((await res.json() as { message?: string }).message).toContain('not a mesh address');
  });
});
