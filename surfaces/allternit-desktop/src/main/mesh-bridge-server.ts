/**
 * The local mesh bridge (ACI P4 remote computers): allternit-api can't route
 * to tailnet addresses itself (the mesh runs in userspace), so it asks main
 * for a loopback port that reaches a mesh TCP target. Loopback-only, and
 * every request must carry the API's spawn-time desktop secret.
 *
 * POST /mesh/tcp {"target": "100.x.y.z:5900"} → {"address": "127.0.0.1:port"}
 */
import * as http from 'node:http';
import log from 'electron-log';

export interface MeshBridgeDeps {
  secret: () => string | null;
  loopbackFor: (target: string) => Promise<string>;
}

export function handleMeshBridgeRequest(
  deps: MeshBridgeDeps,
  req: http.IncomingMessage,
  res: http.ServerResponse,
): void {
  const reply = (status: number, body: unknown) => {
    res.writeHead(status, { 'content-type': 'application/json' });
    res.end(JSON.stringify(body));
  };
  if (req.method !== 'POST' || req.url !== '/mesh/tcp') return reply(404, { error: 'not_found' });
  const secret = deps.secret();
  if (!secret || req.headers['x-allternit-desktop-access-token'] !== secret) return reply(403, { error: 'forbidden' });
  let raw = '';
  req.on('data', (chunk: Buffer) => {
    raw += chunk.toString();
    if (raw.length > 4096) req.destroy();
  });
  req.on('end', () => {
    let target: unknown;
    try {
      target = (JSON.parse(raw) as { target?: unknown }).target;
    } catch {
      return reply(400, { error: 'bad_json' });
    }
    if (typeof target !== 'string') return reply(400, { error: 'target_required' });
    deps.loopbackFor(target).then(
      (address) => reply(200, { address }),
      (error: unknown) => {
        log.warn('[MeshBridge] could not reach', target, error);
        reply(502, { error: 'mesh_unreachable', message: error instanceof Error ? error.message : String(error) });
      },
    );
  });
}

/** Start the bridge on a random loopback port; resolves with its base URL. */
export function startMeshBridgeServer(deps: MeshBridgeDeps): Promise<{ url: string; close: () => void }> {
  const server = http.createServer((req, res) => handleMeshBridgeRequest(deps, req, res));
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      const port = typeof address === 'object' && address ? address.port : 0;
      resolve({ url: `http://127.0.0.1:${port}`, close: () => server.close() });
    });
  });
}
